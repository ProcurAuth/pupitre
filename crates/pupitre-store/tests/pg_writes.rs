// SPDX-License-Identifier: MIT OR Apache-2.0
//! 2.37 on PostgreSQL: the writes of the documents. Each test needs the server of
//! `tests/ci/store-pg.sh`.
#![cfg(feature = "conformance")]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{doc, env_key, meta, migrated, ready, unavailable};
use pupitre_store::postgres::PgStore;
use pupitre_store::{Precondition, Store, StoreError};

/// 2.37: two writers in two connections. One transaction locks the row, so one wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn eight_writers_with_one_base_etag_give_one_winner() {
    let (_db, options) = migrated().await;
    let mut handles = Vec::new();
    for _ in 0..8 {
        handles.push(Arc::new(PgStore::open(options.clone()).await.expect("open")));
    }
    let key = env_key("production");
    let base = doc("Base");
    handles[0].put(&key, &base, &Precondition::Create, &meta("alice")).await.expect("create");
    let mut tasks = Vec::new();
    for (n, store) in handles.iter().enumerate() {
        let (store, key, base_etag) = (Arc::clone(store), key.clone(), base.etag);
        tasks.push(tokio::spawn(async move {
            let next = doc(&format!("Writer {n}"));
            let result = store.put(&key, &next, &Precondition::Match(base_etag), &meta("writer")).await;
            (next.etag, result)
        }));
    }
    let mut winners = Vec::new();
    let mut losers = Vec::new();
    for task in tasks {
        let (etag, result) = task.await.expect("the task");
        match result {
            Ok(saved) => winners.push((etag, saved)),
            Err(e) => losers.push(e),
        }
    }
    assert_eq!(winners.len(), 1, "exactly one writer wins");
    assert_eq!(winners[0].1.version, 2);
    for loser in losers {
        assert_eq!(loser, StoreError::Conflict { current: Some(winners[0].0) });
    }
    let history = handles[0].history(&key, None, 10).await.expect("history");
    assert_eq!(history.len(), 2, "no lost update and no gap");
    assert_eq!(handles[0].get(&key).await.expect("get").etag, winners[0].0);
}

/// 2.20: eight creates of one new key at the same time: one wins, the others get `AlreadyExists`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn eight_creates_of_one_key_give_one_winner() {
    let (_db, options) = migrated().await;
    let mut tasks = Vec::new();
    for n in 0..8 {
        let store = PgStore::open(options.clone()).await.expect("open");
        tasks.push(tokio::spawn(async move {
            store.put(&env_key("production"), &doc(&format!("Creator {n}")), &Precondition::Create, &meta("a")).await
        }));
    }
    let mut ok = 0;
    for task in tasks {
        match task.await.expect("the task") {
            Ok(saved) => {
                assert_eq!(saved.version, 1);
                ok += 1;
            }
            Err(e) => assert_eq!(e, StoreError::AlreadyExists),
        }
    }
    assert_eq!(ok, 1);
}

/// 2.37: a statement has the timeout `statement_timeout`. A write that waits for a row lock
/// longer than the timeout fails with `Unavailable` and leaves nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_statement_has_a_timeout_and_a_failed_write_leaves_nothing() {
    let (db, mut options) = migrated().await;
    options.statement_timeout = Duration::from_millis(800);
    let store = PgStore::open(options).await.expect("open");
    let key = env_key("production");
    let v1 = doc("One");
    store.put(&key, &v1, &Precondition::Create, &meta("alice")).await.expect("create");
    // Another session holds the lock of the row.
    let holder = db.admin_session().await;
    holder
        .batch_execute("BEGIN; SELECT 1 FROM documents WHERE key = 'environments/production' FOR UPDATE")
        .await
        .expect("lock");
    let started = Instant::now();
    let text = unavailable(store.put(&key, &doc("Two"), &Precondition::Match(v1.etag), &meta("bob")).await);
    assert!(started.elapsed() < Duration::from_secs(5), "the timeout ends the wait");
    assert!(text.contains("57014"), "the SQLSTATE of a canceled statement: {text}");
    holder.batch_execute("ROLLBACK").await.expect("release");
    let current = store.get(&key).await.expect("get");
    assert_eq!((current.version, current.etag), (1, v1.etag));
    assert_eq!(store.history(&key, None, 10).await.expect("history").len(), 1, "no version without a document");
    // After the release, the same write works.
    assert_eq!(
        store.put(&key, &doc("Two"), &Precondition::Match(v1.etag), &meta("bob")).await.expect("save").version,
        2
    );
}

/// 2.37: a pool of `max_connections` serves all calls. A call that waits longer than
/// `connect_timeout` for a connection fails with `Unavailable`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_call_that_waits_for_a_connection_fails_after_the_connect_timeout() {
    let (db, mut options) = migrated().await;
    options.max_connections = 2;
    options.connect_timeout = Duration::from_millis(500);
    options.statement_timeout = Duration::from_secs(6);
    let store = Arc::new(PgStore::open(options).await.expect("open"));
    let key = env_key("production");
    let v1 = doc("One");
    store.put(&key, &v1, &Precondition::Create, &meta("alice")).await.expect("create");
    let holder = db.admin_session().await;
    holder
        .batch_execute("BEGIN; SELECT 1 FROM documents WHERE key = 'environments/production' FOR UPDATE")
        .await
        .expect("lock");
    // Two writes take the two connections and wait for the lock.
    let mut blocked = Vec::new();
    for n in 0..2 {
        let (store, key) = (Arc::clone(&store), key.clone());
        blocked.push(tokio::spawn(async move {
            store.put(&key, &doc(&format!("Blocked {n}")), &Precondition::Match(v1.etag), &meta("bob")).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    let text = unavailable(store.get(&key).await);
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(400) && waited < Duration::from_secs(3), "waited {waited:?}");
    assert!(text.contains("timeout"), "{text}");
    holder.batch_execute("ROLLBACK").await.expect("release");
    let results: Vec<_> = futures_join(blocked).await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "one of the two writes wins");
    assert_eq!(store.get(&key).await.expect("get").version, 2, "the pool works again");
}

async fn futures_join<T: Send + 'static>(tasks: Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.expect("the task"));
    }
    out
}

/// 2.37: one transaction writes the version and the document. A crash after the version
/// leaves neither. A delete keeps the row of the key with no bytes and no etag, so a poll sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_version_and_the_document_are_one_transaction() {
    let (db, store) = ready().await;
    let key = env_key("production");
    let v1 = doc("One");
    store.put(&key, &v1, &Precondition::Create, &meta("alice")).await.expect("create");
    store.faults().crash_after_version();
    let crashed = store.put(&key, &doc("Two"), &Precondition::Match(v1.etag), &meta("bob")).await;
    assert!(crashed.is_err());
    let admin = db.admin_session().await;
    let versions: i64 = admin.query_one("SELECT count(*) FROM document_versions", &[]).await.expect("count").get(0);
    assert_eq!(versions, 1, "the version of the failed write is not in the table");
    // The fault fires one time: the next write works and gets the version 2.
    assert_eq!(
        store.put(&key, &doc("Two"), &Precondition::Match(v1.etag), &meta("bob")).await.expect("save").version,
        2
    );
    let current = store.get(&key).await.expect("get");
    store.delete(&key, &current.etag, &meta("carol")).await.expect("delete");
    let row = admin
        .query_one(
            "SELECT bytes IS NULL, etag IS NULL, version FROM documents WHERE key = 'environments/production'",
            &[],
        )
        .await
        .expect("the row stays");
    assert!(row.get::<_, bool>(0) && row.get::<_, bool>(1));
    assert_eq!(row.get::<_, i64>(2), 3);
    assert_eq!(store.get(&key).await, Err(StoreError::NotFound));
}

/// 2.18: the store keeps the newest `history_keep` versions in the table, and the key keeps its numbers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_table_of_versions_keeps_history_keep_rows() {
    let (db, mut options) = migrated().await;
    options.history_keep = 4;
    let store = PgStore::open(options).await.expect("open");
    let key = env_key("production");
    let mut previous = None;
    for n in 0..12 {
        let next = doc(&format!("Write {n}"));
        let pre = previous.map_or(Precondition::Create, Precondition::Match);
        store.put(&key, &next, &pre, &meta("alice")).await.expect("write");
        previous = Some(next.etag);
    }
    let admin = db.admin_session().await;
    let rows = admin.query("SELECT version FROM document_versions ORDER BY version", &[]).await.expect("rows");
    let versions: Vec<i64> = rows.iter().map(|r| r.get(0)).collect();
    assert_eq!(versions, vec![9, 10, 11, 12]);
    let current: i64 = admin.query_one("SELECT version FROM documents", &[]).await.expect("row").get(0);
    assert_eq!(current, 12);
}

/// The size of a document that the store keeps is not bounded by the store: the server checks
/// `limits.max_document_bytes` before the write (2.42). The store keeps 256 KiB of any bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_large_document_round_trips() {
    let (_db, store) = ready().await;
    let label = "x".repeat(250_000);
    let big = doc(&label);
    assert!(big.bytes.len() > 250_000);
    store.put(&env_key("production"), &big, &Precondition::Create, &meta("alice")).await.expect("create");
    let got = store.get(&env_key("production")).await.expect("get");
    assert_eq!(got.bytes, big.bytes);
}
