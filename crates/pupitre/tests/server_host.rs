// SPDX-License-Identifier: MIT OR Apache-2.0
//! The check of the header `Host` against `server.allowed_hosts` (7.4): a request for a name that
//! the configuration does not list gets `421 misdirected_request`, before each route.

mod server_common;

use pupitre::config::hosts::HostPattern;
use server_common::*;

const BODY: &str = r#"{"id":"staging","document":{"format":1,"label":"S"}}"#;

#[tokio::test]
async fn a_rebound_name_gets_421_also_with_a_matching_origin_7_4() {
    let f = open_fixture().await;
    // The attack of the review: the name is the attacker's, and `Origin` and `Sec-Fetch-Site`
    // agree with it.
    let post = Req::post("/api/environments")
        .header("Origin", "http://attacker.example")
        .header("Sec-Fetch-Site", "same-origin")
        .json(BODY);
    let r = send_with_host(f.addr, "attacker.example", post).await;
    assert_eq!((r.status, r.error().as_str()), (421, "misdirected_request"), "{}", r.text());
    assert!(f.store.document(&pupitre_core::doc::DocKey::parse("environments/staging").unwrap()).is_none());
    // A read has the same answer: a page that a rebound name serves reads the answers.
    for path in
        ["/api/session", "/", "/static/x/js/app.js", "/e/production", "/nothing", "/.well-known/procurauth/actions"]
    {
        let r = send_with_host(f.addr, "attacker.example:8080", Req::get(path)).await;
        assert_eq!((r.status, r.error().as_str()), (421, "misdirected_request"), "{path}");
        assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(r.header("cache-control"), Some("no-store"));
    }
    // The same request with a name of the list works.
    for host in ["localhost", "LOCALHOST:9999", "127.0.0.1:1", "[::1]:8080"] {
        let r = send_with_host(f.addr, host, Req::get("/api/session")).await;
        assert_eq!(r.status, 200, "{host}");
    }
    f.stop().await;
}

#[tokio::test]
async fn a_name_that_only_looks_like_a_listed_name_gets_421() {
    let f = open_fixture().await;
    for host in [
        "localhost.attacker.example",
        "attacker.example.localhost.attacker.example",
        "127.0.0.1.attacker.example",
        "localhost@attacker.example",
        "localhost:80:80",
        "localhost:x",
        "",
        " ",
        "[::2]",
        "127.0.0.2",
    ] {
        let r = send_with_host(f.addr, host, Req::get("/api/session")).await;
        assert_eq!((r.status, r.error().as_str()), (421, "misdirected_request"), "{host:?}");
    }
    f.stop().await;
}

#[tokio::test]
async fn no_host_two_hosts_or_a_foreign_authority_get_421() {
    let f = open_fixture().await;
    let one = raw(f.addr, b"GET /api/session HTTP/1.0\r\n\r\n").await;
    assert_eq!((one.status, one.error().as_str()), (421, "misdirected_request"));
    let two = raw(
        f.addr,
        b"GET /api/session HTTP/1.1\r\nHost: localhost\r\nHost: attacker.example\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!((two.status, two.error().as_str()), (421, "misdirected_request"));
    let absolute = raw(
        f.addr,
        b"GET http://attacker.example/api/session HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!((absolute.status, absolute.error().as_str()), (421, "misdirected_request"));
    let own =
        raw(f.addr, b"GET http://localhost/api/session HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await;
    assert_eq!(own.status, 200);
    f.stop().await;
}

#[tokio::test]
async fn a_body_with_a_foreign_host_closes_the_connection() {
    let f = open_fixture().await;
    let r = send_with_host(f.addr, "attacker.example", Req::post("/api/environments").json(BODY)).await;
    assert_eq!(r.status, 421);
    assert_eq!(r.header("connection"), Some("close"));
    f.stop().await;
}

#[tokio::test]
async fn the_configured_list_decides_with_names_and_ports_7_4() {
    let store = std::sync::Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    s.allowed_hosts = ["pupitre.example.invalid", "edge.example.invalid:8443"]
        .iter()
        .map(|t| HostPattern::parse(t).unwrap())
        .collect();
    let f = start(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess), s, store).await;
    for (host, status) in [
        ("pupitre.example.invalid", 200),
        ("Pupitre.Example.Invalid:8080", 200),
        ("edge.example.invalid:8443", 200),
        ("edge.example.invalid", 421),
        ("edge.example.invalid:8444", 421),
        ("localhost", 421),
        ("127.0.0.1", 421),
    ] {
        let r = send_with_host(f.addr, host, Req::get("/api/session")).await;
        assert_eq!(r.status, status, "{host}");
    }
    f.stop().await;
}
