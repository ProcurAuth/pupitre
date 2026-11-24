// SPDX-License-Identifier: MIT OR Apache-2.0
//! The cases S01 to S10 of 2.43: the documents.

use std::sync::atomic::{AtomicBool, Ordering};

use pupitre_core::doc::DocKey;
use pupitre_core::etag::Etag;

use super::harness::{CrashOutcome, FreshOptions, Harness};
use super::support::{common_key, env, env_doc, env_key, meta, number_of, numbered_doc};
use crate::store::{AnyStore, Store};
use crate::types::{Precondition, StoreError, VersionKind};

async fn fresh<H: Harness>(h: &H) -> AnyStore {
    h.fresh(FreshOptions::default()).await.expect("a fresh store")
}

/// S01: a create then a `get` give the same bytes, etag and version 1.
pub async fn s01_create_and_get<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let key = env_key("production");
    let doc = env_doc("Production");
    let saved = store.put(&key, &doc, &Precondition::Create, &meta("alice", "first")).await.expect("create");
    assert_eq!(saved.version, 1);
    assert_eq!(saved.etag, Some(doc.etag));
    let got = store.get(&key).await.expect("get");
    assert_eq!(got.key, key);
    assert_eq!(got.bytes, doc.bytes);
    assert_eq!(got.etag, doc.etag);
    assert_eq!(got.etag, Etag::of(&doc.bytes));
    assert_eq!(got.version, 1);
    assert_eq!(store.get(&env_key("staging")).await, Err(StoreError::NotFound));
}

/// S02: a save with the current etag makes version 2. A save with an old etag gives `Conflict`
/// with the current etag, and changes nothing.
pub async fn s02_save_and_conflict<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let key = env_key("production");
    let (v1, v2, v3) = (env_doc("One"), env_doc("Two"), env_doc("Three"));
    store.put(&key, &v1, &Precondition::Create, &meta("alice", "")).await.expect("create");
    let saved = store.put(&key, &v2, &Precondition::Match(v1.etag), &meta("bob", "second")).await.expect("save");
    assert_eq!(saved.version, 2);
    assert_eq!(saved.etag, Some(v2.etag));
    // The base etag is old: the answer names the current etag.
    let refused = store.put(&key, &v3, &Precondition::Match(v1.etag), &meta("carol", "")).await;
    assert_eq!(refused, Err(StoreError::Conflict { current: Some(v2.etag) }));
    let got = store.get(&key).await.expect("get");
    assert_eq!((got.bytes, got.version), (v2.bytes.clone(), 2));
    assert_eq!(store.history(&key, None, 10).await.expect("history").len(), 2);
    // A save on a key with no current document is `NotFound`.
    let missing = store.put(&env_key("staging"), &v3, &Precondition::Match(v1.etag), &meta("carol", "")).await;
    assert_eq!(missing, Err(StoreError::NotFound));
}

/// S03: two saves at the same time with one base etag: exactly one succeeds.
pub async fn s03_two_writers_one_winner<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let other = h.other_handle(&store).await.expect("a second handle");
    let key = env_key("production");
    let base = env_doc("Base");
    store.put(&key, &base, &Precondition::Create, &meta("alice", "")).await.expect("create");
    for round in 0..8u64 {
        let current = store.get(&key).await.expect("get").etag;
        let (a, b) = (numbered_doc(round * 2), numbered_doc(round * 2 + 1));
        let second = other.as_ref().unwrap_or(&store);
        let (pre, alice, bob) = (Precondition::Match(current), meta("alice", ""), meta("bob", ""));
        let (ra, rb) = tokio::join!(store.put(&key, &a, &pre, &alice), second.put(&key, &b, &pre, &bob));
        let wins = usize::from(ra.is_ok()) + usize::from(rb.is_ok());
        assert_eq!(wins, 1, "{}: round {round}: {ra:?} {rb:?}", h.name());
        let loser = if ra.is_ok() { rb } else { ra };
        assert!(matches!(loser, Err(StoreError::Conflict { current: Some(_) })), "{loser:?}");
        // No lost update: the document is the one of the winner, and the history has no gap.
        let now = store.get(&key).await.expect("get");
        assert!(now.etag == a.etag || now.etag == b.etag);
        assert_eq!(now.version, 2 + round);
    }
    // Two creates at the same time: exactly one succeeds.
    let fresh_key = env_key("staging");
    let (c, d) = (env_doc("C"), env_doc("D"));
    let second = other.as_ref().unwrap_or(&store);
    let (alice, bob) = (meta("alice", ""), meta("bob", ""));
    let (rc, rd) = tokio::join!(
        store.put(&fresh_key, &c, &Precondition::Create, &alice),
        second.put(&fresh_key, &d, &Precondition::Create, &bob),
    );
    assert_eq!(usize::from(rc.is_ok()) + usize::from(rd.is_ok()), 1, "{rc:?} {rd:?}");
    assert!(matches!(rc.as_ref().err().or(rd.as_ref().err()), Some(StoreError::AlreadyExists)));
}

/// S04: a create on a key with a current document gives `AlreadyExists`. A create after a
/// delete gives the next version.
pub async fn s04_create_exists_and_after_delete<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let key = env_key("production");
    let doc = env_doc("One");
    store.put(&key, &doc, &Precondition::Create, &meta("alice", "")).await.expect("create");
    let again = store.put(&key, &env_doc("Two"), &Precondition::Create, &meta("alice", "")).await;
    assert_eq!(again, Err(StoreError::AlreadyExists));
    assert_eq!(store.get(&key).await.expect("get").bytes, doc.bytes);
    store.delete(&key, &doc.etag, &meta("alice", "")).await.expect("delete");
    // A save on a deleted key has no current document.
    let save = store.put(&key, &env_doc("Two"), &Precondition::Match(doc.etag), &meta("alice", "")).await;
    assert_eq!(save, Err(StoreError::NotFound));
    let created = store.put(&key, &env_doc("Three"), &Precondition::Create, &meta("alice", "")).await.expect("create");
    assert_eq!(created.version, 3);
    let kinds: Vec<VersionKind> =
        store.history(&key, None, 10).await.expect("history").iter().map(|v| v.kind).collect();
    assert_eq!(kinds, vec![VersionKind::Create, VersionKind::Delete, VersionKind::Create]);
}

/// S05: a delete writes a version `delete`. `get` gives `NotFound`. A poll reports the key as gone.
pub async fn s05_delete_and_poll<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let key = env_key("production");
    let doc = env_doc("One");
    store.put(&key, &doc, &Precondition::Create, &meta("alice", "")).await.expect("create");
    let listing = store.load().await.expect("load");
    assert_eq!(listing.docs.len(), 1);
    // A wrong etag is a conflict, and a key with no document is not found.
    let wrong = store.delete(&key, &Etag::of(b"other"), &meta("bob", "")).await;
    assert_eq!(wrong, Err(StoreError::Conflict { current: Some(doc.etag) }));
    assert_eq!(store.delete(&env_key("staging"), &doc.etag, &meta("bob", "")).await, Err(StoreError::NotFound));
    let saved = store.delete(&key, &doc.etag, &meta("bob", "gone")).await.expect("delete");
    assert_eq!((saved.version, saved.etag), (2, None));
    assert_eq!(store.get(&key).await, Err(StoreError::NotFound));
    let newest = store.history(&key, None, 1).await.expect("history").remove(0);
    assert_eq!((newest.version, newest.kind, newest.etag), (2, VersionKind::Delete, None));
    assert_eq!(newest.meta.saved_by.as_str(), "bob");
    let version = store.version(&key, 2).await.expect("version");
    assert_eq!(version.bytes, None);
    // A second delete has no current document.
    assert_eq!(store.delete(&key, &doc.etag, &meta("bob", "")).await, Err(StoreError::NotFound));
    let delta = store.poll(&listing.marks).await.expect("poll");
    assert!(delta.removed.contains(&key), "the poll reports the key as gone");
    assert!(delta.changed.iter().all(|d| d.key != key));
    assert!(store.load().await.expect("load").docs.is_empty());
}

/// S06: `history` gives the versions newest first, with pages. `version` gives the bytes of
/// each version.
pub async fn s06_history_and_version<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let key = common_key("production");
    let mut docs = Vec::new();
    let mut previous: Option<Etag> = None;
    for n in 1..=5u64 {
        let doc = numbered_doc(n);
        let precondition = previous.map_or(Precondition::Create, Precondition::Match);
        let saved = store.put(&key, &doc, &precondition, &meta("alice", &format!("write {n}"))).await.expect("write");
        assert_eq!(saved.version, n);
        previous = Some(doc.etag);
        docs.push(doc);
    }
    let page = store.history(&key, None, 3).await.expect("history");
    let numbers: Vec<u64> = page.iter().map(|v| v.version).collect();
    assert_eq!(numbers, vec![5, 4, 3]);
    assert_eq!(page[0].kind, VersionKind::Save);
    assert_eq!(page[0].etag, Some(docs[4].etag));
    assert_eq!(page[0].meta.saved_by.as_str(), "alice");
    assert_eq!(page[0].meta.message.as_str(), "write 5");
    let rest = store.history(&key, Some(3), 10).await.expect("history");
    assert_eq!(rest.iter().map(|v| v.version).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(rest[1].kind, VersionKind::Create);
    assert_eq!(store.history(&key, Some(1), 10).await.expect("history"), vec![]);
    assert_eq!(store.history(&env_key("staging"), None, 10).await.expect("history"), vec![]);
    for (n, doc) in (1u64..).zip(&docs) {
        let version = store.version(&key, n).await.expect("version");
        assert_eq!(version.bytes.as_deref(), Some(doc.bytes.as_slice()));
        assert_eq!(version.meta.version, n);
        assert_eq!(number_of(&version.bytes.expect("bytes")), n);
    }
    assert_eq!(store.version(&key, 6).await, Err(StoreError::NotFound));
}

/// S07: `history_keep` removes the oldest versions and keeps the numbers.
pub async fn s07_history_keep<H: Harness>(h: &H) {
    let store = h.fresh(FreshOptions { history_keep: 3, ..FreshOptions::default() }).await.expect("a fresh store");
    let key = common_key("production");
    let mut previous: Option<Etag> = None;
    for n in 1..=6u64 {
        let doc = numbered_doc(n);
        let precondition = previous.map_or(Precondition::Create, Precondition::Match);
        store.put(&key, &doc, &precondition, &meta("alice", "")).await.expect("write");
        previous = Some(doc.etag);
    }
    let numbers: Vec<u64> = store.history(&key, None, 10).await.expect("history").iter().map(|v| v.version).collect();
    assert_eq!(numbers, vec![6, 5, 4]);
    assert_eq!(store.version(&key, 3).await, Err(StoreError::NotFound));
    assert_eq!(store.version(&key, 4).await.expect("version").meta.version, 4);
    // The numbers never change: the next write is 7.
    let saved = store.put(&key, &numbered_doc(7), &Precondition::Match(previous.expect("etag")), &meta("a", "")).await;
    assert_eq!(saved.expect("write").version, 7);
    // A delete is a version too, and the key keeps its numbers after the oldest versions go.
    let current = store.get(&key).await.expect("get").etag;
    assert_eq!(store.delete(&key, &current, &meta("a", "")).await.expect("delete").version, 8);
    let created = store.put(&key, &numbered_doc(9), &Precondition::Create, &meta("a", "")).await.expect("create");
    assert_eq!(created.version, 9);
    let numbers: Vec<u64> = store.history(&key, None, 10).await.expect("history").iter().map(|v| v.version).collect();
    assert_eq!(numbers, vec![9, 8, 7]);
}

/// S08: a crash between the version and the document leaves the store in a state where the
/// current document is the newest version (2.19). The backend tells if the open repaired the
/// document or if the write left nothing.
pub async fn s08_crash_between_version_and_document<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let key = env_key("production");
    let (v1, v2) = (env_doc("One"), env_doc("Two"));
    store.put(&key, &v1, &Precondition::Create, &meta("alice", "")).await.expect("create");
    let (store, outcome) = h
        .crash_after_version(store, &key, &v2, &Precondition::Match(v1.etag), &meta("bob", "crash"))
        .await
        .expect("the crash and the open");
    let current = store.get(&key).await.expect("get");
    let newest = store.history(&key, None, 1).await.expect("history").remove(0);
    // The invariant: the history never holds a change that the document does not hold.
    assert_eq!(current.version, newest.version, "{}: the document is the newest version", h.name());
    assert_eq!(newest.etag, Some(current.etag));
    match outcome {
        CrashOutcome::Repaired => {
            assert_eq!((current.version, current.bytes), (2, v2.bytes.clone()));
            assert_eq!(newest.meta.saved_by.as_str(), "bob");
        }
        CrashOutcome::RolledBack => {
            assert_eq!((current.version, current.bytes), (1, v1.bytes.clone()));
            assert_eq!(store.version(&key, 2).await, Err(StoreError::NotFound));
        }
    }
    // The key works after the crash: a save with the etag of the current document.
    let next = env_doc("Three");
    let saved = store.put(&key, &next, &Precondition::Match(current.etag), &meta("carol", "")).await.expect("save");
    assert_eq!(saved.version, current.version + 1);
}

/// S09: a poll after a write of another handle reports the document. A poll with no change
/// reports nothing new.
pub async fn s09_poll<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let other = h.other_handle(&store).await.expect("a second handle");
    let writer = other.as_ref().unwrap_or(&store);
    let listing = store.load().await.expect("load");
    assert!(listing.docs.is_empty());
    let key = env_key("production");
    let doc = env_doc("One");
    writer.put(&key, &doc, &Precondition::Create, &meta("alice", "")).await.expect("create");
    let first = store.poll(&listing.marks).await.expect("poll");
    let reported: Vec<_> = first.changed.iter().filter(|d| d.key == key).collect();
    assert_eq!(reported.len(), 1, "{}: the poll reports the document", h.name());
    assert_eq!((reported[0].etag, reported[0].version, &reported[0].bytes), (doc.etag, 1, &doc.bytes));
    assert!(first.removed.is_empty());
    // No change: a poll can report a document again, but never a new one.
    let second = store.poll(&first.marks).await.expect("poll");
    for d in &second.changed {
        assert_eq!((d.key.clone(), d.etag, d.version), (key.clone(), doc.etag, 1));
    }
    assert!(second.removed.is_empty());
    // A change after the poll is reported.
    let doc2 = env_doc("Two");
    writer.put(&key, &doc2, &Precondition::Match(doc.etag), &meta("alice", "")).await.expect("save");
    let third = store.poll(&second.marks).await.expect("poll");
    assert!(third.changed.iter().any(|d| d.key == key && d.etag == doc2.etag && d.version == 2));
    // A load after the writes gives the same state as the polls.
    let loaded = store.load().await.expect("load");
    assert_eq!(loaded.docs.len(), 1);
    assert_eq!(loaded.docs[0].etag, doc2.etag);
}

/// S10: `read_environment` never shows half of a write. A writer changes two documents of one
/// environment in turn, the first and then the second. A consistent state has the number of the
/// first document at the number of the second, or one above it.
pub async fn s10_read_environment_is_consistent<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let (a_key, b_key) = (env_key("production"), common_key("production"));
    // Other environments must not show: `prod` is a prefix of `production`.
    store.put(&env_key("prod"), &env_doc("Prod"), &Precondition::Create, &meta("alice", "")).await.expect("create");
    store
        .put(&env_key("staging"), &env_doc("Staging"), &Precondition::Create, &meta("alice", ""))
        .await
        .expect("create");
    let (a0, b0) = (numbered_doc(0), numbered_doc(0));
    store.put(&a_key, &a0, &Precondition::Create, &meta("alice", "")).await.expect("create");
    store.put(&b_key, &b0, &Precondition::Create, &meta("alice", "")).await.expect("create");
    let only = store.read_environment(&env("prod")).await.expect("read");
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].key, env_key("prod"));
    let done = AtomicBool::new(false);
    let writer = async {
        let (mut a_etag, mut b_etag) = (a0.etag, b0.etag);
        for n in 1..=60u64 {
            let (a, b) = (numbered_doc(n), numbered_doc(n));
            store.put(&a_key, &a, &Precondition::Match(a_etag), &meta("alice", "")).await.expect("write a");
            a_etag = a.etag;
            store.put(&b_key, &b, &Precondition::Match(b_etag), &meta("alice", "")).await.expect("write b");
            b_etag = b.etag;
        }
        done.store(true, Ordering::SeqCst);
    };
    let reader = async {
        let mut reads = 0u32;
        while !done.load(Ordering::SeqCst) || reads < 3 {
            let docs = store.read_environment(&env("production")).await.expect("read");
            let find = |key: &DocKey| docs.iter().find(|d| d.key == *key).expect("the document is in the state");
            assert_eq!(docs.len(), 2, "only the documents of the environment");
            let (a, b) = (find(&a_key), find(&b_key));
            for d in [a, b] {
                assert_eq!(d.etag, Etag::of(&d.bytes), "the etag is the hash of the bytes");
                assert_eq!(number_of(&d.bytes) + 1, d.version, "the version is the one of the bytes");
            }
            let (an, bn) = (number_of(&a.bytes), number_of(&b.bytes));
            assert!(bn <= an && an <= bn + 1, "a half write is visible: a = {an}, b = {bn}");
            reads += 1;
        }
    };
    tokio::join!(writer, reader);
}
