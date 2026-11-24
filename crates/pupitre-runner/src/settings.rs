// SPDX-License-Identifier: MIT OR Apache-2.0
//! The settings of the runner node, from the block `runner` of the configuration (6.12).

use std::path::PathBuf;
use std::time::Duration;

/// The typed keys of the block `runner` that the runner node reads. The configuration module
/// of the binary builds this structure one time at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerSettings {
    /// `runner.work_dir`: the folders of the runs (4.10).
    pub work_dir: PathBuf,
    /// `runner.secrets_dir`: the private folders on `tmpfs` (4.13).
    pub secrets_dir: PathBuf,
    /// `runner.ansible_playbook`: the one program that the runner starts (4.17).
    pub ansible_playbook: PathBuf,
    /// `runner.path`: the `PATH` of the child (4.18).
    pub path: String,
    /// `runner.ssh_user` (4.15)
    pub ssh_user: String,
    /// `runner.ssh_key_file` (4.15, 7.8)
    pub ssh_key_file: PathBuf,
    /// `runner.known_hosts_file` (4.15, 7.8)
    pub known_hosts_file: PathBuf,
    /// `runner.releases_dir` (4.14), when the file has it.
    pub releases_dir: Option<PathBuf>,
    /// `runner.forks` (4.15)
    pub forks: u32,
    /// `runner.max_parallel_runs` (4.6)
    pub max_parallel_runs: u32,
    /// `runner.check_max_age` (4.24)
    pub check_max_age: Duration,
    /// `runner.run_timeout` (4.28)
    pub run_timeout: Duration,
    /// `runner.cancel_grace` (4.19, 4.27)
    pub cancel_grace: Duration,
    /// `runner.lease_ttl` (4.7, 4.8)
    pub lease_ttl: Duration,
    /// `runner.max_log_bytes` (4.22)
    pub max_log_bytes: u64,
    /// `runner.max_events` (4.22)
    pub max_events: u64,
    /// `runner.max_event_bytes` (4.20)
    pub max_event_bytes: u64,
    /// `runner.runs_keep` (4.31)
    pub runs_keep: u32,
    /// The period of the loop when no run request wakes it: the poll period of the backend (4.6).
    pub poll_interval: Duration,
}
