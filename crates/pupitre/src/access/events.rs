// SPDX-License-Identifier: MIT OR Apache-2.0
//! The events and the counters of the access layer (6.27, 6.28).
//!
//! The trait `Access` has no argument for a log. The access layer thus keeps its events in a
//! small queue, and the server takes them with `take_events` after each request. The queue has
//! a bound. At the bound, the layer drops the new event and counts it. No event holds a token,
//! a credential or a value of a document.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};

/// The most events that wait for the server.
pub const MAX_EVENTS: usize = 1024;

/// One event of the access layer. The names are the events of 6.27.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessEvent {
    /// `access.decision`: one question and its answer.
    Decision {
        /// The subject of the question.
        subject: String,
        /// The action.
        action: String,
        /// The node with its chain, for example `service:pupitre/environment:production`.
        resource: String,
        /// `node` or `subtree`.
        match_mode: &'static str,
        /// The answer.
        decision: bool,
        /// True when a kept answer served the question and no request went to the PDP.
        cached: bool,
    },
    /// `access.pdp_failed`: no connection, a timeout, a status other than 200, or no place in the pool.
    PdpFailed {
        /// The action.
        action: String,
        /// The node with its chain.
        resource: String,
        /// The cause.
        error: String,
    },
    /// `access.pdp_invalid_answer`: the PDP answered with a body that is not valid.
    PdpInvalidAnswer {
        /// The action.
        action: String,
        /// The node with its chain.
        resource: String,
        /// The cause.
        error: String,
    },
    /// `access.ceiling_unenforceable`: a `true` answer with a ceiling that Pupitre cannot apply.
    CeilingUnenforceable {
        /// The action.
        action: String,
        /// The node with its chain.
        resource: String,
        /// The cause.
        error: String,
    },
}

/// The counters of 6.28: the questions of the PDP by result, and the kept answers that served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccessCounters {
    /// `pupitre_pdp_requests_total{result="allow"}`: `true` answers of the PDP.
    pub allow: u64,
    /// `pupitre_pdp_requests_total{result="deny"}`: `false` answers of the PDP.
    pub deny: u64,
    /// `pupitre_pdp_requests_total{result="error"}`: no valid answer.
    pub error: u64,
    /// `pupitre_pdp_cache_hits_total`: questions that a kept answer served.
    pub cache_hits: u64,
    /// The events that the bound of the queue dropped.
    pub dropped_events: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Recorder {
    queue: Mutex<VecDeque<AccessEvent>>,
    allow: AtomicU64,
    deny: AtomicU64,
    error: AtomicU64,
    cache_hits: AtomicU64,
    dropped: AtomicU64,
}

impl Recorder {
    pub(crate) fn push(&self, event: AccessEvent) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.len() >= MAX_EVENTS {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        } else {
            queue.push_back(event);
        }
    }

    pub(crate) fn take(&self) -> Vec<AccessEvent> {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.drain(..).collect()
    }

    /// Count one question: `cached` questions are hits, the others are `allow` or `deny`.
    pub(crate) fn count_answer(&self, decision: bool, cached: bool) {
        let counter = if cached {
            &self.cache_hits
        } else if decision {
            &self.allow
        } else {
            &self.deny
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn count_error(&self) {
        self.error.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn counters(&self) -> AccessCounters {
        AccessCounters {
            allow: self.allow.load(Ordering::Relaxed),
            deny: self.deny.load(Ordering::Relaxed),
            error: self.error.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            dropped_events: self.dropped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> AccessEvent {
        AccessEvent::PdpFailed { action: "config.read".into(), resource: "service:pupitre".into(), error: "x".into() }
    }

    #[test]
    fn the_queue_has_a_bound_and_counts_what_it_drops() {
        let r = Recorder::default();
        for _ in 0..MAX_EVENTS + 3 {
            r.push(failed());
        }
        assert_eq!(r.counters().dropped_events, 3);
        assert_eq!(r.take().len(), MAX_EVENTS);
        assert!(r.take().is_empty());
    }

    #[test]
    fn the_counters_follow_the_kind_of_answer() {
        let r = Recorder::default();
        r.count_answer(true, false);
        r.count_answer(false, false);
        r.count_answer(true, true);
        r.count_error();
        let c = r.counters();
        assert_eq!((c.allow, c.deny, c.cache_hits, c.error), (1, 1, 1, 1));
    }
}
