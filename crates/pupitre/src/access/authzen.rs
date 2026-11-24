// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mode `authzen` (3.1 to 3.21): the PEP of the library `procurauth-pep`, the link to the
//! PDP with TLS, the pool, the cache policy and the map of the answers.
//!
//! The library is synchronous. Each call that can reach the PDP runs on a blocking thread, after
//! the task gets a place in the pool of `access.pdp.max_connections` places (3.5). A task that
//! waits longer than `access.pdp.timeout` for a place fails with `PdpUnavailable`.
//!
//! # The two PEPs and the cache policy (3.17)
//!
//! The access layer builds two PEPs on one link. The first one keeps answers for
//! `access.pdp.cache_ttl`. It serves `CachePolicy::MayUseKept`. The second one has
//! `cache_ttl = 0`: it never reads a kept answer and never keeps one. It serves
//! `CachePolicy::Fresh`, and the question of 3.14.
//!
//! # The identity of a decision
//!
//! The trait `Access` gives the subject as a `Subject`. The library asks for an `Identity`, and
//! only a checked token can make an `Identity`. The layer holds one private key of Ed25519. It
//! makes the key at start with random bytes. For each decision, the layer signs a short token for
//! the subject. A third PEP checks the token. This PEP trusts only the private key. The key never
//! leaves the process. No PEP that serves a request trusts it, so no request can use such a token.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use procurauth_core::authzen::Reason;
use procurauth_core::jws::{SigningKey, sign_compact};
use procurauth_pep::access::{Access as GateAnswer, AccessGate};
use procurauth_pep::clock::{Clock, SystemClock};
use procurauth_pep::config::{PdpConfig, PepConfig, Secret};
use procurauth_pep::operation::{NodePath, Operation, Permission, node as library_node};
use procurauth_pep::refusal::{DenialCause, UnavailableCause};
use procurauth_pep::transport::PdpTransport;
use procurauth_pep::{Identity, Outcome, Pep, Refusal as PepRefusal};
use pupitre_core::action::{ACCESS_ACTION, Action, MAX_HOSTS};
use pupitre_core::ids::Subject;
use pupitre_core::node::{Node, NodeType};
use serde_json::json;
use tokio::sync::Semaphore;

use super::document::{action_document_bytes, library_document};
use super::events::{AccessCounters, AccessEvent, Recorder};
use super::keys::{base64url_decode, parse_public_keys};
use super::tls::{PdpLink, count_requests};
use super::{Access, CachePolicy, Granted, MatchMode, Question, Refusal, Visibility};
use crate::config::AuthzenConfig;

/// The largest identity token that Pupitre reads, in bytes.
pub const MAX_TOKEN_BYTES: usize = 16_384;
/// The largest answer of the PDP that the link reads, in bytes.
pub const MAX_RESPONSE_BYTES: u64 = 1_048_576;
/// The idle time of a connection to the PDP, until the PDP publishes another time (P 10.7).
pub const IDLE_CLOSE_AFTER: Duration = Duration::from_secs(5);
/// The life of the private tokens of a decision, in seconds. A token lives for one call.
const MINT_LIFE_SECONDS: u64 = 60;
/// The subjects that a token cannot have: they are the subjects of the history (2.4).
const RESERVED_SUBJECTS: [&str; 3] = [Subject::ANONYMOUS, Subject::EXTERNAL, Subject::IMPORT];

/// Why a call to the PDP gave no answer.
#[derive(Debug)]
enum CallFailure {
    /// No place in the pool before the timeout.
    Busy,
    /// The blocking thread ended with no result.
    Lost,
}

/// The kept answers to the question `access` (3.20, P 10.1c). Only a `true` answer is kept.
#[derive(Debug)]
struct AccessCache {
    ttl_ms: u64,
    max_entries: usize,
    entries: HashMap<String, u64>,
}

impl AccessCache {
    fn has(&mut self, subject: &str, now_ms: u64) -> bool {
        match self.entries.get(subject) {
            Some(expires) if now_ms < *expires => true,
            Some(_) => {
                self.entries.remove(subject);
                false
            }
            None => false,
        }
    }

    fn put(&mut self, subject: &str, now_ms: u64) {
        if self.ttl_ms == 0 {
            return;
        }
        if self.entries.len() >= self.max_entries && !self.entries.contains_key(subject) {
            self.entries.retain(|_, expires| *expires > now_ms);
            if self.entries.len() >= self.max_entries {
                return;
            }
        }
        self.entries.insert(subject.to_owned(), now_ms.saturating_add(self.ttl_ms));
    }
}

/// What the layer needs to build a PEP of the library again.
struct PepParts {
    config: AuthzenConfig,
    runner_enabled: bool,
    runner_max_hosts: u64,
    transport: Arc<dyn PdpTransport>,
    clock: Arc<dyn Clock>,
}

impl PepParts {
    /// A PEP on the link of the layer, with this life of a kept answer.
    fn build(&self, cache_ttl: Duration) -> Result<Pep, String> {
        let mut pep_config = pep_config(&self.config, parse_public_keys(&self.config.public_keys)?)?;
        pep_config.cache_ttl = cache_ttl;
        let document = library_document(&self.config.audience, self.runner_enabled, self.runner_max_hosts)?;
        Pep::with_clock(pep_config, document, self.transport.clone(), self.clock.clone())
            .map_err(|errors| config_errors(&errors))
    }
}

/// The private identities of a decision (see the module text).
struct Minter {
    key: SigningKey,
    pep: Pep,
    issuer: String,
    audience: String,
    clock: Arc<dyn Clock>,
}

impl Minter {
    fn identity(&self, subject: &str) -> Result<Identity, ()> {
        let now = self.clock.now_seconds();
        let header = json!({"alg": "Ed25519", "typ": "JWT"});
        let payload = json!({
            "iss": self.issuer,
            "sub": subject,
            "aud": self.audience,
            "iat": now,
            "exp": now.saturating_add(MINT_LIFE_SECONDS),
        });
        let token = sign_compact(&header, &payload, &self.key);
        self.pep.authenticate_token(&token).map_err(|_| ())
    }
}

/// The mode `authzen`.
pub struct AuthzenAccess {
    document: Vec<u8>,
    service: String,
    token_header: String,
    skew_seconds: u64,
    published: Vec<Action>,
    /// The PEP that keeps answers. `forget_kept_answers` replaces it.
    pep: Mutex<Arc<Pep>>,
    parts: PepParts,
    fresh: Arc<Pep>,
    minter: Arc<Minter>,
    gate: Arc<AccessGate>,
    access_cache: Mutex<AccessCache>,
    pool: Arc<Semaphore>,
    wait: Duration,
    clock: Arc<dyn Clock>,
    recorder: Arc<Recorder>,
    closed: AtomicBool,
}

impl fmt::Debug for AuthzenAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthzenAccess").field("service", &self.service).finish_non_exhaustive()
    }
}

/// The text of a node with its chain: `service:pupitre/environment:production`.
fn path_text(path: &NodePath) -> String {
    path.nodes().iter().map(|n| format!("{}:{}", n.node_type, n.id)).collect::<Vec<_>>().join("/")
}

fn node_path(service: &str, node: &Node) -> Option<NodePath> {
    let below: Vec<(&str, &str)> = match node {
        Node::Service => Vec::new(),
        Node::Environment(e) => vec![("environment", e.as_str())],
        Node::Unit(e, u) => vec![("environment", e.as_str()), ("unit", u.as_str())],
        Node::Host(e, u, h) => vec![("environment", e.as_str()), ("unit", u.as_str()), ("host", h.as_str())],
    };
    library_node(service, &below).ok()
}

fn is_approval_reason(reason: Option<&Reason>) -> bool {
    matches!(
        reason,
        Some(Reason::ApprovalRequired | Reason::ApprovalPending | Reason::ApprovalDenied | Reason::ApprovalExpired)
    )
}

/// The name of an action as a static text.
fn static_name(action: Action) -> &'static str {
    action.name()
}

/// Rename a key of the library to the key of the configuration file.
fn config_key(library_key: &str) -> &str {
    match library_key {
        "service_id" => "access.identity.audience",
        "token_header" => "access.identity.header",
        "issuer" => "access.identity.issuer",
        "trusted_keys" => "access.identity.public_keys",
        "max_token_age_seconds" => "access.identity.max_age",
        "pdp.url" => "access.pdp.url",
        "pdp.credential" => "access.pdp.token",
        "pdp.request_timeout" => "access.pdp.timeout",
        "pdp.max_connections" => "access.pdp.max_connections",
        "cache_max_entries" => "access.pdp.cache_entries",
        "display_max_questions" => "access.options_max_questions",
        other => other,
    }
}

fn config_errors(errors: &[procurauth_pep::config::ConfigError]) -> String {
    errors
        .iter()
        .map(|e| format!("{}: expected {}, found {}", config_key(&e.key), e.expected, e.found))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The most actions that one type of node lists: the fewest questions that one item needs (3.15).
fn most_actions_of_a_type(published: &[Action]) -> usize {
    [NodeType::Environment, NodeType::Unit, NodeType::Host]
        .into_iter()
        .map(|t| published.iter().filter(|a| a.resource_types().contains(&t)).count())
        .max()
        .unwrap_or(0)
}

impl AuthzenAccess {
    /// Build the PEP of the configuration: the keys of the edge, the link to the PDP, the cache
    /// and the action document (3.1, 3.5, 3.11). It sends nothing to the PDP: the first question
    /// reads the metadata. `runner_enabled` and `runner_max_hosts` are `runner.enabled` and
    /// `runner.max_hosts` (6.12).
    pub fn new(config: &AuthzenConfig, runner_enabled: bool, runner_max_hosts: u64) -> Result<AuthzenAccess, String> {
        let link = PdpLink::new(
            config.plain_http,
            config.pdp_ca_pem.as_deref(),
            config.pdp_timeout,
            config.pdp_max_connections as usize,
            MAX_RESPONSE_BYTES,
            IDLE_CLOSE_AFTER,
        )?;
        AuthzenAccess::with_transport(config, runner_enabled, runner_max_hosts, Arc::new(link), Arc::new(SystemClock))
    }

    /// Build the PEP with the transport and the clock that the caller gives. The tests use it.
    pub fn with_transport(
        config: &AuthzenConfig,
        runner_enabled: bool,
        runner_max_hosts: u64,
        transport: Arc<dyn PdpTransport>,
        clock: Arc<dyn Clock>,
    ) -> Result<AuthzenAccess, String> {
        let published = Action::published(runner_enabled);
        let needed = most_actions_of_a_type(&published);
        let questions = config.options_max_questions as usize;
        if questions < needed {
            return Err(format!(
                "access.options_max_questions: expected {needed} or more (the most actions that one type lists, so that one item can end), found {questions}"
            ));
        }
        let document = action_document_bytes(&config.audience, runner_enabled, runner_max_hosts)
            .map_err(|e| format!("the action document is not valid: {e}"))?;
        let parts = PepParts {
            config: config.clone(),
            runner_enabled,
            runner_max_hosts,
            transport: transport.clone(),
            clock: clock.clone(),
        };
        let pep = parts.build(config.cache_ttl)?;
        let fresh = parts.build(Duration::ZERO)?;
        // The private identities: a PEP that trusts one random key of this process.
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| format!("no random bytes for the start of the access layer: {e}"))?;
        let key = SigningKey::from_ed25519_seed(&seed);
        let mut minter_config =
            pep_config(config, procurauth_core::jws::KeySet::from_keys(vec![key.public_key(None)]))?;
        minter_config.max_token_age_seconds = MINT_LIFE_SECONDS;
        minter_config.clock_skew_seconds = 0;
        let minter_pep = Pep::with_clock(
            minter_config,
            library_document(&config.audience, runner_enabled, runner_max_hosts)?,
            transport.clone(),
            clock.clone(),
        )
        .map_err(|errors| config_errors(&errors))?;
        let gate = AccessGate::new(&config.audience, &pdp_config(config), transport.clone(), clock.clone())
            .map_err(|errors| config_errors(&errors))?;
        Ok(AuthzenAccess {
            document,
            service: config.audience.clone(),
            token_header: config.header.clone(),
            skew_seconds: config.clock_skew.as_secs(),
            published,
            pep: Mutex::new(Arc::new(pep)),
            parts,
            fresh: Arc::new(fresh),
            minter: Arc::new(Minter {
                key,
                pep: minter_pep,
                issuer: config.issuer.clone(),
                audience: config.audience.clone(),
                clock: clock.clone(),
            }),
            gate: Arc::new(gate),
            access_cache: Mutex::new(AccessCache {
                ttl_ms: u64::try_from(config.cache_ttl.as_millis()).unwrap_or(u64::MAX),
                max_entries: config.cache_entries as usize,
                entries: HashMap::new(),
            }),
            pool: Arc::new(Semaphore::new(config.pdp_max_connections as usize)),
            wait: config.pdp_timeout,
            clock,
            recorder: Arc::new(Recorder::default()),
            closed: AtomicBool::new(false),
        })
    }

    /// The PEP that keeps answers.
    fn cached_pep(&self) -> Arc<Pep> {
        self.pep.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Drop each answer that the PEP of the reads keeps: replace the PEP by a new one.
    fn forget_kept_answers(&self) {
        if let Ok(pep) = self.parts.build(self.parts.config.cache_ttl) {
            *self.pep.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(pep);
        }
    }

    /// The events of 6.27 since the last call. The server writes them to the log.
    pub fn take_events(&self) -> Vec<AccessEvent> {
        self.recorder.take()
    }

    /// The counters of 6.28.
    pub fn counters(&self) -> AccessCounters {
        self.recorder.counters()
    }

    /// The name of the header with the identity token (3.4).
    pub fn token_header(&self) -> &str {
        &self.token_header
    }

    /// Stop new questions: each later question gives `PdpUnavailable`. The server calls it at
    /// its stop.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Run `f` on a blocking thread after the task gets a place in the pool (3.5). The second
    /// value is the number of requests that `f` sent to the PDP.
    async fn call<T, F>(&self, f: F) -> Result<(T, u32), CallFailure>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CallFailure::Busy);
        }
        let permit = match tokio::time::timeout(self.wait, self.pool.clone().acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            _ => return Err(CallFailure::Busy),
        };
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            count_requests(f)
        })
        .await
        .map_err(|_| CallFailure::Lost)
    }

    fn identity(&self, subject: &Subject) -> Result<Identity, Refusal> {
        self.minter.identity(subject.as_str()).map_err(|()| Refusal::Unauthenticated)
    }

    fn failed_call(&self, failure: &CallFailure, action: &str, resource: &str) -> Refusal {
        self.recorder.count_error();
        let error = match failure {
            CallFailure::Busy => "no place in the pool of requests to the PDP before access.pdp.timeout",
            CallFailure::Lost => "the call to the PDP ended with no result",
        };
        self.recorder.push(AccessEvent::PdpFailed {
            action: action.to_owned(),
            resource: resource.to_owned(),
            error: error.to_owned(),
        });
        Refusal::PdpUnavailable
    }

    /// Count and log a failure of the PDP (3.18).
    fn unavailable(&self, cause: &UnavailableCause, action: &str, resource: &str) -> Refusal {
        self.recorder.count_error();
        let (action, resource) = (action.to_owned(), resource.to_owned());
        self.recorder.push(match cause {
            UnavailableCause::Transport(text) => AccessEvent::PdpFailed { action, resource, error: text.clone() },
            UnavailableCause::Status(code) => AccessEvent::PdpFailed {
                action,
                resource,
                error: format!("the PDP answered with the HTTP status {code}"),
            },
            UnavailableCause::Answer(text) | UnavailableCause::Metadata(text) => {
                AccessEvent::PdpInvalidAnswer { action, resource, error: text.clone() }
            }
        });
        Refusal::PdpUnavailable
    }

    fn decision_event(
        &self,
        subject: &Subject,
        action: &str,
        resource: &str,
        mode: &'static str,
        decision: bool,
        cached: bool,
    ) {
        self.recorder.count_answer(decision, cached);
        self.recorder.push(AccessEvent::Decision {
            subject: subject.as_str().to_owned(),
            action: action.to_owned(),
            resource: resource.to_owned(),
            match_mode: mode,
            decision,
            cached,
        });
    }

    /// Map the outcome of one use to the result of 3.18.
    fn map_outcome(
        &self,
        subject: &Subject,
        action: Action,
        resource: &str,
        outcome: Outcome,
        requests: u32,
    ) -> Result<Granted, Refusal> {
        let cached = requests == 0;
        let name = static_name(action);
        match outcome {
            Outcome::Allowed(allowed) => {
                let max_hosts = allowed.ceilings().get(MAX_HOSTS);
                // A ceiling below the `min` of its declaration is not valid (3.18).
                if let Some(value) = max_hosts.filter(|v| *v < 1) {
                    self.recorder.push(AccessEvent::CeilingUnenforceable {
                        action: name.to_owned(),
                        resource: resource.to_owned(),
                        error: format!("the ceiling `{MAX_HOSTS}` is {value}, and its minimum is 1"),
                    });
                    return Err(Refusal::Forbidden { action: name });
                }
                self.decision_event(subject, name, resource, "node", true, cached);
                Ok(Granted { max_hosts })
            }
            Outcome::Refused(refusal) => match refusal {
                PepRefusal::Forbidden { reason, cause } => match cause {
                    DenialCause::UnknownCeiling(ceiling) => {
                        self.recorder.push(AccessEvent::CeilingUnenforceable {
                            action: name.to_owned(),
                            resource: resource.to_owned(),
                            error: format!("the answer has the ceiling `{ceiling}`, which the action does not list"),
                        });
                        Err(Refusal::Forbidden { action: name })
                    }
                    DenialCause::NoUsageUnits => Err(Refusal::Forbidden { action: name }),
                    DenialCause::Decision => {
                        self.decision_event(subject, name, resource, "node", false, cached);
                        if is_approval_reason(reason.as_ref()) {
                            Err(Refusal::ApprovalCannotWait)
                        } else {
                            Err(Refusal::Forbidden { action: name })
                        }
                    }
                },
                PepRefusal::RateLimited { .. } => {
                    // Pupitre declares no budget: a correct PDP never gives this reason (3.18).
                    self.decision_event(subject, name, resource, "node", false, cached);
                    Err(Refusal::Forbidden { action: name })
                }
                PepRefusal::Unavailable(cause) => Err(self.unavailable(&cause, name, resource)),
                PepRefusal::Unauthenticated(_) => Err(Refusal::Unauthenticated),
                PepRefusal::Internal(_) | PepRefusal::Approval(_) => Err(Refusal::Forbidden { action: name }),
            },
            Outcome::ApprovalRequired(_) => Err(Refusal::Forbidden { action: name }),
        }
    }

    /// The display filter of 3.15 with the number of questions that it asked. The options route
    /// uses the count to keep its own budget (3.16).
    pub(crate) async fn visible_counted(
        &self,
        subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        budget: u32,
    ) -> Result<(Visibility, u32), Refusal> {
        let identity = self.identity(subject)?;
        let budget = budget as usize;
        let mut asked = 0usize;
        let mut result = Visibility::default();
        let mut index = 0usize;
        while index < items.len() {
            let remaining = budget - asked;
            if remaining == 0 {
                result.stopped_at = Some(index);
                break;
            }
            // A window never costs more than the budget in the worst case, so that the first
            // item of a window always ends: with the budget of the start (see `new`) the walk
            // makes progress.
            let worst = items[index..]
                .iter()
                .map(|(_, a, _)| a.iter().filter(|x| self.published.contains(x)).count())
                .max()
                .unwrap_or(1)
                .max(1);
            let take = (remaining / worst).max(1).min(items.len() - index);
            let window = &items[index..index + take];
            // The questions of one item: its published actions, in order (3.15).
            let lists: Vec<Vec<Action>> = window
                .iter()
                .map(|(_, actions, _)| actions.iter().copied().filter(|a| self.published.contains(a)).collect())
                .collect();
            let mut state: Vec<Option<bool>> =
                lists.iter().map(|l| if l.is_empty() { Some(false) } else { None }).collect();
            let mut next = vec![0usize; take];
            let mut cutoff: Option<usize> = None;
            loop {
                let pending: Vec<usize> = (0..take).filter(|&i| state[i].is_none()).collect();
                if pending.is_empty() {
                    break;
                }
                let available = budget - asked;
                if available == 0 {
                    cutoff = pending.first().copied();
                    break;
                }
                let batch: Vec<usize> = pending.into_iter().take(available).collect();
                let questions: Vec<(Permission, MatchMode, Action)> = batch
                    .iter()
                    .filter_map(|&i| {
                        let action = lists[i][next[i]];
                        let path = node_path(&self.service, &window[i].0)?;
                        Some((Permission::new(action.name(), path), window[i].2, action))
                    })
                    .collect();
                if questions.len() != batch.len() {
                    // A node that is not a path of the tree: the question cannot be sent.
                    return Err(Refusal::PdpUnavailable);
                }
                let (answers, requests) = self.ask_round(&identity, &questions).await?;
                asked += batch.len();
                for (k, &i) in batch.iter().enumerate() {
                    let (permission, mode, action) = &questions[k];
                    let seen = answers[k];
                    let mode_text = if *mode == MatchMode::Subtree { "subtree" } else { "node" };
                    self.decision_event(
                        subject,
                        action.name(),
                        &path_text(&permission.node),
                        mode_text,
                        seen,
                        requests == 0,
                    );
                    if seen {
                        state[i] = Some(true);
                    } else {
                        next[i] += 1;
                        if next[i] >= lists[i].len() {
                            state[i] = Some(false);
                        }
                    }
                }
            }
            let processed = cutoff.unwrap_or(take);
            for (i, seen) in state.iter().enumerate().take(processed) {
                if *seen == Some(true) {
                    result.visible.push(index + i);
                }
            }
            if let Some(c) = cutoff {
                result.stopped_at = Some(index + c);
                break;
            }
            index += take;
        }
        Ok((result, u32::try_from(asked).unwrap_or(u32::MAX)))
    }

    /// Ask one round of display questions in one call. The questions with the match `subtree`
    /// go in one batch; the others go one by one (3.15). The answers keep the order.
    async fn ask_round(
        &self,
        identity: &Identity,
        questions: &[(Permission, MatchMode, Action)],
    ) -> Result<(Vec<bool>, u32), Refusal> {
        let pep = self.cached_pep();
        let who = identity.clone();
        let list: Vec<(Permission, MatchMode)> = questions.iter().map(|(p, m, _)| (p.clone(), *m)).collect();
        let first = questions.first().map(|(p, _, a)| (a.name().to_owned(), path_text(&p.node)));
        let done = self
            .call(move || -> Result<Vec<bool>, PepRefusal> {
                let subtree: Vec<Permission> =
                    list.iter().filter(|(_, m)| *m == MatchMode::Subtree).map(|(p, _)| p.clone()).collect();
                let mut subtree_answers = if subtree.is_empty() { Vec::new() } else { pep.display(&who, &subtree)? };
                subtree_answers.reverse();
                let mut out = Vec::with_capacity(list.len());
                for (permission, mode) in &list {
                    if *mode == MatchMode::Subtree {
                        out.push(subtree_answers.pop().unwrap_or(false));
                    } else {
                        let operation = Operation::new(permission.action.clone(), permission.node.clone());
                        match pep.check(&who, &operation) {
                            Outcome::Allowed(_) => out.push(true),
                            Outcome::Refused(
                                refusal @ (PepRefusal::Unavailable(_) | PepRefusal::Unauthenticated(_)),
                            ) => {
                                return Err(refusal);
                            }
                            Outcome::Refused(_) | Outcome::ApprovalRequired(_) => out.push(false),
                        }
                    }
                }
                Ok(out)
            })
            .await;
        let (action, resource) = first.unwrap_or_default();
        match done {
            Err(failure) => Err(self.failed_call(&failure, &action, &resource)),
            Ok((Ok(answers), requests)) => Ok((answers, requests)),
            Ok((Err(PepRefusal::Unavailable(cause)), _)) => Err(self.unavailable(&cause, &action, &resource)),
            // The library refuses a question that is not in the document: the layer filters them.
            Ok((Err(_), _)) => Err(Refusal::PdpUnavailable),
        }
    }
}

fn pdp_config(config: &AuthzenConfig) -> PdpConfig {
    let mut pdp = PdpConfig::new(config.pdp_url.clone(), Secret::new(config.pdp_token.expose().to_owned()));
    pdp.allow_insecure_pdp_link = config.plain_http;
    pdp.request_timeout = config.pdp_timeout;
    pdp.idle_close_after = IDLE_CLOSE_AFTER;
    pdp.max_connections = config.pdp_max_connections as usize;
    pdp.max_response_bytes = MAX_RESPONSE_BYTES;
    pdp
}

fn pep_config(config: &AuthzenConfig, keys: procurauth_core::jws::KeySet) -> Result<PepConfig, String> {
    let mut pep = PepConfig::new(
        config.audience.clone(),
        config.header.clone(),
        config.issuer.clone(),
        keys,
        config.max_age.as_secs(),
        config.clock_skew.as_secs(),
        pdp_config(config),
    );
    pep.cache_ttl = config.cache_ttl;
    pep.cache_max_entries = config.cache_entries as usize;
    pep.display_max_questions = config.options_max_questions as usize;
    Ok(pep)
}

impl AuthzenAccess {
    /// The rule of P 9.3 and P 9.4 for `nbf`: when the token has it, it is a JSON integer with
    /// digits only, and it is not later than the clock plus the skew. The signature is already
    /// checked, so the payload is the text that the edge signed.
    fn check_not_before(&self, token: &str) -> Result<(), Refusal> {
        let payload = token.split('.').nth(1).ok_or(Refusal::Unauthenticated)?;
        let bytes = base64url_decode(payload).ok_or(Refusal::Unauthenticated)?;
        let claims: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| Refusal::Unauthenticated)?;
        let Some(nbf) = claims.get("nbf") else { return Ok(()) };
        let nbf = nbf.as_u64().ok_or(Refusal::Unauthenticated)?;
        if nbf > self.clock.now_seconds().saturating_add(self.skew_seconds) {
            return Err(Refusal::Unauthenticated);
        }
        Ok(())
    }
}

impl Access for AuthzenAccess {
    fn authenticate(&self, values: &[&[u8]]) -> Result<Subject, Refusal> {
        // Two copies of the header, or none, give 401 (3.4).
        let [value] = values else { return Err(Refusal::Unauthenticated) };
        if value.len() > MAX_TOKEN_BYTES {
            return Err(Refusal::Unauthenticated);
        }
        let token = std::str::from_utf8(value).map_err(|_| Refusal::Unauthenticated)?;
        let identity = self
            .cached_pep()
            .authenticate([(self.token_header.as_str(), token)])
            .map_err(|_| Refusal::Unauthenticated)?;
        // The library at the pinned commit does not check `nbf` (P 9.4): the layer does.
        self.check_not_before(token)?;
        let sub = identity.sub();
        if RESERVED_SUBJECTS.contains(&sub) {
            return Err(Refusal::Unauthenticated);
        }
        Subject::parse(sub).map_err(|_| Refusal::Unauthenticated)
    }

    async fn decide_access(&self, subject: &Subject) -> Result<(), Refusal> {
        let now = self.clock.now_millis();
        let service = format!("service:{}", self.service);
        if self.access_cache.lock().unwrap_or_else(PoisonError::into_inner).has(subject.as_str(), now) {
            self.decision_event(subject, ACCESS_ACTION, &service, "node", true, true);
            return Ok(());
        }
        let gate = self.gate.clone();
        let who = subject.as_str().to_owned();
        let done = self.call(move || gate.check(&who)).await;
        match done {
            Err(failure) => Err(self.failed_call(&failure, ACCESS_ACTION, &service)),
            Ok((Err(cause), _)) => Err(self.unavailable(&cause, ACCESS_ACTION, &service)),
            Ok((Ok(GateAnswer::Granted), _)) => {
                self.decision_event(subject, ACCESS_ACTION, &service, "node", true, false);
                self.access_cache.lock().unwrap_or_else(PoisonError::into_inner).put(subject.as_str(), now);
                Ok(())
            }
            Ok((Ok(GateAnswer::Denied(reason)), _)) => {
                self.decision_event(subject, ACCESS_ACTION, &service, "node", false, false);
                if is_approval_reason(reason.as_ref()) {
                    // A reason of P 17.1 for `access` is an answer that is not valid (P 10.1c).
                    return Err(self.unavailable(
                        &UnavailableCause::Answer("the answer to `access` has a reason of the approvals".to_owned()),
                        ACCESS_ACTION,
                        &service,
                    ));
                }
                Err(Refusal::Forbidden { action: ACCESS_ACTION })
            }
        }
    }

    async fn decide(&self, subject: &Subject, question: &Question, policy: CachePolicy) -> Result<Granted, Refusal> {
        let action = question.action;
        let name = static_name(action);
        // The match `subtree` is for the display only: it never lets an operation run (P 4.4).
        if question.match_mode != MatchMode::Node || !self.published.contains(&action) {
            return Err(Refusal::Forbidden { action: name });
        }
        let Some(path) = node_path(&self.service, &question.node) else {
            return Err(Refusal::Forbidden { action: name });
        };
        let resource = path_text(&path);
        let identity = self.identity(subject)?;
        let pep = match policy {
            CachePolicy::MayUseKept => self.cached_pep(),
            CachePolicy::Fresh => self.fresh.clone(),
        };
        let operation = Operation::new(name, path);
        match self.call(move || pep.decide(&identity, &operation)).await {
            Err(failure) => Err(self.failed_call(&failure, name, &resource)),
            Ok((outcome, requests)) => {
                let result = self.map_outcome(subject, action, &resource, outcome, requests);
                // The library at the pinned commit keeps an answer with a reason of P 17.1. P 10.1
                // forbids it, so the layer drops each kept answer, and the next question asks the PDP.
                if policy == CachePolicy::MayUseKept && result == Err(Refusal::ApprovalCannotWait) {
                    self.forget_kept_answers();
                }
                result
            }
        }
    }

    async fn decide_unknown(&self, subject: &Subject, action: Action) -> Refusal {
        let name = static_name(action);
        let service = format!("service:{}", self.service);
        if !self.published.contains(&action) {
            return Refusal::Forbidden { action: name };
        }
        let Ok(identity) = self.identity(subject) else { return Refusal::Unauthenticated };
        // Never a kept answer, and none is kept (3.14, rule 2): the PEP with no cache.
        let pep = self.fresh.clone();
        let operation = Operation::new(name, NodePath::service(self.service.clone()));
        match self.call(move || pep.decide(&identity, &operation)).await {
            Err(failure) => self.failed_call(&failure, name, &service),
            Ok((outcome, requests)) => match self.map_outcome(subject, action, &service, outcome, requests) {
                Ok(_) => Refusal::NotFound,
                Err(refusal) => refusal,
            },
        }
    }

    async fn visible(
        &self,
        subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        budget: u32,
    ) -> Result<Visibility, Refusal> {
        self.visible_counted(subject, items, budget).await.map(|(visibility, _)| visibility)
    }

    fn action_document(&self) -> Option<&[u8]> {
        Some(&self.document)
    }
}
