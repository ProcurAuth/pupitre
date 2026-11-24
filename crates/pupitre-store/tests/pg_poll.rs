// SPDX-License-Identifier: MIT OR Apache-2.0
//! 2.38 on PostgreSQL: the poll by change number. Each test needs the server of
//! `tests/ci/store-pg.sh`.
#![cfg(feature = "conformance")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{doc, env_key, meta, ready};
use pupitre_store::{Precondition, Store, StoreEvent};

/// 2.38: writer A takes its change number first and commits last, and writer B commits in between.
/// A poll that begins while A is open must wait for A, then see both documents. Without the
/// barrier the poll returns at once with B only, and the marks then skip A for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_poll_waits_for_an_out_of_order_commit_and_sees_both_documents() {
    let (db, store) = ready().await;
    let store = Arc::new(store);
    let listing = store.load().await.expect("load");
    // Writer A: the real write path, held after its change number and before its commit.
    let hold = store.faults().hold_before_commit();
    let slow_doc = doc("Slow");
    let writer_a = {
        let store = Arc::clone(&store);
        let slow_doc = slow_doc.clone();
        tokio::spawn(async move {
            store.put(&env_key("slow"), &slow_doc, &Precondition::Create, &meta("alice")).await.expect("create A")
        })
    };
    hold.reached().await;
    // Writer B: it joins the barrier in shared mode, takes a higher number and commits.
    let fast_doc = doc("Fast");
    store.put(&env_key("fast"), &fast_doc, &Precondition::Create, &meta("bob")).await.expect("create B");
    // The poll starts now. It must wait for A.
    let poller = {
        let store = Arc::clone(&store);
        let marks = listing.marks.clone();
        tokio::spawn(async move { store.poll(&marks).await.expect("poll") })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!poller.is_finished(), "the poll must wait for the writer that holds a change number");
    hold.release();
    writer_a.await.expect("join A");
    let delta = poller.await.expect("join poll");
    let keys: Vec<String> = delta.changed.iter().map(|d| d.key.text()).collect();
    assert_eq!(keys, vec!["environments/fast", "environments/slow"], "the poll sees both documents");
    // The change numbers have the order that breaks a poll with no barrier: A < B.
    let admin = db.admin_session().await;
    let rows = admin.query("SELECT key FROM documents ORDER BY change_seq", &[]).await.expect("rows");
    let order: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(order, vec!["environments/slow", "environments/fast"], "A took the lower number and committed last");
    // The next poll has nothing: the marks are above both numbers.
    let next = store.poll(&delta.marks).await.expect("poll");
    assert!(next.changed.is_empty() && next.removed.is_empty());
}

/// 2.38: the load takes the barrier too. A load that begins while a writer is open waits for it,
/// so the marks of the load never skip a change that commits later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_load_waits_for_an_open_writer_so_its_marks_skip_nothing() {
    let (_db, store) = ready().await;
    let store = Arc::new(store);
    let hold = store.faults().hold_before_commit();
    let writer_a = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store.put(&env_key("slow"), &doc("Slow"), &Precondition::Create, &meta("alice")).await.expect("create A")
        })
    };
    hold.reached().await;
    store.put(&env_key("fast"), &doc("Fast"), &Precondition::Create, &meta("bob")).await.expect("create B");
    let loader = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.load().await.expect("load") })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!loader.is_finished(), "the load must wait for the writer that holds a change number");
    hold.release();
    writer_a.await.expect("join A");
    let listing = loader.await.expect("join load");
    assert_eq!(listing.docs.len(), 2);
    let next = store.poll(&listing.marks).await.expect("poll");
    assert!(next.changed.is_empty(), "nothing is above the marks of the load");
}

/// 2.38: the poll gives the rows above the marks and no more. A poll with no change is empty at once,
/// and a new write comes with the next poll.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_poll_gives_the_rows_above_the_marks_and_no_more() {
    let (_db, store) = ready().await;
    let listing = store.load().await.expect("load");
    store.put(&env_key("production"), &doc("One"), &Precondition::Create, &meta("alice")).await.expect("create");
    let first = store.poll(&listing.marks).await.expect("poll");
    assert_eq!(first.changed.len(), 1);
    let again = store.poll(&first.marks).await.expect("poll");
    assert!(again.changed.is_empty() && again.removed.is_empty(), "nothing again");
    store.put(&env_key("staging"), &doc("Two"), &Precondition::Create, &meta("alice")).await.expect("create");
    let new = store.poll(&again.marks).await.expect("poll");
    assert_eq!(new.changed.len(), 1);
    assert_eq!(new.changed[0].key, env_key("staging"));
}

/// 2.3: a row whose key is not a key of the six forms is not a document. The store ignores it
/// and keeps the event `store.entry_ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_row_with_a_bad_key_is_ignored_with_an_event() {
    let (db, store) = ready().await;
    let admin = db.admin_session().await;
    let bytes = doc("Odd").bytes;
    admin
        .execute(
            "INSERT INTO documents (key, kind, bytes, etag, version, change_seq, updated_at) \
             VALUES ('environments/Bad_Id', 'environment', $1, $2, 1, nextval('pupitre_change_seq'), now())",
            &[&bytes, &doc("Odd").etag.as_bytes().as_slice()],
        )
        .await
        .expect("insert");
    store.put(&env_key("production"), &doc("One"), &Precondition::Create, &meta("alice")).await.expect("create");
    let listing = store.load().await.expect("load");
    assert_eq!(listing.docs.len(), 1);
    assert_eq!(listing.docs[0].key, env_key("production"));
    let events = store.drain_events();
    assert!(
        events.iter().any(|e| matches!(e, StoreEvent::EntryIgnored { path, .. } if path == "environments/Bad_Id")),
        "{events:?}"
    );
    assert!(store.drain_events().is_empty(), "the events are taken one time");
}

/// The marks of a poll are the ones of the backend. Marks of another backend, or none, are not a valid state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_poll_with_empty_marks_reads_everything_and_a_load_gives_the_same_state() {
    let (_db, store) = ready().await;
    for (n, id) in ["a", "b", "c"].iter().enumerate() {
        store
            .put(&env_key(id), &doc(&format!("Env {n}")), &Precondition::Create, &meta("alice"))
            .await
            .expect("create");
    }
    let delta = store.poll(&pupitre_store::Marks::default()).await.expect("poll");
    assert_eq!(delta.changed.len(), 3);
    let listing = store.load().await.expect("load");
    assert_eq!(listing.docs, delta.changed);
    assert_eq!(listing.marks, delta.marks);
    let keys: Vec<String> = listing.docs.iter().map(|d| d.key.text()).collect();
    assert_eq!(keys, vec!["environments/a", "environments/b", "environments/c"], "the byte order of the keys");
}
