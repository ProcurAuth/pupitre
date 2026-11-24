// SPDX-License-Identifier: MIT OR Apache-2.0
//! A fake `Access` for the tests of the server. It records each question, so a test can compare
//! the questions of a route with the table of 3.12.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use pupitre_core::action::Action;
use pupitre_core::ids::Subject;
use pupitre_core::node::Node;

use pupitre::access::{Access, CachePolicy, Granted, MatchMode, Question, Refusal, Visibility};

/// One question that the server asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// `decide`: the subject, the action, the node path (empty for the service), the match and
    /// the cache policy.
    Use { subject: String, action: &'static str, node: String, mode: &'static str, policy: &'static str },
    /// `decide_unknown`: the subject and the action.
    Unknown { subject: String, action: &'static str },
    /// `decide_access`
    Access { subject: String },
    /// `visible`: the items (node path and actions), and the budget.
    Visible { items: Vec<(String, Vec<&'static str>)>, budget: u32 },
}

type Decide = dyn Fn(&str, Action, &Node) -> Result<Granted, Refusal> + Send + Sync;
type Show = dyn Fn(&Node) -> bool + Send + Sync;

/// The state of the fake.
pub struct FakeInner {
    pub asked: Mutex<Vec<Asked>>,
    decide: Mutex<Arc<Decide>>,
    show: Mutex<Arc<Show>>,
    access: Mutex<Result<(), Refusal>>,
    stop_after: Mutex<Option<usize>>,
    visible_error: Mutex<Option<Refusal>>,
    document: Vec<u8>,
}

/// The fake, shared with the test.
#[derive(Clone)]
pub struct FakeAccess(pub Arc<FakeInner>);

pub const TOKEN_ALICE: &str = "tok-alice";
pub const TOKEN_BOB: &str = "tok-bob";

impl FakeAccess {
    /// A fake that allows each question, with the bytes of an action document.
    pub fn allow_all() -> FakeAccess {
        FakeAccess(Arc::new(FakeInner {
            asked: Mutex::new(Vec::new()),
            decide: Mutex::new(Arc::new(|_, _, _| Ok(Granted::default()))),
            show: Mutex::new(Arc::new(|_| true)),
            access: Mutex::new(Ok(())),
            stop_after: Mutex::new(None),
            visible_error: Mutex::new(None),
            document: br#"{"server":"pupitre-test"}"#.to_vec(),
        }))
    }

    /// Change the answer to `decide`.
    pub fn decide_with(&self, f: impl Fn(&str, Action, &Node) -> Result<Granted, Refusal> + Send + Sync + 'static) {
        *self.0.decide.lock().unwrap() = Arc::new(f);
    }

    /// Change the nodes that the display filter shows.
    pub fn show_with(&self, f: impl Fn(&Node) -> bool + Send + Sync + 'static) {
        *self.0.show.lock().unwrap() = Arc::new(f);
    }

    /// Change the answer to the question `access`.
    pub fn set_access(&self, r: Result<(), Refusal>) {
        *self.0.access.lock().unwrap() = r;
    }

    /// Make the display filter stop after this count of items, as its budget does.
    pub fn stop_after(&self, n: Option<usize>) {
        *self.0.stop_after.lock().unwrap() = n;
    }

    /// Make the display filter fail, as a PDP outage does.
    pub fn fail_visible(&self, r: Option<Refusal>) {
        *self.0.visible_error.lock().unwrap() = r;
    }

    /// The questions so far.
    pub fn asked(&self) -> Vec<Asked> {
        self.0.asked.lock().unwrap().clone()
    }

    /// Forget the questions.
    pub fn clear(&self) {
        self.0.asked.lock().unwrap().clear();
    }
}

fn mode_text(m: MatchMode) -> &'static str {
    match m {
        MatchMode::Node => "node",
        MatchMode::Subtree => "subtree",
    }
}

fn policy_text(p: CachePolicy) -> &'static str {
    match p {
        CachePolicy::MayUseKept => "kept",
        CachePolicy::Fresh => "fresh",
    }
}

impl Access for FakeAccess {
    fn authenticate(&self, values: &[&[u8]]) -> Result<Subject, Refusal> {
        match values {
            [v] => match std::str::from_utf8(v).unwrap_or("") {
                TOKEN_ALICE => Ok(Subject::parse("alice").unwrap()),
                TOKEN_BOB => Ok(Subject::parse("bob").unwrap()),
                _ => Err(Refusal::Unauthenticated),
            },
            _ => Err(Refusal::Unauthenticated),
        }
    }

    async fn decide_access(&self, subject: &Subject) -> Result<(), Refusal> {
        self.0.asked.lock().unwrap().push(Asked::Access { subject: subject.as_str().to_string() });
        self.0.access.lock().unwrap().clone()
    }

    async fn decide(&self, subject: &Subject, question: &Question, policy: CachePolicy) -> Result<Granted, Refusal> {
        self.0.asked.lock().unwrap().push(Asked::Use {
            subject: subject.as_str().to_string(),
            action: question.action.name(),
            node: question.node.path(),
            mode: mode_text(question.match_mode),
            policy: policy_text(policy),
        });
        let f = self.0.decide.lock().unwrap().clone();
        f(subject.as_str(), question.action, &question.node)
    }

    async fn decide_unknown(&self, subject: &Subject, action: Action) -> Refusal {
        self.0
            .asked
            .lock()
            .unwrap()
            .push(Asked::Unknown { subject: subject.as_str().to_string(), action: action.name() });
        let f = self.0.decide.lock().unwrap().clone();
        match f(subject.as_str(), action, &Node::Service) {
            Ok(_) => Refusal::NotFound,
            Err(Refusal::Forbidden { .. }) => Refusal::Forbidden { action: action.name() },
            Err(other) => other,
        }
    }

    async fn visible(
        &self,
        _subject: &Subject,
        items: &[(Node, Vec<Action>, MatchMode)],
        budget: u32,
    ) -> Result<Visibility, Refusal> {
        self.0.asked.lock().unwrap().push(Asked::Visible {
            items: items.iter().map(|(n, a, _)| (n.path(), a.iter().map(|x| x.name()).collect())).collect(),
            budget,
        });
        if let Some(r) = self.0.visible_error.lock().unwrap().clone() {
            return Err(r);
        }
        let show = self.0.show.lock().unwrap().clone();
        let stop = *self.0.stop_after.lock().unwrap();
        let limit = stop.unwrap_or(items.len()).min(items.len());
        let visible = (0..limit).filter(|i| show(&items[*i].0)).collect();
        Ok(Visibility { visible, stopped_at: stop.filter(|n| *n < items.len()) })
    }

    fn action_document(&self) -> Option<&[u8]> {
        Some(&self.0.document)
    }
}
