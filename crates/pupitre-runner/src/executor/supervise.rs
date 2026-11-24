// SPDX-License-Identifier: MIT OR Apache-2.0
//! The life of one run on the runner node.
//!
//! This module builds the snapshot and checks the guard (4.9 to 4.16, 4.24). It starts the
//! phases and the child (4.17 to 4.19, 4.25) and runs the pump (4.20 to 4.22). It handles cancel
//! and timeout (4.27, 4.28) and the end of the run (4.23). It also follows a child that lives
//! after a restart of Pupitre (4.29).

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use pupitre_core::doc::{DocKey, DocKind, Document};
use pupitre_core::ids::{HostId, KeyPath};
use pupitre_core::json::{self, Json};
use pupitre_core::manifest::SnapshotHash;
use pupitre_core::node::SecretScope;
use pupitre_core::run::{HostStats, RunKind, RunMode, RunReason, RunRecord, RunStatus, RunnerFacts};
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::secret::SecretValue;
use pupitre_store::types::{LeaseName, StoreError, StoredDoc};
use tokio::time::Instant;

use crate::child::{Phase, RunFolders, child_environment, command_args};
use crate::executor::adopt::AdoptInfo;
use crate::executor::guard::check_apply_guard;
use crate::executor::log::{self, Level};
use crate::executor::pump::{Progress, Pump, PumpLimits, Resume, scan_stats};
use crate::executor::shared::{ActiveRun, Shared};
use crate::process::{Child, Exit, Probe, ProcessControl, ProcessIdentity, SpawnSpec, StopSignal};
use crate::scrub::Scrubber;
use crate::snapshot::{SnapshotError, SnapshotInput, build_snapshot, disk};

/// How a supervisor starts.
pub(crate) enum Start {
    /// A run that this process just took from the queue.
    Fresh,
    /// A run whose child lives after a restart of Pupitre (4.29).
    Adopted(AdoptInfo),
}

/// The supervisor ends with no change of the record: the runner stops, or the record is gone.
struct Stop;

/// The end of a run (4.23).
#[derive(Debug, Clone)]
pub(crate) struct Outcome {
    status: RunStatus,
    reason: Option<RunReason>,
    exit_code: Option<u64>,
    stats: Option<BTreeMap<HostId, HostStats>>,
}

impl Outcome {
    fn new(status: RunStatus, reason: Option<RunReason>) -> Outcome {
        Outcome { status, reason, exit_code: None, stats: None }
    }

    fn failed(reason: RunReason) -> Outcome {
        Outcome::new(RunStatus::Failed, Some(reason))
    }
}

/// A failure before the child starts: the reason, and the lines for the output.
struct Failure {
    reason: RunReason,
    lines: Vec<String>,
}

impl Failure {
    fn new(reason: RunReason, line: impl Into<String>) -> Failure {
        Failure { reason, lines: vec![line.into()] }
    }
}

/// What the run needs after the build.
struct Prepared {
    main_playbook: std::path::PathBuf,
    render_playbook: Option<std::path::PathBuf>,
    has_params: bool,
    scrub_values: Vec<Vec<u8>>,
}

/// The child that a phase follows.
enum Handle<C> {
    Own(C),
    Adopted(ProcessIdentity),
}

/// The end of a child.
enum Ended {
    Exit(Exit),
    /// The child of an adopted run is gone: its exit code is not readable (4.29).
    Unknown,
}

impl<C: Child> Handle<C> {
    fn identity(&self) -> ProcessIdentity {
        match self {
            Handle::Own(c) => c.identity().clone(),
            Handle::Adopted(i) => i.clone(),
        }
    }

    async fn wait<P: ProcessControl<Child = C>>(&mut self, process: &P, period: Duration) -> Ended {
        match self {
            Handle::Own(child) => match child.wait().await {
                Ok(exit) => Ended::Exit(exit),
                Err(e) => {
                    log::store_error("wait for the child", &e);
                    Ended::Unknown
                }
            },
            Handle::Adopted(identity) => loop {
                // The check of 4.29: the same pid with the same start time.
                if process.probe(identity) == crate::process::Probe::Gone {
                    return Ended::Unknown;
                }
                tokio::time::sleep(period).await;
            },
        }
    }
}

/// The end of the follow of one phase.
enum End {
    Exited(Exit),
    Unknown,
    Canceled,
    TimedOut,
    LeaseLost,
}

enum Next {
    Continue,
    Finish(Outcome),
}

pub(crate) async fn supervise<S, P>(sh: Arc<Shared<S, P>>, active: Arc<ActiveRun>, run: RunRecord, start: Start)
where
    S: Store + 'static,
    P: ProcessControl + 'static,
    P::Child: 'static,
{
    let folders = RunFolders::of(&sh.settings, &run.id.to_string());
    let timeout = sh.settings.run_timeout;
    let spent = run.started_at.map_or(0, |t| UtcMillis::now().since(t));
    let remaining = timeout.saturating_sub(Duration::from_millis(spent));
    // The run leaves the list of active runs when the task ends, also after a panic.
    let _forget = Forget { sh: sh.clone(), id: run.id };
    let mut sup = Supervisor {
        sh: sh.clone(),
        active,
        run: run.clone(),
        folders,
        deadline: Instant::now() + remaining,
        hash: None,
    };
    // An error means that the supervisor stops with no change of the record: the run stays as it is.
    if sup.execute(start).await.is_err() && !sh.is_stopping() {
        // The record is gone, it has another status, or the store refused a write. No supervisor
        // follows the run any more, so no timeout and no cancel reach its child. Stop the child
        // and remove the private folder (4.13) before the environment is free (4.7): else a new
        // run could start next to the child.
        sup.abandon().await;
        let name = LeaseName::Environment(run.environment.clone());
        let _ = sh.store.release_lease(&name, &sh.holder(&run.id)).await;
        sh.wake.wake();
    }
}

/// Remove a run from the active runs when its task ends.
struct Forget<S: Store, P: ProcessControl> {
    sh: Arc<Shared<S, P>>,
    id: pupitre_core::ids::RunId,
}

impl<S: Store, P: ProcessControl> Drop for Forget<S, P> {
    fn drop(&mut self) {
        if let Ok(mut map) = self.sh.active.lock() {
            map.remove(&self.id);
        }
    }
}

struct Supervisor<S: Store, P: ProcessControl> {
    sh: Arc<Shared<S, P>>,
    active: Arc<ActiveRun>,
    run: RunRecord,
    folders: RunFolders,
    deadline: Instant,
    hash: Option<SnapshotHash>,
}

fn limits_of(settings: &crate::settings::RunnerSettings) -> PumpLimits {
    PumpLimits {
        max_log_bytes: settings.max_log_bytes,
        max_events: settings.max_events,
        max_event_bytes: usize::try_from(settings.max_event_bytes).unwrap_or(usize::MAX),
    }
}

impl<S, P> Supervisor<S, P>
where
    S: Store + 'static,
    P: ProcessControl + 'static,
    P::Child: 'static,
{
    async fn execute(&mut self, start: Start) -> Result<(), Stop> {
        let (prepared, phases, adopted, mut pump) = match start {
            Start::Fresh => {
                let prepared = match self.prepare().await? {
                    Ok(p) => p,
                    Err(failure) => return self.finish_failure(failure).await,
                };
                let render = prepared.render_playbook.is_some() && self.run.mode == RunMode::Check;
                let phases = if render { vec![Phase::Render, Phase::Main] } else { vec![Phase::Main] };
                let scrub = Scrubber::new(prepared.scrub_values.iter().map(Vec::as_slice));
                let pump = Pump::fresh(
                    self.sh.store.clone(),
                    self.run.id,
                    &self.folders.run,
                    scrub,
                    limits_of(&self.sh.settings),
                    phases[0],
                );
                (prepared, phases, None, pump)
            }
            Start::Adopted(info) => {
                self.hash = self.run.snapshot;
                let Some(main) = self.sh.caps.playbooks.get(&self.run.playbook).map(|p| p.path.clone()) else {
                    // The catalog does not hold the playbook any more: the child is not ours to follow.
                    let _ = self.sh.process.signal(&info.identity, StopSignal::Term);
                    return self
                        .finish_outcome(Outcome::new(RunStatus::Lost, Some(RunReason::ProcessGone)), None)
                        .await;
                };
                let prepared = Prepared {
                    main_playbook: main,
                    render_playbook: None,
                    has_params: self.run.kind == RunKind::Command,
                    scrub_values: info.scrub_values.clone(),
                };
                let phases =
                    if info.phase == Phase::Render { vec![Phase::Render, Phase::Main] } else { vec![Phase::Main] };
                let scrub = Scrubber::new(prepared.scrub_values.iter().map(Vec::as_slice));
                let marks = (self.run.output_truncated, self.run.events_truncated);
                let pump = match self
                    .sh
                    .call("read_output", || {
                        Pump::resume(
                            self.sh.store.clone(),
                            self.run.id,
                            &self.folders.run,
                            scrub.clone(),
                            limits_of(&self.sh.settings),
                            Resume { progress: info.progress, phase: info.phase, marks },
                        )
                    })
                    .await
                {
                    Some(Ok(p)) => p,
                    Some(Err(e)) => {
                        log::store_error("adopt", &e);
                        return Err(Stop);
                    }
                    None => return Err(Stop),
                };
                (prepared, phases, Some(info.identity), pump)
            }
        };
        let outcome = self.run_phases(&prepared, &phases, adopted, &mut pump).await?;
        self.finish_outcome(outcome, Some(&pump)).await
    }

    /// Stop the child of the run, when one runs, and remove the private folder of the run. The
    /// supervisor calls it when it stops with no record to write and the runner does not stop.
    async fn abandon(&self) {
        if let Some(identity) = self.active.identity() {
            log::store_error("the record of the run cannot change: stop its child", &self.run.id);
            stop_unfollowed_child(&self.sh, &identity).await;
            self.active.set_identity(None);
        }
        let secrets = self.folders.secrets.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || disk::remove_tree(&secrets)).await.unwrap_or(Ok(())) {
            log::store_error("remove the secrets folder", &e);
        }
    }

    // The snapshot and the guard (4.9 to 4.16, 4.24).

    async fn prepare(&mut self) -> Result<Result<Prepared, Failure>, Stop> {
        let sh = self.sh.clone();
        let run = self.run.clone();
        for _ in 0..3 {
            let docs = match sh.call("read_environment", || sh.store.read_environment(&run.environment)).await {
                None => return Err(Stop),
                Some(Ok(d)) => d,
                Some(Err(e)) => {
                    return Ok(Err(Failure::new(RunReason::SnapshotFailed, format!("read the environment: {e}"))));
                }
            };
            let scopes = scopes_of(&docs, &run);
            let before = match self.fingerprints(&scopes).await? {
                Ok(f) => f,
                Err(failure) => return Ok(Err(failure)),
            };
            let secrets = match sh.call("read_secrets", || sh.store.read_secrets(&scopes)).await {
                None => return Err(Stop),
                Some(Ok(s)) => s,
                Some(Err(StoreError::SecretUnreadable)) => {
                    return Ok(Err(Failure::new(RunReason::SecretUnreadable, "a secret value does not decrypt")));
                }
                Some(Err(e)) => {
                    return Ok(Err(Failure::new(RunReason::SnapshotFailed, format!("read the secrets: {e}"))));
                }
            };
            let after = match self.fingerprints(&scopes).await? {
                Ok(f) => f,
                Err(failure) => return Ok(Err(failure)),
            };
            if before != after {
                // A secret changed between the two reads: read again.
                continue;
            }
            let scrub_values: Vec<Vec<u8>> = secrets.iter().map(|(_, _, v)| v.expose().to_vec()).collect();
            let built = self.build(docs, secrets, before).await;
            let built = match built {
                Ok(b) => b,
                Err(SnapshotError::InvalidValues(errors)) => {
                    let mut lines = vec!["the values of the run are not valid".to_string()];
                    lines.extend(errors.iter().map(|e| match e.layer {
                        Some(layer) => format!("{e} (layer {layer})"),
                        None => e.to_string(),
                    }));
                    return Ok(Err(Failure { reason: RunReason::InvalidValues, lines }));
                }
                Err(SnapshotError::Io(text)) => {
                    return Ok(Err(Failure::new(
                        RunReason::SnapshotFailed,
                        format!("the snapshot cannot be built: {text}"),
                    )));
                }
            };
            self.hash = Some(built.hash);
            let guard = self.guard(&built.hash).await?;
            if let Err(failure) = guard {
                return Ok(Err(failure));
            }
            let main = built.manifest.playbooks.values().find(|p| p.id == run.playbook);
            let Some(main) = main else {
                return Ok(Err(Failure::new(RunReason::SnapshotFailed, "the snapshot has no main playbook")));
            };
            return Ok(Ok(Prepared {
                main_playbook: main.path.clone().into(),
                render_playbook: built.manifest.playbooks.get("render").map(|p| p.path.clone().into()),
                has_params: built.has_params,
                scrub_values,
            }));
        }
        Ok(Err(Failure::new(RunReason::SnapshotFailed, "a secret changed three times while the snapshot was built")))
    }

    /// The fingerprints of the secrets of the scopes (2.26).
    async fn fingerprints(
        &self,
        scopes: &[SecretScope],
    ) -> Result<Result<BTreeMap<(SecretScope, KeyPath), String>, Failure>, Stop> {
        let sh = &self.sh;
        let mut out = BTreeMap::new();
        for scope in scopes {
            match sh.call("list_secrets", || sh.store.list_secrets(scope)).await {
                None => return Err(Stop),
                Some(Ok(list)) => {
                    for meta in list {
                        out.insert((meta.scope, meta.name), meta.fingerprint);
                    }
                }
                Some(Err(e)) => {
                    return Ok(Err(Failure::new(RunReason::SnapshotFailed, format!("list the secrets: {e}"))));
                }
            }
        }
        Ok(Ok(out))
    }

    async fn build(
        &self,
        docs: Vec<StoredDoc>,
        secrets: Vec<(SecretScope, KeyPath, SecretValue)>,
        fingerprints: BTreeMap<(SecretScope, KeyPath), String>,
    ) -> Result<crate::snapshot::BuiltSnapshot, SnapshotError> {
        let run = self.run.clone();
        let caps = self.sh.caps.clone();
        let settings = self.sh.settings.clone();
        let folders = self.folders.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let input = SnapshotInput {
                run: &run,
                documents: &docs,
                capabilities: &caps,
                secrets: &secrets,
                settings: &settings,
                fingerprints: &fingerprints,
            };
            build_snapshot(&input, &folders)
        })
        .await;
        joined.unwrap_or_else(|e| Err(SnapshotError::Io(format!("the build stopped: {e}"))))
    }

    /// The guard of an apply at the start (4.24).
    async fn guard(&self, hash: &SnapshotHash) -> Result<Result<(), Failure>, Stop> {
        let Some(check_id) = self.run.check_run else { return Ok(Ok(())) };
        let sh = &self.sh;
        let check = match sh.call("get_run", || sh.store.get_run(&check_id)).await {
            None => return Err(Stop),
            Some(Ok(c)) => c,
            Some(Err(StoreError::NotFound)) => {
                return Ok(Err(Failure::new(RunReason::SnapshotChanged, "the check run does not exist")));
            }
            Some(Err(e)) => {
                return Ok(Err(Failure::new(RunReason::SnapshotFailed, format!("read the check run: {e}"))));
            }
        };
        Ok(check_apply_guard(&self.run, &check, hash, UtcMillis::now(), sh.settings.check_max_age).map_err(|reason| {
            let text = match reason {
                RunReason::CheckTooOld => "the check run is older than runner.check_max_age",
                _ => "the snapshot of the apply is not the snapshot of its check",
            };
            Failure::new(reason, text)
        }))
    }

    // The phases and the child (4.17 to 4.19, 4.25).

    async fn run_phases(
        &mut self,
        prepared: &Prepared,
        phases: &[Phase],
        adopted: Option<ProcessIdentity>,
        pump: &mut Pump<S>,
    ) -> Result<Outcome, Stop> {
        let mut adopted = adopted;
        for phase in phases {
            let handle = match adopted.take() {
                Some(identity) => Handle::Adopted(identity),
                None => {
                    if self.active.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
                        return Ok(Outcome::new(RunStatus::Canceled, None));
                    }
                    if self.active.lease_lost.load(std::sync::atomic::Ordering::SeqCst) {
                        return Ok(Outcome::new(RunStatus::Lost, Some(RunReason::NodeChanged)));
                    }
                    pump.set_phase(*phase);
                    match self.start_phase(prepared, *phase).await? {
                        Ok(h) => h,
                        Err(failure) => {
                            self.append_lines(&failure.lines).await?;
                            return Ok(Outcome::failed(failure.reason));
                        }
                    }
                }
            };
            let end = self.follow(handle, pump).await?;
            self.active.set_identity(None);
            self.drain(pump).await?;
            match self.decide(*phase, end, pump).await {
                Next::Continue => {}
                Next::Finish(mut outcome) => {
                    if outcome.stats.is_none() && *phase == Phase::Main {
                        outcome.stats = pump.stats().cloned();
                    }
                    return Ok(outcome);
                }
            }
        }
        // Not reached: the last phase is `Main`, and `decide` finishes it.
        Ok(Outcome::failed(RunReason::ExitCode))
    }

    async fn start_phase(
        &mut self,
        prepared: &Prepared,
        phase: Phase,
    ) -> Result<Result<Handle<P::Child>, Failure>, Stop> {
        let sh = self.sh.clone();
        let playbook = match phase {
            Phase::Render => prepared.render_playbook.clone().unwrap_or_default(),
            Phase::Main => prepared.main_playbook.clone(),
        };
        // The render phase runs in the mode `apply` (4.25). Only the main phase of a check
        // adds `--check --diff`.
        let check = phase == Phase::Main && self.run.mode == RunMode::Check;
        let spec = SpawnSpec {
            program: sh.settings.ansible_playbook.clone(),
            args: command_args(&self.folders, &playbook, check, prepared.has_params),
            env: child_environment(&sh.settings, &self.folders, phase),
            cwd: self.folders.run.clone(),
            output: self.folders.run.join("output.log"),
        };
        let child = match sh.process.spawn(&spec) {
            Ok(c) => c,
            Err(e) => return Ok(Err(Failure::new(RunReason::SpawnFailed, format!("the program cannot start: {e}")))),
        };
        let identity = child.identity().clone();
        self.active.set_identity(Some(identity.clone()));
        let pid_file = Json::object([
            ("boot_id", Json::str(identity.boot_id.clone())),
            ("phase", Json::str(phase.name())),
            ("pid", Json::Int(u64::from(identity.pid))),
            ("start_time", Json::Int(identity.start_time)),
        ]);
        if let Err(e) = disk::write_file(&self.folders.run.join("pid"), &json::to_canonical(&pid_file), 0o600) {
            // Without the file, a restart cannot adopt the run (4.29): the run still goes on.
            log::store_error("write the file pid", &e);
        }
        let facts = RunnerFacts {
            node: sh.node_id.clone(),
            pid: u64::from(identity.pid),
            start_time: identity.start_time,
            boot_id: identity.boot_id.clone(),
        };
        let hash = self.hash;
        self.update_record(move |r| {
            r.runner = Some(facts.clone());
            r.snapshot = hash;
        })
        .await?;
        Ok(Ok(Handle::Own(child)))
    }

    /// Follow a child until its end, a cancel, the timeout or the loss of the lease. The pump
    /// ticks while the child runs (4.22).
    async fn follow(&mut self, handle: Handle<P::Child>, pump: &mut Pump<S>) -> Result<End, Stop> {
        let mut handle = handle;
        let sh = self.sh.clone();
        let active = self.active.clone();
        let identity = handle.identity();
        let timing = sh.options.timing;
        let wait = handle.wait(&sh.process, timing.probe);
        tokio::pin!(wait);
        let mut ticker = tokio::time::interval(timing.pump);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut marks = (self.run.output_truncated, self.run.events_truncated);
        let end = loop {
            tokio::select! {
                ended = &mut wait => {
                    break match ended {
                        Ended::Exit(exit) => End::Exited(exit),
                        Ended::Unknown => End::Unknown,
                    };
                }
                () = active.cancel.notified() => {
                    stop_child(&sh, &identity, wait.as_mut()).await;
                    break End::Canceled;
                }
                () = tokio::time::sleep_until(self.deadline) => {
                    stop_child(&sh, &identity, wait.as_mut()).await;
                    break End::TimedOut;
                }
                () = active.lost.notified() => {
                    stop_child(&sh, &identity, wait.as_mut()).await;
                    break End::LeaseLost;
                }
                _ = ticker.tick() => {
                    if let Err(e) = pump.tick().await {
                        log::store_error("add output or events", &e);
                    }
                    let now = (pump.output_truncated(), pump.events_truncated());
                    if now != marks {
                        marks = now;
                        self.update_record(move |r| {
                            r.output_truncated |= now.0;
                            r.events_truncated |= now.1;
                        })
                        .await?;
                    }
                }
            }
        };
        Ok(end)
    }

    /// Add all that remains of the output and the events (4.22).
    async fn drain(&mut self, pump: &mut Pump<S>) -> Result<(), Stop> {
        loop {
            match pump.drain().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::store_error("add output or events", &e);
                    if self.sh.is_stopping() {
                        return Err(Stop);
                    }
                    tokio::time::sleep(self.sh.settings.poll_interval).await;
                }
            }
        }
    }

    /// The status of a run after a phase (4.23).
    async fn decide(&self, phase: Phase, end: End, pump: &Pump<S>) -> Next {
        let fail_reason = if phase == Phase::Render { RunReason::RenderFailed } else { RunReason::ExitCode };
        match end {
            End::Canceled => Next::Finish(Outcome::new(RunStatus::Canceled, None)),
            End::TimedOut => Next::Finish(Outcome::failed(RunReason::Timeout)),
            End::LeaseLost => Next::Finish(Outcome::new(RunStatus::Lost, Some(RunReason::NodeChanged))),
            End::Exited(Exit::Code(0)) => match phase {
                Phase::Render => Next::Continue,
                Phase::Main => {
                    let mut o = Outcome::new(RunStatus::Passed, None);
                    o.exit_code = Some(0);
                    Next::Finish(o)
                }
            },
            End::Exited(Exit::Code(code)) => {
                let mut o = Outcome::failed(fail_reason);
                o.exit_code = Some(u64::from(code));
                Next::Finish(o)
            }
            End::Exited(Exit::Signal(_)) => Next::Finish(Outcome::failed(fail_reason)),
            End::Unknown => {
                // The exit code of an adopted child is not readable: the event `stats` tells (4.23).
                let path = self.folders.run.join("events.jsonl");
                let from = pump.phase_events_start();
                let max = usize::try_from(self.sh.settings.max_event_bytes).unwrap_or(usize::MAX);
                let stats = tokio::task::spawn_blocking(move || scan_stats(&path, from, max)).await.ok().flatten();
                match stats {
                    None => Next::Finish(Outcome::failed(RunReason::LostExit)),
                    Some(stats) => {
                        let bad = stats.values().any(|s| s.failed > 0 || s.unreachable > 0);
                        match (phase, bad) {
                            (Phase::Render, false) => Next::Continue,
                            (Phase::Render, true) => Next::Finish(Outcome::failed(RunReason::RenderFailed)),
                            (Phase::Main, false) => {
                                let mut o = Outcome::new(RunStatus::Passed, None);
                                o.stats = Some(stats);
                                Next::Finish(o)
                            }
                            (Phase::Main, true) => {
                                let mut o = Outcome::failed(RunReason::HostFailed);
                                o.stats = Some(stats);
                                Next::Finish(o)
                            }
                        }
                    }
                }
            }
        }
    }

    // The record (4.38) and the end (4.23).

    /// Change the record of the run: read it again, change it, and write it with a compare
    /// and set of the status `running` (2.27). A transient error starts the call again.
    async fn update_record<F>(&self, change: F) -> Result<RunRecord, Stop>
    where
        F: Fn(&mut RunRecord),
    {
        let sh = &self.sh;
        let id = self.run.id;
        let result = sh
            .call("update_run", || async {
                let mut record = sh.store.get_run(&id).await?;
                change(&mut record);
                sh.store.update_run(RunStatus::Running, &record).await?;
                Ok(record)
            })
            .await;
        match result {
            Some(Ok(record)) => Ok(record),
            // The record is gone or it is not `running`: nothing more to write.
            Some(Err(_)) | None => Err(Stop),
        }
    }

    /// Add lines of text to the output of a run that did not start a child.
    async fn append_lines(&self, lines: &[String]) -> Result<(), Stop> {
        let sh = &self.sh;
        let mut text = String::new();
        for line in lines {
            text.push_str("pupitre: ");
            text.push_str(line);
            text.push('\n');
        }
        let id = self.run.id;
        let result = sh
            .call("append_output", || async {
                let length = sh.store.read_output(&id, 0, 0).await?.length;
                sh.store.append_output(&id, length, text.as_bytes()).await
            })
            .await;
        match result {
            None => Err(Stop),
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => {
                log::store_error("append_output", &e);
                Ok(())
            }
        }
    }

    async fn finish_failure(&mut self, failure: Failure) -> Result<(), Stop> {
        self.append_lines(&failure.lines).await?;
        self.finish_outcome(Outcome::failed(failure.reason), None).await
    }

    /// Write the end of the run, release the lease, and clean (4.23, 4.31).
    async fn finish_outcome(&mut self, outcome: Outcome, pump: Option<&Pump<S>>) -> Result<(), Stop> {
        // The secrets folder goes first, whatever the status (4.13).
        let secrets = self.folders.secrets.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || disk::remove_tree(&secrets)).await.unwrap_or(Ok(())) {
            log::store_error("remove the secrets folder", &e);
        }
        let hash = self.hash;
        let marks = pump.map(|p| (p.output_truncated(), p.events_truncated())).unwrap_or((false, false));
        let o = outcome.clone();
        let now = UtcMillis::now();
        let record = self
            .update_record(move |r| {
                r.status = o.status;
                r.reason = o.reason;
                r.exit_code = o.exit_code;
                r.finished_at = Some(now);
                r.stats = o.stats.clone();
                if r.snapshot.is_none() {
                    r.snapshot = hash;
                }
                r.output_truncated |= marks.0;
                r.events_truncated |= marks.1;
            })
            .await?;
        let sh = self.sh.clone();
        let name = LeaseName::Environment(self.run.environment.clone());
        if let Err(e) = sh.store.release_lease(&name, &sh.holder(&self.run.id)).await {
            log::store_error("release_lease", &e);
        }
        if let Ok(mut map) = sh.active.lock() {
            map.remove(&self.run.id);
        }
        log_finished(&record);
        // The next queued run can start at once.
        sh.wake.wake();
        crate::executor::cleanup::after_run(&sh).await;
        Ok(())
    }
}

/// The log lines of the end of a run (6.27).
pub(crate) fn log_finished(record: &RunRecord) {
    let mut fields = vec![
        ("run", Json::str(record.id.to_string())),
        ("kind", Json::str(record.kind.name())),
        ("mode", Json::str(record.mode.name())),
        ("node", Json::str(record.node.path())),
        ("hosts", Json::Arr(record.hosts.iter().map(|h| Json::str(h.as_str())).collect())),
        ("status", Json::str(record.status.name())),
        ("subject", Json::str(record.subject.as_str())),
    ];
    if let Some(r) = record.reason {
        fields.push(("reason", Json::str(r.name())));
    }
    if let Some(h) = &record.snapshot {
        fields.push(("snapshot", Json::str(h.to_string())));
    }
    log::emit(Level::Info, "run.finished", &fields);
    if record.status == RunStatus::Lost {
        let reason = record.reason.map_or("", RunReason::name);
        log::emit(Level::Warn, "run.lost", &[("run", Json::str(record.id.to_string())), ("reason", Json::str(reason))]);
    }
}

/// Stop a child (4.19, 4.27): `SIGTERM` to the process group, then `SIGKILL` after
/// `runner.cancel_grace`.
async fn stop_child<S, P, F>(sh: &Shared<S, P>, identity: &ProcessIdentity, mut wait: Pin<&mut F>)
where
    S: Store,
    P: ProcessControl,
    F: Future<Output = Ended>,
{
    if let Err(e) = sh.process.signal(identity, StopSignal::Term) {
        log::store_error("send SIGTERM", &e);
    }
    if tokio::time::timeout(sh.settings.cancel_grace, wait.as_mut()).await.is_err() {
        if let Err(e) = sh.process.signal(identity, StopSignal::Kill) {
            log::store_error("send SIGKILL", &e);
        }
        wait.await;
    }
}

/// Stop a child that no `wait` follows (4.19): `SIGTERM` to the process group, then `SIGKILL`
/// after `runner.cancel_grace`. The probe tells when the child is gone.
async fn stop_unfollowed_child<S: Store, P: ProcessControl>(sh: &Shared<S, P>, identity: &ProcessIdentity) {
    if let Err(e) = sh.process.signal(identity, StopSignal::Term) {
        log::store_error("send SIGTERM", &e);
    }
    let end = Instant::now() + sh.settings.cancel_grace;
    while Instant::now() < end && sh.process.probe(identity) == Probe::Alive {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if sh.process.probe(identity) == Probe::Alive
        && let Err(e) = sh.process.signal(identity, StopSignal::Kill)
    {
        log::store_error("send SIGKILL", &e);
    }
}

/// The scopes of the secrets that a run reads: the environment, and each unit that holds a
/// host of the run (4.13). A document that is not valid is left to the snapshot builder.
fn scopes_of(docs: &[StoredDoc], run: &RunRecord) -> Vec<SecretScope> {
    let hosts: BTreeSet<&HostId> = run.hosts.iter().collect();
    let mut scopes = vec![SecretScope::Environment(run.environment.clone())];
    for stored in docs {
        let DocKey::Unit(env, unit_id) = &stored.key else { continue };
        if env != &run.environment {
            continue;
        }
        if let Ok(Document::Unit(unit)) = Document::from_stored(DocKind::Unit, &stored.bytes, stored.bytes.len())
            && unit.hosts.iter().any(|h| hosts.contains(h))
        {
            scopes.push(SecretScope::Unit(env.clone(), unit_id.clone()));
        }
    }
    scopes.sort();
    scopes.dedup();
    scopes
}

/// Read the values of a private folder of a run (4.13): the scrubber of an adopted run (4.29).
pub(crate) fn read_private_values(folders: &RunFolders) -> Option<Vec<Vec<u8>>> {
    fn walk(dir: &std::path::Path, out: &mut Vec<Vec<u8>>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), out)?;
            } else {
                out.push(std::fs::read(entry.path())?);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(&folders.secrets.join("values"), &mut out).ok()?;
    Some(out)
}

/// The place of the pump after a restart (4.29), from `progress.json`.
pub(crate) fn read_progress(folders: &RunFolders) -> Option<Progress> {
    Progress::read(&folders.run.join("progress.json"))
}
