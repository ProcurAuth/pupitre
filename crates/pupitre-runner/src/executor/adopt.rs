// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adoption of the runs after a restart of Pupitre (4.29) and the runs of another runner
//! node (4.30).

use std::sync::Arc;

use pupitre_core::json::{self, Json, Members, ReadLimits};
use pupitre_core::run::{RunReason, RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::types::{LeaseName, LeaseOutcome, RunFilter};

use crate::child::{Phase, RunFolders};
use crate::executor::RunnerError;
use crate::executor::log::{self, Level};
use crate::executor::pump::Progress;
use crate::executor::shared::{ActiveRun, Shared};
use crate::executor::supervise::{log_finished, read_private_values, read_progress};
use crate::process::{Probe, ProcessControl, ProcessIdentity, StopSignal};

/// What the supervisor of an adopted run needs.
#[derive(Debug, Clone)]
pub(crate) struct AdoptInfo {
    /// The child that lives.
    pub identity: ProcessIdentity,
    /// The phase of that child.
    pub phase: Phase,
    /// The place of the pump at the last flush.
    pub progress: Option<Progress>,
    /// The values of the private folder, for the scrubber.
    pub scrub_values: Vec<Vec<u8>>,
}

/// An adopted run.
pub(crate) struct Adoption {
    pub run: RunRecord,
    pub active: Arc<ActiveRun>,
    pub info: AdoptInfo,
}

/// Read `<run>/pid` (4.19): the identity of the child and its phase.
fn read_pid_file(folders: &RunFolders) -> Option<(ProcessIdentity, Phase)> {
    let bytes = std::fs::read(folders.run.join("pid")).ok()?;
    let value = json::parse(&bytes, ReadLimits::bytes(4096)).ok()?;
    let mut m = Members::of(&value).ok()?;
    let Json::Str(boot_id) = m.take("boot_id")? else { return None };
    let phase = match m.take("phase")? {
        Json::Str(p) if p == "render" => Phase::Render,
        Json::Str(p) if p == "main" => Phase::Main,
        _ => return None,
    };
    let Json::Int(pid) = m.take("pid")? else { return None };
    let Json::Int(start_time) = m.take("start_time")? else { return None };
    if !m.finish().is_empty() {
        return None;
    }
    Some((ProcessIdentity { pid: u32::try_from(pid).ok()?, start_time, boot_id }, phase))
}

/// Write the status `lost` (4.23).
async fn mark_lost<S: Store, P: ProcessControl>(sh: &Shared<S, P>, run: &RunRecord, reason: RunReason) {
    let mut record = run.clone();
    record.status = RunStatus::Lost;
    record.reason = Some(reason);
    record.finished_at = Some(UtcMillis::now());
    match sh.store.update_run(RunStatus::Running, &record).await {
        Ok(()) => log_finished(&record),
        Err(e) => log::store_error("update_run", &e),
    }
}

/// At start (4.29, 4.30): read each run with the status `running`. A run of this node whose
/// child lives is adopted. Another run becomes `lost`.
pub(crate) async fn adopt_runs<S: Store, P: ProcessControl>(
    sh: &Arc<Shared<S, P>>,
) -> Result<Vec<Adoption>, RunnerError> {
    let filter = RunFilter { environment: None, status: Some(RunStatus::Running) };
    let mut running: Vec<RunRecord> = Vec::new();
    let mut before = None;
    loop {
        let page =
            sh.store.list_runs(&filter, before.as_ref(), 500).await.map_err(|e| RunnerError::Store(e.to_string()))?;
        let Some(last) = page.last() else { break };
        before = Some(last.id);
        running.extend(page);
    }
    // The oldest run first.
    running.reverse();
    let mut adopted = Vec::new();
    for run in running {
        let folders = RunFolders::of(&sh.settings, &run.id.to_string());
        // 4.30: a run of another node.
        let Some(facts) = &run.runner else {
            mark_lost(sh, &run, RunReason::ProcessGone).await;
            continue;
        };
        if facts.node != sh.node_id {
            mark_lost(sh, &run, RunReason::NodeChanged).await;
            continue;
        }
        // 4.29, steps 1 and 2: the pid file, then the same process.
        let Some((identity, phase)) = read_pid_file(&folders) else {
            mark_lost(sh, &run, RunReason::ProcessGone).await;
            continue;
        };
        if sh.process.probe(&identity) != Probe::Alive {
            mark_lost(sh, &run, RunReason::ProcessGone).await;
            continue;
        }
        // The values for the scrubber: without them the output is not safe to store.
        let Some(scrub_values) = read_private_values(&folders) else {
            let _ = sh.process.signal(&identity, StopSignal::Term);
            mark_lost(sh, &run, RunReason::ProcessGone).await;
            continue;
        };
        // The lease of the environment again (4.7).
        let name = LeaseName::Environment(run.environment.clone());
        match sh.store.acquire_lease(&name, &sh.holder(&run.id), sh.settings.lease_ttl).await {
            Ok(LeaseOutcome::Acquired { .. }) => {}
            Ok(LeaseOutcome::HeldBy { .. }) => {
                // Another run holds the environment: two runs would work in it.
                let _ = sh.process.signal(&identity, StopSignal::Term);
                mark_lost(sh, &run, RunReason::NodeChanged).await;
                continue;
            }
            Err(e) => return Err(RunnerError::Store(e.to_string())),
        }
        let active = Arc::new(ActiveRun::new(run.environment.clone()));
        active.set_identity(Some(identity.clone()));
        if let Ok(mut map) = sh.active.lock() {
            map.insert(run.id, active.clone());
        }
        log::emit(
            Level::Warn,
            "run.adopted",
            &[("run", Json::str(run.id.to_string())), ("reason", Json::str("restart"))],
        );
        let progress = read_progress(&folders);
        adopted.push(Adoption { run, active, info: AdoptInfo { identity, phase, progress, scrub_values } });
    }
    Ok(adopted)
}
