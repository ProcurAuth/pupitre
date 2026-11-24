// SPDX-License-Identifier: MIT OR Apache-2.0
//! The cases S14 to S18 of 2.43: the runs, their output and events, and the leases.

use std::time::Duration;

use pupitre_core::ids::RunId;
use pupitre_core::run::{RunMode, RunStatus};

use super::harness::{FreshOptions, Harness};
use super::support::{env, run_id, run_record, with_status};
use crate::store::{AnyStore, Store};
use crate::types::{LeaseName, LeaseOutcome, LeaseRenewal, RunFilter, StoreError};

async fn fresh<H: Harness>(h: &H) -> AnyStore {
    h.fresh(FreshOptions::default()).await.expect("a fresh store")
}

/// S14: `create_run` above `max_queued` gives `QueueFull`. Two applies that consume one check:
/// exactly one succeeds.
pub async fn s14_queue_and_check_use<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let (r1, r2, r3) = (
        run_record("production", RunMode::Check),
        run_record("production", RunMode::Check),
        run_record("staging", RunMode::Check),
    );
    store.create_run(&r1, 2, None).await.expect("first");
    store.create_run(&r2, 2, None).await.expect("second");
    assert_eq!(store.create_run(&r3, 2, None).await, Err(StoreError::QueueFull));
    assert_eq!(store.get_run(&r3.id).await, Err(StoreError::NotFound), "a refused run is not stored");
    assert_eq!(store.create_run(&r1, 5, None).await, Err(StoreError::AlreadyExists));
    // A run that leaves the queue makes room.
    store.update_run(RunStatus::Queued, &with_status(&r1, RunStatus::Running)).await.expect("start");
    store.create_run(&r3, 2, None).await.expect("room");
    // One check gives one apply.
    let check = run_record("production", RunMode::Check);
    store.create_run(&check, 100, None).await.expect("check");
    let running = with_status(&check, RunStatus::Running);
    store.update_run(RunStatus::Queued, &running).await.expect("start");
    store.update_run(RunStatus::Running, &with_status(&running, RunStatus::Passed)).await.expect("end");
    let (a1, a2) = (run_record("production", RunMode::Apply), run_record("production", RunMode::Apply));
    let (x, y) = tokio::join!(store.create_run(&a1, 100, Some(&check.id)), store.create_run(&a2, 100, Some(&check.id)));
    assert_eq!(usize::from(x.is_ok()) + usize::from(y.is_ok()), 1, "{x:?} {y:?}");
    let (winner, loser, error) = if x.is_ok() { (&a1, &a2, y) } else { (&a2, &a1, x) };
    assert_eq!(error, Err(StoreError::CheckUsed));
    assert_eq!(store.get_run(&check.id).await.expect("check").applied_by, Some(winner.id));
    assert_eq!(store.get_run(&loser.id).await, Err(StoreError::NotFound), "the refused apply is not stored");
    // A third apply is refused too. An unknown check is not found.
    let a3 = run_record("production", RunMode::Apply);
    assert_eq!(store.create_run(&a3, 100, Some(&check.id)).await, Err(StoreError::CheckUsed));
    assert_eq!(store.create_run(&a3, 100, Some(&run_id())).await, Err(StoreError::NotFound));
    // A record must have the status `queued`.
    let started = with_status(&run_record("production", RunMode::Check), RunStatus::Running);
    assert!(matches!(store.create_run(&started, 100, None).await, Err(StoreError::BadTransition { .. })));
    // The list: newest first, a filter, a page, and the queue order for the runner.
    let all = store.list_runs(&RunFilter::default(), None, 100).await.expect("list");
    assert!(all.windows(2).all(|w| w[0].id > w[1].id), "newest first");
    let staging = RunFilter { environment: Some(env("staging")), status: None };
    assert_eq!(store.list_runs(&staging, None, 100).await.expect("list").len(), 1);
    let passed = RunFilter { environment: None, status: Some(RunStatus::Passed) };
    assert_eq!(store.list_runs(&passed, None, 100).await.expect("list")[0].id, check.id);
    let page = store.list_runs(&RunFilter::default(), Some(&all[1].id), 2).await.expect("list");
    assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), vec![all[2].id, all[3].id]);
    // `list_runs_page` leaves out the first `skip` runs and gives the same runs as a longer `list_runs` (L8).
    let skipped = store.list_runs_page(&RunFilter::default(), None, 2, 2).await.expect("list");
    assert_eq!(skipped.iter().map(|r| r.id).collect::<Vec<_>>(), vec![all[2].id, all[3].id]);
    let after = store.list_runs_page(&RunFilter::default(), Some(&all[0].id), 1, 100).await.expect("list");
    assert_eq!(after.iter().map(|r| r.id).collect::<Vec<_>>(), all[2..].iter().map(|r| r.id).collect::<Vec<_>>());
    assert!(store.list_runs_page(&RunFilter::default(), None, 100, 5).await.expect("list").is_empty());
    let one = store.list_runs_page(&passed, None, 0, 100).await.expect("list");
    assert_eq!(one.iter().map(|r| r.id).collect::<Vec<_>>(), vec![check.id]);
    let queued = store.next_queued(&[], 100).await.expect("queue");
    assert!(queued.windows(2).all(|w| w[0].id < w[1].id), "oldest first");
    assert!(queued.iter().all(|r| r.status == RunStatus::Queued));
    let skipping = store.next_queued(&[env("staging")], 100).await.expect("queue");
    assert!(skipping.iter().all(|r| r.environment.as_str() != "staging"));
    assert_eq!(store.next_queued(&[], 1).await.expect("queue").len(), 1);
}

/// S15: `update_run` with a wrong expected status gives `Conflict`. A transition outside 4.39
/// is refused.
pub async fn s15_update_run_transitions<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let queued = run_record("production", RunMode::Check);
    store.create_run(&queued, 10, None).await.expect("create");
    let running = with_status(&queued, RunStatus::Running);
    // The stored status is `queued`, so an update that expects `running` is a conflict.
    assert_eq!(
        store.update_run(RunStatus::Running, &with_status(&queued, RunStatus::Passed)).await,
        Err(StoreError::Conflict { current: None })
    );
    // A transition outside 4.39.
    for to in [RunStatus::Passed, RunStatus::Failed, RunStatus::Lost] {
        let refused = store.update_run(RunStatus::Queued, &with_status(&queued, to)).await;
        assert_eq!(refused, Err(StoreError::BadTransition { from: RunStatus::Queued, to }));
    }
    assert_eq!(store.get_run(&queued.id).await.expect("get").status, RunStatus::Queued, "nothing changed");
    store.update_run(RunStatus::Queued, &running).await.expect("start");
    assert_eq!(store.get_run(&queued.id).await.expect("get"), running);
    // Two updates with the same expected status: the second is a conflict.
    let again = store.update_run(RunStatus::Queued, &running).await;
    assert_eq!(again, Err(StoreError::Conflict { current: None }));
    assert_eq!(
        store.update_run(RunStatus::Running, &with_status(&running, RunStatus::Queued)).await,
        Err(StoreError::BadTransition { from: RunStatus::Running, to: RunStatus::Queued })
    );
    // The record changes with the same status.
    let mut note = running.clone();
    note.output_truncated = true;
    store.update_run(RunStatus::Running, &note).await.expect("same status");
    assert!(store.get_run(&queued.id).await.expect("get").output_truncated);
    let passed = with_status(&note, RunStatus::Passed);
    store.update_run(RunStatus::Running, &passed).await.expect("end");
    // A final status never changes.
    for to in [RunStatus::Running, RunStatus::Failed, RunStatus::Canceled, RunStatus::Queued] {
        let refused = store.update_run(RunStatus::Passed, &with_status(&passed, to)).await;
        assert_eq!(refused, Err(StoreError::BadTransition { from: RunStatus::Passed, to }));
    }
    // A queued run can be canceled. An unknown run is not found.
    let other = run_record("production", RunMode::Check);
    store.create_run(&other, 10, None).await.expect("create");
    store.update_run(RunStatus::Queued, &with_status(&other, RunStatus::Canceled)).await.expect("cancel");
    let unknown = run_record("production", RunMode::Check);
    assert_eq!(store.update_run(RunStatus::Queued, &unknown).await, Err(StoreError::NotFound));
}

/// S16: `append_output` and `append_events` with a wrong offset or number give `Conflict` and
/// add nothing.
pub async fn s16_output_and_events<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let run = run_record("production", RunMode::Check);
    store.create_run(&run, 10, None).await.expect("create");
    let id = run.id;
    assert_eq!(store.append_output(&id, 0, b"abc").await, Ok(3));
    assert_eq!(store.append_output(&id, 0, b"zzz").await, Err(StoreError::Conflict { current: None }));
    assert_eq!(store.append_output(&id, 9, b"zzz").await, Err(StoreError::Conflict { current: None }));
    assert_eq!(store.append_output(&id, 3, b"def").await, Ok(6));
    assert_eq!(store.append_output(&id, 6, b"").await, Ok(6));
    assert_eq!(store.append_output(&id, 6, b"g\x00\xff").await, Ok(9));
    let all = store.read_output(&id, 0, 100).await.expect("read");
    assert_eq!((all.bytes.as_slice(), all.length), (&b"abcdefg\x00\xff"[..], 9), "a wrong offset added nothing");
    let middle = store.read_output(&id, 2, 3).await.expect("read");
    assert_eq!((middle.bytes.as_slice(), middle.length), (&b"cde"[..], 9));
    let end = store.read_output(&id, 9, 10).await.expect("read");
    assert_eq!((end.bytes.len(), end.length), (0, 9));
    let past = store.read_output(&id, 50, 10).await.expect("read");
    assert_eq!((past.bytes.len(), past.length), (0, 9));
    assert_eq!(store.read_output(&id, 0, 0).await.expect("read").bytes.len(), 0);
    // The events: the next number only.
    let (a, b, c) = (b"{\"n\":1}".to_vec(), b"{\"n\":2}".to_vec(), b"{\"n\":3}".to_vec());
    store.append_events(&id, 1, &[a.clone(), b.clone()]).await.expect("events");
    assert_eq!(
        store.append_events(&id, 1, std::slice::from_ref(&c)).await,
        Err(StoreError::Conflict { current: None })
    );
    assert_eq!(
        store.append_events(&id, 4, std::slice::from_ref(&c)).await,
        Err(StoreError::Conflict { current: None })
    );
    store.append_events(&id, 3, std::slice::from_ref(&c)).await.expect("events");
    let lines = store.read_events(&id, 0, 10).await.expect("read");
    assert_eq!(lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(lines.iter().map(|l| l.bytes.clone()).collect::<Vec<_>>(), vec![a, b.clone(), c]);
    let after = store.read_events(&id, 1, 1).await.expect("read");
    assert_eq!((after.len(), after[0].seq, after[0].bytes.clone()), (1, 2, b));
    assert!(store.read_events(&id, 3, 10).await.expect("read").is_empty());
    // An unknown run is not found.
    let unknown = run_id();
    assert_eq!(store.append_output(&unknown, 0, b"x").await, Err(StoreError::NotFound));
    assert_eq!(store.append_events(&unknown, 1, &[b"x".to_vec()]).await, Err(StoreError::NotFound));
    assert_eq!(store.read_output(&unknown, 0, 10).await, Err(StoreError::NotFound));
    assert_eq!(store.read_events(&unknown, 0, 10).await, Err(StoreError::NotFound));
}

/// S17: a lease. A first holder acquires. A second holder gets the holder. The first holder
/// renews. The end time passes. The second holder acquires. The renew of the first holder gives
/// `Lost`.
pub async fn s17_leases<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let name = LeaseName::Runner;
    let ttl = Duration::from_millis(600);
    let LeaseOutcome::Acquired { until: first } = store.acquire_lease(&name, "node-a", ttl).await.expect("acquire")
    else {
        panic!("the first holder acquires");
    };
    match store.acquire_lease(&name, "node-b", ttl).await.expect("acquire") {
        LeaseOutcome::HeldBy { holder, until } => assert_eq!((holder.as_str(), until), ("node-a", first)),
        other => panic!("the second holder must get the holder: {other:?}"),
    }
    assert_eq!(store.renew_lease(&name, "node-b", ttl).await, Ok(LeaseRenewal::Lost), "a renew needs the holder");
    // The end time has a resolution of one millisecond: wait so that the new end time is later.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let LeaseRenewal::Renewed { until: renewed } = store.renew_lease(&name, "node-a", ttl).await.expect("renew") else {
        panic!("the holder renews");
    };
    assert!(renewed > first, "the end time moves");
    // Another name is another lease.
    let env_lease = LeaseName::Environment(env("production"));
    assert!(matches!(store.acquire_lease(&env_lease, "node-b", ttl).await, Ok(LeaseOutcome::Acquired { .. })));
    // A release by another holder changes nothing.
    store.release_lease(&name, "node-b").await.expect("release");
    assert!(matches!(store.acquire_lease(&name, "node-b", ttl).await, Ok(LeaseOutcome::HeldBy { .. })));
    // The end time passes: the second holder acquires, and the first holder has lost it.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(matches!(store.acquire_lease(&name, "node-b", ttl).await, Ok(LeaseOutcome::Acquired { .. })));
    assert_eq!(store.renew_lease(&name, "node-a", ttl).await, Ok(LeaseRenewal::Lost));
    // A renew after the end time gives `Lost`, also for the last holder.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(store.renew_lease(&name, "node-b", ttl).await, Ok(LeaseRenewal::Lost));
    // A release frees the lease at once.
    assert!(matches!(
        store.acquire_lease(&name, "node-a", Duration::from_secs(60)).await,
        Ok(LeaseOutcome::Acquired { .. })
    ));
    store.release_lease(&name, "node-a").await.expect("release");
    assert!(matches!(
        store.acquire_lease(&name, "node-b", Duration::from_secs(60)).await,
        Ok(LeaseOutcome::Acquired { .. })
    ));
}

/// S18: `prune_runs` removes the oldest final runs with their output. It never removes a run
/// with the status `queued` or `running`.
pub async fn s18_prune_runs<H: Harness>(h: &H) {
    let store = fresh(h).await;
    let mut finals: Vec<RunId> = Vec::new();
    for status in [RunStatus::Passed, RunStatus::Failed, RunStatus::Canceled] {
        let run = run_record("production", RunMode::Check);
        store.create_run(&run, 100, None).await.expect("create");
        store.append_output(&run.id, 0, b"output").await.expect("output");
        store.append_events(&run.id, 1, &[b"{}".to_vec()]).await.expect("events");
        if status == RunStatus::Canceled {
            store.update_run(RunStatus::Queued, &with_status(&run, status)).await.expect("cancel");
        } else {
            let running = with_status(&run, RunStatus::Running);
            store.update_run(RunStatus::Queued, &running).await.expect("start");
            store.update_run(RunStatus::Running, &with_status(&running, status)).await.expect("end");
        }
        finals.push(run.id);
    }
    let running = run_record("production", RunMode::Apply);
    store.create_run(&running, 100, None).await.expect("create");
    store.update_run(RunStatus::Queued, &with_status(&running, RunStatus::Running)).await.expect("start");
    store.append_output(&running.id, 0, b"live").await.expect("output");
    let queued = run_record("production", RunMode::Apply);
    store.create_run(&queued, 100, None).await.expect("create");
    // Keep 5: nothing goes.
    assert!(store.prune_runs(5).await.expect("prune").is_empty());
    // Keep 1: the two oldest final runs go, oldest first, with their output and events.
    let removed = store.prune_runs(1).await.expect("prune");
    assert_eq!(removed, vec![finals[0], finals[1]]);
    for id in &removed {
        assert_eq!(store.get_run(id).await, Err(StoreError::NotFound));
        assert_eq!(store.read_output(id, 0, 10).await, Err(StoreError::NotFound));
        assert_eq!(store.read_events(id, 0, 10).await, Err(StoreError::NotFound));
    }
    assert_eq!(store.read_output(&finals[2], 0, 10).await.expect("read").bytes, b"output");
    // Keep 0: the last final run goes. A queued run and a running run stay.
    assert_eq!(store.prune_runs(0).await.expect("prune"), vec![finals[2]]);
    assert!(store.prune_runs(0).await.expect("prune").is_empty());
    assert_eq!(store.get_run(&running.id).await.expect("running").status, RunStatus::Running);
    assert_eq!(store.get_run(&queued.id).await.expect("queued").status, RunStatus::Queued);
    assert_eq!(store.read_output(&running.id, 0, 10).await.expect("read").bytes, b"live");
    let left = store.list_runs(&RunFilter::default(), None, 100).await.expect("list");
    assert_eq!(left.len(), 2);
}
