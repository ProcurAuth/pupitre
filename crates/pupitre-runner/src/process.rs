// SPDX-License-Identifier: MIT OR Apache-2.0
//! The process steps of a run (4.19, 4.27, 4.29) behind one trait, so that a test can use a
//! fake. The real implementation is `SystemProcess`.

use std::fmt;
use std::future::Future;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// What the runner starts (4.17 to 4.19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    /// `runner.ansible_playbook`
    pub program: PathBuf,
    /// The arguments of 4.17.
    pub args: Vec<String>,
    /// The environment of 4.18: exactly these variables.
    pub env: Vec<(String, String)>,
    /// The folder where the child starts: the folder of the run.
    pub cwd: PathBuf,
    /// The file of the standard output and the standard error, opened with `O_APPEND`.
    pub output: PathBuf,
}

/// The identity of a child that survives a restart (4.19, 4.29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// The pid, which is also the id of the process group.
    pub pid: u32,
    /// Field 22 of `/proc/<pid>/stat`.
    pub start_time: u64,
    /// The text of `/proc/sys/kernel/random/boot_id`.
    pub boot_id: String,
}

/// The state of a process that the runner looks for (4.29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// The same process still runs.
    Alive,
    /// No process has the pid, or another process has it, or the machine restarted.
    Gone,
}

/// A signal of 4.19.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopSignal {
    /// `SIGTERM` to the process group.
    Term,
    /// `SIGKILL` to the process group.
    Kill,
}

/// The end of a child that the runner started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The child ended with this code.
    Code(u8),
    /// A signal ended the child.
    Signal(i32),
}

/// An error of a process step. The text names the step and the cause, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessError(pub String);

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProcessError {}

/// A child that the runner started in this process.
pub trait Child: Send {
    /// The identity to write in `<run>/pid` (4.19).
    fn identity(&self) -> &ProcessIdentity;
    /// Wait for the end of the child.
    fn wait(&mut self) -> impl Future<Output = Result<Exit, ProcessError>> + Send;
}

/// The process steps (4.19, 4.27, 4.29).
pub trait ProcessControl: Send + Sync {
    /// The type of a child.
    type Child: Child;
    /// Start a child in a new process group, with the spec. The runner never kills it when the
    /// process of Pupitre stops.
    fn spawn(&self, spec: &SpawnSpec) -> Result<Self::Child, ProcessError>;
    /// Find whether the process of an identity still runs (4.29).
    fn probe(&self, identity: &ProcessIdentity) -> Probe;
    /// Send a signal to the process group of an identity, when the process is the same.
    fn signal(&self, identity: &ProcessIdentity, signal: StopSignal) -> Result<(), ProcessError>;
}

/// The process steps of the machine: `tokio::process`, `/proc` and the signals of `rustix`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcess;

/// A child of `SystemProcess`.
#[derive(Debug)]
pub struct SystemChild {
    identity: ProcessIdentity,
    child: tokio::process::Child,
}

impl Child for SystemChild {
    fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    async fn wait(&mut self) -> Result<Exit, ProcessError> {
        let status = self.child.wait().await.map_err(|e| ProcessError(format!("wait for the child: {e}")))?;
        match (status.code(), status.signal()) {
            (Some(code), _) => Ok(Exit::Code(u8::try_from(code & 0xff).unwrap_or(u8::MAX))),
            (None, Some(signal)) => Ok(Exit::Signal(signal)),
            (None, None) => Err(ProcessError("wait for the child: no exit code and no signal".to_string())),
        }
    }
}

/// Forbid each other process of the same user to read the memory and the files of
/// `/proc/<pid>` of this process, for example `environ` (7.8). The flag is `PR_SET_DUMPABLE` with
/// the value 0. It also stops a core dump. The child of a run gets the default flag again at its
/// `exec`, so the runner can still read `/proc/<child pid>/stat` (4.29).
pub fn forbid_inspection() -> Result<(), ProcessError> {
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .map_err(|e| ProcessError(format!("prctl PR_SET_DUMPABLE 0: {e}")))
}

/// The path of the boot id of the machine.
const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

/// The text of the boot id (4.19), without the line end.
pub fn boot_id() -> Result<String, ProcessError> {
    let text = std::fs::read_to_string(BOOT_ID_PATH).map_err(|e| ProcessError(format!("read {BOOT_ID_PATH}: {e}")))?;
    Ok(text.trim_end_matches('\n').to_string())
}

/// The state letter and the start time of a process, from the text of `/proc/<pid>/stat`.
/// The name of the process is between parentheses and can hold spaces and parentheses, so the
/// parse starts after the last `)`. The start time is field 22 (4.19).
pub fn parse_stat(text: &str) -> Option<(char, u64)> {
    let rest = &text[text.rfind(')')? + 1..];
    let mut fields = rest.split_ascii_whitespace();
    // Field 3 is the state. Field 22 is the 20th item after the `)`.
    let state = fields.next()?.chars().next()?;
    let start_time = fields.nth(18)?.parse().ok()?;
    Some((state, start_time))
}

/// Read `/proc/<pid>/stat`: the state and the start time. `None` when the process is not there.
pub fn read_stat(pid: u32) -> Option<(char, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&text)
}

fn pid_of(pid: u32) -> Result<rustix::process::Pid, ProcessError> {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| ProcessError(format!("the pid {pid} is not valid")))
}

impl SystemProcess {
    fn open_output(path: &Path) -> Result<std::fs::File, ProcessError> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| ProcessError(format!("open the output file: {e}")))
    }
}

impl ProcessControl for SystemProcess {
    type Child = SystemChild;

    fn spawn(&self, spec: &SpawnSpec) -> Result<SystemChild, ProcessError> {
        let out = SystemProcess::open_output(&spec.output)?;
        let err = out.try_clone().map_err(|e| ProcessError(format!("open the output file: {e}")))?;
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            // A new process group whose id is the pid (4.19).
            .process_group(0)
            // A stop of Pupitre leaves the child alone (4.19).
            .kill_on_drop(false);
        let child = command.spawn().map_err(|e| ProcessError(format!("start the program: {e}")))?;
        let pid = child.id().ok_or_else(|| ProcessError("the child ended before its pid was read".to_string()))?;
        let (_, start_time) =
            read_stat(pid).ok_or_else(|| ProcessError(format!("read /proc/{pid}/stat after the start")))?;
        Ok(SystemChild { identity: ProcessIdentity { pid, start_time, boot_id: boot_id()? }, child })
    }

    fn probe(&self, identity: &ProcessIdentity) -> Probe {
        let same_boot = boot_id().is_ok_and(|b| b == identity.boot_id);
        match read_stat(identity.pid) {
            // A zombie has ended: nothing runs any more.
            Some((state, start_time))
                if same_boot && start_time == identity.start_time && !matches!(state, 'Z' | 'X') =>
            {
                Probe::Alive
            }
            _ => Probe::Gone,
        }
    }

    fn signal(&self, identity: &ProcessIdentity, signal: StopSignal) -> Result<(), ProcessError> {
        if self.probe(identity) != Probe::Alive {
            // Another process can have this pid now: send nothing.
            return Ok(());
        }
        let sig = match signal {
            StopSignal::Term => rustix::process::Signal::TERM,
            StopSignal::Kill => rustix::process::Signal::KILL,
        };
        rustix::process::kill_process_group(pid_of(identity.pid)?, sig)
            .map_err(|e| ProcessError(format!("send a signal to the process group {}: {e}", identity.pid)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stat_parse_handles_a_name_with_spaces_and_parentheses() {
        let text = "1234 (my (odd) name) S 1 1234 1234 0 -1 4194560 100 0 0 0 0 0 0 0 20 0 1 0 987654 1000 100 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0";
        assert_eq!(parse_stat(text), Some(('S', 987_654)));
        assert_eq!(parse_stat("1 (x) S"), None);
        assert_eq!(parse_stat("no parenthesis"), None);
    }

    #[test]
    fn probes_this_process_and_a_wrong_start_time() {
        let pid = std::process::id();
        let (_, start_time) = read_stat(pid).expect("stat of this process");
        let id = ProcessIdentity { pid, start_time, boot_id: boot_id().unwrap() };
        assert_eq!(SystemProcess.probe(&id), Probe::Alive);
        assert_eq!(SystemProcess.probe(&ProcessIdentity { start_time: start_time + 1, ..id.clone() }), Probe::Gone);
        assert_eq!(SystemProcess.probe(&ProcessIdentity { boot_id: "other".into(), ..id }), Probe::Gone);
    }
}
