// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the loop and the life of a run (4.6, 4.9 to 4.28) with a fake `ProcessControl` and
//! an in-memory store.

mod support;

use std::time::Duration;

use pupitre_core::json::{self, Json, ReadLimits};
use pupitre_core::run::{RunKind, RunMode, RunReason, RunStatus};
use pupitre_runner::process::{Exit, StopSignal};
use pupitre_store::types::{LeaseName, StoreError};
use support::fake_process::Script;
use support::fixture;
use support::harness::{H, forever, quick, stats_line};
use support::wait_for;

const PLAYBOOK_START: &str = r#"{"check":true,"kind":"playbook_start","playbook":"deploy-relay.yml","v":1}"#;
const TASK_START: &str = r#"{"kind":"task_start","task":"Copy the file","v":1}"#;

fn parse(bytes: &[u8]) -> std::collections::BTreeMap<String, Json> {
    json::parse(bytes, ReadLimits::bytes(1 << 20)).unwrap().as_object().unwrap().clone()
}

/// 4.3, 4.6, 4.23, 4.25: a `deploy` check with a render playbook has two phases.
#[tokio::test(flavor = "multi_thread")]
async fn a_check_runs_the_render_phase_then_the_main_phase() {
    let mut h = H::new();
    h.process.push_script(Script {
        output: b"render output\n".to_vec(),
        events: vec![PLAYBOOK_START.into(), TASK_START.into(), stats_line(&[("host-a", 0, 0)])],
        ..quick()
    });
    h.process.push_script(Script {
        output: b"main output with relay-pass-1234 inside\n".to_vec(),
        events: vec![
            PLAYBOOK_START.into(),
            r#"{"diff":"--- a\n+++ b\n","host":"host-a","kind":"host_result","status":"changed","task":"Copy the file","v":1}"#.into(),
            stats_line(&[("host-a", 0, 0), ("host-b", 0, 0)]),
        ],
        ..quick()
    });
    h.start().await;
    let run = h.check_run(1);
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Passed).await;

    // The two children: the render playbook with no `--check`, then the main playbook with it.
    let spawns = h.process.spawns();
    assert_eq!(spawns.len(), 2);
    assert!(spawns[0].args.last().unwrap().ends_with("render-relay.yml"));
    assert!(!spawns[0].args.iter().any(|a| a == "--check" || a == "--diff"));
    assert!(spawns[1].args.last().unwrap().ends_with("deploy-relay.yml"));
    assert!(spawns[1].args.iter().any(|a| a == "--check") && spawns[1].args.iter().any(|a| a == "--diff"));
    let phase = |i: usize| spawns[i].env.iter().find(|(k, _)| k == "PUPITRE_EVENTS_PHASE").unwrap().1.clone();
    assert_eq!((phase(0).as_str(), phase(1).as_str()), ("render", "main"));
    assert_eq!(spawns[0].program, h.node.settings.ansible_playbook);

    // The record (4.38).
    assert!(done.reason.is_none());
    assert_eq!(done.exit_code, Some(0));
    assert!(done.started_at.is_some() && done.finished_at.is_some() && done.snapshot.is_some());
    let facts = done.runner.clone().unwrap();
    assert_eq!(facts.node, h.node_id());
    assert_eq!(done.stats.as_ref().map(|s| s.len()), Some(2), "the stats of the main phase");

    // The output: scrubbed (4.21). The events: numbered, with their phase (4.20).
    let out = String::from_utf8(h.store.output(&run.id)).unwrap();
    assert!(out.contains("render output\n") && out.contains("main output with [secret] inside\n"), "{out}");
    assert!(!out.contains("relay-pass-1234"));
    let events = h.store.events(&run.id);
    assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), (1..=6).collect::<Vec<_>>());
    let phases: Vec<String> = events.iter().map(|e| parse(&e.bytes)["phase"].as_str().unwrap().to_string()).collect();
    assert_eq!(phases, ["render", "render", "render", "main", "main", "main"]);
    assert!(events.iter().all(|e| parse(&e.bytes).contains_key("t")));

    // The files and the lease: the secrets folder is gone, the lease is released.
    // The private folder goes at the end of the run, with the files of the render phase (4.13).
    assert!(!h.secrets_dir_of(&run.id).exists());
    assert!(!h.folders(&run.id).run.join("render").exists(), "no render folder on the disk of the run");
    assert!(h.folders(&run.id).snapshot().is_dir());
    let pid: Json =
        json::parse(&std::fs::read(h.folders(&run.id).run.join("pid")).unwrap(), ReadLimits::bytes(4096)).unwrap();
    assert_eq!(pid.as_object().unwrap()["phase"].as_str(), Some("main"));
    support::wait_for("the lease is released", 5, || {
        h.store.lease_holder(&LeaseName::Environment(fixture::env_id())).is_none()
    })
    .await;
    h.stop().await;
}

/// 4.23: a failed render phase ends the run, and the main phase does not start (4.25).
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_render_phase_ends_the_run() {
    let mut h = H::new();
    h.process.push_script(Script { exit: Some((Duration::from_millis(10), Exit::Code(3))), ..Script::default() });
    h.start().await;
    let run = h.check_run(1);
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::RenderFailed));
    assert_eq!(done.exit_code, Some(3));
    assert_eq!(h.process.spawns().len(), 1);
    h.stop().await;
}

/// 4.23: the exit code of the main phase.
#[tokio::test(flavor = "multi_thread")]
async fn the_exit_code_of_the_main_phase_gives_the_status() {
    let mut h = H::new();
    // A run with no render playbook: a status run.
    h.process.push_script(Script { exit: Some((Duration::from_millis(10), Exit::Code(2))), ..Script::default() });
    h.process.push_script(Script { exit: Some((Duration::from_millis(10), Exit::Signal(11))), ..Script::default() });
    h.start().await;
    let status = |n| {
        fixture::run_record(
            fixture::run_id(n),
            RunKind::Status,
            RunMode::Apply,
            "environment:production/unit:relay",
            &["host-a"],
            "status-relay",
        )
    };
    h.queue(status(1));
    let done = h.wait_status(&fixture::run_id(1), RunStatus::Failed).await;
    assert_eq!((done.reason, done.exit_code), (Some(RunReason::ExitCode), Some(2)));
    assert_eq!(h.process.spawns().len(), 1, "no render phase for a status run");
    assert!(!h.process.spawns()[0].args.iter().any(|a| a == "--check"), "a status run has the mode apply");
    h.queue(status(2));
    let done = h.wait_status(&fixture::run_id(2), RunStatus::Failed).await;
    assert_eq!((done.reason, done.exit_code), (Some(RunReason::ExitCode), None), "a signal gives no code");
    h.stop().await;
}

/// 4.7, 4.13, 4.19: a store error that is not transient after the spawn ends the supervisor. Its
/// child gets `SIGTERM`, and the private folder goes, before the lease of the environment is free.
/// Else a child would run with no timeout and no cancel, next to a new run of the environment.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_error_after_the_spawn_stops_the_child_before_the_lease_is_free() {
    let mut h = H::new();
    h.process.set_default(forever());
    // The supervisor reads the record with `get_run` after the spawn: the call fails for good.
    h.store.fail_with("get_run", StoreError::Corrupt("injected".into()));
    h.start().await;
    let run = h.check_run(1);
    h.queue(run.clone());
    let store = h.store.clone();
    let process = h.process.clone();
    let lease = LeaseName::Environment(fixture::env_id());
    wait_for("the lease of the environment to be free after the spawn", 20, move || {
        !process.spawns().is_empty() && store.lease_holder(&lease).is_none()
    })
    .await;
    assert_eq!(h.process.signals().first(), Some(&StopSignal::Term), "the child gets SIGTERM");
    let pid = h.process.last_pid().unwrap();
    let identity =
        pupitre_runner::process::ProcessIdentity { pid, start_time: 1000 + u64::from(pid), boot_id: "boot-1".into() };
    use pupitre_runner::process::ProcessControl;
    assert_eq!(h.process.probe(&identity), pupitre_runner::process::Probe::Gone, "the child ended before the lease");
    assert!(!h.secrets_dir_of(&run.id).exists(), "the private folder goes (4.13)");
    assert_eq!(h.process.spawns().len(), 1, "no other phase starts");
    h.stop().await;
}

/// 4.23: the program cannot start.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_cannot_start_gives_spawn_failed() {
    let mut h = H::new();
    h.process.push_script(Script { fail_spawn: true, ..Script::default() });
    h.start().await;
    let run = h.check_run(1);
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::SpawnFailed));
    assert!(done.runner.is_none());
    let out = String::from_utf8(h.store.output(&run.id)).unwrap();
    assert!(out.contains("spawn_failed") || out.contains("cannot start"), "{out}");
    assert!(!h.secrets_dir_of(&run.id).exists(), "the secrets folder goes whatever the status (4.13)");
    h.stop().await;
}

/// 4.9, 4.23: a value that fails stops the run before any process starts. The output holds the errors.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_values_stop_the_run_before_the_child() {
    let mut h = H::new();
    h.store.add_doc(fixture::stored(
        pupitre_core::doc::DocKey::UnitValues(fixture::env_id(), pupitre_core::ids::UnitId::parse("relay").unwrap()),
        r#"{"format": 1, "values": {"log.level": "trace"}}"#,
    ));
    h.start().await;
    let run = h.check_run(1);
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::InvalidValues));
    assert!(h.process.spawns().is_empty());
    let out = String::from_utf8(h.store.output(&run.id)).unwrap();
    assert!(out.contains("log.level"), "{out}");
    assert!(!h.folders(&run.id).snapshot().exists());
    h.stop().await;
}

/// 4.23, 2.39: a secret that does not decrypt.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_that_does_not_decrypt_stops_the_run() {
    let mut h = H::new();
    h.store.fail_with("read_secrets", StoreError::SecretUnreadable);
    h.start().await;
    let run = h.check_run(1);
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::SecretUnreadable));
    assert!(h.process.spawns().is_empty());
    h.stop().await;
}

/// 4.6: an error of the store leaves the run in its status; the runner logs it and tries again.
#[tokio::test(flavor = "multi_thread")]
async fn an_error_of_the_store_is_retried() {
    let mut h = H::new();
    h.process.push_script(quick());
    h.process.push_script(quick());
    h.store.fail("next_queued", true);
    h.start().await;
    let run = h.check_run(1);
    h.queue(run.clone());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.store.run(&run.id).unwrap().status, RunStatus::Queued, "the run stays queued");
    h.store.fail("next_queued", false);
    h.wait_status(&run.id, RunStatus::Passed).await;

    // The same for the steps of a running run: the environment cannot be read for a while.
    let second = h.check_run(2);
    h.process.push_script(quick());
    h.process.push_script(quick());
    h.store.fail("read_environment", true);
    h.queue(second.clone());
    wait_for("the run starts", 5, || h.store.run(&second.id).unwrap().status == RunStatus::Running).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.store.run(&second.id).unwrap().status, RunStatus::Running);
    h.store.fail("read_environment", false);
    h.wait_status(&second.id, RunStatus::Passed).await;
    h.stop().await;
}

/// 4.27: a cancel sends `SIGTERM` to the group and the status becomes `canceled`.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_the_child_with_sigterm() {
    let mut h = H::new();
    h.process.push_script(forever());
    h.start().await;
    // A status run: one phase.
    let run = fixture::run_record(
        fixture::run_id(1),
        RunKind::Status,
        RunMode::Apply,
        "environment:production/unit:relay",
        &["host-a"],
        "status-relay",
    );
    h.queue(run.clone());
    wait_for("the child starts", 5, || !h.process.spawns().is_empty()).await;
    h.store.edit_run(&run.id, |r| {
        r.cancel = Some(pupitre_core::run::CancelRequest {
            by: pupitre_core::ids::Subject::parse("bob").unwrap(),
            at: pupitre_core::time::UtcMillis::now(),
        });
    });
    h.wake.wake();
    let done = h.wait_status(&run.id, RunStatus::Canceled).await;
    assert!(done.reason.is_none());
    assert_eq!(h.process.signals(), vec![StopSignal::Term], "no SIGKILL for a child that stops");
    assert!(done.cancel.is_some(), "the cancel member stays in the record");
    h.stop().await;
}

/// 4.19, 4.27: a child that ignores `SIGTERM` gets `SIGKILL` after `runner.cancel_grace`.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_ignores_sigterm_gets_sigkill_after_the_grace() {
    let mut h = H::new();
    h.process.push_script(Script { ignore_term: true, ..Script::default() });
    h.settings().cancel_grace = Duration::from_millis(400);
    h.start().await;
    let run = fixture::run_record(
        fixture::run_id(1),
        RunKind::Status,
        RunMode::Apply,
        "environment:production/unit:relay",
        &["host-a"],
        "status-relay",
    );
    h.queue(run.clone());
    wait_for("the child starts", 5, || !h.process.spawns().is_empty()).await;
    let at = std::time::Instant::now();
    h.store.edit_run(&run.id, |r| {
        r.cancel = Some(pupitre_core::run::CancelRequest {
            by: pupitre_core::ids::Subject::parse("bob").unwrap(),
            at: pupitre_core::time::UtcMillis::now(),
        });
    });
    h.wake.wake();
    h.wait_status(&run.id, RunStatus::Canceled).await;
    assert_eq!(h.process.signals(), vec![StopSignal::Term, StopSignal::Kill]);
    assert!(at.elapsed() >= Duration::from_millis(400), "the grace time passed before the SIGKILL");
    h.stop().await;
}

/// 4.28: the timeout gives the steps of a cancel with the status `failed` and the reason `timeout`.
#[tokio::test(flavor = "multi_thread")]
async fn the_timeout_stops_the_child() {
    let mut h = H::new();
    h.process.push_script(forever());
    h.settings().run_timeout = Duration::from_millis(500);
    h.start().await;
    let run = fixture::run_record(
        fixture::run_id(1),
        RunKind::Status,
        RunMode::Apply,
        "environment:production/unit:relay",
        &["host-a"],
        "status-relay",
    );
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::Timeout));
    assert_eq!(h.process.signals(), vec![StopSignal::Term]);
    h.stop().await;
}

/// 4.27: a queued run that a person cancelled never starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_canceled_queued_run_does_not_start() {
    let mut h = H::new();
    h.start().await;
    let mut run = h.check_run(1);
    run.status = RunStatus::Canceled;
    h.queue(run);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(h.process.spawns().is_empty());
}

/// 4.27: a cancel that arrives before the child starts ends the run with no child.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_before_the_child_starts_gives_canceled() {
    let mut h = H::new();
    h.start().await;
    // The environment cannot be read: the run is `running` and waits for the snapshot.
    h.store.fail("read_environment", true);
    let run = fixture::run_record(
        fixture::run_id(1),
        RunKind::Status,
        RunMode::Apply,
        "environment:production/unit:relay",
        &["host-a"],
        "status-relay",
    );
    h.queue(run.clone());
    wait_for("the run starts", 5, || h.store.run(&run.id).unwrap().status == RunStatus::Running).await;
    h.store.edit_run(&run.id, |r| {
        r.cancel = Some(pupitre_core::run::CancelRequest {
            by: pupitre_core::ids::Subject::parse("bob").unwrap(),
            at: pupitre_core::time::UtcMillis::now(),
        });
    });
    h.wake.wake();
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.store.fail("read_environment", false);
    let done = h.wait_status(&run.id, RunStatus::Canceled).await;
    assert!(done.reason.is_none());
    assert!(h.process.spawns().is_empty());
    h.stop().await;
}
