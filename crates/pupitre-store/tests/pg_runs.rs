// SPDX-License-Identifier: MIT OR Apache-2.0
//! 2.27 and 2.28 on PostgreSQL: the runs, the output and the events. Each test needs the server
//! of `tests/ci/store-pg.sh`.
#![cfg(feature = "conformance")]

mod common;

use std::sync::Arc;

use common::{migrated, ready};
use pupitre_core::ids::{EnvId, HostId, PlaybookId, RunId, Subject};
use pupitre_core::json;
use pupitre_core::node::Node;
use pupitre_core::run::{RunKind, RunMode, RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;
use pupitre_store::postgres::PgStore;
use pupitre_store::{Store, StoreError};

fn record(n: u64, environment: &str) -> RunRecord {
    let mut random = [0u8; 10];
    random[2..].copy_from_slice(&n.to_be_bytes());
    RunRecord {
        id: RunId::from_parts(1_795_514_400_000 + n, random),
        kind: RunKind::Deploy,
        mode: RunMode::Check,
        environment: EnvId::parse(environment).expect("id"),
        node: Node::Environment(EnvId::parse(environment).expect("id")),
        hosts: vec![HostId::parse("host-a").expect("id")],
        playbook: PlaybookId::parse("deploy-relay").expect("id"),
        command: None,
        params: None,
        check_run: None,
        applied_by: None,
        message: None,
        subject: Subject::parse("alice").expect("subject"),
        created_at: UtcMillis(1_795_514_400_000 + n),
        started_at: None,
        finished_at: None,
        status: RunStatus::Queued,
        reason: None,
        snapshot: None,
        runner: None,
        exit_code: None,
        stats: None,
        cancel: None,
        output_truncated: false,
        events_truncated: false,
    }
}

/// 2.27: the count of the queue is exact with many writers at the same time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn twenty_creates_at_the_same_time_fill_a_queue_of_five() {
    let (_db, options) = migrated().await;
    let store = Arc::new(PgStore::open(options).await.expect("open"));
    let mut tasks = Vec::new();
    for n in 0..20 {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move { store.create_run(&record(n, "production"), 5, None).await }));
    }
    let (mut ok, mut full) = (0, 0);
    for task in tasks {
        match task.await.expect("the task") {
            Ok(()) => ok += 1,
            Err(StoreError::QueueFull) => full += 1,
            Err(other) => panic!("{other:?}"),
        }
    }
    assert_eq!((ok, full), (5, 15));
}

/// 2.28: the output is a list of chunks. A read crosses the borders of the chunks and gives
/// the bytes of the bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_read_of_the_output_crosses_chunks() {
    let (db, store) = ready().await;
    let run = record(1, "production");
    store.create_run(&run, 10, None).await.expect("create");
    let mut offset = 0u64;
    let parts: [&[u8]; 5] = [b"aaa", b"bbbb", b"c", b"dddddd", b"ee"];
    for part in parts {
        offset = store.append_output(&run.id, offset, part).await.expect("append");
    }
    assert_eq!(offset, 16);
    let admin = db.admin_session().await;
    let chunks: i64 = admin.query_one("SELECT count(*) FROM run_output", &[]).await.expect("count").get(0);
    assert_eq!(chunks, 5, "one chunk for each non-empty append");
    let all = b"aaabbbbcddddddee";
    for start in 0..=16usize {
        for max in [0usize, 1, 3, 7, 16, 100] {
            let got = store.read_output(&run.id, start as u64, max as u32).await.expect("read");
            let end = (start + max).min(all.len());
            assert_eq!(got.bytes, all[start..end].to_vec(), "offset {start}, max {max}");
            assert_eq!(got.length, 16);
        }
    }
}

/// 2.28: eight appends at one offset at the same time: one wins and the others get `Conflict`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn appends_at_one_offset_give_one_winner() {
    let (_db, options) = migrated().await;
    let store = Arc::new(PgStore::open(options).await.expect("open"));
    let run = record(1, "production");
    store.create_run(&run, 10, None).await.expect("create");
    let mut tasks = Vec::new();
    for n in 0..8u8 {
        let (store, id) = (Arc::clone(&store), run.id);
        tasks.push(tokio::spawn(async move {
            let out = store.append_output(&id, 0, &[b'a' + n; 4]).await;
            let ev = store.append_events(&id, 1, &[vec![b'e' + n]]).await;
            (out, ev)
        }));
    }
    let (mut out_ok, mut ev_ok) = (0, 0);
    for task in tasks {
        let (out, ev) = task.await.expect("the task");
        match out {
            Ok(4) => out_ok += 1,
            Err(StoreError::Conflict { .. }) => {}
            other => panic!("{other:?}"),
        }
        match ev {
            Ok(()) => ev_ok += 1,
            Err(StoreError::Conflict { .. }) => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!((out_ok, ev_ok), (1, 1));
    assert_eq!(store.read_output(&run.id, 0, 100).await.expect("read").length, 4);
    assert_eq!(store.read_events(&run.id, 0, 100).await.expect("read").len(), 1);
}

/// 2.27: the record is stored in the canonical form, and the columns repeat its members.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_record_is_canonical_and_the_columns_follow_it() {
    let (db, store) = ready().await;
    let run = record(1, "production");
    store.create_run(&run, 10, None).await.expect("create");
    let admin = db.admin_session().await;
    let row = admin.query_one("SELECT id, environment, status, record FROM runs", &[]).await.expect("row");
    assert_eq!(row.get::<_, String>(0), run.id.to_string());
    assert_eq!(row.get::<_, String>(1), "production");
    assert_eq!(row.get::<_, String>(2), "queued");
    assert_eq!(row.get::<_, Vec<u8>>(3), json::to_canonical(&run.to_json()));
    let mut running = run.clone();
    running.status = RunStatus::Running;
    running.started_at = Some(UtcMillis(1_795_514_401_000));
    store.update_run(RunStatus::Queued, &running).await.expect("update");
    let row = admin.query_one("SELECT status, record FROM runs", &[]).await.expect("row");
    assert_eq!(row.get::<_, String>(0), "running");
    assert_eq!(row.get::<_, Vec<u8>>(1), json::to_canonical(&running.to_json()));
}

/// 2.27: `prune_runs` removes the output and the events with the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn prune_removes_the_rows_of_output_and_events() {
    let (db, store) = ready().await;
    let run = record(1, "production");
    store.create_run(&run, 10, None).await.expect("create");
    store.append_output(&run.id, 0, b"output").await.expect("append");
    store.append_events(&run.id, 1, &[b"{}".to_vec()]).await.expect("append");
    let mut done = run.clone();
    done.status = RunStatus::Canceled;
    store.update_run(RunStatus::Queued, &done).await.expect("cancel");
    assert_eq!(store.prune_runs(0).await.expect("prune"), vec![run.id]);
    let admin = db.admin_session().await;
    for table in ["runs", "run_output", "run_events"] {
        let n: i64 = admin.query_one(&format!("SELECT count(*) FROM {table}"), &[]).await.expect("count").get(0);
        assert_eq!(n, 0, "{table}");
    }
}
