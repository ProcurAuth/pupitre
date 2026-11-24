// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the leases (4.7, 4.8), the parallel runs (4.6) and the loss of the lease `runner`.

mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use pupitre_core::run::{RunKind, RunMode, RunReason, RunStatus};
use pupitre_runner::RunnerError;
use pupitre_runner::process::StopSignal;
use pupitre_store::types::LeaseName;
use support::fake_process::Script;
use support::fixture;
use support::harness::{EXIT_CODE, H, forever, quick};
use support::wait_for;

fn status_run(env: &str, n: u8) -> pupitre_core::run::RunRecord {
    fixture::run_record_in(
        env,
        fixture::run_id(n),
        RunKind::Status,
        RunMode::Apply,
        &format!("environment:{env}/unit:relay"),
        &["host-a"],
        "status-relay",
    )
}

fn env_lease() -> LeaseName {
    LeaseName::Environment(fixture::env_id())
}

/// 4.7: one run at a time in an environment, of any kind and mode, in the order of the ids.
#[tokio::test(flavor = "multi_thread")]
async fn one_run_at_a_time_in_an_environment() {
    let mut h = H::new();
    h.settings().max_parallel_runs = 4;
    h.process.push_script(forever());
    h.process.push_script(quick());
    h.start().await;
    h.queue(status_run("production", 1));
    h.queue(status_run("production", 2));
    wait_for("the first run starts", 5, || h.process.spawns().len() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.store.run(&fixture::run_id(1)).unwrap().status, RunStatus::Running);
    assert_eq!(h.store.run(&fixture::run_id(2)).unwrap().status, RunStatus::Queued, "the environment is busy");
    assert_eq!(h.store.lease_holder(&env_lease()), Some(format!("{}/{}", h.node_id(), fixture::run_id(1))));
    // The end of the first run frees the environment: the second run starts at once.
    h.process.finish(h.process.last_pid().unwrap(), pupitre_runner::process::Exit::Code(0));
    h.wait_status(&fixture::run_id(1), RunStatus::Passed).await;
    h.wait_status(&fixture::run_id(2), RunStatus::Passed).await;
    h.stop().await;
}

/// 4.6: `runner.max_parallel_runs` bounds the runs of different environments.
#[tokio::test(flavor = "multi_thread")]
async fn max_parallel_runs_bounds_the_active_runs() {
    for (max, both) in [(1u32, false), (2, true)] {
        let mut h = H::new();
        for doc in fixture::documents_in("staging") {
            h.store.add_doc(doc);
        }
        h.settings().max_parallel_runs = max;
        h.process.set_default(forever());
        h.start().await;
        h.queue(status_run("production", 1));
        h.queue(status_run("staging", 2));
        wait_for("a child starts", 5, || !h.process.spawns().is_empty()).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(h.process.spawns().len(), if both { 2 } else { 1 }, "max {max}");
        let running = h.store.all_runs().iter().filter(|r| r.status == RunStatus::Running).count();
        assert_eq!(running, if both { 2 } else { 1 });
        h.stop().await;
    }
}

/// 4.7: a run stays queued while another holder has the lease of its environment, then it starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_waits_for_the_lease_of_its_environment() {
    let mut h = H::new();
    h.process.push_script(quick());
    h.store.force_lease(&env_lease(), "other-node/run", Duration::from_millis(700));
    h.start().await;
    let run = status_run("production", 1);
    h.queue(run.clone());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.store.run(&run.id).unwrap().status, RunStatus::Queued);
    assert!(h.process.spawns().is_empty(), "a run never starts before it holds the lease");
    h.wait_status(&run.id, RunStatus::Passed).await;
    h.stop().await;
}

/// 4.7: the runner renews the lease while the run is `running`, at one third of the time to live.
#[tokio::test(flavor = "multi_thread")]
async fn the_lease_is_renewed_while_the_run_runs() {
    let mut h = H::new();
    h.settings().lease_ttl = Duration::from_millis(600);
    h.process.push_script(Script {
        exit: Some((Duration::from_millis(1800), pupitre_runner::process::Exit::Code(0))),
        ..Script::default()
    });
    h.start().await;
    let run = status_run("production", 1);
    h.queue(run.clone());
    wait_for("the child starts", 5, || h.process.spawns().len() == 1).await;
    // Three times the time to live later, the run still holds the lease.
    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert_eq!(h.store.run(&run.id).unwrap().status, RunStatus::Running);
    assert!(h.store.lease_holder(&env_lease()).is_some(), "the lease is renewed");
    assert!(h.store.calls("renew_lease") >= 4, "renewals: {}", h.store.calls("renew_lease"));
    h.wait_status(&run.id, RunStatus::Passed).await;
    h.stop().await;
}

/// 4.8: a second instance with `runner.execute = true` stops at start. The same node id takes the
/// lease again at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_runner_node_stops_at_start() {
    let mut first = H::new();
    first.start().await;
    let mut second = H::new();
    second.store = first.store.clone();
    match second.try_start().await {
        Err(RunnerError::LeaseHeld { holder, until }) => {
            assert_eq!(holder, first.node_id());
            assert!(until.ends_with('Z') && until.contains('T'), "{until}");
        }
        other => panic!("expected LeaseHeld, found {other:?}"),
    }
    let message = second.try_start().await.unwrap_err().to_string();
    assert!(message.starts_with("another runner node holds the lease until "), "{message}");
    // The node id of the first node takes the lease again after a restart.
    first.stop().await;
    first.start().await;
    first.stop().await;
}

/// The node id: written one time with the mode 0600, the same after a restart (4.8).
#[tokio::test(flavor = "multi_thread")]
async fn the_node_id_is_written_one_time() {
    use std::os::unix::fs::PermissionsExt;
    let mut h = H::new();
    h.start().await;
    let id = h.node_id();
    assert!(pupitre_runner::executor::checks::is_uuid(&id), "{id}");
    let path = h.node.settings.work_dir.join("node-id");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    h.stop().await;
    h.start().await;
    assert_eq!(h.node_id(), id);
    h.stop().await;
    // A file that is not a UUID stops the start.
    std::fs::write(&path, "not a uuid\n").unwrap();
    assert!(matches!(h.try_start().await, Err(RunnerError::Check(_))));
}

static EXIT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 4.8: when a renewal gives `Lost`, the runner node stops each child, writes `runner.lease_lost`
/// and the process exits with code 1.
#[tokio::test(flavor = "multi_thread")]
async fn the_loss_of_the_lease_runner_stops_the_children_and_exits() {
    let _guard = EXIT_LOCK.lock().await;
    EXIT_CODE.store(-1, Ordering::SeqCst);
    let mut h = H::new();
    h.settings().lease_ttl = Duration::from_millis(600);
    h.process.push_script(forever());
    h.start().await;
    let run = status_run("production", 1);
    h.queue(run.clone());
    wait_for("the child starts", 5, || h.process.spawns().len() == 1).await;
    // Another node takes the lease `runner`.
    h.store.force_lease(&LeaseName::Runner, "another-node", Duration::from_secs(30));
    wait_for("the process exits", 5, || EXIT_CODE.load(Ordering::SeqCst) == 1).await;
    assert!(h.process.signals().contains(&StopSignal::Term), "the child gets SIGTERM");
    // The runner writes nothing more: the new runner node decides about the run (4.30).
    assert_eq!(h.store.run(&run.id).unwrap().status, RunStatus::Running);
    h.stop().await;
}

/// 4.8: no renewal for `runner.lease_ttl` has the same effect.
#[tokio::test(flavor = "multi_thread")]
async fn no_renewal_for_the_time_to_live_stops_the_runner_node() {
    let _guard = EXIT_LOCK.lock().await;
    EXIT_CODE.store(-1, Ordering::SeqCst);
    let mut h = H::new();
    h.settings().lease_ttl = Duration::from_millis(600);
    h.start().await;
    h.store.fail("renew_lease", true);
    wait_for("the process exits", 5, || EXIT_CODE.load(Ordering::SeqCst) == 1).await;
    h.stop().await;
}

/// 4.7: when the lease of an environment is lost, the run stops (two runs must not work in one environment).
#[tokio::test(flavor = "multi_thread")]
async fn the_loss_of_the_lease_of_an_environment_stops_its_run() {
    let _guard = EXIT_LOCK.lock().await;
    EXIT_CODE.store(-1, Ordering::SeqCst);
    let mut h = H::new();
    h.settings().lease_ttl = Duration::from_millis(600);
    h.process.push_script(forever());
    h.start().await;
    let run = status_run("production", 1);
    h.queue(run.clone());
    wait_for("the child starts", 5, || h.process.spawns().len() == 1).await;
    h.store.force_lease(&env_lease(), "another-node/run", Duration::from_secs(30));
    let done = h.wait_status(&run.id, RunStatus::Lost).await;
    assert_eq!(done.reason, Some(RunReason::NodeChanged));
    assert!(h.process.signals().contains(&StopSignal::Term));
    assert_eq!(EXIT_CODE.load(Ordering::SeqCst), -1, "the runner node goes on");
    h.stop().await;
}

/// 4.6: the loop reacts to `Wake` before the period of the poll.
#[tokio::test(flavor = "multi_thread")]
async fn a_wake_starts_the_loop_before_the_poll_period() {
    let mut h = H::new();
    h.settings().poll_interval = Duration::from_secs(30);
    h.process.push_script(quick());
    h.start().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let run = status_run("production", 1);
    h.queue(run.clone());
    h.wait_status(&run.id, RunStatus::Passed).await;
    h.stop().await;
}
