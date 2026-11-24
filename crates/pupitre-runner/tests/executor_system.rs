// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the runner with real child processes: a fake `ansible-playbook` script, the real
//! `SystemProcess`, the real `/proc` for the adoption (4.19, 4.29).

mod support;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use pupitre_core::run::{RunKind, RunMode, RunStatus};
use pupitre_runner::executor::{Options, Timing, start_with};
use pupitre_runner::process::SystemProcess;
use pupitre_runner::{RunnerHandle, Wake};
use support::fixture;
use support::harness::H;
use support::wait_for;

/// A fake `ansible-playbook`: it writes its arguments and two events, waits, then ends.
fn write_script(h: &H, wait_seconds: u32) {
    let path = &h.node.settings.ansible_playbook;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let stats = support::harness::stats_line(&[("host-a", 0, 0)]);
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\n\
             echo \"args: $*\"\n\
             echo \"password is relay-pass-1234\"\n\
             echo '{{\"kind\":\"playbook_start\",\"playbook\":\"status-relay.yml\",\"check\":false,\"v\":1}}' >> \"$PUPITRE_EVENTS_FILE\"\n\
             sleep {wait_seconds}\n\
             echo '{stats}' >> \"$PUPITRE_EVENTS_FILE\"\n\
             exit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn start(h: &H) -> RunnerHandle {
    start_with(
        h.node.settings.clone(),
        Arc::new(h.node.caps.clone()),
        h.store.clone(),
        Wake::default(),
        SystemProcess,
        Options {
            timing: Timing { pump: Duration::from_millis(100), probe: Duration::from_millis(100) },
            ..Options::default()
        },
    )
    .await
    .unwrap()
}

fn status_run(n: u8) -> pupitre_core::run::RunRecord {
    fixture::run_record(
        fixture::run_id(n),
        RunKind::Status,
        RunMode::Apply,
        "environment:production/unit:relay",
        &["host-a"],
        "status-relay",
    )
}

/// 4.17 to 4.23 with a real child: the arguments, the scrubbed output, the events and the record.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_child_runs_a_run_to_its_end() {
    let h = H::new();
    write_script(&h, 0);
    let handle = start(&h).await;
    let run = status_run(1);
    h.store.insert_run(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Passed).await;
    let out = String::from_utf8(h.store.output(&run.id)).unwrap();
    let folders = h.folders(&run.id);
    assert!(out.contains(&format!("--inventory {}/snapshot/inventory/hosts.yml", folders.run.display())), "{out}");
    assert!(
        out.contains("--limit pupitre_run ") && out.contains("vars/run.json") && out.contains("run-vars.json"),
        "{out}"
    );
    assert!(!out.contains("--check"), "a status run has the mode apply");
    assert!(out.contains("password is [secret]") && !out.contains("relay-pass-1234"), "{out}");
    assert_eq!(h.store.events(&run.id).len(), 2);
    assert_eq!(done.exit_code, Some(0));
    assert!(done.runner.as_ref().unwrap().pid > 0 && done.runner.unwrap().node == h.node_id());
    handle.stop(Duration::from_secs(2)).await;
}

/// 4.29: a restart of Pupitre during a run. The real child lives in its own process group. The new
/// loop checks `/proc/<pid>/stat` and the boot id, adopts the run, and reads the event `stats` at the end.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_child_survives_the_stop_and_is_adopted() {
    let h = H::new();
    write_script(&h, 2);
    let handle = start(&h).await;
    let run = status_run(1);
    h.store.insert_run(run.clone());
    wait_for("the child writes its first event", 10, || {
        h.store.run(&run.id).is_some_and(|r| r.runner.is_some()) && !h.store.events(&run.id).is_empty()
    })
    .await;
    let pid = h.store.run(&run.id).unwrap().runner.unwrap().pid;
    handle.stop(Duration::from_secs(2)).await;
    // The stop of Pupitre left the child alone.
    assert!(std::path::Path::new(&format!("/proc/{pid}")).exists(), "the child lives");
    assert_eq!(h.store.run(&run.id).unwrap().status, RunStatus::Running);

    let handle = start(&h).await;
    let done = h.wait_status(&run.id, RunStatus::Passed).await;
    assert!(done.exit_code.is_none(), "the exit code of an adopted child is not known (4.29)");
    assert!(done.stats.is_some(), "the status comes from the event `stats`");
    let out = String::from_utf8(h.store.output(&run.id)).unwrap();
    assert_eq!(out.matches("args: ").count(), 1, "the child started one time: {out}");
    assert_eq!(h.store.events(&run.id).len(), 2);
    handle.stop(Duration::from_secs(2)).await;
}
