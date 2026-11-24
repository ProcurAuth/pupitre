// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes outside Pupitre (2.33).

mod files_support;

use files_support::*;
use pupitre_core::doc::{DocKind, Document};
use pupitre_core::etag::Etag;
use pupitre_store::files::FilesStore;
use pupitre_store::{Marks, Precondition, Store, StoreEvent, VersionKind};

fn write_doc(fx: &Fixture, relative: &str, bytes: &[u8]) {
    let path = fx.docs().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn an_outside_change_that_is_valid_gets_a_version_external() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let a = env_doc("A");
    store.put(&k, &a, &Precondition::Create, &meta("alice", "mine")).await.unwrap();
    let listing = store.load().await.unwrap();
    store.drain_events();
    // A person edits the file, for example with `git pull`.
    let b = env_doc("B");
    write_doc(&fx, "environments/p/environment.json", &b.bytes);
    let delta = store.poll(&listing.marks).await.unwrap();
    assert_eq!(delta.changed.len(), 1);
    assert_eq!((delta.changed[0].version, delta.changed[0].etag), (2, b.etag));
    assert_eq!(delta.changed[0].bytes, b.bytes);
    let versions = store.history(&k, None, 10).await.unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].kind, VersionKind::External);
    assert_eq!(versions[0].meta.saved_by.as_str(), "external");
    assert_eq!(versions[0].meta.message.as_str(), "");
    assert_eq!(versions[0].etag, Some(b.etag));
    assert_eq!(store.version(&k, 2).await.unwrap().bytes, Some(b.bytes.clone()));
    let events = store.drain_events();
    assert!(events.contains(&StoreEvent::ExternalChange { key: k.clone(), before: Some(a.etag), after: Some(b.etag) }));
    // A second poll writes no second version, and the file stays as the person wrote it.
    assert!(store.poll(&delta.marks).await.unwrap().changed.is_empty());
    assert_eq!(store.history(&k, None, 10).await.unwrap().len(), 2);
    assert_eq!(std::fs::read(fx.docs().join("environments/p/environment.json")).unwrap(), b.bytes);
    // A write of the store after the outside change uses the new etag.
    let c = env_doc("C");
    assert!(store.put(&k, &c, &Precondition::Match(a.etag), &meta("alice", "")).await.is_err());
    assert_eq!(store.put(&k, &c, &Precondition::Match(b.etag), &meta("alice", "")).await.unwrap().version, 3);
}

#[tokio::test]
async fn an_outside_change_is_seen_by_a_write_before_the_poll() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let a = env_doc("A");
    store.put(&k, &a, &Precondition::Create, &meta("alice", "")).await.unwrap();
    let b = env_doc("B");
    write_doc(&fx, "environments/p/environment.json", &b.bytes);
    // No poll ran. The conflict shows the etag of the outside change, not the etag of the store.
    let err = store.put(&k, &env_doc("C"), &Precondition::Match(a.etag), &meta("alice", "")).await.unwrap_err();
    assert_eq!(err, pupitre_store::StoreError::Conflict { current: Some(b.etag) });
    assert_eq!(store.history(&k, Some(10), 10).await.unwrap()[0].kind, VersionKind::External);
}

#[tokio::test]
async fn an_outside_delete_gets_a_version_delete_and_a_new_file_gets_a_version_1() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    store.put(&k, &env_doc("A"), &Precondition::Create, &meta("alice", "")).await.unwrap();
    let listing = store.load().await.unwrap();
    std::fs::remove_file(fx.docs().join("environments/p/environment.json")).unwrap();
    let h = key("environments/p/hosts/host-a");
    let hd = host_doc("host.example.invalid");
    write_doc(&fx, "environments/p/hosts/host-a.json", &hd.bytes);
    let delta = store.poll(&listing.marks).await.unwrap();
    assert_eq!(delta.removed, vec![k.clone()]);
    assert_eq!(delta.changed.len(), 1);
    assert_eq!((delta.changed[0].key.clone(), delta.changed[0].version), (h.clone(), 1));
    let newest = &store.history(&k, None, 1).await.unwrap()[0];
    assert_eq!((newest.version, newest.kind, newest.etag), (2, VersionKind::Delete, None));
    assert_eq!(newest.meta.saved_by.as_str(), "external");
    let first = &store.history(&h, None, 1).await.unwrap()[0];
    assert_eq!((first.version, first.kind, first.meta.saved_by.as_str()), (1, VersionKind::External, "external"));
    // The key is free again for a create (2.20).
    assert_eq!(store.put(&k, &env_doc("B"), &Precondition::Create, &meta("a", "")).await.unwrap().version, 3);
}

#[tokio::test]
async fn an_outside_change_while_the_store_is_down_is_found_at_open() {
    let fx = Fixture::new();
    let k = key("environments/p");
    {
        let store = fx.open();
        store.put(&k, &env_doc("A"), &Precondition::Create, &meta("alice", "")).await.unwrap();
    }
    let b = env_doc("B");
    write_doc(&fx, "environments/p/environment.json", &b.bytes);
    write_doc(&fx, "environments/p/hosts/host-a.json", &host_doc("host.example.invalid").bytes);
    let store = fx.reopen();
    assert_eq!(store.get(&k).await.unwrap().version, 2);
    assert_eq!(store.history(&k, None, 1).await.unwrap()[0].kind, VersionKind::External);
    assert_eq!(store.get(&key("environments/p/hosts/host-a")).await.unwrap().version, 1);
}

#[tokio::test]
async fn an_outside_change_back_to_a_known_content_makes_no_version() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let a = env_doc("A");
    store.put(&k, &a, &Precondition::Create, &meta("alice", "")).await.unwrap();
    let listing = store.load().await.unwrap();
    // The file is touched and written again with the same bytes: the signature is new, the etag is not.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_doc(&fx, "environments/p/environment.json", &a.bytes);
    let delta = store.poll(&listing.marks).await.unwrap();
    assert_eq!(delta.changed.len(), 1);
    assert_eq!(store.history(&k, None, 10).await.unwrap().len(), 1);
}

fn invalid_bytes() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("not canonical", b"{\"format\":1,\"label\":\"A\"}".to_vec()),
        ("unknown member", b"{\n  \"extra\": 1,\n  \"format\": 1,\n  \"label\": \"A\"\n}\n".to_vec()),
        ("not JSON", b"label: A\n".to_vec()),
        ("null", b"{\n  \"format\": 1,\n  \"label\": null\n}\n".to_vec()),
        ("empty", Vec::new()),
    ]
}

#[tokio::test]
async fn an_outside_change_that_is_not_valid_stays_out_with_no_version() {
    for (why, bytes) in invalid_bytes() {
        let fx = Fixture::new();
        let store = fx.open();
        let k = key("environments/p");
        let a = env_doc("A");
        store.put(&k, &a, &Precondition::Create, &meta("alice", "")).await.unwrap();
        let listing = store.load().await.unwrap();
        write_doc(&fx, "environments/p/environment.json", &bytes);
        let delta = store.poll(&listing.marks).await.unwrap();
        // The store gives the bytes as stored: the catalog finds the errors (2.41).
        assert_eq!(delta.changed.len(), 1, "{why}");
        assert_eq!(delta.changed[0].bytes, bytes, "{why}");
        assert_eq!(delta.changed[0].etag, Etag::of(&bytes), "{why}");
        assert!(Document::from_stored(DocKind::Environment, &delta.changed[0].bytes, 1 << 20).is_err(), "{why}");
        // No version, and no event of a change.
        assert_eq!(store.history(&k, None, 10).await.unwrap().len(), 1, "{why}");
        assert!(!store.drain_events().iter().any(|e| matches!(e, StoreEvent::ExternalChange { .. })), "{why}");
        // The store writes no file of `environments/` for a change outside Pupitre.
        assert_eq!(std::fs::read(fx.docs().join("environments/p/environment.json")).unwrap(), bytes, "{why}");
        // A second poll does not read it again.
        assert!(store.poll(&delta.marks).await.unwrap().changed.is_empty(), "{why}");
        // The person fixes the file: now it gets a version.
        let fixed = env_doc("Fixed");
        write_doc(&fx, "environments/p/environment.json", &fixed.bytes);
        let delta = store.poll(&delta.marks).await.unwrap();
        assert_eq!(delta.changed[0].version, 2, "{why}");
        assert_eq!(store.history(&k, None, 1).await.unwrap()[0].kind, VersionKind::External, "{why}");
    }
}

#[tokio::test]
async fn an_invalid_document_that_is_new_has_version_0_and_a_write_over_it_needs_its_etag() {
    let fx = Fixture::new();
    let store = fx.open();
    let bytes = b"not a document".to_vec();
    write_doc(&fx, "environments/p/environment.json", &bytes);
    let listing = store.load().await.unwrap();
    assert_eq!(listing.docs.len(), 1);
    assert_eq!(listing.docs[0].version, 0);
    let k = key("environments/p");
    assert_eq!(store.get(&k).await.unwrap().bytes, bytes);
    assert!(store.history(&k, None, 10).await.unwrap().is_empty());
    let err = store.put(&k, &env_doc("A"), &Precondition::Create, &meta("a", "")).await.unwrap_err();
    assert_eq!(err, pupitre_store::StoreError::AlreadyExists);
    let saved = store.put(&k, &env_doc("A"), &Precondition::Match(Etag::of(&bytes)), &meta("a", "")).await.unwrap();
    assert_eq!(saved.version, 1);
}

#[tokio::test]
async fn a_file_above_the_document_limit_is_read_as_an_error_not_in_full() {
    let fx = Fixture::new();
    let store = fx.open();
    let big = vec![b' '; 300_000];
    write_doc(&fx, "environments/p/environment.json", &big);
    let listing = store.load().await.unwrap();
    // The bytes stop one byte above the limit, so the catalog refuses them for their size.
    assert_eq!(listing.docs[0].bytes.len(), 262_145);
    assert!(Document::from_stored(DocKind::Environment, &listing.docs[0].bytes, 262_144).is_err());
    assert_eq!(listing.docs[0].version, 0);
}

#[tokio::test]
async fn an_entry_that_is_not_of_the_layout_is_ignored_with_an_event_one_time() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    store.put(&k, &env_doc("A"), &Precondition::Create, &meta("a", "")).await.unwrap();
    store.drain_events();
    write_doc(&fx, "environments/p/notes.txt", b"hello");
    write_doc(&fx, "environments/Bad/environment.json", &env_doc("B").bytes);
    write_doc(&fx, "environments/p/hosts/Bad.json", &host_doc("host.example.invalid").bytes);
    write_doc(&fx, "environments/p/units/relay/other.json", b"{}");
    write_doc(&fx, "environments/loose-file", b"x");
    let mut marks = Marks::default();
    for _ in 0..3 {
        let delta = store.poll(&marks).await.unwrap();
        marks = delta.marks;
    }
    let paths: Vec<String> = store
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            StoreEvent::EntryIgnored { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "environments/Bad",
            "environments/loose-file",
            "environments/p/hosts/Bad.json",
            "environments/p/notes.txt",
            "environments/p/units/relay/other.json"
        ]
    );
    // Only the document of the layout is in the store.
    assert_eq!(store.load().await.unwrap().docs.len(), 1);
}

#[tokio::test]
async fn a_crash_is_not_taken_for_an_outside_change() {
    // The version 2 is in `.history`, and the document is the version 1. Then a person changes
    // the file to a third content before the next open: the third content is an outside change.
    let fx = Fixture::new();
    let k = key("environments/p");
    let (a, b, c) = (env_doc("A"), env_doc("B"), env_doc("C"));
    {
        let faults = pupitre_store::files::Faults::new();
        FilesStore::migrate(&fx.options()).unwrap();
        let store = FilesStore::open_with_faults(fx.options(), faults.clone()).unwrap();
        store.put(&k, &a, &Precondition::Create, &meta("a", "")).await.unwrap();
        faults.fail_once(pupitre_store::files::FaultPoint::AfterHistory, 5);
        assert!(store.put(&k, &b, &Precondition::Match(a.etag), &meta("a", "")).await.is_err());
    }
    write_doc(&fx, "environments/p/environment.json", &c.bytes);
    let store = fx.reopen();
    let got = store.get(&k).await.unwrap();
    assert_eq!((got.bytes, got.version), (c.bytes.clone(), 3));
    let kinds: Vec<_> = store.history(&k, None, 10).await.unwrap().iter().map(|v| (v.version, v.kind)).collect();
    assert_eq!(kinds, [(3, VersionKind::External), (2, VersionKind::Save), (1, VersionKind::Create)]);
}

#[tokio::test]
async fn the_history_keep_bound_applies_to_the_external_versions() {
    let fx = Fixture::with_history_keep(2);
    let store = fx.open();
    let k = key("environments/p");
    store.put(&k, &env_doc("A"), &Precondition::Create, &meta("a", "")).await.unwrap();
    let mut marks = store.load().await.unwrap().marks;
    for i in 0..4 {
        write_doc(&fx, "environments/p/environment.json", &env_doc(&format!("E{i}")).bytes);
        marks = store.poll(&marks).await.unwrap().marks;
    }
    let versions: Vec<u64> = store.history(&k, None, 10).await.unwrap().iter().map(|v| v.version).collect();
    assert_eq!(versions, [5, 4]);
}

fn history_meta(fx: &Fixture, key: &str, version: u64) -> std::path::PathBuf {
    fx.docs().join(".history").join(key).join(format!("{version:010}.meta.json"))
}

fn ignored_paths(events: &[StoreEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            StoreEvent::EntryIgnored { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

/// L10: a version record that is not valid does not stop the store. The store ignores the entry
/// with the event `store.entry_ignored`, and the other keys work.
#[tokio::test]
async fn a_bad_newest_version_record_does_not_stop_the_store() {
    let fx = Fixture::new();
    let (p, q) = (key("environments/p"), key("environments/q"));
    {
        let store = fx.open();
        store.put(&p, &env_doc("A"), &Precondition::Create, &meta("alice", "")).await.unwrap();
        store.put(&p, &env_doc("B"), &Precondition::Match(env_doc("A").etag), &meta("alice", "")).await.unwrap();
        store.put(&q, &env_doc("Q"), &Precondition::Create, &meta("alice", "")).await.unwrap();
    }
    std::fs::write(history_meta(&fx, "environments/p", 2), b"{ this is not a version record").unwrap();
    let store = fx.reopen();
    let events = store.drain_events();
    assert_eq!(ignored_paths(&events), vec![".history/environments/p/0000000002.meta.json".to_string()], "{events:?}");
    // The other key is not affected. The key with the bad record still reads.
    assert_eq!(store.get(&q).await.unwrap().version, 1);
    assert_eq!(store.get(&p).await.unwrap().etag, env_doc("B").etag);
    // The record that reads gives the state: the document of version 2 is an outside change over version 1.
    let versions = store.history(&p, None, 10).await.unwrap();
    assert_eq!(versions.iter().map(|v| v.version).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(versions[0].kind, VersionKind::External);
}

/// L10: a bad record of an older version leaves the history of the key without it, with an event.
#[tokio::test]
async fn a_bad_older_version_record_is_left_out_of_the_history() {
    let fx = Fixture::new();
    let p = key("environments/p");
    let store = fx.open();
    store.put(&p, &env_doc("A"), &Precondition::Create, &meta("alice", "")).await.unwrap();
    store.put(&p, &env_doc("B"), &Precondition::Match(env_doc("A").etag), &meta("alice", "")).await.unwrap();
    std::fs::write(history_meta(&fx, "environments/p", 1), b"[]").unwrap();
    store.drain_events();
    let versions = store.history(&p, None, 10).await.unwrap();
    assert_eq!(versions.iter().map(|v| v.version).collect::<Vec<_>>(), vec![2]);
    assert_eq!(ignored_paths(&store.drain_events()), vec![".history/environments/p/0000000001.meta.json".to_string()]);
    // The next write goes on from the version 2.
    let saved =
        store.put(&p, &env_doc("C"), &Precondition::Match(env_doc("B").etag), &meta("alice", "")).await.unwrap();
    assert_eq!(saved.version, 3);
}

/// L10: a key whose records are all bad does not stop the open. The file is seen as an outside change.
#[tokio::test]
async fn a_key_with_no_readable_record_does_not_stop_the_open() {
    let fx = Fixture::new();
    let p = key("environments/p");
    {
        let store = fx.open();
        store.put(&p, &env_doc("A"), &Precondition::Create, &meta("alice", "")).await.unwrap();
    }
    std::fs::write(history_meta(&fx, "environments/p", 1), b"").unwrap();
    let store = fx.reopen();
    assert_eq!(ignored_paths(&store.drain_events()), vec![".history/environments/p/0000000001.meta.json".to_string()]);
    assert_eq!(store.get(&p).await.unwrap().etag, env_doc("A").etag);
}
