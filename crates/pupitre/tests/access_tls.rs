// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the link to the PDP with TLS (3.5): the trust roots, the refusal of a wrong CA, the
//! flag `plain_http`, and the pool of connections.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod access_support;

use access_support::*;
use pupitre::access::{Access, AccessEvent, AuthzenAccess, CachePolicy, Question, Refusal};
use pupitre_core::action::Action;
use pupitre_core::ids::Subject;
use pupitre_core::node::Node;

fn question() -> Question {
    Question { action: Action::ConfigRead, node: Node::Service, match_mode: pupitre::access::MatchMode::Node }
}

fn subject(access: &AuthzenAccess) -> Subject {
    let t = token(&ed_key(), Some("ed-1"), "alice");
    access.authenticate(&[t.as_bytes()]).unwrap()
}

#[tokio::test]
async fn p3_5_a_link_with_tls_and_the_right_ca_works_and_keeps_its_connection() {
    let pdp = FakePdp::start_tls(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    let s = subject(&access);
    for _ in 0..4 {
        assert!(access.decide(&s, &question(), CachePolicy::Fresh).await.is_ok());
    }
    assert_eq!(pdp.connections(), 1);
    // The credential goes only over the secured link, in `Authorization: Bearer`.
    let requests = pdp.evaluation_requests();
    assert_eq!(requests[0].authorization.as_deref(), Some(&*format!("Bearer {CREDENTIAL}")));
}

#[tokio::test]
async fn p3_5_the_address_form_of_the_url_works_when_the_certificate_has_the_address() {
    let pdp = FakePdp::start_tls(|_| Answer::yes());
    let url = format!("https://127.0.0.1:{}", pdp.addr.port());
    let access = AuthzenAccess::new(&config(&url), true, 100).unwrap();
    assert!(access.decide(&subject(&access), &question(), CachePolicy::Fresh).await.is_ok());
}

#[tokio::test]
async fn p3_5_a_server_that_another_ca_signed_is_refused() {
    let pdp = FakePdp::start_tls(|_| Answer::yes());
    let mut c = config(&pdp.url());
    c.pdp_ca_pem = Some(CA_B.as_bytes().to_vec());
    let access = AuthzenAccess::new(&c, true, 100).unwrap();
    let s = subject(&access);
    assert_eq!(access.decide(&s, &question(), CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
    let events = access.take_events();
    assert!(events.iter().any(|e| matches!(e, AccessEvent::PdpFailed { .. })), "{events:?}");
    // The PDP never saw a request: the handshake failed.
    assert!(pdp.http_requests().is_empty());
}

#[tokio::test]
async fn p3_5_a_tls_link_to_a_server_with_no_tls_fails_closed() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let mut c = config(&format!("https://127.0.0.1:{}", pdp.addr.port()));
    c.pdp_timeout = std::time::Duration::from_millis(500);
    let access = AuthzenAccess::new(&c, true, 100).unwrap();
    let s = subject(&access);
    assert_eq!(access.decide(&s, &question(), CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
    assert!(pdp.evaluation_requests().is_empty());
}

#[tokio::test]
async fn p3_5_a_redirect_is_never_followed() {
    let pdp = FakePdp::start_tls(|_| Answer::yes());
    let access = AuthzenAccess::new(&config(&pdp.url()), true, 100).unwrap();
    let s = subject(&access);
    pdp.set_mode(Mode::Status(302));
    assert_eq!(access.decide(&s, &question(), CachePolicy::Fresh).await, Err(Refusal::PdpUnavailable));
}

#[test]
fn p3_5_the_start_refuses_a_link_that_cannot_be_secured() {
    let pdp = FakePdp::start_tls(|_| Answer::yes());
    // An `https` URL needs the CA certificates.
    let mut c = config(&pdp.url());
    c.pdp_ca_pem = None;
    let e = AuthzenAccess::new(&c, true, 100).unwrap_err();
    assert!(e.contains("access.pdp.ca_file"), "{e}");
    // A CA text with no certificate.
    c.pdp_ca_pem = Some(b"no certificate here".to_vec());
    assert!(AuthzenAccess::new(&c, true, 100).is_err());
    // An `http` URL needs `plain_http = true`.
    let plain = FakePdp::start(|_| Answer::yes());
    let mut c = config(&plain.url());
    c.plain_http = false;
    assert!(AuthzenAccess::new(&c, true, 100).is_err());
    // The flag with an `https` URL: the link has no TLS, so the scheme is not usable.
    let mut c = config(&pdp.url());
    c.plain_http = true;
    assert!(AuthzenAccess::new(&c, true, 100).is_err());
}

#[test]
fn the_start_refuses_values_that_the_library_refuses_and_names_the_key_of_the_file() {
    let pdp = FakePdp::start(|_| Answer::yes());
    let mut c = config(&pdp.url());
    c.pdp_token = pupitre::config::Secret::new("has a space".to_owned());
    let e = AuthzenAccess::new(&c, true, 100).unwrap_err();
    assert!(e.contains("access.pdp.token"), "{e}");
    assert!(!e.contains("has a space"));
    let mut c = config(&pdp.url());
    c.header = "Authorization".to_owned();
    assert!(AuthzenAccess::new(&c, true, 100).unwrap_err().contains("access.identity.header"));
    let mut c = config(&pdp.url());
    c.cache_ttl = std::time::Duration::from_secs(5);
    c.cache_entries = 0;
    assert!(AuthzenAccess::new(&c, true, 100).unwrap_err().contains("access.pdp.cache_entries"));
}
