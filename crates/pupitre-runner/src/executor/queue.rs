// SPDX-License-Identifier: MIT OR Apache-2.0
//! The loop of the runner node (4.6) and the lease task (4.7, 4.8).

use std::sync::Arc;
use std::time::Duration;

use pupitre_core::ids::EnvId;
use pupitre_core::json::Json;
use pupitre_core::run::{RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::types::{LeaseName, LeaseOutcome, LeaseRenewal, StoreError};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::executor::adopt::Adoption;
use crate::executor::log::{self, Level};
use crate::executor::shared::{ActiveRun, Shared};
use crate::executor::supervise::{self, Start};
use crate::process::{Probe, ProcessControl, StopSignal};

/// Run the loop of 4.6 until the runner stops. `adopted` are the runs that the start adopted.
pub(crate) async fn run_loop<S, P>(sh: Arc<Shared<S, P>>, adopted: Vec<Adoption>)
where
    S: Store + 'static,
    P: ProcessControl + 'static,
    P::Child: 'static,
{
    let mut tasks: JoinSet<()> = JoinSet::new();
    for item in adopted {
        let active = item.active.clone();
        let shared = sh.clone();
        tasks.spawn(supervise::supervise(shared, active, item.run, Start::Adopted(item.info)));
    }
    loop {
        if sh.is_stopping() {
            break;
        }
        while let Some(joined) = tasks.try_join_next() {
            if let Err(e) = joined {
                log::store_error("a task of a run", &e);
            }
        }
        cancel_step(&sh).await;
        claim_step(&sh, &mut tasks).await;
        tokio::select! {
            () = sh.wake.wait() => {}
            () = tokio::time::sleep(sh.settings.poll_interval) => {}
            () = sh.stop.wait() => {}
        }
    }
    // The supervisors stop now, and the task waits until each one ended: no supervisor writes
    // in the store after the loop ends. The children stay alone (4.19).
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

/// Step 1 of 4.6: read the cancel requests of the runs of this node.
async fn cancel_step<S: Store, P: ProcessControl>(sh: &Shared<S, P>) {
    for (id, active) in sh.active_runs() {
        if active.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
            continue;
        }
        match sh.store.get_run(&id).await {
            Ok(record) if record.cancel.is_some() => active.request_cancel(),
            Ok(_) | Err(StoreError::NotFound) => {}
            Err(e) => log::store_error("get_run", &e),
        }
    }
}

/// Steps 2 and 3 of 4.6. Take queued runs while fewer than `runner.max_parallel_runs` are
/// active. Take the lease of their environment, then change their status to `running`.
async fn claim_step<S, P>(sh: &Arc<Shared<S, P>>, tasks: &mut JoinSet<()>)
where
    S: Store + 'static,
    P: ProcessControl + 'static,
    P::Child: 'static,
{
    let mut busy: Vec<EnvId> = sh.active_runs().into_iter().map(|(_, a)| a.env.clone()).collect();
    let max = sh.settings.max_parallel_runs as usize;
    while sh.active_count() < max && !sh.is_stopping() {
        let next = match sh.store.next_queued(&busy, 1).await {
            Ok(mut runs) => runs.pop(),
            Err(e) => {
                log::store_error("next_queued", &e);
                return;
            }
        };
        let Some(run) = next else { return };
        // One try for each environment in a step, whatever the answer.
        busy.push(run.environment.clone());
        if let Some((active, record)) = start_run(sh, &run).await {
            tasks.spawn(supervise::supervise(sh.clone(), active, record, Start::Fresh));
        }
    }
}

/// Take the lease of the environment and write the status `running` (4.6, step 3). The answer
/// is the active run and its record when the run starts.
async fn start_run<S, P>(sh: &Arc<Shared<S, P>>, run: &RunRecord) -> Option<(Arc<ActiveRun>, RunRecord)>
where
    S: Store,
    P: ProcessControl,
{
    let name = LeaseName::Environment(run.environment.clone());
    let holder = sh.holder(&run.id);
    match sh.store.acquire_lease(&name, &holder, sh.settings.lease_ttl).await {
        Ok(LeaseOutcome::Acquired { .. }) => {}
        // Another run holds the environment: the run stays queued.
        Ok(LeaseOutcome::HeldBy { .. }) => return None,
        Err(e) => {
            log::store_error("acquire_lease", &e);
            return None;
        }
    }
    let mut record = run.clone();
    record.status = RunStatus::Running;
    record.started_at = Some(UtcMillis::now());
    match sh.store.update_run(RunStatus::Queued, &record).await {
        Ok(()) => {}
        Err(e) => {
            // A cancel took the run, or the store failed: the run does not start.
            if !matches!(e, StoreError::Conflict { .. } | StoreError::BadTransition { .. } | StoreError::NotFound) {
                log::store_error("update_run", &e);
            }
            let _ = sh.store.release_lease(&name, &holder).await;
            return None;
        }
    }
    let active = Arc::new(ActiveRun::new(run.environment.clone()));
    if let Ok(mut map) = sh.active.lock() {
        map.insert(run.id, active.clone());
    }
    log::emit(
        Level::Info,
        "run.started",
        &[
            ("run", Json::str(run.id.to_string())),
            ("kind", Json::str(run.kind.name())),
            ("mode", Json::str(run.mode.name())),
            ("node", Json::str(run.node.path())),
            ("hosts", Json::Arr(run.hosts.iter().map(|h| Json::str(h.as_str())).collect())),
            ("subject", Json::str(run.subject.as_str())),
        ],
    );
    Some((active, record))
}

/// The task that renews the leases at one third of `runner.lease_ttl` (4.6, step 5; 4.7; 4.8).
pub(crate) async fn lease_loop<S, P>(sh: Arc<Shared<S, P>>)
where
    S: Store + 'static,
    P: ProcessControl + 'static,
{
    let ttl = sh.settings.lease_ttl;
    let every = ttl / 3;
    let mut runner_ok = Instant::now();
    loop {
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            () = sh.stop.wait() => {}
        }
        if sh.is_stopping() {
            return;
        }
        match sh.store.renew_lease(&LeaseName::Runner, &sh.node_id, ttl).await {
            Ok(LeaseRenewal::Renewed { .. }) => runner_ok = Instant::now(),
            Ok(LeaseRenewal::Lost) => return lose_runner_lease(&sh, "the lease runner has another holder").await,
            Err(e) => {
                log::store_error("renew_lease", &e);
                if runner_ok.elapsed() >= ttl {
                    return lose_runner_lease(&sh, "no renewal of the lease runner for the time to live").await;
                }
            }
        }
        for (id, active) in sh.active_runs() {
            let name = LeaseName::Environment(active.env.clone());
            match sh.store.renew_lease(&name, &sh.holder(&id), ttl).await {
                Ok(LeaseRenewal::Renewed { .. }) => {
                    if let Ok(mut g) = active.lease_ok.lock() {
                        *g = Instant::now();
                    }
                }
                Ok(LeaseRenewal::Lost) => active.mark_lost(),
                Err(e) => {
                    log::store_error("renew_lease", &e);
                    let last = active.lease_ok.lock().map(|g| *g).unwrap_or_else(|_| Instant::now());
                    if last.elapsed() >= ttl {
                        active.mark_lost();
                    }
                }
            }
        }
    }
}

/// 4.8: stop each child (4.27), write `runner.lease_lost`, and end the process with code 1.
async fn lose_runner_lease<S: Store, P: ProcessControl>(sh: &Shared<S, P>, cause: &str) {
    sh.stop.set();
    log::emit(Level::Error, "runner.lease_lost", &[("error", Json::str(cause))]);
    let children: Vec<_> = sh.active_runs().into_iter().filter_map(|(_, a)| a.identity()).collect();
    for identity in &children {
        let _ = sh.process.signal(identity, StopSignal::Term);
    }
    let end = Instant::now() + sh.settings.cancel_grace;
    while Instant::now() < end && children.iter().any(|i| sh.process.probe(i) == Probe::Alive) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for identity in &children {
        if sh.process.probe(identity) == Probe::Alive {
            let _ = sh.process.signal(identity, StopSignal::Kill);
        }
    }
    (sh.options.exit)(1);
}
