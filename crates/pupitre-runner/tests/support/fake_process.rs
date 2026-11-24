// SPDX-License-Identifier: MIT OR Apache-2.0
//! A fake `ProcessControl`. A child does what a script says. It writes bytes in the output file
//! and lines in the events file. Then it ends after a delay or when the test says so. A signal
//! ends it, unless the script ignores `SIGTERM`.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pupitre_runner::process::{
    Child, Exit, Probe, ProcessControl, ProcessError, ProcessIdentity, SpawnSpec, StopSignal,
};
use tokio::sync::oneshot;

/// What a fake child does.
#[derive(Debug, Clone, Default)]
pub struct Script {
    /// Bytes for the output file, written at the start.
    pub output: Vec<u8>,
    /// Lines for the events file, written at the start. Each line gets a line end.
    pub events: Vec<String>,
    /// Bytes for the events file after the lines, with no line end.
    pub events_tail: String,
    /// The end of the child: after `after`, with this exit. `None` waits for `finish` or a signal.
    pub exit: Option<(Duration, Exit)>,
    /// True when `SIGTERM` does nothing.
    pub ignore_term: bool,
    /// True when the spawn fails.
    pub fail_spawn: bool,
}

struct Live {
    tx: Option<oneshot::Sender<Exit>>,
    alive: bool,
    ignore_term: bool,
    identity: ProcessIdentity,
}

#[derive(Default)]
struct Inner {
    next_pid: u32,
    live: BTreeMap<u32, Live>,
    external: BTreeMap<u32, bool>,
    scripts: VecDeque<Script>,
    default_script: Script,
    spawns: Vec<SpawnSpec>,
    signals: Vec<(u32, StopSignal)>,
}

/// The fake. Clone it: the clones share the state.
#[derive(Clone, Default)]
pub struct FakeProcess {
    inner: Arc<Mutex<Inner>>,
}

pub struct FakeChild {
    identity: ProcessIdentity,
    rx: Option<oneshot::Receiver<Exit>>,
}

impl Child for FakeChild {
    fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    async fn wait(&mut self) -> Result<Exit, ProcessError> {
        match self.rx.take() {
            Some(rx) => rx.await.map_err(|_| ProcessError("the fake child was dropped".into())),
            None => Err(ProcessError("wait called two times".into())),
        }
    }
}

impl FakeProcess {
    pub fn new() -> FakeProcess {
        let f = FakeProcess::default();
        f.inner.lock().unwrap().next_pid = 4000;
        f
    }

    /// The script of the next spawn.
    pub fn push_script(&self, script: Script) {
        self.inner.lock().unwrap().scripts.push_back(script);
    }

    /// The script when the queue is empty.
    pub fn set_default(&self, script: Script) {
        self.inner.lock().unwrap().default_script = script;
    }

    pub fn spawns(&self) -> Vec<SpawnSpec> {
        self.inner.lock().unwrap().spawns.clone()
    }

    pub fn signals(&self) -> Vec<StopSignal> {
        self.inner.lock().unwrap().signals.iter().map(|(_, s)| *s).collect()
    }

    /// End the child of a pid with an exit.
    pub fn finish(&self, pid: u32, exit: Exit) {
        let mut g = self.inner.lock().unwrap();
        if let Some(l) = g.live.get_mut(&pid) {
            l.alive = false;
            if let Some(tx) = l.tx.take() {
                let _ = tx.send(exit);
            }
        }
    }

    /// The pid of the last spawned child.
    pub fn last_pid(&self) -> Option<u32> {
        self.inner.lock().unwrap().live.keys().next_back().copied()
    }

    /// A process that lives outside this process, for an adoption test. It is alive until
    /// `kill_external`.
    pub fn add_external(&self, pid: u32) -> ProcessIdentity {
        let identity = ProcessIdentity { pid, start_time: 777, boot_id: "boot-1".to_string() };
        self.inner.lock().unwrap().external.insert(pid, true);
        identity
    }

    pub fn kill_external(&self, pid: u32) {
        self.inner.lock().unwrap().external.insert(pid, false);
    }
}

impl ProcessControl for FakeProcess {
    type Child = FakeChild;

    fn spawn(&self, spec: &SpawnSpec) -> Result<FakeChild, ProcessError> {
        let (script, pid) = {
            let mut g = self.inner.lock().unwrap();
            g.spawns.push(spec.clone());
            let script = g.scripts.pop_front().unwrap_or_else(|| g.default_script.clone());
            g.next_pid += 1;
            (script, g.next_pid)
        };
        if script.fail_spawn {
            return Err(ProcessError("the fake program cannot start".into()));
        }
        if !script.output.is_empty() {
            let mut f = std::fs::OpenOptions::new().append(true).create(true).open(&spec.output).unwrap();
            f.write_all(&script.output).unwrap();
        }
        if !script.events.is_empty() || !script.events_tail.is_empty() {
            let path = spec.env.iter().find(|(k, _)| k == "PUPITRE_EVENTS_FILE").map(|(_, v)| v.clone()).unwrap();
            let mut f = std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
            for line in &script.events {
                writeln!(f, "{line}").unwrap();
            }
            write!(f, "{}", script.events_tail).unwrap();
        }
        let (tx, rx) = oneshot::channel();
        let identity = ProcessIdentity { pid, start_time: 1000 + u64::from(pid), boot_id: "boot-1".to_string() };
        self.inner.lock().unwrap().live.insert(
            pid,
            Live { tx: Some(tx), alive: true, ignore_term: script.ignore_term, identity: identity.clone() },
        );
        if let Some((after, exit)) = script.exit {
            let me = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                me.finish(pid, exit);
            });
        }
        Ok(FakeChild { identity, rx: Some(rx) })
    }

    fn probe(&self, identity: &ProcessIdentity) -> Probe {
        let g = self.inner.lock().unwrap();
        if let Some(l) = g.live.get(&identity.pid) {
            return if l.alive && l.identity == *identity { Probe::Alive } else { Probe::Gone };
        }
        match g.external.get(&identity.pid) {
            Some(true) if identity.start_time == 777 && identity.boot_id == "boot-1" => Probe::Alive,
            _ => Probe::Gone,
        }
    }

    fn signal(&self, identity: &ProcessIdentity, signal: StopSignal) -> Result<(), ProcessError> {
        let mut g = self.inner.lock().unwrap();
        g.signals.push((identity.pid, signal));
        if let Some(l) = g.live.get_mut(&identity.pid) {
            let ends = match signal {
                StopSignal::Kill => true,
                StopSignal::Term => !l.ignore_term,
            };
            if ends && l.alive {
                l.alive = false;
                if let Some(tx) = l.tx.take() {
                    let _ = tx.send(Exit::Signal(if signal == StopSignal::Kill { 9 } else { 15 }));
                }
            }
        } else if g.external.get(&identity.pid) == Some(&true) {
            g.external.insert(identity.pid, false);
        }
        Ok(())
    }
}
