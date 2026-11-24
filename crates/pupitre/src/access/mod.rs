// SPDX-License-Identifier: MIT OR Apache-2.0
//! The access layer (section 3): the trait `Access`, the enum `AnyAccess`, the mode `open`, and
//! the action document of 3.11.
//!
//! The mode `open` is here. The mode `authzen` is in `authzen.rs`, with the link to the PDP in
//! `tls.rs`, the public keys in `keys.rs`, and the options route in `options.rs`. The server
//! calls only this interface (3.23).

mod authzen;
mod document;
mod events;
mod keys;
pub mod options;
mod tls;

use std::fmt;
use std::future::Future;

use pupitre_core::action::Action;
use pupitre_core::ids::Subject;
use pupitre_core::node::Node;

pub use authzen::{AuthzenAccess, IDLE_CLOSE_AFTER, MAX_RESPONSE_BYTES, MAX_TOKEN_BYTES};
pub use document::{action_document, action_document_bytes, library_document};
pub use events::{AccessCounters, AccessEvent, MAX_EVENTS};
pub use keys::{check_public_keys, parse_public_keys};
pub use tls::PdpLink;

/// The match of a question (P 4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// `node`: a decision about the node. The default.
    Node,
    /// `subtree`: for the display only (3.15). It never lets an operation run.
    Subtree,
}

/// One question: an action on a node with its chain (3.8, 3.12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The action.
    pub action: Action,
    /// The node. The server builds it from the catalog (P 4.5).
    pub node: Node,
    /// The match.
    pub match_mode: MatchMode,
}

/// The question of a route with no action (3.20): `access` on the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessQuestion;

/// Whether a kept answer can serve a question (3.17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// A read route: a kept answer can serve, and the answer can be kept.
    MayUseKept,
    /// A write route, a run route or the question of 3.14: no kept answer, and none is kept.
    Fresh,
}

/// The result of a `true` answer: the ceilings that apply (3.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Granted {
    /// The value of `context.limits.max_hosts`, when the answer has it.
    pub max_hosts: Option<u64>,
}

/// A refusal of the access layer (3.21). The server maps it to the error of 5.4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `401 unauthenticated`
    Unauthenticated,
    /// `403 forbidden`, with the use action of the route.
    Forbidden {
        /// The use action of the route (3.21).
        action: &'static str,
    },
    /// `403 approval_cannot_wait` (3.19)
    ApprovalCannotWait,
    /// `404 not_found` after the question of 3.14.
    NotFound,
    /// `503 pdp_unavailable` (3.18)
    PdpUnavailable,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Unauthenticated => f.write_str("unauthenticated"),
            Refusal::Forbidden { action } => write!(f, "forbidden: {action}"),
            Refusal::ApprovalCannotWait => f.write_str("approval_cannot_wait"),
            Refusal::NotFound => f.write_str("not_found"),
            Refusal::PdpUnavailable => f.write_str("pdp_unavailable"),
        }
    }
}

/// The result of the display filter (3.15).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Visibility {
    /// The indexes of the visible items, in order.
    pub visible: Vec<usize>,
    /// The index of the first item that the budget of questions did not reach, when the budget
    /// stopped the walk. The list route then gives a `next` cursor (5.6).
    pub stopped_at: Option<usize>,
}

/// The interface of the access layer (3.23).
pub trait Access: Send + Sync {
    /// Check the identity token (3.4). `values` are the values of the header
    /// `access.identity.header`, one for each copy of the header.
    fn authenticate(&self, values: &[&[u8]]) -> Result<Subject, Refusal>;
    /// Ask the question of a route with no action (3.20).
    fn decide_access(&self, subject: &Subject) -> impl Future<Output = Result<(), Refusal>> + Send;
    /// Ask the use of an operation (3.12). The answer gives the ceilings.
    fn decide(
        &self,
        subject: &Subject,
        question: &Question,
        policy: CachePolicy,
    ) -> impl Future<Output = Result<Granted, Refusal>> + Send;
    /// The question of 3.14 for an unknown item: the use action on the service, never kept.
    /// The answer is `NotFound` after a `true` answer, else the refusal.
    fn decide_unknown(&self, subject: &Subject, action: Action) -> impl Future<Output = Refusal> + Send;
    /// The display filter of 3.15: for each item, the actions to ask, in order. One request asks
    /// at most `budget` questions.
    fn visible(
        &self,
        subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        budget: u32,
    ) -> impl Future<Output = Result<Visibility, Refusal>> + Send;
    /// The display filter with the count of the questions that it asked. The options route
    /// (3.16) uses the count to bound a walk. A layer that asks no question gives a count of 0.
    fn visible_counted(
        &self,
        subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        budget: u32,
    ) -> impl Future<Output = Result<(Visibility, u32), Refusal>> + Send {
        async move { Ok((self.visible(subject, items, budget).await?, 0)) }
    }
    /// The bytes of the action document (3.11), or `None` in the mode `open`.
    fn action_document(&self) -> Option<&[u8]>;
}

/// The mode `open` (3.22): no token and no question. The subject is `-`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAccess;

impl Access for OpenAccess {
    fn authenticate(&self, _values: &[&[u8]]) -> Result<Subject, Refusal> {
        Ok(Subject::anonymous())
    }

    async fn decide_access(&self, _subject: &Subject) -> Result<(), Refusal> {
        Ok(())
    }

    async fn decide(&self, _subject: &Subject, _question: &Question, _policy: CachePolicy) -> Result<Granted, Refusal> {
        Ok(Granted::default())
    }

    async fn decide_unknown(&self, _subject: &Subject, _action: Action) -> Refusal {
        Refusal::NotFound
    }

    async fn visible(
        &self,
        _subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        _budget: u32,
    ) -> Result<Visibility, Refusal> {
        Ok(Visibility { visible: (0..items.len()).collect(), stopped_at: None })
    }

    fn action_document(&self) -> Option<&[u8]> {
        None
    }
}

/// The access layer of the process: an enum, with no `dyn` (1.5).
#[derive(Debug)]
pub enum AnyAccess {
    /// The mode `open`.
    Open(OpenAccess),
    /// The mode `authzen`.
    Authzen(Box<AuthzenAccess>),
}

macro_rules! dispatch {
    ($self:ident, $a:ident => $call:expr) => {
        match $self {
            AnyAccess::Open($a) => $call,
            AnyAccess::Authzen($a) => $call,
        }
    };
}

impl Access for AnyAccess {
    fn authenticate(&self, values: &[&[u8]]) -> Result<Subject, Refusal> {
        dispatch!(self, a => a.authenticate(values))
    }
    async fn decide_access(&self, subject: &Subject) -> Result<(), Refusal> {
        dispatch!(self, a => a.decide_access(subject).await)
    }
    async fn decide(&self, subject: &Subject, question: &Question, policy: CachePolicy) -> Result<Granted, Refusal> {
        dispatch!(self, a => a.decide(subject, question, policy).await)
    }
    async fn decide_unknown(&self, subject: &Subject, action: Action) -> Refusal {
        dispatch!(self, a => a.decide_unknown(subject, action).await)
    }
    async fn visible(
        &self,
        subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        budget: u32,
    ) -> Result<Visibility, Refusal> {
        dispatch!(self, a => a.visible(subject, items, budget).await)
    }
    async fn visible_counted(
        &self,
        subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        budget: u32,
    ) -> Result<(Visibility, u32), Refusal> {
        match self {
            AnyAccess::Open(_) => Ok((Visibility { visible: (0..items.len()).collect(), stopped_at: None }, 0)),
            AnyAccess::Authzen(a) => a.visible_counted(subject, items, budget).await,
        }
    }
    fn action_document(&self) -> Option<&[u8]> {
        dispatch!(self, a => a.action_document())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_mode_open_asks_nothing() {
        let a = AnyAccess::Open(OpenAccess);
        let subject = a.authenticate(&[]).unwrap();
        assert_eq!(subject.as_str(), "-");
        let q = Question { action: Action::RunApply, node: Node::Service, match_mode: MatchMode::Node };
        assert_eq!(a.decide(&subject, &q, CachePolicy::Fresh).await, Ok(Granted::default()));
        assert_eq!(a.decide_unknown(&subject, Action::ConfigRead).await, Refusal::NotFound);
        assert!(a.action_document().is_none());
        let items = vec![(Node::Service, vec![Action::ConfigRead], MatchMode::Subtree); 3];
        assert_eq!(a.visible(&subject, &items, 1).await.unwrap().visible, vec![0, 1, 2]);
    }
}
