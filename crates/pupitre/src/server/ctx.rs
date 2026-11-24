// SPDX-License-Identifier: MIT OR Apache-2.0
//! The context of one request for a handler. It holds the shared state, the subject, the route,
//! the parameters and the body. It has the helpers that the order of 3.13 needs.

use std::sync::Arc;

use bytes::Bytes;
use http::HeaderMap;
use pupitre_core::action::Action;
use pupitre_core::doc::DocKey;
use pupitre_core::ids::{EnvId, HostId, Subject, UnitId};
use pupitre_core::json::{self, Json, ReadLimits};
use pupitre_core::node::Node;
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::types::{Message, StoreError, WriteMeta};

use super::error::{ApiError, Code};
use super::query::Query;
use super::routes::{Params, RouteSpec};
use super::state::AppState;
use super::telemetry::Trace;
use crate::access::{Access, CachePolicy, Granted, MatchMode, Question, Refusal};
use crate::config::LogLevel;

/// The request that a handler serves.
pub struct Ctx<A, S> {
    /// The shared state.
    pub state: Arc<AppState<A, S>>,
    /// The id of the request (5.3).
    pub request_id: String,
    /// The subject: `-` in the mode `open`.
    pub subject: Subject,
    /// The route.
    pub route: &'static RouteSpec,
    /// The variables of the path.
    pub params: Params,
    /// The query parameters that the route names.
    pub query: Query,
    /// The raw query string, for the options route (3.16).
    pub raw_query: Option<String>,
    /// The headers of the request.
    pub headers: HeaderMap,
    /// The body, read within its limit (3.13, step 5).
    pub body: Bytes,
    /// The spans of the request.
    pub trace: Arc<Trace>,
}

/// The map of a refusal of the access layer to the error of 5.4 (3.21). The member `action` of
/// `forbidden` is the use action of the route. The access layer cannot change it. Thus a `403`
/// does not show if a node exists.
pub fn refusal_error_for(r: &Refusal, use_action: &str) -> ApiError {
    match r {
        Refusal::Forbidden { .. } => ApiError::forbidden(use_action),
        other => refusal_error(other),
    }
}

/// The map of a refusal of the access layer to the error of 5.4 (3.21).
pub fn refusal_error(r: &Refusal) -> ApiError {
    match r {
        Refusal::Unauthenticated => ApiError::new(Code::Unauthenticated),
        Refusal::Forbidden { action } => ApiError::forbidden(action),
        Refusal::ApprovalCannotWait => ApiError::new(Code::ApprovalCannotWait),
        Refusal::NotFound => ApiError::not_found(),
        Refusal::PdpUnavailable => ApiError::new(Code::PdpUnavailable),
    }
}

impl<A: Access, S: Store> Ctx<A, S> {
    /// The map of a store error to the error of 5.4. A failure of the store is counted. A
    /// defect is logged with its detail, and the answer has no detail.
    pub fn store_error(&self, method: &'static str, e: &StoreError) -> ApiError {
        match e {
            StoreError::NotFound => ApiError::not_found(),
            StoreError::Conflict { current } => ApiError::conflict(current.as_ref()),
            StoreError::AlreadyExists => ApiError::new(Code::AlreadyExists),
            StoreError::QueueFull => ApiError::new(Code::RunQueueFull),
            StoreError::CheckUsed => ApiError::new(Code::CheckUsed),
            StoreError::Unavailable(_) => {
                self.state.metrics.store_error(method);
                self.state
                    .logger
                    .event(LogLevel::Warn, "store.unavailable")
                    .request_id(&self.request_id)
                    .text("method", method)
                    .emit();
                ApiError::new(Code::StoreUnavailable)
            }
            other => {
                self.state.metrics.store_error(method);
                self.internal(&format!("store {method}: {other}"))
            }
        }
    }

    /// A defect: the log has the detail, the answer has none (5.4).
    pub fn internal(&self, detail: &str) -> ApiError {
        self.state
            .logger
            .event(LogLevel::Error, "server.internal_error")
            .request_id(&self.request_id)
            .text("route", &self.route.label)
            .text("detail", detail)
            .emit();
        ApiError::new(Code::InternalError)
    }

    /// The metadata of a write (2.18).
    pub fn write_meta(&self, message: Message) -> WriteMeta {
        WriteMeta { saved_by: self.subject.clone(), saved_at: UtcMillis::now(), message }
    }

    /// Read the body as one JSON value with the strict reader of 2.5 (3.13, step 6). The body
    /// has at most `server.max_request_bytes` bytes. A body that is not valid gives `400`.
    pub fn body_json(&self) -> Result<Json, ApiError> {
        let max = usize::try_from(self.state.settings.max_request_bytes).unwrap_or(usize::MAX);
        json::parse(&self.body, ReadLimits::bytes(max)).map_err(|_| ApiError::bad_request())
    }

    /// A path variable as an environment id. A text that breaks the rule of 2.1 gives `None`:
    /// no such node exists (3.14).
    pub fn env(&self) -> Option<EnvId> {
        self.params.get("e").and_then(|t| EnvId::parse(t).ok())
    }

    /// A path variable as a unit id.
    pub fn unit(&self) -> Option<UnitId> {
        self.params.get("u").and_then(|t| UnitId::parse(t).ok())
    }

    /// A path variable as a host id.
    pub fn host(&self) -> Option<HostId> {
        self.params.get("h").and_then(|t| HostId::parse(t).ok())
    }

    /// The key of the document of a kind, from the variables of the path. `None` when a
    /// variable breaks its rule.
    pub fn doc_key(&self, kind: pupitre_core::doc::DocKind) -> Option<DocKey> {
        use pupitre_core::doc::DocKind::{Common, Environment, Host, HostValues, Unit, UnitValues};
        Some(match kind {
            Environment => DocKey::Environment(self.env()?),
            Common => DocKey::Common(self.env()?),
            Host => DocKey::Host(self.env()?, self.host()?),
            Unit => DocKey::Unit(self.env()?, self.unit()?),
            UnitValues => DocKey::UnitValues(self.env()?, self.unit()?),
            HostValues => DocKey::HostValues(self.env()?, self.unit()?, self.host()?),
        })
    }

    /// The decision of the use (3.13, steps 7 and 8) for a node of the tree.
    ///
    /// `node` is `None` when the path names no node of the catalog. The question of 3.14 then
    /// asks the use action on the service with the match `node`, and the answer is
    /// `404 not_found` after a `true` answer. A node that the catalog holds gets the question
    /// of the use. A read route can use a kept answer. A write route and a run route never use
    /// one (3.17).
    pub async fn authorize(
        &self,
        action: Action,
        node: Option<Node>,
        policy: CachePolicy,
    ) -> Result<Granted, ApiError> {
        let Some(node) = node else {
            let refusal = self.state.access.decide_unknown(&self.subject, action).await;
            self.record_decision(action, "node", &Node::Service, Err(&refusal));
            return Err(refusal_error_for(&refusal, action.name()));
        };
        let question = Question { action, node: node.clone(), match_mode: MatchMode::Node };
        let span = self.trace.child("access.decide").attr("action", action.name());
        let result = self.state.access.decide(&self.subject, &question, policy).await;
        span.end(matches!(result, Err(Refusal::PdpUnavailable)));
        match result {
            Ok(granted) => {
                self.record_decision(action, "node", &node, Ok(()));
                Ok(granted)
            }
            Err(refusal) => {
                self.record_decision(action, "node", &node, Err(&refusal));
                Err(refusal_error_for(&refusal, action.name()))
            }
        }
    }

    /// The question `access` of a route with no action (3.20).
    pub async fn authorize_access(&self) -> Result<(), ApiError> {
        let span = self.trace.child("access.decide").attr("action", "access");
        let result = self.state.access.decide_access(&self.subject).await;
        span.end(matches!(result, Err(Refusal::PdpUnavailable)));
        match result {
            Ok(()) => {
                self.record_access_decision(Ok(()));
                Ok(())
            }
            Err(refusal) => {
                self.record_access_decision(Err(&refusal));
                Err(refusal_error_for(&refusal, pupitre_core::action::ACCESS_ACTION))
            }
        }
    }

    fn record_access_decision(&self, result: Result<(), &Refusal>) {
        if self.state.settings.is_open() {
            return;
        }
        self.count_pdp(result);
        self.state
            .logger
            .event(LogLevel::Info, "access.decision")
            .request_id(&self.request_id)
            .text("subject", self.subject.as_str())
            .text("action", "access")
            .text("resource", "")
            .text("match", "node")
            .text("decision", decision_text(result))
            .emit();
    }

    fn record_decision(&self, action: Action, match_text: &str, node: &Node, result: Result<(), &Refusal>) {
        if self.state.settings.is_open() {
            return;
        }
        self.count_pdp(result);
        self.state
            .logger
            .event(LogLevel::Info, "access.decision")
            .request_id(&self.request_id)
            .text("subject", self.subject.as_str())
            .text("action", action.name())
            .text("resource", &node.path())
            .text("match", match_text)
            .text("decision", decision_text(result))
            .emit();
    }

    fn count_pdp(&self, result: Result<(), &Refusal>) {
        self.state.metrics.pdp_request(match result {
            Ok(()) => "allow",
            Err(Refusal::PdpUnavailable) => "error",
            Err(_) => "deny",
        });
    }
}

fn decision_text(result: Result<(), &Refusal>) -> &'static str {
    match result {
        Ok(()) => "allow",
        Err(Refusal::PdpUnavailable) => "unavailable",
        Err(_) => "deny",
    }
}
