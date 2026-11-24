// SPDX-License-Identifier: MIT OR Apache-2.0
//! The state that the loop, the lease task and the supervisors of runs share.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pupitre_core::capability::Capabilities;
use pupitre_core::ids::{EnvId, RunId};
use pupitre_store::Store;
use pupitre_store::types::StoreError;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::executor::Wake;
use crate::executor::log;
use crate::process::{ProcessControl, ProcessIdentity};
use crate::settings::RunnerSettings;

/// The periods of the supervisor of a run. The defaults are the periods of the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// The period of the pump: at most one chunk of output and one chunk of events (4.22).
    pub pump: Duration,
    /// The period of the check that an adopted child still runs (4.29).
    pub probe: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing { pump: Duration::from_secs(1), probe: Duration::from_secs(1) }
    }
}

/// The call that ends the process with a code. The runner node calls it with 1 when it loses
/// its lease (4.8). A test gives another function.
pub type ExitFn = fn(i32);

/// The options of the loop that the specification does not make keys of the configuration.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// The periods.
    pub timing: Timing,
    /// The end of the process after the loss of the lease.
    pub exit: ExitFn,
}

impl Default for Options {
    fn default() -> Options {
        Options { timing: Timing::default(), exit: |code| std::process::exit(code) }
    }
}

/// The flag that stops the loop. The handle of the runner and the tasks share it.
#[derive(Debug, Default)]
pub(crate) struct StopFlag {
    flag: AtomicBool,
    notify: Notify,
}

impl StopFlag {
    pub(crate) fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub(crate) fn set(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Wait for `set`. The loops also wake at their own period, so a call to `set` between
    /// the check and the wait is not lost for long.
    pub(crate) async fn wait(&self) {
        if !self.is_set() {
            self.notify.notified().await;
        }
    }
}

/// A run that this process follows.
#[derive(Debug)]
pub(crate) struct ActiveRun {
    /// The environment of the run.
    pub env: EnvId,
    /// True after the loop read a cancel request (4.27).
    pub cancel_requested: AtomicBool,
    /// Wakes the supervisor after a cancel request.
    pub cancel: Notify,
    /// True after the lease of the environment is lost (4.7).
    pub lease_lost: AtomicBool,
    /// Wakes the supervisor after the loss of the lease.
    pub lost: Notify,
    /// The last time that the lease was renewed.
    pub lease_ok: Mutex<Instant>,
    /// The child of the run, when it runs.
    pub identity: Mutex<Option<ProcessIdentity>>,
}

impl ActiveRun {
    pub(crate) fn new(env: EnvId) -> ActiveRun {
        ActiveRun {
            env,
            cancel_requested: AtomicBool::new(false),
            cancel: Notify::new(),
            lease_lost: AtomicBool::new(false),
            lost: Notify::new(),
            lease_ok: Mutex::new(Instant::now()),
            identity: Mutex::new(None),
        }
    }

    pub(crate) fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
        self.cancel.notify_one();
    }

    pub(crate) fn mark_lost(&self) {
        self.lease_lost.store(true, Ordering::SeqCst);
        self.lost.notify_one();
    }

    pub(crate) fn identity(&self) -> Option<ProcessIdentity> {
        self.identity.lock().ok().and_then(|g| g.clone())
    }

    pub(crate) fn set_identity(&self, identity: Option<ProcessIdentity>) {
        if let Ok(mut g) = self.identity.lock() {
            *g = identity;
        }
    }
}

pub(crate) struct Shared<S: Store, P: ProcessControl> {
    pub settings: RunnerSettings,
    pub caps: std::sync::Arc<Capabilities>,
    pub store: std::sync::Arc<S>,
    pub process: P,
    pub node_id: String,
    pub options: Options,
    pub wake: Wake,
    pub active: Mutex<BTreeMap<RunId, std::sync::Arc<ActiveRun>>>,
    pub stop: std::sync::Arc<StopFlag>,
}

impl<S: Store, P: ProcessControl> Shared<S, P> {
    pub(crate) fn is_stopping(&self) -> bool {
        self.stop.is_set()
    }

    pub(crate) fn active_runs(&self) -> Vec<(RunId, std::sync::Arc<ActiveRun>)> {
        self.active.lock().map(|g| g.iter().map(|(k, v)| (*k, v.clone())).collect()).unwrap_or_default()
    }

    pub(crate) fn active_count(&self) -> usize {
        self.active.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub(crate) fn holder(&self, run: &RunId) -> String {
        format!("{}/{}", self.node_id, run)
    }

    /// Call the store. A transient error (the store did not answer) is logged and the call
    /// starts again after one period of the poll (4.6). Another error goes back to the caller.
    /// `None` means that the runner stops.
    pub(crate) async fn call<T, F, Fut>(&self, what: &'static str, f: F) -> Option<Result<T, StoreError>>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, StoreError>>,
    {
        loop {
            match f().await {
                Ok(v) => return Some(Ok(v)),
                Err(e @ StoreError::Unavailable(_)) => {
                    log::store_error(what, &e);
                    if self.is_stopping() {
                        return None;
                    }
                    tokio::time::sleep(self.settings.poll_interval).await;
                    if self.is_stopping() {
                        return None;
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}
