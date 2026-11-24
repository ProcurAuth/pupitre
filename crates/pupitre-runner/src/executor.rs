// SPDX-License-Identifier: MIT OR Apache-2.0
//! The loop of the runner node (4.6 to 4.8, 4.27 to 4.31) and the port that the server uses.
//!
//! | Submodule | Content |
//! | --- | --- |
//! | `checks` | The checks of the runner node (7.8) and its node id (4.8) |
//! | `queue` | The loop (4.6) and the lease task (4.7, 4.8) |
//! | `supervise` | The life of one run (4.9 to 4.28) |
//! | `pump` | The output and the events of a run (4.20 to 4.22) |
//! | `adopt` | The adoption after a restart (4.29, 4.30) |
//! | `cleanup` | The cleanup (4.31) |
//! | `guard` | The guard of an apply (4.24) |

pub mod checks;
pub mod guard;

mod adopt;
mod cleanup;
mod log;
mod pump;
mod queue;
mod shared;
mod supervise;

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use pupitre_core::capability::Capabilities;
use pupitre_store::types::{LeaseName, LeaseOutcome};
use pupitre_store::{AnyStore, Store};
use tokio::task::JoinHandle;

use crate::process::{ProcessControl, SystemProcess};
use crate::settings::RunnerSettings;

pub use shared::{ExitFn, Options, Timing};

/// An error of the start of the runner node. The process stops at start with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerError {
    /// Another node holds the lease `runner` (4.8).
    LeaseHeld {
        /// The holder.
        holder: String,
        /// The end time, in the text form of 5.3.
        until: String,
    },
    /// A check of 7.8 failed: the text names the key and the cause.
    Check(String),
    /// The store failed.
    Store(String),
}

impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunnerError::LeaseHeld { until, .. } => write!(f, "another runner node holds the lease until {until}"),
            RunnerError::Check(text) => f.write_str(text),
            RunnerError::Store(text) => write!(f, "the store failed: {text}"),
        }
    }
}

impl std::error::Error for RunnerError {}

/// The signal that wakes the loop (4.6). The server keeps a copy and calls `wake` after it
/// queues a run or a cancel request.
#[derive(Debug, Clone, Default)]
pub struct Wake(Arc<tokio::sync::Notify>);

impl Wake {
    /// Wake the loop. A call when the loop runs makes it run one more time.
    pub fn wake(&self) {
        self.0.notify_one();
    }

    /// Wait for the next call of `wake`.
    pub async fn wait(&self) {
        self.0.notified().await;
    }
}

/// The handle of the loop of the runner node.
#[derive(Debug)]
pub struct RunnerHandle {
    wake: Wake,
    stop: Arc<shared::StopFlag>,
    loop_task: JoinHandle<()>,
    lease_task: JoinHandle<()>,
}

impl RunnerHandle {
    /// The signal of the loop.
    pub fn wake(&self) -> Wake {
        self.wake.clone()
    }

    /// Stop the loop. The children stay alone (4.19). The leases of the environments stay
    /// until their end time, so that a restart can adopt the runs (4.29). The loop gets
    /// `grace` to end its step. Then the tasks of the runs stop.
    pub async fn stop(self, grace: Duration) {
        self.stop.set();
        let mut loop_task = self.loop_task;
        if tokio::time::timeout(grace, &mut loop_task).await.is_err() {
            // The task of the loop owns the tasks of the runs: they stop with it.
            loop_task.abort();
            let _ = loop_task.await;
        }
        self.lease_task.abort();
        let _ = self.lease_task.await;
    }
}

/// Start the loop of 4.6 on the runner node. First check 7.8 (the node and the folders of the playbooks), forbid the inspection of this
/// process by its children (7.8) and take the lease `runner` (4.8). Then adopt the runs (4.29),
/// clean the secrets folder (4.31) and run the loop.
pub async fn start_executor(
    settings: RunnerSettings,
    capabilities: Arc<Capabilities>,
    store: Arc<AnyStore>,
    wake: Wake,
) -> Result<RunnerHandle, RunnerError> {
    checks::check_runner_node(&settings)?;
    // A playbook folder with `group_vars` or `host_vars` changes the variables of a run (4.2, 7.8).
    checks::check_playbook_folders(&capabilities)?;
    // A child of a run must not read the environment of this process: it can hold the secrets
    // of the configuration (`_env`, 6.5). The check comes before the first child (7.8).
    crate::process::forbid_inspection().map_err(|e| RunnerError::Check(e.to_string()))?;
    start_with(settings, capabilities, store, wake, SystemProcess, Options::default()).await
}

/// The start of `start_executor` after the checks of 7.8, with the store and the process
/// steps of the caller. The tests of the runner use it with a fake `ProcessControl`.
pub async fn start_with<S, P>(
    settings: RunnerSettings,
    capabilities: Arc<Capabilities>,
    store: Arc<S>,
    wake: Wake,
    process: P,
    options: Options,
) -> Result<RunnerHandle, RunnerError>
where
    S: Store + 'static,
    P: ProcessControl + 'static,
    P::Child: 'static,
{
    std::fs::create_dir_all(settings.work_dir.join("runs"))
        .map_err(|e| RunnerError::Check(format!("runner.work_dir: cannot make the folder runs: {e}")))?;
    let node_id = checks::node_id(&settings)?;
    // 4.8: the lease `runner`. The same node id takes it again at once.
    match store
        .acquire_lease(&LeaseName::Runner, &node_id, settings.lease_ttl)
        .await
        .map_err(|e| RunnerError::Store(e.to_string()))?
    {
        LeaseOutcome::Acquired { .. } => {}
        LeaseOutcome::HeldBy { holder, until } => {
            return Err(RunnerError::LeaseHeld { holder, until: until.to_string() });
        }
    }
    let stop = Arc::new(shared::StopFlag::default());
    let shared = Arc::new(shared::Shared {
        settings,
        caps: capabilities,
        store,
        process,
        node_id,
        options,
        wake: wake.clone(),
        active: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        stop: stop.clone(),
    });
    let adopted = adopt::adopt_runs(&shared).await?;
    let adopted_ids: BTreeSet<_> = adopted.iter().map(|a| a.run.id).collect();
    cleanup::at_start(&shared, &adopted_ids).await;
    let loop_task = tokio::spawn(queue::run_loop(shared.clone(), adopted));
    let lease_task = tokio::spawn(queue::lease_loop(shared));
    wake.wake();
    Ok(RunnerHandle { wake, stop, loop_task, lease_task })
}
