// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the adoption after a restart (4.29), the runs of another node (4.30) and the cleanup (4.31).

mod support;

use std::time::Duration;

use pupitre_core::json::Json;
use pupitre_core::json::{self};
use pupitre_core::run::{RunKind, RunMode, RunReason, RunRecord, RunStatus, RunnerFacts};
use pupitre_core::time::UtcMillis;
use pupitre_runner::process::{Exit, ProcessIdentity, StopSignal};
use pupitre_store::Store;
use pupitre_store::types::LeaseName;
use support::fake_process::Script;
use support::fixture;
use support::harness::{H, forever, quick, stats_line};
use support::wait_for;

const NODE_ID: &str = "018bcfe5-6801-4101-8101-0a0a0a0a0a0a";
const TASK: &str = r#"{"kind":"task_start","task":"t","v":1}"#;

fn status_run(n: u8) -> RunRecord {
    fixture::run_record(
        fixture::run_id(n),
        RunKind::Status,
        RunMode::Apply,
        "environment:production/unit:relay",
        &["host-a"],
        "status-relay",
    )
}

struct Setup {
    node: &'static str,
    identity: ProcessIdentity,
    pid_file: bool,
    phase: &'static str,
    events: Vec<String>,
    output: &'static str,
    stored_output: &'static str,
    private_folder: bool,
    started_ago: Duration,
}

impl Setup {
    fn new(identity: ProcessIdentity) -> Setup {
        Setup {
            node: NODE_ID,
            identity,
            pid_file: true,
            phase: "main",
            events: vec![],
            output: "",
            stored_output: "",
            private_folder: true,
            started_ago: Duration::from_secs(1),
        }
    }
}

/// Make the state that a runner node leaves when it stops. This is a `running` record, the run
/// folder with `pid`, `output.log` and `events.jsonl`, and the private folder of the secrets.
fn leave_running(h: &H, n: u8, s: &Setup) -> RunRecord {
    std::fs::create_dir_all(&h.node.settings.work_dir).unwrap();
    std::fs::write(h.node.settings.work_dir.join("node-id"), format!("{NODE_ID}\n")).unwrap();
    let mut run = status_run(n);
    run.status = RunStatus::Running;
    run.started_at = Some(UtcMillis(UtcMillis::now().0 - s.started_ago.as_millis() as u64));
    run.runner = Some(RunnerFacts {
        node: s.node.to_string(),
        pid: u64::from(s.identity.pid),
        start_time: s.identity.start_time,
        boot_id: s.identity.boot_id.clone(),
    });
    let folders = h.folders(&run.id);
    std::fs::create_dir_all(&folders.run).unwrap();
    if s.pid_file {
        let pid = Json::object([
            ("boot_id", Json::str(s.identity.boot_id.clone())),
            ("phase", Json::str(s.phase)),
            ("pid", Json::Int(u64::from(s.identity.pid))),
            ("start_time", Json::Int(s.identity.start_time)),
        ]);
        std::fs::write(folders.run.join("pid"), json::to_canonical(&pid)).unwrap();
    }
    std::fs::write(folders.run.join("output.log"), s.output).unwrap();
    let mut events = String::new();
    for e in &s.events {
        events.push_str(e);
        events.push('\n');
    }
    std::fs::write(folders.run.join("events.jsonl"), events).unwrap();
    if s.private_folder {
        let dir = folders.secrets.join("values/environments/production/units/relay");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("storage.password"), "relay-pass-1234").unwrap();
    }
    h.store.insert_run(run.clone());
    if !s.stored_output.is_empty() {
        let store = h.store.clone();
        let id = run.id;
        let bytes = s.stored_output.as_bytes().to_vec();
        futures_block(async move { store.append_output(&id, 0, &bytes).await.unwrap() });
    }
    run
}

/// Run a short future from a sync helper.
fn futures_block<F: std::future::Future>(f: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}

fn external(h: &H, pid: u32) -> ProcessIdentity {
    h.process.add_external(pid)
}

/// 4.29: the pid, the start time and the boot id are the same, so the run is adopted. The output
/// and the events go on from the offsets of the store. The status comes from the event `stats`.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_lives_is_adopted_and_followed() {
    let mut h = H::new();
    let mut s = Setup::new(external(&h, 5001));
    s.output = "line1\nline2 relay-pass-1234\nline3\n";
    s.stored_output = "line1\n";
    s.events = vec![TASK.into(), TASK.into(), stats_line(&[("host-a", 0, 0)])];
    let run = leave_running(&h, 1, &s);
    // The store holds the first event only.
    h.store
        .append_events(
            &run.id,
            1,
            &[br#"{"kind":"task_start","phase":"main","seq":1,"t":"2026-11-24T10:00:00.000Z","task":"t","v":1}"#
                .to_vec()],
        )
        .await
        .unwrap();
    h.start().await;
    // The stream keeps `longest - 1` bytes until the next chunk or the end (4.21): the last 7 bytes wait.
    wait_for("the output goes on", 5, || h.store.output(&run.id).len() >= 20).await;
    assert_eq!(h.store.run(&run.id).unwrap().status, RunStatus::Running, "the child still lives");
    assert_eq!(
        h.store.output(&run.id),
        b"line1\nline2 [secret]",
        "no copy of the first line, and the scrubber knows the secret"
    );
    wait_for("the events go on", 5, || h.store.events(&run.id).len() == 3).await;
    assert_eq!(h.store.events(&run.id).iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(h.store.lease_holder(&LeaseName::Environment(fixture::env_id())).is_some(), "the lease is taken again");
    h.process.kill_external(5001);
    let done = h.wait_status(&run.id, RunStatus::Passed).await;
    assert_eq!(h.store.output(&run.id), b"line1\nline2 [secret]\nline3\n", "the end adds what the stream held");
    assert!(done.stats.is_some() && done.exit_code.is_none(), "the exit code of an adopted child is not known");
    assert!(h.process.spawns().is_empty(), "no new child");
    assert!(!h.secrets_dir_of(&run.id).exists(), "the private folder goes at the end");
    h.stop().await;
}

/// 4.23: an adopted child that ends with no `stats` gives `lost_exit`. With a failed host, `host_failed`.
#[tokio::test(flavor = "multi_thread")]
async fn the_status_of_an_adopted_child_comes_from_the_event_stats() {
    // A child that lives at the start and ends after it.
    let cases: [(Vec<String>, RunStatus, Option<RunReason>); 4] = [
        (vec![TASK.into()], RunStatus::Failed, Some(RunReason::LostExit)),
        (vec![TASK.into(), stats_line(&[("host-a", 1, 0)])], RunStatus::Failed, Some(RunReason::HostFailed)),
        (vec![stats_line(&[("host-a", 0, 1)])], RunStatus::Failed, Some(RunReason::HostFailed)),
        (vec![stats_line(&[("host-a", 0, 0)])], RunStatus::Passed, None),
    ];
    for (i, (events, status, reason)) in cases.into_iter().enumerate() {
        let mut h = H::new();
        let pid = 5200 + i as u32;
        let mut s = Setup::new(external(&h, pid));
        s.events = events;
        let run = leave_running(&h, 1, &s);
        h.start().await;
        wait_for("the run is adopted", 5, || {
            h.store.lease_holder(&LeaseName::Environment(fixture::env_id())).is_some()
        })
        .await;
        h.process.kill_external(pid);
        let done = h.wait_status(&run.id, status).await;
        assert_eq!(done.reason, reason, "case {i}");
        h.stop().await;
    }
}

/// 4.29, step 3: another start time, another boot id, no pid file, or no facts: the run is `lost` with `process_gone`.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_whose_child_is_not_found_is_lost() {
    for case in ["start_time", "boot_id", "no_pid_file", "no_facts", "no_private_folder"] {
        let mut h = H::new();
        let live = external(&h, 5300);
        let mut s = Setup::new(match case {
            "start_time" => ProcessIdentity { start_time: live.start_time + 1, ..live.clone() },
            "boot_id" => ProcessIdentity { boot_id: "another-boot".into(), ..live.clone() },
            _ => live.clone(),
        });
        s.pid_file = case != "no_pid_file";
        s.private_folder = case != "no_private_folder";
        let run = leave_running(&h, 1, &s);
        if case == "no_facts" {
            h.store.edit_run(&run.id, |r| r.runner = None);
        }
        h.start().await;
        let done = h.wait_status(&run.id, RunStatus::Lost).await;
        assert_eq!(done.reason, Some(RunReason::ProcessGone), "{case}");
        assert!(done.finished_at.is_some());
        assert!(!h.secrets_dir_of(&run.id).exists(), "{case}: the private folder goes at start (4.31)");
        assert!(h.process.spawns().is_empty());
        h.stop().await;
    }
}

/// 4.30: a run of another node is `lost` with `node_changed`.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_of_another_runner_node_is_lost() {
    let mut h = H::new();
    let mut s = Setup::new(external(&h, 5400));
    s.node = "018bcfe5-6801-4101-8101-ffffffffffff";
    let run = leave_running(&h, 1, &s);
    h.start().await;
    let done = h.wait_status(&run.id, RunStatus::Lost).await;
    assert_eq!(done.reason, Some(RunReason::NodeChanged));
    h.stop().await;
}

/// 4.27, 4.28 on an adopted run: a cancel and the timeout signal the process group.
#[tokio::test(flavor = "multi_thread")]
async fn an_adopted_run_can_be_canceled_and_times_out() {
    // Cancel.
    let mut h = H::new();
    let s = Setup::new(external(&h, 5500));
    let run = leave_running(&h, 1, &s);
    h.store.edit_run(&run.id, |r| {
        r.cancel = Some(pupitre_core::run::CancelRequest {
            by: pupitre_core::ids::Subject::parse("bob").unwrap(),
            at: UtcMillis::now(),
        });
    });
    h.start().await;
    let done = h.wait_status(&run.id, RunStatus::Canceled).await;
    assert!(done.reason.is_none());
    assert_eq!(h.process.signals(), vec![StopSignal::Term]);
    h.stop().await;

    // Timeout: the time counts from `started_at`, also before the restart.
    let mut h = H::new();
    h.settings().run_timeout = Duration::from_secs(60);
    let mut s = Setup::new(external(&h, 5501));
    s.started_ago = Duration::from_secs(61);
    let run = leave_running(&h, 1, &s);
    h.start().await;
    let done = h.wait_status(&run.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::Timeout));
    assert_eq!(h.process.signals(), vec![StopSignal::Term]);
    h.stop().await;
}

/// 4.25, 4.29: a child of the render phase that is adopted and ends well is followed by the main phase.
#[tokio::test(flavor = "multi_thread")]
async fn an_adopted_render_phase_goes_on_with_the_main_phase() {
    let mut h = H::new();
    h.process.push_script(Script { events: vec![stats_line(&[("host-a", 0, 0)])], ..quick() });
    let mut s = Setup::new(external(&h, 5600));
    s.phase = "render";
    s.events = vec![stats_line(&[("host-a", 0, 0)])];
    let mut run = leave_running(&h, 1, &s);
    run.kind = RunKind::Deploy;
    run.mode = RunMode::Check;
    run.playbook = pupitre_core::ids::PlaybookId::parse("deploy-relay").unwrap();
    h.store.insert_run(run.clone());
    h.start().await;
    wait_for("the run is adopted", 5, || h.store.lease_holder(&LeaseName::Environment(fixture::env_id())).is_some())
        .await;
    h.process.kill_external(5600);
    let done = h.wait_status(&run.id, RunStatus::Passed).await;
    let spawns = h.process.spawns();
    assert_eq!(spawns.len(), 1, "the main phase starts as a child of the new process");
    assert!(spawns[0].args.iter().any(|a| a == "--check"));
    assert_eq!(done.exit_code, Some(0));
    h.stop().await;
}

/// 4.29: a restart of Pupitre during a run. The child lives, the new loop adopts it, and the
/// output and the events have no gap and no copy.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_during_a_run_adopts_the_child() {
    let mut h = H::new();
    let output: Vec<u8> = (0..100_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    let events: Vec<String> = (0..20)
        .map(|i| format!(r#"{{"kind":"task_start","task":"t{i}","v":1}}"#))
        .chain([stats_line(&[("host-a", 0, 0)])])
        .collect();
    h.process.push_script(Script { output: output.clone(), events, ..forever() });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    wait_for("some output is stored", 5, || !h.store.output(&run.id).is_empty()).await;
    // The process of Pupitre stops. The child lives.
    h.stop().await;
    let pid = h.process.last_pid().unwrap();
    let before = h.store.output(&run.id).len();
    assert!(before <= output.len());
    // A new process of Pupitre starts on the same store and the same folders.
    h.start().await;
    // The stream holds the last 14 bytes until the end (4.21).
    wait_for("the output is complete", 10, || h.store.output(&run.id).len() == output.len() - 14).await;
    assert_eq!(h.store.output(&run.id), output[..output.len() - 14], "no gap and no copy");
    wait_for("the events are complete", 10, || h.store.events(&run.id).len() == 21).await;
    assert_eq!(h.store.events(&run.id).iter().map(|e| e.seq).collect::<Vec<_>>(), (1..=21).collect::<Vec<_>>());
    assert_eq!(h.process.spawns().len(), 1, "the child was not started again");
    h.process.finish(pid, Exit::Code(0));
    let done = h.wait_status(&run.id, RunStatus::Passed).await;
    assert_eq!(h.store.output(&run.id), output);
    assert_eq!(done.runner.unwrap().node, h.node_id());
    h.stop().await;
}

/// 4.31: at start, the runner node removes the secrets folders that are not adopted, and the folders
/// of runs that the store no longer holds. It never touches the folder of a queued or running run.
#[tokio::test(flavor = "multi_thread")]
async fn the_start_removes_old_folders_and_keeps_the_folders_of_live_runs() {
    let mut h = H::new();
    let queued = status_run(1);
    h.store.insert_run(queued.clone());
    std::fs::create_dir_all(h.folders(&queued.id).run.join("snapshot")).unwrap();
    let orphan = fixture::run_id(7);
    std::fs::create_dir_all(h.folders(&orphan).run.join("snapshot")).unwrap();
    std::fs::create_dir_all(h.folders(&orphan).secrets).unwrap();
    std::fs::create_dir_all(h.folders(&queued.id).secrets).unwrap();
    let other = h.node.settings.work_dir.join("runs/not-a-run-id");
    std::fs::create_dir_all(&other).unwrap();
    h.timing = pupitre_runner::executor::Timing { pump: Duration::from_millis(50), probe: Duration::from_millis(50) };
    // The queued run waits: another holder has its environment for a while.
    h.store.force_lease(&LeaseName::Environment(fixture::env_id()), "other/x", Duration::from_secs(30));
    h.start().await;
    assert!(!h.folders(&orphan).run.exists(), "the folder of a run that the store does not hold");
    assert!(!h.folders(&orphan).secrets.exists());
    assert!(!h.folders(&queued.id).secrets.exists(), "a secrets folder that is not adopted");
    assert!(h.folders(&queued.id).run.exists(), "the folder of a queued run stays");
    assert!(other.exists(), "a folder that is not a run id stays");
    h.stop().await;
}

/// 4.31: after a run, `prune_runs(runner.runs_keep)` removes the oldest final runs and their folders.
#[tokio::test(flavor = "multi_thread")]
async fn the_end_of_a_run_prunes_the_old_runs_and_their_folders() {
    let mut h = H::new();
    h.settings().runs_keep = 2;
    h.process.set_default(quick());
    h.start().await;
    for n in 1..=4 {
        h.queue(status_run(n));
        h.wait_status(&fixture::run_id(n), RunStatus::Passed).await;
        let id = fixture::run_id(n);
        wait_for("the lease is released", 5, || {
            h.store.lease_holder(&LeaseName::Environment(fixture::env_id())).is_none()
        })
        .await;
        let _ = id;
    }
    wait_for("the old runs are pruned", 5, || h.store.all_runs().len() == 2).await;
    assert!(h.store.run(&fixture::run_id(1)).is_none() && h.store.run(&fixture::run_id(4)).is_some());
    wait_for("the folders are removed", 5, || !h.folders(&fixture::run_id(1)).run.exists()).await;
    assert!(h.folders(&fixture::run_id(4)).run.exists());
    h.stop().await;
}
