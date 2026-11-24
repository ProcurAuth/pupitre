// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the mode `authzen` (3.4, 3.14, 3.15, 3.17 to 3.20) against a fake PDP on the loopback
//! address. Each test names the paragraph that it checks.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod access_support;

use std::sync::Arc;
use std::time::Duration;

use access_support::*;
use procurauth_core::jws::sign_compact_text;
use procurauth_pep::clock::ManualClock;
use pupitre::access::{
    Access, AccessEvent, AuthzenAccess, CachePolicy, Granted, MatchMode, PdpLink, Question, Refusal,
};
use pupitre_core::action::Action;
use pupitre_core::ids::Subject;
use pupitre_core::node::Node;
use serde_json::json;

const HOST: &str = "environment:production/unit:relay/host:host-a";

fn node(text: &str) -> Node {
    Node::parse_path(text).unwrap()
}

fn question(action: Action, node_text: Option<&str>) -> Question {
    Question { action, node: node_text.map_or(Node::Service, node), match_mode: MatchMode::Node }
}

fn start(policy: impl Fn(&access_support::Question) -> Answer + Send + Sync + 'static) -> (FakePdp, AuthzenAccess) {
    let pdp = FakePdp::start(policy);
    let access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    (pdp, access)
}

fn subject(access: &AuthzenAccess, sub: &str) -> Subject {
    let t = token(&ed_key(), Some("ed-1"), sub);
    access.authenticate(&[t.as_bytes()]).unwrap()
}

fn auth(access: &AuthzenAccess, text: &str) -> Result<Subject, Refusal> {
    access.authenticate(&[text.as_bytes()])
}

// ---- 3.4: the identity token --------------------------------------------------------------

#[test]
fn p3_4_a_good_token_of_each_algorithm_gives_the_subject() {
    let (_pdp, access) = start(|_| Answer::yes());
    assert_eq!(auth(&access, &token(&ed_key(), Some("ed-1"), "alice")).unwrap().as_str(), "alice");
    assert_eq!(auth(&access, &token(&es_key(), Some("es-1"), "key:9b1c")).unwrap().as_str(), "key:9b1c");
}

#[test]
fn p3_4_no_header_or_two_copies_give_401() {
    let (_pdp, access) = start(|_| Answer::yes());
    let t = token(&ed_key(), Some("ed-1"), "alice");
    assert_eq!(access.authenticate(&[]), Err(Refusal::Unauthenticated));
    assert_eq!(access.authenticate(&[t.as_bytes(), t.as_bytes()]), Err(Refusal::Unauthenticated));
}

#[test]
fn p3_4_a_token_that_is_not_text_or_too_large_gives_401() {
    let (_pdp, access) = start(|_| Answer::yes());
    assert_eq!(access.authenticate(&[&[0xff, 0xfe, 0x41]]), Err(Refusal::Unauthenticated));
    let big = "a".repeat(pupitre::access::MAX_TOKEN_BYTES + 1);
    assert_eq!(auth(&access, &big), Err(Refusal::Unauthenticated));
    assert_eq!(auth(&access, ""), Err(Refusal::Unauthenticated));
}

#[test]
fn p3_4_each_claim_check_of_9_4_refuses() {
    let (_pdp, access) = start(|_| Answer::yes());
    let now = now();
    let k = ed_key();
    let bad = |edit: &dyn Fn(&mut serde_json::Value)| {
        let t = token_with(&k, Some("ed-1"), |c| edit(c), "alice");
        assert_eq!(auth(&access, &t), Err(Refusal::Unauthenticated), "{t}");
    };
    bad(&|c| c["iss"] = json!("https://other.example.invalid"));
    bad(&|c| c["aud"] = json!("other"));
    bad(&|c| c["aud"] = json!(["other", "more"]));
    bad(&|c| c["exp"] = json!(now - 1000));
    // The skew is 30 seconds: an `iat` 5 minutes ahead is refused.
    bad(&|c| {
        c["iat"] = json!(now + 300);
        c["exp"] = json!(now + 400);
    });
    // The life is above `max_age` (300 s).
    bad(&|c| c["exp"] = json!(now + 1000));
    bad(&|c| c["nbf"] = json!(now + 600));
    bad(&|c| {
        c.as_object_mut().unwrap().remove("sub");
    });
    bad(&|c| {
        c.as_object_mut().unwrap().remove("iat");
    });
    bad(&|c| c["sub"] = json!(""));
    // `aud` as an array that holds the service id is valid.
    let ok = token_with(&k, Some("ed-1"), |c| c["aud"] = json!(["x", "pupitre"]), "alice");
    assert!(auth(&access, &ok).is_ok());
}

#[test]
fn p3_4_the_numbers_of_the_claims_are_integers_with_digits_only() {
    let (_pdp, access) = start(|_| Answer::yes());
    let n = now();
    let header = r#"{"alg":"Ed25519","typ":"JWT","kid":"ed-1"}"#;
    for iat in [format!("{n}.0"), format!("{n}e0"), "-1".to_owned()] {
        let payload = format!(r#"{{"iss":"{ISSUER}","sub":"alice","aud":"{AUDIENCE}","iat":{iat},"exp":{}}}"#, n + 100);
        let t = sign_compact_text(header, &payload, &ed_key());
        assert_eq!(auth(&access, &t), Err(Refusal::Unauthenticated), "{iat}");
    }
}

#[test]
fn p3_4_the_algorithm_the_type_and_the_kid_are_checked() {
    let (_pdp, access) = start(|_| Answer::yes());
    let now = now();
    let claims = format!(r#"{{"iss":"{ISSUER}","sub":"alice","aud":"{AUDIENCE}","iat":{now},"exp":{}}}"#, now + 100);
    for header in [
        r#"{"alg":"none","typ":"JWT","kid":"ed-1"}"#,
        r#"{"alg":"HS256","typ":"JWT","kid":"ed-1"}"#,
        r#"{"alg":"EdDSA","typ":"JWT","kid":"ed-1"}"#,
        r#"{"alg":"Ed25519","typ":"at+jwt","kid":"ed-1"}"#,
        r#"{"alg":"Ed25519","typ":"JWT"}"#,
        r#"{"alg":"Ed25519","typ":"JWT","kid":"unknown"}"#,
        // The key of the `kid` is the ES256 key, but the token is signed with the Ed25519 key.
        r#"{"alg":"Ed25519","typ":"JWT","kid":"es-1"}"#,
    ] {
        let t = sign_compact_text(header, &claims, &ed_key());
        assert_eq!(auth(&access, &t), Err(Refusal::Unauthenticated), "{header}");
    }
    // A token that an unknown key signed.
    let other = procurauth_core::jws::SigningKey::from_ed25519_seed(&[1; 32]);
    assert_eq!(auth(&access, &token(&other, Some("ed-1"), "alice")), Err(Refusal::Unauthenticated));
}

#[test]
fn p3_4_the_subjects_of_the_history_cannot_be_a_sub() {
    let (_pdp, access) = start(|_| Answer::yes());
    for sub in ["-", "external", "import"] {
        assert_eq!(auth(&access, &token(&ed_key(), Some("ed-1"), sub)), Err(Refusal::Unauthenticated), "{sub}");
    }
}

#[test]
fn p3_4_the_other_headers_are_never_read_so_the_sub_is_only_from_the_values() {
    // The interface gets only the values of the configured header. An empty list is no token.
    let (_pdp, access) = start(|_| Answer::yes());
    assert_eq!(access.token_header(), HEADER);
    assert_eq!(access.authenticate(&[]), Err(Refusal::Unauthenticated));
}

// ---- 3.18: the map of the answers ----------------------------------------------------------

#[tokio::test]
async fn p3_18_true_with_no_ceiling_lets_the_operation_run() {
    let (_pdp, access) = start(|_| Answer::yes());
    let s = subject(&access, "alice");
    let q = question(Action::ConfigRead, Some(HOST));
    assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Ok(Granted { max_hosts: None }));
}

#[tokio::test]
async fn p3_18_true_with_max_hosts_on_a_run_action_gives_the_ceiling() {
    let (_pdp, access) = start(|_| Answer::yes().with_limit("max_hosts", 7));
    let s = subject(&access, "alice");
    for action in [Action::RunCheck, Action::RunApply, Action::CommandRun] {
        let q = question(action, Some("environment:production/unit:relay"));
        assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Ok(Granted { max_hosts: Some(7) }), "{action:?}");
    }
}

#[tokio::test]
async fn p3_18_false_gives_403_with_the_use_action_whatever_the_reason() {
    for reason in
        [None, Some("unknown_subject"), Some("invalid_resource"), Some("other_reason"), Some("budget_exceeded")]
    {
        let (_pdp, access) = start(move |_| match reason {
            // Pupitre declares no budget, but a valid `budget_exceeded` answer has `retry_after`.
            Some("budget_exceeded") => {
                Answer::raw(json!({"decision": false, "context": {"reason": "budget_exceeded", "retry_after": 12}}))
            }
            Some(r) => Answer::no_because(r),
            None => Answer::no(),
        });
        let s = subject(&access, "alice");
        let r =
            access.decide(&s, &question(Action::SecretWrite, Some("environment:production")), CachePolicy::Fresh).await;
        assert_eq!(r, Err(Refusal::Forbidden { action: "secret.write" }), "{reason:?}");
    }
}

#[tokio::test]
async fn p3_18_budget_exceeded_without_retry_after_is_not_a_valid_answer() {
    let (_pdp, access) = start(|_| Answer::no_because("budget_exceeded"));
    let s = subject(&access, "alice");
    let r = access.decide(&s, &question(Action::ConfigRead, None), CachePolicy::Fresh).await;
    assert_eq!(r, Err(Refusal::PdpUnavailable));
}

#[tokio::test]
async fn p3_18_and_3_19_a_reason_of_the_approvals_gives_approval_cannot_wait() {
    for reason in ["approval_required", "approval_pending", "approval_denied", "approval_expired"] {
        let (_pdp, access) = start(move |_| Answer::no_because(reason));
        let s = subject(&access, "key:abc");
        let r = access
            .decide(&s, &question(Action::RunApply, Some("environment:production/unit:relay")), CachePolicy::Fresh)
            .await;
        assert_eq!(r, Err(Refusal::ApprovalCannotWait), "{reason}");
    }
}

#[tokio::test]
async fn p3_18_a_ceiling_that_the_action_does_not_list_gives_403_and_an_event() {
    // `max_hosts` on `config.read`, and a ceiling that the document does not declare.
    for (action, name) in [(Action::ConfigRead, "max_hosts"), (Action::RunApply, "max_rows")] {
        let (_pdp, access) = start(move |_| Answer::yes().with_limit(name, 5));
        let s = subject(&access, "alice");
        let r = access.decide(&s, &question(action, Some(HOST)), CachePolicy::Fresh).await;
        assert_eq!(r, Err(Refusal::Forbidden { action: action.name() }));
        let events = access.take_events();
        assert!(events.iter().any(|e| matches!(e, AccessEvent::CeilingUnenforceable { .. })), "{events:?}");
    }
}

#[tokio::test]
async fn p3_18_a_ceiling_below_its_minimum_gives_403() {
    let (_pdp, access) = start(|_| Answer::yes().with_limit("max_hosts", 0));
    let s = subject(&access, "alice");
    let r = access
        .decide(&s, &question(Action::RunApply, Some("environment:production/unit:relay")), CachePolicy::Fresh)
        .await;
    assert_eq!(r, Err(Refusal::Forbidden { action: "run.apply" }));
    assert!(access.take_events().iter().any(|e| matches!(e, AccessEvent::CeilingUnenforceable { .. })));
}

#[tokio::test]
async fn p3_18_a_ceiling_that_is_not_an_integer_gives_503() {
    for value in [json!("5"), json!(1.5), json!(-1), json!(null)] {
        let v = value.clone();
        let (_pdp, access) =
            start(move |_| Answer::raw(json!({"decision": true, "context": {"limits": {"max_hosts": v}}})));
        let s = subject(&access, "alice");
        let r = access
            .decide(&s, &question(Action::RunApply, Some("environment:production/unit:relay")), CachePolicy::Fresh)
            .await;
        assert_eq!(r, Err(Refusal::PdpUnavailable), "{value}");
    }
}

#[tokio::test]
async fn p3_18_a_pdp_that_fails_gives_503_and_a_log_event() {
    let (pdp, access) = start(|_| Answer::yes());
    let s = subject(&access, "alice");
    let q = question(Action::ConfigRead, Some(HOST));
    // A status other than 200.
    pdp.set_mode(Mode::Status(500));
    assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
    assert!(access.take_events().iter().any(|e| matches!(e, AccessEvent::PdpFailed { .. })));
    // A body that is not an answer.
    pdp.set_mode(Mode::Garbage);
    assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
    let events = access.take_events();
    assert!(events.iter().any(|e| matches!(e, AccessEvent::PdpInvalidAnswer { .. })), "{events:?}");
    // A body of the right form with a decision that is not a boolean.
    pdp.set_mode(Mode::Normal);
    pdp.set_policy(|_| Answer::raw(json!({"decision": "yes"})));
    assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
    // The PDP works again: no state of the failure stays.
    pdp.set_policy(|_| Answer::yes());
    assert!(access.decide(&s, &q, CachePolicy::Fresh).await.is_ok());
    assert!(access.counters().error >= 3);
}

#[tokio::test]
async fn p3_18_no_connection_and_a_timeout_give_503() {
    // No connection: a port that nobody listens on.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let access = AuthzenAccess::new(&config(&format!("http://{dead}")), true, 100).unwrap();
    let s = subject(&access, "alice");
    let q = question(Action::ConfigRead, None);
    assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
    // A timeout: the PDP reads the request and never answers.
    let (pdp, access) = {
        let pdp = FakePdp::start(|_| Answer::yes());
        let mut c = config(&pdp.url());
        c.pdp_timeout = Duration::from_millis(300);
        let access = AuthzenAccess::new(&c, true, 100).unwrap();
        (pdp, access)
    };
    let s = subject(&access, "alice");
    pdp.set_mode(Mode::Hang);
    let started = std::time::Instant::now();
    assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn p3_18_a_failure_never_gives_a_kept_answer_or_keeps_one() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let mut c = config(&pdp.url());
    c.cache_ttl = Duration::from_secs(60);
    let access = AuthzenAccess::new(&c, true, 100).unwrap();
    let s = subject(&access, "alice");
    let q = question(Action::ConfigRead, Some(HOST));
    pdp.set_mode(Mode::Status(503));
    assert_eq!(access.decide(&s, &q, CachePolicy::MayUseKept).await, Err(Refusal::PdpUnavailable));
    pdp.set_mode(Mode::Normal);
    pdp.clear();
    assert!(access.decide(&s, &q, CachePolicy::MayUseKept).await.is_ok());
    assert_eq!(pdp.questions().len(), 1, "the failure left no kept answer");
}

#[tokio::test]
async fn p3_18_the_credential_goes_in_authorization_bearer_and_the_subject_is_only_the_sub() {
    let (pdp, access) = start(|_| Answer::yes());
    let s = subject(&access, "key:9b1c04e2aa71f0c3");
    access.decide(&s, &question(Action::ConfigRead, Some(HOST)), CachePolicy::Fresh).await.unwrap();
    let requests = pdp.evaluation_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].authorization.as_deref(), Some(&*format!("Bearer {CREDENTIAL}")));
    let body = &requests[0].body;
    assert_eq!(body["subject"], json!({"type": "user", "id": "key:9b1c04e2aa71f0c3"}));
    assert_eq!(body["action"], json!({"name": "config.read"}));
    // The chain comes from the node, from the service to the parent (3.8).
    assert_eq!(body["resource"]["type"], "host");
    assert_eq!(body["resource"]["id"], "host-a");
    assert_eq!(
        body["resource"]["properties"]["ancestors"],
        json!([
            {"type": "service", "id": "pupitre"},
            {"type": "environment", "id": "production"},
            {"type": "unit", "id": "relay"}
        ])
    );
    assert!(body["context"].get("usage").is_none());
    assert_eq!(pdp.questions()[0].resource, format!("service:pupitre/{HOST}"));
}

#[tokio::test]
async fn p3_18_the_service_node_has_no_ancestors_and_the_match_subtree_never_runs_an_operation() {
    let (pdp, access) = start(|_| Answer::yes());
    let s = subject(&access, "alice");
    access.decide(&s, &question(Action::InventoryWrite, None), CachePolicy::Fresh).await.unwrap();
    let body = &pdp.evaluation_requests()[0].body;
    assert_eq!(body["resource"]["type"], "service");
    assert_eq!(body["resource"]["id"], "pupitre");
    let none = body["resource"].get("properties").and_then(|p| p.get("ancestors"));
    assert!(none.is_none() || none == Some(&json!([])), "{body}");
    // A question with the match `subtree` is for the display only (P 4.4).
    pdp.clear();
    let q = Question { action: Action::ConfigRead, node: Node::Service, match_mode: MatchMode::Subtree };
    assert_eq!(access.decide(&s, &q, CachePolicy::Fresh).await, Err(Refusal::Forbidden { action: "config.read" }));
    assert!(pdp.evaluation_requests().is_empty());
}

#[tokio::test]
async fn p3_3_an_action_that_is_not_published_is_refused_with_no_question() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), false, 100).unwrap();
    let s = subject(&access, "alice");
    let r = access
        .decide(&s, &question(Action::RunApply, Some("environment:production/unit:relay")), CachePolicy::Fresh)
        .await;
    assert_eq!(r, Err(Refusal::Forbidden { action: "run.apply" }));
    assert!(pdp.evaluation_requests().is_empty());
    assert!(access.decide(&s, &question(Action::ConfigRead, None), CachePolicy::Fresh).await.is_ok());
}

// ---- 3.17: the cache policy ----------------------------------------------------------------

fn with_ttl(pdp: &FakePdp, ttl: u64, entries: u32) -> AuthzenAccess {
    let mut c = config(&pdp.url());
    c.cache_ttl = Duration::from_secs(ttl);
    c.cache_entries = entries;
    AuthzenAccess::new(&c, true, 100).unwrap()
}

#[tokio::test]
async fn p3_17_a_read_route_can_use_a_kept_answer() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = with_ttl(&pdp, 60, 100);
    let s = subject(&access, "alice");
    let q = question(Action::ConfigRead, Some(HOST));
    access.decide(&s, &q, CachePolicy::MayUseKept).await.unwrap();
    access.take_events();
    access.decide(&s, &q, CachePolicy::MayUseKept).await.unwrap();
    assert_eq!(pdp.questions().len(), 1);
    // The second question is a kept answer: the event says it.
    let events = access.take_events();
    assert!(
        events.iter().any(|e| matches!(e, AccessEvent::Decision { cached: true, decision: true, .. })),
        "{events:?}"
    );
    assert_eq!(access.counters().cache_hits, 1);
}

#[tokio::test]
async fn p3_17_a_write_route_never_uses_a_kept_answer_and_keeps_none() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = with_ttl(&pdp, 60, 100);
    let s = subject(&access, "alice");
    let q = question(Action::ConfigWrite, Some(HOST));
    // Two fresh questions: two requests to the PDP.
    access.decide(&s, &q, CachePolicy::Fresh).await.unwrap();
    access.decide(&s, &q, CachePolicy::Fresh).await.unwrap();
    assert_eq!(pdp.questions().len(), 2);
    // A fresh question keeps nothing: the next read question asks the PDP again.
    access.decide(&s, &q, CachePolicy::MayUseKept).await.unwrap();
    assert_eq!(pdp.questions().len(), 3);
    // A kept answer exists now, and a fresh question still asks the PDP.
    access.decide(&s, &q, CachePolicy::MayUseKept).await.unwrap();
    assert_eq!(pdp.questions().len(), 3);
    access.decide(&s, &q, CachePolicy::Fresh).await.unwrap();
    assert_eq!(pdp.questions().len(), 4);
}

#[tokio::test]
async fn p3_17_a_revoked_grant_stops_a_write_at_once_and_a_read_after_the_ttl() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let clock = ManualClock::at_seconds(now());
    let mut c = config(&pdp.url());
    c.cache_ttl = Duration::from_secs(30);
    let link = PdpLink::new(true, None, c.pdp_timeout, 4, 1 << 20, Duration::from_secs(5)).unwrap();
    let access = AuthzenAccess::with_transport(&c, true, 100, Arc::new(link), Arc::new(clock.clone())).unwrap();
    let s = subject(&access, "alice");
    let read = question(Action::ConfigRead, Some(HOST));
    let write = question(Action::ConfigWrite, Some(HOST));
    assert!(access.decide(&s, &read, CachePolicy::MayUseKept).await.is_ok());
    assert!(access.decide(&s, &write, CachePolicy::Fresh).await.is_ok());
    pdp.set_policy(|_| Answer::no());
    // The write stops at once. The read still has its kept answer.
    assert_eq!(access.decide(&s, &write, CachePolicy::Fresh).await, Err(Refusal::Forbidden { action: "config.write" }));
    assert!(access.decide(&s, &read, CachePolicy::MayUseKept).await.is_ok());
    // After the time to live, the read stops too.
    clock.advance_millis(31_000);
    assert_eq!(
        access.decide(&s, &read, CachePolicy::MayUseKept).await,
        Err(Refusal::Forbidden { action: "config.read" })
    );
}

#[tokio::test]
async fn p3_17_an_answer_with_a_reason_of_the_approvals_is_never_kept() {
    // P 10.1: a PEP never keeps an answer with a reason of P 17.1. The library at the pinned commit
    // keeps it, so the layer drops it.
    let pdp = FakePdp::start(|_| Answer::no_because("approval_required"));
    let access = with_ttl(&pdp, 60, 100);
    let s = subject(&access, "alice");
    let q = question(Action::ConfigRead, Some(HOST));
    assert_eq!(access.decide(&s, &q, CachePolicy::MayUseKept).await, Err(Refusal::ApprovalCannotWait));
    // The owner removes the `confirm`: the next question sees it at once.
    pdp.set_policy(|_| Answer::yes());
    assert!(access.decide(&s, &q, CachePolicy::MayUseKept).await.is_ok());
    assert_eq!(pdp.questions().len(), 2);
    // A plain answer is still kept.
    assert!(access.decide(&s, &q, CachePolicy::MayUseKept).await.is_ok());
    assert_eq!(pdp.questions().len(), 2);
}

#[tokio::test]
async fn p3_17_cache_ttl_zero_keeps_no_answer() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = with_ttl(&pdp, 0, 100);
    let s = subject(&access, "alice");
    let q = question(Action::ConfigRead, Some(HOST));
    access.decide(&s, &q, CachePolicy::MayUseKept).await.unwrap();
    access.decide(&s, &q, CachePolicy::MayUseKept).await.unwrap();
    assert_eq!(pdp.questions().len(), 2);
}

#[tokio::test]
async fn p3_17_a_full_cache_keeps_no_new_answer() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = with_ttl(&pdp, 60, 1);
    let s = subject(&access, "alice");
    let a = question(Action::ConfigRead, Some("environment:a"));
    let b = question(Action::ConfigRead, Some("environment:b"));
    access.decide(&s, &a, CachePolicy::MayUseKept).await.unwrap();
    access.decide(&s, &b, CachePolicy::MayUseKept).await.unwrap();
    access.decide(&s, &b, CachePolicy::MayUseKept).await.unwrap();
    // `b` was never kept: 3 questions. `a` is still kept: no 4th question.
    assert_eq!(pdp.questions().len(), 3);
    access.decide(&s, &a, CachePolicy::MayUseKept).await.unwrap();
    assert_eq!(pdp.questions().len(), 3);
}

#[tokio::test]
async fn p3_17_the_key_has_the_subject_the_action_the_resource_and_the_match() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = with_ttl(&pdp, 60, 100);
    let alice = subject(&access, "alice");
    let bob = subject(&access, "bob");
    let q = question(Action::ConfigRead, Some(HOST));
    access.decide(&alice, &q, CachePolicy::MayUseKept).await.unwrap();
    // Another subject, another action, another node: each one is a new question.
    access.decide(&bob, &q, CachePolicy::MayUseKept).await.unwrap();
    access.decide(&alice, &question(Action::ConfigWrite, Some(HOST)), CachePolicy::MayUseKept).await.unwrap();
    access
        .decide(
            &alice,
            &question(Action::ConfigRead, Some("environment:production/unit:relay/host:host-b")),
            CachePolicy::MayUseKept,
        )
        .await
        .unwrap();
    assert_eq!(pdp.questions().len(), 4);
    // The display question has the match `subtree`: it does not use the kept `node` answer.
    let items = vec![(node(HOST), vec![Action::ConfigRead], MatchMode::Subtree)];
    access.visible(&alice, &items, 10).await.unwrap();
    assert_eq!(pdp.questions().len(), 5);
    assert_eq!(pdp.questions()[4].match_mode, "subtree");
}

// ---- 3.14: unknown and forbidden -----------------------------------------------------------

#[tokio::test]
async fn p3_14_unknown_and_forbidden_give_the_same_403() {
    let (pdp, access) = start(|_| Answer::no());
    let s = subject(&access, "alice");
    let forbidden = access.decide(&s, &question(Action::ConfigRead, Some(HOST)), CachePolicy::Fresh).await.unwrap_err();
    let unknown = access.decide_unknown(&s, Action::ConfigRead).await;
    assert_eq!(forbidden, unknown);
    assert_eq!(forbidden, Refusal::Forbidden { action: "config.read" });
    // The bytes that the server renders come from the value and its text.
    assert_eq!(forbidden.to_string(), unknown.to_string());
    // The two questions: the use action on the node, and the use action on the service.
    let q = pdp.questions();
    assert_eq!(q.len(), 2);
    assert_eq!(q[0].action, "config.read");
    assert_eq!(q[1].action, "config.read");
    assert_eq!(q[0].match_mode, "node");
    assert_eq!(q[1].match_mode, "node");
    assert_eq!(q[1].resource, "service:pupitre");
}

#[tokio::test]
async fn p3_14_a_true_answer_gives_not_found_and_a_failure_gives_503() {
    let (pdp, access) = start(|_| Answer::yes());
    let s = subject(&access, "alice");
    assert_eq!(access.decide_unknown(&s, Action::ConfigWrite).await, Refusal::NotFound);
    pdp.set_mode(Mode::Status(500));
    assert_eq!(access.decide_unknown(&s, Action::ConfigWrite).await, Refusal::PdpUnavailable);
    // A reason of the approvals gives the refusal of 3.19 for a node too: the same function.
    pdp.set_mode(Mode::Normal);
    pdp.set_policy(|_| Answer::no_because("approval_required"));
    assert_eq!(access.decide_unknown(&s, Action::RunApply).await, Refusal::ApprovalCannotWait);
}

#[tokio::test]
async fn p3_14_the_question_of_an_unknown_item_never_uses_or_keeps_a_kept_answer() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = with_ttl(&pdp, 60, 100);
    let s = subject(&access, "alice");
    // Two questions of 3.14: two requests to the PDP.
    access.decide_unknown(&s, Action::ConfigRead).await;
    access.decide_unknown(&s, Action::ConfigRead).await;
    assert_eq!(pdp.questions().len(), 2);
    // It kept nothing: a read question on the service asks the PDP.
    access.decide(&s, &question(Action::ConfigRead, None), CachePolicy::MayUseKept).await.unwrap();
    assert_eq!(pdp.questions().len(), 3);
    // And a kept answer of a read question does not serve it.
    access.decide_unknown(&s, Action::ConfigRead).await;
    assert_eq!(pdp.questions().len(), 4);
}

// ---- 3.20: the question `access` -----------------------------------------------------------

#[tokio::test]
async fn p3_20_the_question_access_is_asked_on_the_service() {
    let (pdp, access) = start(|q| if q.action == "access" { Answer::yes() } else { Answer::no() });
    let s = subject(&access, "alice");
    access.decide_access(&s).await.unwrap();
    let q = pdp.questions();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].action, "access");
    assert_eq!(q[0].resource, "service:pupitre");
    assert!(!q[0].usage);
    assert_eq!(q[0].subject, "alice");
    assert_eq!(pdp.evaluation_requests()[0].body["subject"]["type"], "user");
}

#[tokio::test]
async fn p3_20_a_false_answer_gives_403_and_a_failure_gives_503() {
    let (pdp, access) = start(|_| Answer::no());
    let s = subject(&access, "alice");
    assert_eq!(access.decide_access(&s).await, Err(Refusal::Forbidden { action: "access" }));
    pdp.set_mode(Mode::Garbage);
    assert_eq!(access.decide_access(&s).await, Err(Refusal::PdpUnavailable));
    pdp.set_mode(Mode::Status(500));
    assert_eq!(access.decide_access(&s).await, Err(Refusal::PdpUnavailable));
}

#[tokio::test]
async fn p3_20_a_reason_of_the_approvals_for_access_is_an_answer_that_is_not_valid() {
    let (_pdp, access) = start(|_| Answer::no_because("approval_required"));
    let s = subject(&access, "alice");
    assert_eq!(access.decide_access(&s).await, Err(Refusal::PdpUnavailable));
    assert!(access.take_events().iter().any(|e| matches!(e, AccessEvent::PdpInvalidAnswer { .. })));
}

#[tokio::test]
async fn p3_20_a_true_answer_can_be_kept_but_a_false_answer_is_not() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = with_ttl(&pdp, 60, 100);
    let s = subject(&access, "alice");
    access.decide_access(&s).await.unwrap();
    access.decide_access(&s).await.unwrap();
    assert_eq!(pdp.questions().len(), 1);
    // Another subject is another question.
    access.decide_access(&subject(&access, "bob")).await.unwrap();
    assert_eq!(pdp.questions().len(), 2);
    // A false answer is never kept: a later `true` shows at once.
    pdp.set_policy(|_| Answer::no());
    let carol = subject(&access, "carol");
    assert!(access.decide_access(&carol).await.is_err());
    pdp.set_policy(|_| Answer::yes());
    assert!(access.decide_access(&carol).await.is_ok());
}

// ---- 3.15: the display filter --------------------------------------------------------------

fn envs(names: &[&str]) -> Vec<(Node, Vec<Action>, MatchMode)> {
    names
        .iter()
        .map(|n| {
            (
                Node::Environment(pupitre_core::ids::EnvId::parse(n).unwrap()),
                vec![Action::ConfigRead, Action::RunRead],
                MatchMode::Subtree,
            )
        })
        .collect()
}

#[tokio::test]
async fn p3_15_the_display_asks_with_subtree_and_stops_at_the_first_true_answer() {
    let (pdp, access) = start(|q| {
        let sees_b_by_the_second_action = q.resource.ends_with("environment:b") && q.action == "run.read";
        if sees_b_by_the_second_action || q.resource.ends_with("environment:a") { Answer::yes() } else { Answer::no() }
    });
    let s = subject(&access, "alice");
    let v = access.visible(&s, &envs(&["a", "b", "c"]), 50).await.unwrap();
    // `a` is seen with the first action, `b` with the second one, `c` with none.
    assert_eq!(v.visible, vec![0, 1]);
    assert_eq!(v.stopped_at, None);
    let q = pdp.questions();
    assert!(q.iter().all(|q| q.match_mode == "subtree"));
    let asked: Vec<(String, String)> = q.iter().map(|q| (q.action.clone(), q.resource.clone())).collect();
    // No second action for `a` after its `true` answer.
    assert!(!asked.contains(&("run.read".to_owned(), "service:pupitre/environment:a".to_owned())));
    assert!(asked.contains(&("run.read".to_owned(), "service:pupitre/environment:b".to_owned())));
    assert!(asked.contains(&("run.read".to_owned(), "service:pupitre/environment:c".to_owned())));
    assert_eq!(q.len(), 5);
}

#[tokio::test]
async fn p3_15_the_budget_stops_the_walk_and_gives_the_place() {
    let (pdp, access) = start(|_| Answer::yes());
    let s = subject(&access, "alice");
    // Each item is seen with its first action: 1 question each. The budget is 4.
    let names = ["a", "b", "c", "d", "e", "f"];
    let v = access.visible(&s, &envs(&names), 4).await.unwrap();
    assert_eq!(v.visible, vec![0, 1, 2, 3]);
    assert_eq!(v.stopped_at, Some(4));
    assert_eq!(pdp.questions().len(), 4);
}

#[tokio::test]
async fn p3_15_an_item_that_the_budget_cannot_end_is_not_listed_and_gives_the_place() {
    // Nothing is seen: each item needs 2 questions. The budget is 3: item 0 costs 2, item 1 is cut.
    let (pdp, access) = start(|_| Answer::no());
    let s = subject(&access, "alice");
    let v = access.visible(&s, &envs(&["a", "b", "c"]), 3).await.unwrap();
    assert!(v.visible.is_empty());
    assert_eq!(v.stopped_at, Some(1));
    assert_eq!(pdp.questions().len(), 3);
}

#[tokio::test]
async fn p3_15_a_failure_of_the_pdp_gives_an_error_never_a_shorter_list() {
    let (pdp, access) = start(|_| Answer::yes());
    let s = subject(&access, "alice");
    pdp.set_mode(Mode::Status(500));
    assert_eq!(access.visible(&s, &envs(&["a", "b"]), 50).await, Err(Refusal::PdpUnavailable));
    pdp.set_mode(Mode::Garbage);
    assert_eq!(access.visible(&s, &envs(&["a", "b"]), 50).await, Err(Refusal::PdpUnavailable));
}

#[tokio::test]
async fn p3_15_a_list_of_runs_asks_the_match_node() {
    let (pdp, access) = start(|q| if q.resource.ends_with("unit:relay") { Answer::yes() } else { Answer::no() });
    let s = subject(&access, "alice");
    let items = vec![
        (node("environment:production/unit:relay"), vec![Action::RunRead], MatchMode::Node),
        (node("environment:production/unit:other"), vec![Action::RunRead], MatchMode::Node),
    ];
    let v = access.visible(&s, &items, 10).await.unwrap();
    assert_eq!(v.visible, vec![0]);
    assert!(pdp.questions().iter().all(|q| q.match_mode == "node" && q.action == "run.read"));
}

#[tokio::test]
async fn p3_15_an_action_that_is_not_published_is_never_asked() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), false, 100).unwrap();
    let s = subject(&access, "alice");
    let items = vec![(node("environment:a"), vec![Action::RunRead, Action::RunApply], MatchMode::Subtree)];
    let v = access.visible(&s, &items, 10).await.unwrap();
    assert!(v.visible.is_empty());
    assert!(pdp.questions().is_empty());
}

#[tokio::test]
async fn p3_15_the_options_budget_must_cover_one_item() {
    // With the runner on, the type `environment` lists 8 actions: a budget below 8 stops the start.
    let pdp = FakePdp::start(|_| Answer::yes());
    let mut c = config(&pdp.url());
    c.options_max_questions = 7;
    let error = AuthzenAccess::new(&c, true, 100).unwrap_err();
    assert!(error.starts_with("access.options_max_questions: expected 8 or more"), "{error}");
    c.options_max_questions = 8;
    assert!(AuthzenAccess::new(&c, true, 100).is_ok());
    // With the runner off, the most is 4.
    c.options_max_questions = 4;
    assert!(AuthzenAccess::new(&c, false, 100).is_ok());
    c.options_max_questions = 3;
    assert!(AuthzenAccess::new(&c, false, 100).is_err());
}

// ---- 3.5: the pool -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3_5_a_task_that_waits_longer_than_the_timeout_for_a_place_fails_with_503() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let mut c = config(&pdp.url());
    c.pdp_max_connections = 1;
    c.pdp_timeout = Duration::from_millis(300);
    let access = Arc::new(AuthzenAccess::new(&c, true, 100).unwrap());
    let s = subject(&access, "alice");
    // Warm up: the first question reads the metadata.
    access.decide(&s, &question(Action::ConfigRead, None), CachePolicy::Fresh).await.unwrap();
    pdp.set_delay(Duration::from_millis(250));
    let mut tasks = Vec::new();
    for i in 0..3 {
        let (access, s) = (access.clone(), s.clone());
        tasks.push(tokio::spawn(async move {
            access
                .decide(&s, &question(Action::ConfigRead, Some(&format!("environment:e{i}"))), CachePolicy::Fresh)
                .await
        }));
    }
    let mut ok = 0;
    let mut unavailable = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(_) => ok += 1,
            Err(Refusal::PdpUnavailable) => unavailable += 1,
            Err(other) => panic!("{other:?}"),
        }
    }
    assert!(ok >= 1 && unavailable >= 1, "ok {ok}, unavailable {unavailable}");
    assert!(
        access
            .take_events()
            .iter()
            .any(|e| matches!(e, AccessEvent::PdpFailed { error, .. } if error.contains("pool")))
    );
}

#[tokio::test]
async fn p3_5_the_link_keeps_its_connection_between_questions() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    let s = subject(&access, "alice");
    for i in 0..5 {
        access
            .decide(&s, &question(Action::ConfigRead, Some(&format!("environment:e{i}"))), CachePolicy::Fresh)
            .await
            .unwrap();
    }
    assert_eq!(pdp.connections(), 1);
}

// ---- 3.11: the action document -------------------------------------------------------------

#[test]
fn p3_11_the_served_bytes_are_the_document_of_the_specification() {
    let pdp = FakePdp::start(|_| Answer::yes());
    for (enabled, max) in [(true, 100u64), (true, 1), (false, 100)] {
        let access = AuthzenAccess::new(&config(&pdp.url()), enabled, max).unwrap();
        let expected = pupitre_core::json::to_compact(&pupitre::access::action_document(AUDIENCE, enabled, max));
        let served = access.action_document().unwrap();
        assert_eq!(String::from_utf8_lossy(served), String::from_utf8_lossy(&expected));
        // The same bytes at each read, below 8 KiB.
        assert_eq!(access.action_document().unwrap(), served);
        assert!(served.len() < 8192);
    }
    assert!(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess).action_document().is_none());
}

#[test]
fn p3_11_the_example_of_the_specification_is_the_document() {
    // The example of 3.11: the document for `pupitre`, the runner on and 100 hosts.
    let spec = include_str!("../../../docs/03-access.md");
    let from = spec.find("The document for `audience = \"pupitre\"`").unwrap();
    let block = &spec[from..];
    let start = block.find("```json\n").unwrap() + "```json\n".len();
    let end = start + block[start..].find("```").unwrap();
    let example: serde_json::Value = serde_json::from_str(&block[start..end]).unwrap();
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    let served: serde_json::Value = serde_json::from_slice(access.action_document().unwrap()).unwrap();
    assert_eq!(served, example);
}

#[test]
fn p3_11_the_document_has_no_member_approval_and_no_resources() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    let text = String::from_utf8(access.action_document().unwrap().to_vec()).unwrap();
    assert!(!text.contains("\"approval\""));
    assert!(!text.contains("\"resources\""));
    assert!(!text.contains("\"access\""));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["server"], "pupitre");
    assert_eq!(v["limits"][0]["max"], 100);
}

#[test]
fn p3_11_the_service_id_of_the_document_is_the_audience() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let mut c = config(&pdp.url());
    c.audience = "pupitre-two".to_owned();
    let access = AuthzenAccess::new(&c, true, 100).unwrap();
    let v: serde_json::Value = serde_json::from_slice(access.action_document().unwrap()).unwrap();
    assert_eq!(v["server"], "pupitre-two");
}

// ---- The start ------------------------------------------------------------------------------

#[test]
fn the_start_sends_nothing_to_the_pdp() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let _access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    assert!(pdp.http_requests().is_empty());
    assert_eq!(pdp.connections(), 0);
}

#[tokio::test]
async fn the_first_question_reads_the_metadata_of_the_pdp() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    let s = subject(&access, "alice");
    access.decide(&s, &question(Action::ConfigRead, None), CachePolicy::Fresh).await.unwrap();
    let paths: Vec<String> = pdp.http_requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths[0], "/.well-known/authzen-configuration");
    assert_eq!(paths[1], "/access/v1/evaluation");
}

#[test]
fn a_link_with_http_needs_the_explicit_flag() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let mut c = config(&pdp.url());
    c.plain_http = false;
    c.pdp_ca_pem = None;
    assert!(AuthzenAccess::new(&c, true, 100).is_err());
}
