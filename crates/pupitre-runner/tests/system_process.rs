// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of `SystemProcess` with real child processes (4.17 to 4.19, 4.29): the environment, the
//! files, the process group and the signals.

mod support;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use pupitre_runner::child::{Phase, RunFolders, child_environment};
use pupitre_runner::process::{
    Child, Exit, Probe, ProcessControl, ProcessIdentity, SpawnSpec, StopSignal, SystemProcess,
};
use support::fixture;

fn spec(dir: &Path, program: &str, args: &[&str]) -> SpawnSpec {
    let settings = fixture::settings(dir);
    let folders = RunFolders::of(&settings, "018bcfe5-6801-7101-8101-010101010101");
    std::fs::create_dir_all(folders.run.join("home")).unwrap();
    SpawnSpec {
        program: program.into(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        env: child_environment(&settings, &folders, Phase::Main),
        cwd: folders.run.clone(),
        output: folders.run.join("output.log"),
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn proc_file(pid: u32, name: &str) -> Vec<u8> {
    std::fs::read(format!("/proc/{pid}/{name}")).unwrap()
}

/// 4.18: the child gets exactly the environment of 4.18 and no other variable. 4.19: the standard
/// input is `/dev/null`, the output goes to `output.log` opened with `O_APPEND`, the child leads a new process group.
#[tokio::test(flavor = "multi_thread")]
async fn the_child_gets_exactly_the_environment_of_4_18() {
    let dir = tempfile::tempdir().unwrap();
    // The environment of this test process has other variables (`CARGO_HOME`, `HOME`): none reaches the child.
    assert!(std::env::var_os("PATH").is_some());
    let s = spec(dir.path(), "/bin/sleep", &["30"]);
    std::fs::write(&s.output, "old\n").unwrap();
    let mut child = SystemProcess.spawn(&s).unwrap();
    let id = child.identity().clone();
    // The child reads the environment of the program after its `exec`: wait for it.
    let started = std::time::Instant::now();
    while !proc_file(id.pid, "cmdline").starts_with(b"/bin/sleep\0") {
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "the child did not start /bin/sleep");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let environ = proc_file(id.pid, "environ");
    let actual: BTreeSet<String> =
        environ.split(|b| *b == 0).filter(|v| !v.is_empty()).map(|v| String::from_utf8(v.to_vec()).unwrap()).collect();
    let expected: BTreeSet<String> = s.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    assert_eq!(actual, expected, "the environment of 4.18, and no other variable");
    assert!(!actual.iter().any(|v| v.starts_with("SSH_AUTH_SOCK")));

    let link = |fd: u32| std::fs::read_link(format!("/proc/{}/fd/{fd}", id.pid)).unwrap();
    assert_eq!(link(0), Path::new("/dev/null"), "the standard input");
    assert_eq!(link(1), s.output);
    assert_eq!(link(2), s.output);
    let fdinfo = String::from_utf8(proc_file(id.pid, "fdinfo/1")).unwrap();
    let flags = fdinfo.lines().find_map(|l| l.strip_prefix("flags:")).unwrap().trim();
    assert!(u32::from_str_radix(flags, 8).unwrap() & 0o2000 != 0, "O_APPEND: {flags}");
    assert_eq!(std::fs::read_to_string(&s.output).unwrap(), "old\n", "the old content stays");
    // A new process group whose id is the pid (field 5 of the stat file).
    let stat = String::from_utf8(proc_file(id.pid, "stat")).unwrap();
    let rest = &stat[stat.rfind(')').unwrap() + 2..];
    let pgrp: u32 = rest.split(' ').nth(2).unwrap().parse().unwrap();
    assert_eq!(pgrp, id.pid);
    assert_eq!(
        std::fs::read_link(format!("/proc/{}/cwd", id.pid)).unwrap(),
        s.cwd,
        "the working folder is the run folder"
    );

    assert_eq!(SystemProcess.probe(&id), Probe::Alive);
    SystemProcess.signal(&id, StopSignal::Term).unwrap();
    assert_eq!(child.wait().await.unwrap(), Exit::Signal(15));
}

/// 4.23: the exit code of the child.
#[tokio::test(flavor = "multi_thread")]
async fn the_exit_code_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = SystemProcess.spawn(&spec(dir.path(), "/bin/sh", &["-c", "exit 7"])).unwrap();
    assert_eq!(child.wait().await.unwrap(), Exit::Code(7));
    let mut child = SystemProcess.spawn(&spec(dir.path(), "/bin/sh", &["-c", "echo out; echo err >&2"])).unwrap();
    assert_eq!(child.wait().await.unwrap(), Exit::Code(0));
    let out =
        std::fs::read_to_string(dir.path().join("work/runs/018bcfe5-6801-7101-8101-010101010101/output.log")).unwrap();
    assert!(out.contains("out\n") && out.contains("err\n"), "the output and the error go to one file");
}

/// 4.4: a program that does not exist gives an error: the run gets `spawn_failed`.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_program_gives_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(SystemProcess.spawn(&spec(dir.path(), "/nonexistent/ansible-playbook", &[])).is_err());
}

/// 4.19: `SIGTERM` goes to the process group, so a grandchild stops too. A child that ignores
/// `SIGTERM` needs `SIGKILL`.
#[tokio::test(flavor = "multi_thread")]
async fn signals_go_to_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    // The shell ignores TERM, starts a grandchild, and prints its pid.
    let script = "trap '' TERM; /bin/sleep 60 & echo $!; wait";
    let s = spec(dir.path(), "/bin/sh", &["-c", script]);
    let mut child = SystemProcess.spawn(&s).unwrap();
    let id = child.identity().clone();
    wait_until("the grandchild pid is written", || std::fs::read_to_string(&s.output).is_ok_and(|t| t.ends_with('\n')));
    let grandchild: u32 = std::fs::read_to_string(&s.output).unwrap().trim().parse().unwrap();
    assert_ne!(grandchild, id.pid);
    SystemProcess.signal(&id, StopSignal::Term).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(SystemProcess.probe(&id), Probe::Alive, "the child ignores SIGTERM");
    SystemProcess.signal(&id, StopSignal::Kill).unwrap();
    assert_eq!(child.wait().await.unwrap(), Exit::Signal(9));
    // The grandchild is in the group: it ends too (a zombie of the old init also counts as ended).
    wait_until("the grandchild ends", || {
        std::fs::read_to_string(format!("/proc/{grandchild}/stat")).map_or(true, |t| t.contains(") Z "))
    });
}

/// 4.29: a signal goes to a process only when the identity is the same. Another start time sends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_signal_needs_the_same_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = SystemProcess.spawn(&spec(dir.path(), "/bin/sleep", &["30"])).unwrap();
    let id = child.identity().clone();
    let wrong = ProcessIdentity { start_time: id.start_time + 1, ..id.clone() };
    assert_eq!(SystemProcess.probe(&wrong), Probe::Gone);
    SystemProcess.signal(&wrong, StopSignal::Kill).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(SystemProcess.probe(&id), Probe::Alive, "nothing was sent");
    SystemProcess.signal(&id, StopSignal::Kill).unwrap();
    assert_eq!(child.wait().await.unwrap(), Exit::Signal(9));
    assert_eq!(SystemProcess.probe(&id), Probe::Gone);
}

/// 4.19: the runner never kills the child when the handle goes away. A stop of Pupitre leaves the child alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_handle_does_not_kill_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let child = SystemProcess.spawn(&spec(dir.path(), "/bin/sleep", &["30"])).unwrap();
    let id = child.identity().clone();
    drop(child);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(SystemProcess.probe(&id), Probe::Alive);
    SystemProcess.signal(&id, StopSignal::Kill).unwrap();
    wait_until("the child ends", || SystemProcess.probe(&id) == Probe::Gone);
}
