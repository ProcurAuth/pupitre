// SPDX-License-Identifier: MIT OR Apache-2.0
//! Runs: kinds and modes (4.3), the request (4.4), the record (4.38) and the status
//! transitions (4.39).

use std::collections::BTreeMap;

use crate::action::Action;
use crate::command::CanonicalParams;
use crate::error::FieldError;
use crate::ids::{CommandId, EnvId, HostId, IdError, PlaybookId, RunId, Subject};
use crate::json::{Json, Members};
use crate::manifest::SnapshotHash;
use crate::node::Node;
use crate::time::UtcMillis;

/// The kind of a run (4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunKind {
    /// `deploy`
    Deploy,
    /// `status`
    Status,
    /// `command`
    Command,
}

/// The mode of a run (4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunMode {
    /// `check`: `--check --diff`.
    Check,
    /// `apply`: no such flag.
    Apply,
}

/// The status of a run (4.38, 4.39).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    /// `queued`
    Queued,
    /// `running`
    Running,
    /// `passed`
    Passed,
    /// `failed`
    Failed,
    /// `canceled`
    Canceled,
    /// `lost`
    Lost,
}

/// The reason of a status `failed` or `lost` (4.23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunReason {
    /// `exit_code`
    ExitCode,
    /// `render_failed`
    RenderFailed,
    /// `invalid_values`
    InvalidValues,
    /// `secret_unreadable`
    SecretUnreadable,
    /// `snapshot_failed`
    SnapshotFailed,
    /// `snapshot_changed`
    SnapshotChanged,
    /// `check_too_old`
    CheckTooOld,
    /// `spawn_failed`
    SpawnFailed,
    /// `timeout`
    Timeout,
    /// `process_gone`
    ProcessGone,
    /// `node_changed`
    NodeChanged,
    /// `lost_exit`
    LostExit,
    /// `host_failed`
    HostFailed,
}

macro_rules! names {
    ($ty:ty, [$($variant:ident => $name:expr),* $(,)?]) => {
        impl $ty {
            /// The name of the value.
            pub fn name(self) -> &'static str {
                match self { $(Self::$variant => $name),* }
            }

            /// The value of a name.
            pub fn from_name(name: &str) -> Option<Self> {
                match name { $($name => Some(Self::$variant),)* _ => None }
            }
        }
    };
}

names!(RunKind, [Deploy => "deploy", Status => "status", Command => "command"]);
names!(RunMode, [Check => "check", Apply => "apply"]);
names!(RunStatus, [Queued => "queued", Running => "running", Passed => "passed", Failed => "failed", Canceled => "canceled", Lost => "lost"]);
names!(RunReason, [
    ExitCode => "exit_code", RenderFailed => "render_failed", InvalidValues => "invalid_values",
    SecretUnreadable => "secret_unreadable", SnapshotFailed => "snapshot_failed", SnapshotChanged => "snapshot_changed",
    CheckTooOld => "check_too_old", SpawnFailed => "spawn_failed", Timeout => "timeout", ProcessGone => "process_gone",
    NodeChanged => "node_changed", LostExit => "lost_exit", HostFailed => "host_failed",
]);

impl RunStatus {
    /// True for `passed`, `failed`, `canceled` and `lost`.
    pub fn is_final(self) -> bool {
        matches!(self, RunStatus::Passed | RunStatus::Failed | RunStatus::Canceled | RunStatus::Lost)
    }

    /// The transitions of 4.39.
    pub fn can_move_to(self, next: RunStatus) -> bool {
        matches!(
            (self, next),
            (RunStatus::Queued, RunStatus::Running | RunStatus::Canceled)
                | (RunStatus::Running, RunStatus::Passed | RunStatus::Failed | RunStatus::Canceled | RunStatus::Lost)
        )
    }
}

/// The use action of a kind and a mode (4.3).
pub fn use_action(kind: RunKind, mode: RunMode) -> Action {
    match (kind, mode) {
        (RunKind::Deploy, RunMode::Check) | (RunKind::Status, _) => Action::RunCheck,
        (RunKind::Deploy, RunMode::Apply) => Action::RunApply,
        (RunKind::Command, _) => Action::CommandRun,
    }
}

/// A run request with the shape of 4.4. The server checks the rest (4.4, rules 1 to 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunRequest {
    /// A request with a node and hosts.
    Fresh {
        /// The kind.
        kind: RunKind,
        /// The mode.
        mode: RunMode,
        /// The node.
        node: Node,
        /// The hosts, in byte order, each one time.
        hosts: Vec<HostId>,
        /// The command, for a command.
        command: Option<CommandId>,
        /// The parameters, for a command, as the request gives them.
        params: Option<Json>,
        /// The message.
        message: Option<String>,
    },
    /// An apply that names its check run.
    AfterCheck {
        /// The kind: `deploy` or `command`.
        kind: RunKind,
        /// The check run.
        check_run: RunId,
        /// The message.
        message: Option<String>,
    },
}

/// The message of a run request (4.4): 0 to 500 characters with no control character except
/// U+000A and no format character (2.1).
fn message_ok(text: &str) -> bool {
    text.chars().count() <= 500 && !crate::text::has_hidden(text, &['\n'])
}

fn id_list(value: Option<Json>, errors: &mut Vec<FieldError>) -> Option<Vec<HostId>> {
    let Some(Json::Arr(items)) = value else {
        errors.push(FieldError::new("hosts", "expected an array of host ids"));
        return None;
    };
    let mut out: Vec<HostId> = Vec::with_capacity(items.len());
    for item in items {
        let id = match item.as_str().map(HostId::parse) {
            Some(Ok(id)) => id,
            Some(Err(e)) => {
                errors.push(FieldError::new("hosts", e.to_string()));
                return None;
            }
            None => {
                errors.push(FieldError::new("hosts", "expected an array of host ids"));
                return None;
            }
        };
        if out.last().is_some_and(|last| *last >= id) {
            errors.push(FieldError::new("hosts", "expected the hosts in byte order, each one time"));
            return None;
        }
        out.push(id);
    }
    if out.is_empty() {
        errors.push(FieldError::new("hosts", "expected 1 or more hosts"));
        return None;
    }
    Some(out)
}

impl RunRequest {
    /// Read the shape of a request (4.4). A wrong member, a missing member or a wrong combination
    /// of kind and mode gives the errors.
    pub fn parse(value: &Json) -> Result<RunRequest, Vec<FieldError>> {
        let mut m = Members::of(value).map_err(|e| vec![FieldError::new("", e)])?;
        let mut errors = Vec::new();
        let kind = m.take("kind").and_then(|v| v.as_str().and_then(RunKind::from_name));
        let mode = m.take("mode").and_then(|v| v.as_str().and_then(RunMode::from_name));
        let message = match m.take("message") {
            None => None,
            Some(Json::Str(t)) if message_ok(&t) => Some(t),
            Some(_) => {
                errors.push(FieldError::new(
                    "message",
                    "expected a text of 0 to 500 characters with no control character except U+000A and no format character",
                ));
                None
            }
        };
        let (Some(kind), Some(mode)) = (kind, mode) else {
            return Err(vec![FieldError::new(
                "kind",
                "expected kind deploy, status or command and mode check or apply",
            )]);
        };
        let check_run = m.take("check_run");
        let request = match (kind, mode, check_run) {
            (RunKind::Status, RunMode::Check, _) => {
                errors.push(FieldError::new("mode", "a status run has the mode apply"));
                None
            }
            (RunKind::Deploy, RunMode::Apply, None) => {
                errors.push(FieldError::new("check_run", "an apply of a unit names its check run"));
                None
            }
            (RunKind::Deploy | RunKind::Command, RunMode::Apply, Some(v)) => match v.as_str().map(RunId::parse) {
                Some(Ok(check_run)) => Some(RunRequest::AfterCheck { kind, check_run, message: message.clone() }),
                _ => {
                    errors.push(FieldError::new("check_run", "expected a run id"));
                    None
                }
            },
            (_, _, Some(_)) => {
                errors.push(FieldError::new("check_run", "only an apply names a check run"));
                None
            }
            (_, _, None) => {
                let node = match m.take("node").and_then(|v| v.as_str().map(Node::parse_path)) {
                    Some(Ok(n)) => Some(n),
                    Some(Err(e)) => {
                        errors.push(FieldError::new("node", e.to_string()));
                        None
                    }
                    None => {
                        errors.push(FieldError::new("node", "expected a node path"));
                        None
                    }
                };
                let hosts = id_list(m.take("hosts"), &mut errors);
                let (command, params) = if kind == RunKind::Command {
                    let command = match m.take("command").and_then(|v| v.as_str().map(CommandId::parse)) {
                        Some(Ok(c)) => Some(c),
                        Some(Err(e)) => {
                            errors.push(FieldError::new("command", e.to_string()));
                            None
                        }
                        None => {
                            errors.push(FieldError::new("command", "expected a command id"));
                            None
                        }
                    };
                    let params = m.take("params");
                    if params.is_none() {
                        errors.push(FieldError::new("params", "expected an object"));
                    }
                    (command, params)
                } else {
                    (None, None)
                };
                match (node, hosts) {
                    (Some(node), Some(hosts))
                        if kind != RunKind::Command || (command.is_some() && params.is_some()) =>
                    {
                        Some(RunRequest::Fresh { kind, mode, node, hosts, command, params, message: message.clone() })
                    }
                    _ => None,
                }
            }
        };
        for unknown in m.finish() {
            errors.push(FieldError::new(unknown, "unknown member"));
        }
        match request {
            Some(r) if errors.is_empty() => Ok(r),
            _ => Err(errors),
        }
    }
}

/// The facts of the child of a run (4.19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerFacts {
    /// The node id of the runner node (4.8).
    pub node: String,
    /// The pid of the child.
    pub pid: u64,
    /// Field 22 of `/proc/<pid>/stat`.
    pub start_time: u64,
    /// The boot id of the machine.
    pub boot_id: String,
}

/// The counts of one host in the event `stats` (4.20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostStats {
    /// `changed`
    pub changed: u64,
    /// `failed`
    pub failed: u64,
    /// `ignored`
    pub ignored: u64,
    /// `ok`
    pub ok: u64,
    /// `rescued`
    pub rescued: u64,
    /// `skipped`
    pub skipped: u64,
    /// `unreachable`
    pub unreachable: u64,
}

impl HostStats {
    const NAMES: [&'static str; 7] = ["changed", "failed", "ignored", "ok", "rescued", "skipped", "unreachable"];

    fn values(&self) -> [u64; 7] {
        [self.changed, self.failed, self.ignored, self.ok, self.rescued, self.skipped, self.unreachable]
    }

    /// The object of 4.20.
    pub fn to_json(&self) -> Json {
        Json::object(HostStats::NAMES.iter().zip(self.values()).map(|(n, v)| (*n, Json::Int(v))))
    }

    /// Read the object of 4.20.
    pub fn from_json(value: &Json) -> Option<HostStats> {
        let map = value.as_object()?;
        if map.len() != 7 {
            return None;
        }
        let get = |n: &str| map.get(n).and_then(Json::as_int);
        Some(HostStats {
            changed: get("changed")?,
            failed: get("failed")?,
            ignored: get("ignored")?,
            ok: get("ok")?,
            rescued: get("rescued")?,
            skipped: get("skipped")?,
            unreachable: get("unreachable")?,
        })
    }
}

/// A cancel request (4.27).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelRequest {
    /// The subject.
    pub by: Subject,
    /// The time.
    pub at: UtcMillis,
}

/// A run record (4.38).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    /// The run id.
    pub id: RunId,
    /// The kind.
    pub kind: RunKind,
    /// The mode.
    pub mode: RunMode,
    /// The environment.
    pub environment: EnvId,
    /// The node.
    pub node: Node,
    /// The hosts.
    pub hosts: Vec<HostId>,
    /// The main playbook.
    pub playbook: PlaybookId,
    /// The command.
    pub command: Option<CommandId>,
    /// The parameters of a command.
    pub params: Option<CanonicalParams>,
    /// The check run of an apply.
    pub check_run: Option<RunId>,
    /// The apply that consumed this check.
    pub applied_by: Option<RunId>,
    /// The message of the request.
    pub message: Option<String>,
    /// The subject of the request.
    pub subject: Subject,
    /// The time of the request.
    pub created_at: UtcMillis,
    /// The time of the status `running`.
    pub started_at: Option<UtcMillis>,
    /// The time of the final status.
    pub finished_at: Option<UtcMillis>,
    /// The status.
    pub status: RunStatus,
    /// The reason of `failed` or `lost`.
    pub reason: Option<RunReason>,
    /// The snapshot hash.
    pub snapshot: Option<SnapshotHash>,
    /// The facts of the child.
    pub runner: Option<RunnerFacts>,
    /// The exit code of the main phase.
    pub exit_code: Option<u64>,
    /// The counts of the event `stats`.
    pub stats: Option<BTreeMap<HostId, HostStats>>,
    /// The cancel request.
    pub cancel: Option<CancelRequest>,
    /// True after the bound of the output (4.22).
    pub output_truncated: bool,
    /// True after the bound of the events (4.22).
    pub events_truncated: bool,
}

fn s(text: impl Into<String>) -> Json {
    Json::Str(text.into())
}

impl RunRecord {
    /// The record as JSON (4.38).
    pub fn to_json(&self) -> Json {
        let mut m = BTreeMap::new();
        let mut put = |k: &str, v: Json| {
            m.insert(k.to_string(), v);
        };
        put("format", Json::Int(1));
        put("id", s(self.id.to_string()));
        put("kind", s(self.kind.name()));
        put("mode", s(self.mode.name()));
        put("environment", s(self.environment.as_str()));
        put("node", s(self.node.path()));
        put("hosts", Json::Arr(self.hosts.iter().map(|h| s(h.as_str())).collect()));
        put("playbook", s(self.playbook.as_str()));
        if let Some(c) = &self.command {
            put("command", s(c.as_str()));
        }
        if let Some(p) = &self.params {
            put("params", p.to_json());
        }
        if let Some(c) = &self.check_run {
            put("check_run", s(c.to_string()));
        }
        if let Some(a) = &self.applied_by {
            put("applied_by", s(a.to_string()));
        }
        if let Some(t) = &self.message {
            put("message", s(t.clone()));
        }
        put("subject", s(self.subject.as_str()));
        put("created_at", s(self.created_at.to_string()));
        if let Some(t) = self.started_at {
            put("started_at", s(t.to_string()));
        }
        if let Some(t) = self.finished_at {
            put("finished_at", s(t.to_string()));
        }
        put("status", s(self.status.name()));
        if let Some(r) = self.reason {
            put("reason", s(r.name()));
        }
        if let Some(h) = &self.snapshot {
            put("snapshot", s(h.to_string()));
        }
        if let Some(r) = &self.runner {
            put(
                "runner",
                Json::object([
                    ("boot_id", s(r.boot_id.clone())),
                    ("node", s(r.node.clone())),
                    ("pid", Json::Int(r.pid)),
                    ("start_time", Json::Int(r.start_time)),
                ]),
            );
        }
        if let Some(c) = self.exit_code {
            put("exit_code", Json::Int(c));
        }
        if let Some(stats) = &self.stats {
            put("stats", Json::Obj(stats.iter().map(|(h, st)| (h.as_str().to_string(), st.to_json())).collect()));
        }
        if let Some(c) = &self.cancel {
            put("cancel", Json::object([("at", s(c.at.to_string())), ("by", s(c.by.as_str()))]));
        }
        if self.output_truncated {
            put("output_truncated", Json::Bool(true));
        }
        if self.events_truncated {
            put("events_truncated", Json::Bool(true));
        }
        Json::Obj(m)
    }

    /// Read a record (4.38). A record of the store that fails is a defect of the store.
    pub fn from_json(value: &Json) -> Result<RunRecord, Vec<FieldError>> {
        let mut m = Members::of(value).map_err(|e| vec![FieldError::new("", e)])?;
        let mut errors: Vec<FieldError> = Vec::new();
        let text = |m: &mut Members, name: &str, required: bool, errors: &mut Vec<FieldError>| -> Option<String> {
            match m.take(name) {
                Some(Json::Str(t)) => Some(t),
                None if !required => None,
                _ => {
                    errors.push(FieldError::new(name, "expected a string"));
                    None
                }
            }
        };
        fn id<T>(
            v: Option<String>,
            name: &str,
            f: fn(&str) -> Result<T, IdError>,
            errors: &mut Vec<FieldError>,
        ) -> Option<T> {
            v.and_then(|t| f(&t).map_err(|e| errors.push(FieldError::new(name, e.to_string()))).ok())
        }
        fn time(v: Option<String>, name: &str, errors: &mut Vec<FieldError>) -> Option<UtcMillis> {
            v.and_then(|t| {
                let parsed = UtcMillis::parse(&t);
                if parsed.is_none() {
                    errors.push(FieldError::new(name, "expected a time of 5.3"));
                }
                parsed
            })
        }
        if m.take("format") != Some(Json::Int(1)) {
            errors.push(FieldError::new("format", "expected the integer 1"));
        }
        let rid = id(text(&mut m, "id", true, &mut errors), "id", RunId::parse, &mut errors);
        let kind = text(&mut m, "kind", true, &mut errors).and_then(|t| RunKind::from_name(&t));
        let mode = text(&mut m, "mode", true, &mut errors).and_then(|t| RunMode::from_name(&t));
        let environment = id(text(&mut m, "environment", true, &mut errors), "environment", EnvId::parse, &mut errors);
        let node = text(&mut m, "node", true, &mut errors).and_then(|t| Node::parse_path(&t).ok());
        let hosts = match m.take("hosts") {
            Some(Json::Arr(items)) => {
                items.iter().map(|i| i.as_str().and_then(|t| HostId::parse(t).ok())).collect::<Option<Vec<_>>>()
            }
            _ => None,
        };
        let playbook = id(text(&mut m, "playbook", true, &mut errors), "playbook", PlaybookId::parse, &mut errors);
        let command = id(text(&mut m, "command", false, &mut errors), "command", CommandId::parse, &mut errors);
        let params = match m.take("params") {
            None => None,
            Some(Json::Obj(map)) => Some(CanonicalParams(map)),
            Some(_) => {
                errors.push(FieldError::new("params", "expected an object"));
                None
            }
        };
        let check_run = id(text(&mut m, "check_run", false, &mut errors), "check_run", RunId::parse, &mut errors);
        let applied_by = id(text(&mut m, "applied_by", false, &mut errors), "applied_by", RunId::parse, &mut errors);
        let message = text(&mut m, "message", false, &mut errors);
        let subject = id(text(&mut m, "subject", true, &mut errors), "subject", Subject::parse, &mut errors);
        let created_at = time(text(&mut m, "created_at", true, &mut errors), "created_at", &mut errors);
        let started_at = time(text(&mut m, "started_at", false, &mut errors), "started_at", &mut errors);
        let finished_at = time(text(&mut m, "finished_at", false, &mut errors), "finished_at", &mut errors);
        let status = text(&mut m, "status", true, &mut errors).and_then(|t| RunStatus::from_name(&t));
        let reason = match text(&mut m, "reason", false, &mut errors) {
            None => Some(None),
            Some(t) => RunReason::from_name(&t).map(Some),
        };
        let snapshot = match text(&mut m, "snapshot", false, &mut errors) {
            None => Some(None),
            Some(t) => SnapshotHash::parse(&t).map(Some),
        };
        let runner = match m.take("runner") {
            None => Some(None),
            Some(Json::Obj(r)) if r.len() == 4 => {
                let facts = (|| {
                    Some(RunnerFacts {
                        boot_id: r.get("boot_id")?.as_str()?.to_string(),
                        node: r.get("node")?.as_str()?.to_string(),
                        pid: r.get("pid")?.as_int()?,
                        start_time: r.get("start_time")?.as_int()?,
                    })
                })();
                facts.map(Some)
            }
            Some(_) => None,
        };
        let exit_code = match m.take("exit_code") {
            None => Some(None),
            Some(Json::Int(n)) => Some(Some(n)),
            Some(_) => None,
        };
        let stats = match m.take("stats") {
            None => Some(None),
            Some(Json::Obj(map)) => map
                .iter()
                .map(|(h, v)| Some((HostId::parse(h).ok()?, HostStats::from_json(v)?)))
                .collect::<Option<BTreeMap<_, _>>>()
                .map(Some),
            Some(_) => None,
        };
        let cancel = match m.take("cancel") {
            None => Some(None),
            Some(Json::Obj(c)) if c.len() == 2 => (|| {
                Some(Some(CancelRequest {
                    at: UtcMillis::parse(c.get("at")?.as_str()?)?,
                    by: Subject::parse(c.get("by")?.as_str()?).ok()?,
                }))
            })(),
            Some(_) => None,
        };
        let flag = |m: &mut Members, name: &str| match m.take(name) {
            None => Some(false),
            Some(Json::Bool(true)) => Some(true),
            Some(_) => None,
        };
        let output_truncated = flag(&mut m, "output_truncated");
        let events_truncated = flag(&mut m, "events_truncated");
        for unknown in m.finish() {
            errors.push(FieldError::new(unknown, "unknown member"));
        }
        let record = (|| {
            Some(RunRecord {
                id: rid?,
                kind: kind?,
                mode: mode?,
                environment: environment?,
                node: node?,
                hosts: hosts?,
                playbook: playbook?,
                command,
                params,
                check_run,
                applied_by,
                message,
                subject: subject?,
                created_at: created_at?,
                started_at,
                finished_at,
                status: status?,
                reason: reason?,
                snapshot: snapshot?,
                runner: runner?,
                exit_code: exit_code?,
                stats: stats?,
                cancel: cancel?,
                output_truncated: output_truncated?,
                events_truncated: events_truncated?,
            })
        })();
        match record {
            Some(r) if errors.is_empty() => Ok(r),
            _ => {
                if errors.is_empty() {
                    errors.push(FieldError::new("", "a member of the run record is not valid"));
                }
                Err(errors)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{self, ReadLimits};

    fn read(text: &str) -> Json {
        json::parse(text.as_bytes(), ReadLimits::bytes(1 << 16)).unwrap()
    }

    #[test]
    fn transitions_of_4_39() {
        use RunStatus::*;
        assert!(Queued.can_move_to(Running));
        assert!(Queued.can_move_to(Canceled));
        assert!(!Queued.can_move_to(Passed));
        assert!(Running.can_move_to(Lost));
        assert!(!Passed.can_move_to(Failed));
        assert!(!Canceled.can_move_to(Running));
        assert!(Lost.is_final() && !Running.is_final());
    }

    #[test]
    fn requests_of_4_4() {
        let r = RunRequest::parse(&read(
            r#"{"kind": "deploy", "mode": "check", "node": "environment:p/unit:relay", "hosts": ["host-a", "host-b"]}"#,
        ))
        .unwrap();
        assert!(matches!(r, RunRequest::Fresh { kind: RunKind::Deploy, mode: RunMode::Check, .. }));
        let id = RunId::from_parts(1_795_514_400_000, [7; 10]);
        let r = RunRequest::parse(&read(&format!(r#"{{"kind": "deploy", "mode": "apply", "check_run": "{id}"}}"#)))
            .unwrap();
        assert_eq!(r, RunRequest::AfterCheck { kind: RunKind::Deploy, check_run: id, message: None });
        for bad in [
            r#"{"kind": "deploy", "mode": "apply", "node": "environment:p/unit:r", "hosts": ["a"]}"#,
            r#"{"kind": "status", "mode": "check", "node": "environment:p/unit:r", "hosts": ["a"]}"#,
            r#"{"kind": "deploy", "mode": "check", "node": "environment:p/unit:r", "hosts": ["b", "a"]}"#,
            r#"{"kind": "deploy", "mode": "check", "node": "environment:p/unit:r", "hosts": []}"#,
            r#"{"kind": "deploy", "mode": "check", "node": "environment:p/unit:r", "hosts": ["a"], "params": {}}"#,
            r#"{"kind": "command", "mode": "check", "node": "environment:p/unit:r", "hosts": ["a"]}"#,
        ] {
            assert!(RunRequest::parse(&read(bad)).is_err(), "{bad}");
        }
        let c = RunRequest::parse(&read(r#"{"kind": "command", "mode": "apply", "command": "restart", "node": "environment:p/unit:r", "hosts": ["a"], "params": {"grace": "1m"}}"#)).unwrap();
        assert!(matches!(c, RunRequest::Fresh { kind: RunKind::Command, mode: RunMode::Apply, .. }));
    }

    /// 2.1, 4.4: a message with a bidirectional control or another format character is refused.
    /// A line end is permitted.
    #[test]
    fn a_message_with_a_format_character_is_refused() {
        let body = |message: &str| {
            Json::object([
                ("hosts", Json::Arr(vec![Json::str("a")])),
                ("kind", Json::str("deploy")),
                ("message", Json::str(message)),
                ("mode", Json::str("check")),
                ("node", Json::str("environment:p/unit:r")),
            ])
        };
        for hidden in ["\u{202E}", "\u{2067}", "\u{200E}", "\u{061C}", "\u{FEFF}", "\u{00AD}"] {
            let errors = RunRequest::parse(&body(&format!("deploy {hidden}now"))).unwrap_err();
            assert_eq!(errors[0].path, "message");
            assert!(errors[0].message.contains("no format character"), "{}", errors[0].message);
        }
        assert!(RunRequest::parse(&body("line 1\nline 2 — é")).is_ok());
    }

    #[test]
    fn records_round_trip() {
        let mut stats = BTreeMap::new();
        stats.insert(HostId::parse("host-a").unwrap(), HostStats { ok: 3, changed: 1, ..HostStats::default() });
        let record = RunRecord {
            id: RunId::from_parts(1_795_514_400_000, [1; 10]),
            kind: RunKind::Command,
            mode: RunMode::Apply,
            environment: EnvId::parse("p").unwrap(),
            node: Node::parse_path("environment:p/unit:r").unwrap(),
            hosts: vec![HostId::parse("host-a").unwrap()],
            playbook: PlaybookId::parse("restart").unwrap(),
            command: Some(CommandId::parse("restart").unwrap()),
            params: Some(CanonicalParams([("grace".to_string(), Json::str("1m"))].into_iter().collect())),
            check_run: None,
            applied_by: None,
            message: Some("Restart after the change.".into()),
            subject: Subject::parse("alice").unwrap(),
            created_at: UtcMillis(1_795_514_400_000),
            started_at: Some(UtcMillis(1_795_514_401_000)),
            finished_at: Some(UtcMillis(1_795_514_402_000)),
            status: RunStatus::Passed,
            reason: None,
            snapshot: Some(SnapshotHash::of_bytes(b"x")),
            runner: Some(RunnerFacts { node: "n".into(), pid: 42, start_time: 7, boot_id: "b".into() }),
            exit_code: Some(0),
            stats: Some(stats),
            cancel: None,
            output_truncated: false,
            events_truncated: true,
        };
        let bytes = json::to_canonical(&record.to_json());
        let again = RunRecord::from_json(&json::parse(&bytes, ReadLimits::bytes(1 << 16)).unwrap()).unwrap();
        assert_eq!(again, record);
        let mut extra = record.to_json();
        if let Json::Obj(map) = &mut extra {
            map.insert("other".into(), Json::Int(1));
        }
        assert!(RunRecord::from_json(&extra).is_err());
    }

    #[test]
    fn use_actions_of_4_3() {
        assert_eq!(use_action(RunKind::Deploy, RunMode::Check), Action::RunCheck);
        assert_eq!(use_action(RunKind::Deploy, RunMode::Apply), Action::RunApply);
        assert_eq!(use_action(RunKind::Status, RunMode::Apply), Action::RunCheck);
        assert_eq!(use_action(RunKind::Command, RunMode::Check), Action::CommandRun);
    }
}
