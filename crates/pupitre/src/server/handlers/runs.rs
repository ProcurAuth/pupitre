// SPDX-License-Identifier: MIT OR Apache-2.0
//! The routes of the runs (5.16 to 5.18). A run request goes into the store with the status
//! `queued` (4.4, 4.5). The runner node reads it (4.6).

use bytes::Bytes;
use http::StatusCode;
use http::header::{CACHE_CONTROL, HeaderValue, LOCATION};
use pupitre_core::action::Action;
use pupitre_core::capability::PlaybookKind;
use pupitre_core::command::{CanonicalParams, Target, check_params};
use pupitre_core::doc::{DocKey, Document};
use pupitre_core::error::FieldError;
use pupitre_core::ids::{EnvId, HostId, RunId};
use pupitre_core::json::{self, Json, ReadLimits};
use pupitre_core::node::{Node, NodeType};
use pupitre_core::run::{CancelRequest, RunKind, RunMode, RunRecord, RunRequest, RunStatus, use_action};
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::catalog::EntryState;
use pupitre_store::types::{RunFilter, StoreError};

use crate::access::{Access, CachePolicy, MatchMode, Refusal};
use crate::config::LogLevel;
use crate::server::ctx::{Ctx, refusal_error};
use crate::server::error::{ApiError, Code};
use crate::server::headers::{X_OUTPUT_LENGTH, X_RUN_COMPLETE};
use crate::server::limits::MAX_OUTPUT_READ;
use crate::server::query::Cursor;
use crate::server::response::{Response, bytes_response, json_response, set_header};
use crate::server::state::ModeSettings;

fn invalid_run(errors: &[FieldError]) -> ApiError {
    ApiError::with_errors(Code::InvalidRunRequest, errors)
}

fn run_json(record: &RunRecord) -> Json {
    record.to_json()
}

fn log_run<A: Access, S: Store>(ctx: &Ctx<A, S>, event: &'static str, record: &RunRecord) {
    let mut line = ctx
        .state
        .logger
        .event(LogLevel::Info, event)
        .request_id(&ctx.request_id)
        .text("run", &record.id.to_string())
        .text("kind", record.kind.name())
        .text("mode", record.mode.name())
        .text("node", &record.node.path())
        .field("hosts", Json::Arr(record.hosts.iter().map(|h| Json::str(h.as_str())).collect()))
        .text("status", record.status.name())
        .text("subject", ctx.subject.as_str());
    if let Some(reason) = record.reason {
        line = line.text("reason", reason.name());
    }
    line.emit();
}

/// The run of a path variable. A text that is not a run id names no run (3.14).
fn run_id<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Option<RunId> {
    ctx.params.get("r").and_then(|t| RunId::parse(t).ok())
}

/// Read the record of the run of the route, then ask `run.read` (or the action that the
/// caller names) on its node. An unknown run gives the question of 3.14.
async fn read_and_authorize<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    action_of: impl Fn(&RunRecord) -> Action,
    unknown_action: Action,
    policy: CachePolicy,
) -> Result<RunRecord, ApiError> {
    let record = match run_id(ctx) {
        None => None,
        Some(id) => {
            let span = ctx.trace.child("store.get_run");
            let found = ctx.state.store.get_run(&id).await;
            span.end(found.is_err());
            match found {
                Ok(r) => Some(r),
                Err(StoreError::NotFound) => None,
                Err(e) => return Err(ctx.store_error("get_run", &e)),
            }
        }
    };
    match record {
        Some(r) => {
            let action = action_of(&r);
            ctx.authorize(action, Some(r.node.clone()), policy).await?;
            Ok(r)
        }
        None => {
            ctx.authorize(unknown_action, None, policy).await?;
            Err(ApiError::not_found())
        }
    }
}

/// The effective bound of the hosts of a run (3.10): the lowest of `runner.max_hosts` and the
/// ceiling of the answer.
fn effective_max_hosts(runner_max: u32, granted: Option<u64>) -> u64 {
    let base = u64::from(runner_max);
    granted.map_or(base, |g| g.min(base))
}

/// `POST /api/runs` (5.16, 4.4).
pub async fn create<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let body = ctx.body_json()?;
    // The shape of the request (3.13, step 6).
    let request = RunRequest::parse(&body).map_err(|_| ApiError::bad_request())?;
    let runner = ctx.state.settings.runner.ok_or_else(ApiError::not_found)?;
    // The node, the action and the check run of the request.
    let (kind, mode, check) = match &request {
        RunRequest::Fresh { kind, mode, .. } => (*kind, *mode, None),
        RunRequest::AfterCheck { kind, check_run, .. } => (*kind, RunMode::Apply, Some(*check_run)),
    };
    let action = use_action(kind, mode);
    let check_record: Option<RunRecord> = match &check {
        None => None,
        Some(id) => {
            let found = ctx.state.store.get_run(id).await;
            match found {
                Ok(r) => Some(r),
                Err(StoreError::NotFound) => None,
                Err(e) => return Err(ctx.store_error("get_run", &e)),
            }
        }
    };
    let node: Option<Node> = match (&request, &check_record) {
        (RunRequest::Fresh { node, .. }, _) => ctx.state.catalog().node_exists(node).then(|| node.clone()),
        (RunRequest::AfterCheck { .. }, Some(rec)) => Some(rec.node.clone()),
        (RunRequest::AfterCheck { .. }, None) => None,
    };
    let granted = ctx.authorize(action, node, CachePolicy::Fresh).await?;
    // Step 9: the process ceilings (3.22, 3.10).
    if let ModeSettings::Open { allow_apply } = &ctx.state.settings.mode
        && !*allow_apply
        && ((kind == RunKind::Deploy && mode == RunMode::Apply) || kind == RunKind::Command)
    {
        return Err(ApiError::new(Code::ApplyDisabled));
    }
    let host_count = match (&request, &check_record) {
        (RunRequest::Fresh { hosts, .. }, _) => hosts.len(),
        (RunRequest::AfterCheck { .. }, Some(rec)) => rec.hosts.len(),
        (RunRequest::AfterCheck { .. }, None) => 0,
    };
    let limit = effective_max_hosts(runner.max_hosts, granted.max_hosts);
    if u64::try_from(host_count).unwrap_or(u64::MAX) > limit {
        return Err(ApiError::new(Code::CeilingExceeded)
            .with("ceiling", Json::str(pupitre_core::action::MAX_HOSTS))
            .with("limit", Json::Int(limit)));
    }
    // Step 10: the rules of 4.4 and the guard of 4.24.
    let (record, consumes) = match request {
        RunRequest::Fresh { kind, mode, node, hosts, command, params, message } => {
            let prepared = prepare_fresh(ctx, kind, mode, &node, &hosts, command.as_ref(), params.as_ref())?;
            let environment = environment_of(ctx, &node)?;
            (build_record(ctx, kind, mode, environment, node, hosts, prepared, None, message), None)
        }
        RunRequest::AfterCheck { kind, check_run, message } => {
            let Some(rec) = check_record else { return Err(ApiError::not_found()) };
            guard_check(ctx, kind, &rec)?;
            let prepared = prepare_after(ctx, kind, &rec)?;
            let record = build_record(
                ctx,
                kind,
                RunMode::Apply,
                rec.environment.clone(),
                rec.node.clone(),
                rec.hosts.clone(),
                prepared,
                Some(check_run),
                message,
            );
            (record, Some(check_run))
        }
    };
    let span = ctx.trace.child("store.create_run");
    let created = ctx.state.store.create_run(&record, runner.max_queued_runs, consumes.as_ref()).await;
    span.end(created.is_err());
    created.map_err(|e| ctx.store_error("create_run", &e))?;
    ctx.state.metrics.run(record.kind.name(), record.mode.name(), "queued");
    log_run(ctx, "run.queued", &record);
    // The runner node starts the run at once when it is this instance (4.6).
    ctx.state.wake.wake();
    let mut response = json_response(StatusCode::CREATED, &run_json(&record));
    let location = format!("{}api/runs/{}", ctx.state.settings.base_path, record.id);
    if let Ok(v) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(LOCATION, v);
    }
    Ok(response)
}

/// The environment of the node of a run. The rules of 4.4 give a node of an environment, a unit
/// or a host, so the service never comes here.
fn environment_of<A: Access, S: Store>(ctx: &Ctx<A, S>, node: &Node) -> Result<EnvId, ApiError> {
    node.environment().cloned().ok_or_else(|| ctx.internal("a run request has the node of the service"))
}

/// What the rules of 4.4 give for the record of a run.
struct Prepared {
    playbook: pupitre_core::ids::PlaybookId,
    command: Option<pupitre_core::ids::CommandId>,
    params: Option<CanonicalParams>,
}

#[allow(clippy::too_many_arguments)]
fn build_record<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    kind: RunKind,
    mode: RunMode,
    environment: EnvId,
    node: Node,
    hosts: Vec<HostId>,
    prepared: Prepared,
    check_run: Option<RunId>,
    message: Option<String>,
) -> RunRecord {
    let now = UtcMillis::now();
    let mut random = [0u8; 10];
    if getrandom::fill(&mut random).is_err() {
        // A run id needs no strong source: the time prefix makes it unique with the counter.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        for (i, b) in random.iter_mut().enumerate() {
            *b = nanos.to_le_bytes()[i % 16];
        }
    }
    RunRecord {
        id: RunId::from_parts(now.0, random),
        kind,
        mode,
        environment,
        node,
        hosts,
        playbook: prepared.playbook,
        command: prepared.command,
        params: prepared.params,
        check_run,
        applied_by: None,
        message,
        subject: ctx.subject.clone(),
        created_at: now,
        started_at: None,
        finished_at: None,
        status: RunStatus::Queued,
        reason: None,
        snapshot: None,
        runner: None,
        exit_code: None,
        stats: None,
        cancel: None,
        output_truncated: false,
        events_truncated: false,
    }
}

/// The checks of 4.4, rules 1, 2, 3 and 5, for a request with a node (step 10 of 3.13).
fn prepare_fresh<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    kind: RunKind,
    mode: RunMode,
    node: &Node,
    hosts: &[HostId],
    command: Option<&pupitre_core::ids::CommandId>,
    params: Option<&Json>,
) -> Result<Prepared, ApiError> {
    let caps = &ctx.state.capabilities;
    let mut errors: Vec<FieldError> = Vec::new();
    let mut playbook = None;
    let mut spec_ref = None;
    let mut canonical = None;
    // Rule 1: the type of the node.
    match kind {
        RunKind::Deploy | RunKind::Status => {
            if !matches!(node.node_type(), NodeType::Unit | NodeType::Host) {
                errors.push(FieldError::new("node", "expected the node of a unit or of a host"));
            }
        }
        RunKind::Command => match command.and_then(|c| caps.commands.get(c)) {
            None => errors.push(FieldError::new("command", "the configuration has no such command")),
            Some(spec) => {
                spec_ref = Some(spec);
                let wanted = spec.target.node_type();
                if node.node_type() != wanted {
                    errors.push(FieldError::new("node", format!("expected the node of {}", target_text(spec.target))));
                }
                // A command with `check = false` accepts the mode `apply` only (4.3).
                if !spec.check && mode == RunMode::Check {
                    errors.push(FieldError::new(
                        "mode",
                        "this command has check = false and accepts the mode apply only",
                    ));
                }
                if spec.check && mode == RunMode::Apply {
                    errors.push(FieldError::new("mode", "an apply of this command needs a check run"));
                }
                playbook = Some(spec.playbook.clone());
            }
        },
    }
    // Rule 2: the unit of the node.
    let unit = match node {
        Node::Unit(e, u) | Node::Host(e, u, _) => {
            let cat = ctx.state.catalog();
            match cat.get(&DocKey::Unit(e.clone(), u.clone())).map(|x| &x.state) {
                Some(EntryState::Valid(Document::Unit(unit)))
                    if cat.entry_errors(&DocKey::Unit(e.clone(), u.clone())).is_empty() =>
                {
                    Some(unit.clone())
                }
                _ => {
                    errors.push(FieldError::new("node", "the unit of the node is not valid in the catalog"));
                    None
                }
            }
        }
        _ => None,
    };
    if let Some(unit) = &unit {
        match kind {
            RunKind::Deploy => {
                playbook = Some(unit.playbooks.deploy.clone());
                check_playbooks(ctx, unit, true, &mut errors);
            }
            RunKind::Status => match &unit.playbooks.status {
                Some(p) => {
                    playbook = Some(p.clone());
                    if caps.playbook_of_kind(p, PlaybookKind::Status).is_none() {
                        errors.push(FieldError::new(
                            "node",
                            "the status playbook of the unit is not in the configuration",
                        ));
                    }
                }
                None => errors.push(FieldError::new("node", "the unit has no status playbook")),
            },
            RunKind::Command => {}
        }
    }
    // Rule 3: the hosts.
    check_hosts(ctx, node, unit.as_ref(), hosts, &mut errors);
    // Rule 5: the parameters of a command.
    if let (Some(spec), Some(p)) = (spec_ref, params) {
        match check_params(spec, p) {
            Ok(c) => canonical = Some(c),
            Err(e) => errors.extend(e),
        }
    }
    if !errors.is_empty() {
        return Err(invalid_run(&errors));
    }
    let playbook = playbook.ok_or_else(|| ctx.internal("a run request has no playbook after its checks"))?;
    Ok(Prepared { playbook, command: command.cloned(), params: canonical })
}

fn target_text(target: Target) -> &'static str {
    match target {
        Target::Environment => "an environment",
        Target::Unit => "a unit",
        Target::Host => "a host",
    }
}

fn check_playbooks<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    unit: &pupitre_core::doc::Unit,
    deploy: bool,
    errors: &mut Vec<FieldError>,
) {
    let caps = &ctx.state.capabilities;
    if deploy && caps.playbook_of_kind(&unit.playbooks.deploy, PlaybookKind::Deploy).is_none() {
        errors.push(FieldError::new("node", "the deploy playbook of the unit is not in the configuration"));
    }
    if let Some(r) = &unit.playbooks.render
        && caps.playbook_of_kind(r, PlaybookKind::Render).is_none()
    {
        errors.push(FieldError::new("node", "the render playbook of the unit is not in the configuration"));
    }
}

fn check_hosts<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    node: &Node,
    unit: Option<&pupitre_core::doc::Unit>,
    hosts: &[HostId],
    errors: &mut Vec<FieldError>,
) {
    match node {
        Node::Unit(..) => {
            if let Some(unit) = unit {
                for h in hosts.iter().filter(|h| !unit.hosts.contains(h)) {
                    errors.push(FieldError::new("hosts", format!("the host {h} is not in the unit")));
                }
            }
        }
        Node::Host(_, _, host) => {
            if hosts != std::slice::from_ref(host) {
                errors.push(FieldError::new("hosts", format!("expected exactly the host {host} of the node")));
            }
        }
        Node::Environment(env) => {
            let cat = ctx.state.catalog();
            for h in hosts {
                if cat.get(&DocKey::Host(env.clone(), h.clone())).is_none() {
                    errors.push(FieldError::new("hosts", format!("the host {h} has no document")));
                }
            }
        }
        Node::Service => errors.push(FieldError::new("node", "expected the node of an environment, a unit or a host")),
    }
}

/// The guard of 4.24 at the request: the check run, its status, its use and its age.
fn guard_check<A: Access, S: Store>(ctx: &Ctx<A, S>, kind: RunKind, check: &RunRecord) -> Result<(), ApiError> {
    if check.kind != kind || check.mode != RunMode::Check || check.status != RunStatus::Passed {
        return Err(ApiError::new(Code::CheckRequired));
    }
    if check.applied_by.is_some() {
        return Err(ApiError::new(Code::CheckUsed));
    }
    let Some(finished) = check.finished_at else { return Err(ApiError::new(Code::CheckRequired)) };
    let max_age = ctx.state.settings.runner.map_or(std::time::Duration::ZERO, |r| r.check_max_age);
    let max_ms = u64::try_from(max_age.as_millis()).unwrap_or(u64::MAX);
    if UtcMillis::now().since(finished) > max_ms {
        return Err(ApiError::new(Code::CheckTooOld));
    }
    Ok(())
}

/// The apply takes the node, the hosts, the command and the parameters of its check run (4.24).
/// The node, the unit and the playbooks must still be valid.
fn prepare_after<A: Access, S: Store>(ctx: &Ctx<A, S>, kind: RunKind, check: &RunRecord) -> Result<Prepared, ApiError> {
    let caps = &ctx.state.capabilities;
    let mut errors = Vec::new();
    if !ctx.state.catalog().node_exists(&check.node) {
        errors.push(FieldError::new("check_run", "the node of the check run no longer exists"));
    }
    let unit = match &check.node {
        Node::Unit(e, u) | Node::Host(e, u, _) => {
            let key = DocKey::Unit(e.clone(), u.clone());
            let cat = ctx.state.catalog();
            match (cat.get(&key).map(|x| &x.state), cat.entry_errors(&key).is_empty()) {
                (Some(EntryState::Valid(Document::Unit(unit))), true) => Some(unit.clone()),
                _ => {
                    errors.push(FieldError::new("check_run", "the unit of the check run is not valid in the catalog"));
                    None
                }
            }
        }
        _ => None,
    };
    let playbook = match kind {
        RunKind::Deploy => {
            if let Some(unit) = &unit {
                check_playbooks(ctx, unit, true, &mut errors);
            }
            unit.as_ref().map(|u| u.playbooks.deploy.clone())
        }
        RunKind::Command => match check.command.as_ref().and_then(|c| caps.commands.get(c)) {
            Some(spec) => Some(spec.playbook.clone()),
            None => {
                errors.push(FieldError::new(
                    "check_run",
                    "the command of the check run is no longer in the configuration",
                ));
                None
            }
        },
        RunKind::Status => None,
    };
    if !errors.is_empty() {
        return Err(invalid_run(&errors));
    }
    let playbook = playbook.ok_or_else(|| ctx.internal("an apply has no playbook after its checks"))?;
    Ok(Prepared { playbook, command: check.command.clone(), params: check.params.clone() })
}

fn status_filter(text: &str) -> Result<RunStatus, ApiError> {
    RunStatus::from_name(text).ok_or_else(ApiError::bad_request)
}

/// `GET /api/runs` (5.16): newest first, with the display filter on `run.read` (3.15).
pub async fn list<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let limit = ctx.page_limit()?;
    let environment = match ctx.query.get("environment") {
        None => None,
        Some(t) => Some(EnvId::parse(t).map_err(|_| ApiError::bad_request())?),
    };
    let status = match ctx.query.get("status") {
        None => None,
        Some(t) => Some(status_filter(t)?),
    };
    let cursor = match ctx.query.get("after") {
        Some(text) => Cursor::decode("runs", text)?,
        None => Cursor::default(),
    };
    let last = match &cursor.last {
        None => None,
        Some(t) => Some(RunId::parse(t).map_err(|_| ApiError::bad_request())?),
    };
    let skip = cursor.skip as usize;
    let filter = RunFilter { environment, status };
    let asked = u32::try_from(limit.saturating_add(1)).unwrap_or(u32::MAX);
    let skipped = u32::try_from(skip).unwrap_or(u32::MAX);
    let span = ctx.trace.child("store.list_runs");
    let listed = ctx.state.store.list_runs_page(&filter, last.as_ref(), skipped, asked).await;
    span.end(listed.is_err());
    let listed = listed.map_err(|e| ctx.store_error("list_runs", &e))?;
    let after_skip: Vec<&RunRecord> = listed.iter().collect();
    let window: Vec<&RunRecord> = after_skip.iter().take(limit).copied().collect();
    let store_has_more = after_skip.len() > window.len();
    let items: Vec<(Node, Vec<Action>, MatchMode)> =
        window.iter().map(|r| (r.node.clone(), vec![Action::RunRead], MatchMode::Node)).collect();
    let budget = ctx.state.settings.question_budget();
    let span = ctx.trace.child("access.decide").attr("action", Action::RunRead.name());
    let result = ctx.state.access.visible(&ctx.subject, &items, budget).await;
    span.end(result.is_err());
    let visibility = result.map_err(|r: Refusal| refusal_error(&r))?;
    let examined = visibility.stopped_at.unwrap_or(window.len()).min(window.len());
    let visible: Vec<usize> = visibility.visible.iter().copied().filter(|i| *i < examined).collect();
    let more = store_has_more || examined < window.len();
    let next = more.then(|| {
        // The cursor holds the last visible run and the hidden runs after it. It never holds
        // the id of a run that the subject cannot see.
        let c = match visible.last() {
            Some(i) => Cursor {
                last: Some(window[*i].id.to_string()),
                skip: u32::try_from(examined - (*i + 1)).unwrap_or(u32::MAX),
            },
            None => Cursor {
                last: cursor.last.clone(),
                skip: cursor.skip.saturating_add(u32::try_from(examined).unwrap_or(u32::MAX)),
            },
        };
        c.encode("runs")
    });
    let items: Vec<Json> = visible.iter().map(|i| run_json(window[*i])).collect();
    let mut pairs = vec![("items", Json::Arr(items))];
    if let Some(next) = next {
        pairs.push(("next", Json::str(next)));
    }
    Ok(json_response(StatusCode::OK, &Json::object(pairs)))
}

/// `GET /api/runs/{r}` (5.16): `run.read` on the node of the run.
pub async fn get<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let record = read_and_authorize(ctx, |_| Action::RunRead, Action::RunRead, CachePolicy::MayUseKept).await?;
    Ok(json_response(StatusCode::OK, &run_json(&record)))
}

/// `GET /api/runs/{r}/events` (5.17).
pub async fn events<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let limit = ctx.page_limit()?;
    let after = ctx.query.integer("after", 0, json::MAX_SAFE_INTEGER)?.unwrap_or(0);
    let record = read_and_authorize(ctx, |_| Action::RunRead, Action::RunRead, CachePolicy::MayUseKept).await?;
    let asked = u32::try_from(limit).unwrap_or(u32::MAX);
    let span = ctx.trace.child("store.read_events");
    let lines = ctx.state.store.read_events(&record.id, after, asked).await;
    span.end(lines.is_err());
    let lines = lines.map_err(|e| ctx.store_error("read_events", &e))?;
    let mut last_seq = after;
    let items: Vec<Json> = lines
        .iter()
        .map(|l| {
            last_seq = last_seq.max(l.seq);
            json::parse(&l.bytes, ReadLimits::bytes(l.bytes.len().max(1)))
                .unwrap_or_else(|_| Json::object([("kind", Json::str("invalid")), ("seq", Json::Int(l.seq))]))
        })
        .collect();
    // The record was read before the lines: a final run has all its lines stored.
    let complete = record.status.is_final() && lines.len() < limit;
    Ok(json_response(
        StatusCode::OK,
        &Json::object([
            ("complete", Json::Bool(complete)),
            ("items", Json::Arr(items)),
            ("last_seq", Json::Int(last_seq)),
        ]),
    ))
}

/// `GET /api/runs/{r}/output` (5.17): raw bytes with two headers.
pub async fn output<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let offset = ctx.query.integer("offset", 0, json::MAX_SAFE_INTEGER)?.unwrap_or(0);
    let limit = ctx.query.integer("limit", 1, u64::from(MAX_OUTPUT_READ))?.unwrap_or(u64::from(MAX_OUTPUT_READ));
    let record = read_and_authorize(ctx, |_| Action::RunRead, Action::RunRead, CachePolicy::MayUseKept).await?;
    let span = ctx.trace.child("store.read_output");
    let chunk = ctx.state.store.read_output(&record.id, offset, u32::try_from(limit).unwrap_or(MAX_OUTPUT_READ)).await;
    span.end(chunk.is_err());
    let chunk = chunk.map_err(|e| ctx.store_error("read_output", &e))?;
    let mut r = bytes_response(StatusCode::OK, "application/octet-stream", Bytes::from(chunk.bytes));
    r.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    set_header(&mut r, X_OUTPUT_LENGTH, &chunk.length.to_string());
    set_header(&mut r, X_RUN_COMPLETE, if record.status.is_final() { "true" } else { "false" });
    Ok(r)
}

/// `POST /api/runs/{r}/cancel` (5.18, 4.27).
pub async fn cancel<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let mut record =
        read_and_authorize(ctx, |r| use_action(r.kind, r.mode), Action::RunCheck, CachePolicy::Fresh).await?;
    for _ in 0..3 {
        if record.status.is_final() {
            return Err(ApiError::new(Code::RunFinished));
        }
        let now = UtcMillis::now();
        let mut next = record.clone();
        let expected = record.status;
        if expected == RunStatus::Queued {
            next.status = RunStatus::Canceled;
            next.finished_at = Some(now);
        } else if record.cancel.is_some() {
            // A cancel request already waits for the runner node.
            return Ok(json_response(StatusCode::OK, &run_json(&record)));
        }
        next.cancel = Some(CancelRequest { by: ctx.subject.clone(), at: now });
        let span = ctx.trace.child("store.update_run");
        let result = ctx.state.store.update_run(expected, &next).await;
        span.end(result.is_err());
        match result {
            Ok(()) => {
                if next.status == RunStatus::Canceled {
                    ctx.state.metrics.run(next.kind.name(), next.mode.name(), "canceled");
                }
                ctx.state
                    .logger
                    .event(LogLevel::Info, "run.cancel")
                    .request_id(&ctx.request_id)
                    .text("run", &next.id.to_string())
                    .text("subject", ctx.subject.as_str())
                    .emit();
                // The runner node reads the cancel request at its next step (4.27).
                ctx.state.wake.wake();
                return Ok(json_response(StatusCode::OK, &run_json(&next)));
            }
            Err(StoreError::Conflict { .. } | StoreError::BadTransition { .. }) => {
                // The status changed after the read: read the record again.
                record = match ctx.state.store.get_run(&record.id).await {
                    Ok(r) => r,
                    Err(e) => return Err(ctx.store_error("get_run", &e)),
                };
            }
            Err(e) => return Err(ctx.store_error("update_run", &e)),
        }
    }
    Err(ApiError::new(Code::Conflict))
}
