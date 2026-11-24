// SPDX-License-Identifier: MIT OR Apache-2.0
//! The limits of 7.10 at their bounds, each with its code, and the write guard of 7.4.

mod server_common;

use std::sync::Arc;
use std::time::Duration;

use pupitre::access::{AnyAccess, OpenAccess};
use pupitre_core::json::Json;
use server_common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn fixture_with(tweak: impl FnOnce(&mut pupitre::server::Settings)) -> Fixture<AnyAccess> {
    let store = Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    tweak(&mut s);
    start(AnyAccess::Open(OpenAccess), s, store).await
}

fn request(target: &str, extra_headers: &str) -> Vec<u8> {
    format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra_headers}\r\n").into_bytes()
}

#[tokio::test]
async fn the_request_target_is_bounded_at_8192_bytes_7_10() {
    let f = open_fixture().await;
    let ok = format!("/api/{}", "a".repeat(8192 - 5));
    assert_eq!(ok.len(), 8192);
    let r = raw(f.addr, &request(&ok, "")).await;
    assert_eq!((r.status, r.error().as_str()), (404, "not_found"));
    let long = format!("/api/{}", "a".repeat(8192 - 4));
    assert_eq!(long.len(), 8193);
    let r = raw(f.addr, &request(&long, "")).await;
    assert_eq!((r.status, r.error().as_str()), (414, "uri_too_long"));
    assert!(r.header("content-security-policy").is_some());
    assert!(r.header("x-request-id").is_some());
    f.stop().await;
}

#[tokio::test]
async fn the_headers_are_bounded_at_100_and_16384_bytes_7_10() {
    let f = open_fixture().await;
    // Host and Connection are two of the 100 headers.
    let headers = |n: usize| (0..n).map(|i| format!("X-H{i}: v\r\n")).collect::<String>();
    let r = raw(f.addr, &request("/api/session", &headers(98))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = raw(f.addr, &request("/api/session", &headers(99))).await;
    assert_eq!((r.status, r.error().as_str()), (431, "headers_too_large"));
    // The bytes: a name, a value and 4 bytes of separators count for each header.
    let big = format!("X-Big: {}\r\n", "v".repeat(16384 - 100));
    let r = raw(f.addr, &request("/api/session", &big)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let big = format!("X-Big: {}\r\n", "v".repeat(16384));
    let r = raw(f.addr, &request("/api/session", &big)).await;
    assert_eq!((r.status, r.error().as_str()), (431, "headers_too_large"));
    f.stop().await;
}

#[tokio::test]
async fn a_head_above_the_buffer_of_the_parser_is_refused_without_a_panic_7_10() {
    let f = open_fixture().await;
    let huge = format!("X-Huge: {}\r\n", "v".repeat(70_000));
    let mut stream = TcpStream::connect(f.addr).await.unwrap();
    let _ = stream.write_all(&request("/api/session", &huge)).await;
    let mut out = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    assert!(read.is_ok(), "the connection ends");
    let text = String::from_utf8_lossy(&out).into_owned();
    assert!(text.is_empty() || text.starts_with("HTTP/1.1 431 ") || text.starts_with("HTTP/1.1 400 "), "{text}");
    // The server still serves.
    assert_eq!(f.get("/api/session").await.status, 200);
    f.stop().await;
}

#[tokio::test]
async fn a_long_method_token_never_reaches_the_log_7_9() {
    let f = open_fixture().await;
    let method = "X".repeat(5000);
    let r = raw(
        f.addr,
        format!("{method} /api/session HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await;
    assert_eq!((r.status, r.error().as_str()), (405, "method_not_allowed"));
    let lines = f.log.events("http.request");
    let line = lines.iter().find(|l| l.as_object().unwrap()["status"] == Json::Int(405)).expect("a log line");
    assert_eq!(line.as_object().unwrap()["method"], Json::str("other"));
    assert!(pupitre_core::json::to_compact(line).len() < 512);
    // A standard method keeps its name in the log.
    assert_eq!(f.send(Req::method("/api/session", "PATCH")).await.status, 405);
    let lines = f.log.events("http.request");
    assert!(lines.iter().any(|l| l.as_object().unwrap()["method"] == Json::str("PATCH")));
    f.stop().await;
}

fn post_env(body: &str) -> Req {
    Req::post("/api/environments").json(body)
}

#[tokio::test]
async fn the_body_is_bounded_by_the_key_7_10() {
    let f = fixture_with(|s| s.max_request_bytes = 1024).await;
    let base = r#"{"id":"staging","document":{"format":1,"label":"S"}}"#;
    // A body of 1,024 bytes passes the limit.
    let at_bound = format!("{base}{}", " ".repeat(1024 - base.len()));
    assert_eq!(at_bound.len(), 1024);
    let r = f.send(post_env(&at_bound)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    // A body of 1,025 bytes gives 413, with no write.
    let head = r#"{"id":"second","document":{"format":1,"label":"S"}}"#;
    let above = format!("{head}{}", " ".repeat(1025 - head.len()));
    assert_eq!(above.len(), 1025);
    let r = f.send(post_env(&above)).await;
    assert_eq!((r.status, r.error().as_str()), (413, "payload_too_large"));
    assert_eq!(f.get("/api/environments/second").await.status, 404);
    // The declared size is checked before the body is read: no byte of it is sent.
    let head = b"POST /api/environments HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 5000\r\n\r\n";
    let r = raw(f.addr, head).await;
    assert_eq!((r.status, r.error().as_str()), (413, "payload_too_large"));
    // A chunked body with no declared size is counted while it is read.
    let chunk = "a".repeat(600);
    let chunked = format!(
        "POST /api/environments HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n{:x}\r\n{chunk}\r\n0\r\n\r\n",
        chunk.len(),
        chunk.len()
    );
    let r = raw(f.addr, chunked.as_bytes()).await;
    assert_eq!((r.status, r.error().as_str()), (413, "payload_too_large"));
    f.stop().await;
}

#[tokio::test]
async fn the_depth_of_a_body_is_16_2_5() {
    let f = open_fixture().await;
    let nested = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    // The document is at the depth 2 of the body; 15 more levels is depth 17.
    let deep = format!(r#"{{"id":"x","document":{{"format":1,"label":"S","z":{}}}}}"#, nested(15));
    let r = f.send(post_env(&deep)).await;
    assert_eq!((r.status, r.error().as_str()), (400, "bad_request"));
    f.stop().await;
}

#[tokio::test]
async fn a_slow_body_gives_408_after_the_key_7_10() {
    let f = fixture_with(|s| s.body_read_timeout = Duration::from_millis(300)).await;
    let mut stream = TcpStream::connect(f.addr).await.unwrap();
    stream
        .write_all(b"POST /api/environments HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"id\"")
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    assert!(text.starts_with("HTTP/1.1 408 "), "{text}");
    assert!(text.contains("\"request_timeout\""));
    f.stop().await;
}

#[tokio::test]
async fn the_bodies_in_flight_are_bounded_7_10() {
    let f = fixture_with(|s| s.max_inflight_bodies = 1).await;
    // The first request holds its place: it sent a part of its body.
    let mut slow = TcpStream::connect(f.addr).await.unwrap();
    slow.write_all(b"POST /api/environments HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"id\"")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let r = f.send(post_env(r#"{"id":"staging","document":{"format":1,"label":"S"}}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (503, "server_busy"));
    assert_eq!(r.header("retry-after"), Some("1"));
    assert_eq!(r.member("retryable"), Json::Bool(true));
    // A route with no body takes no place.
    assert_eq!(f.get("/api/session").await.status, 200);
    // The place comes back when the first request ends.
    drop(slow);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let r = f.send(post_env(r#"{"id":"staging","document":{"format":1,"label":"S"}}"#)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    f.stop().await;
}

#[tokio::test]
async fn the_open_connections_are_bounded_7_10() {
    let f = fixture_with(|s| s.max_connections = 2).await;
    let a = TcpStream::connect(f.addr).await.unwrap();
    let b = TcpStream::connect(f.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut third = TcpStream::connect(f.addr).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), third.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    assert!(text.starts_with("HTTP/1.1 503 "), "{text}");
    assert!(text.contains("Retry-After: 1\r\n"));
    assert!(text.contains("\"server_busy\""));
    assert!(text.to_ascii_lowercase().contains("content-security-policy"));
    // A place comes back when a connection closes.
    drop(a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(f.get("/api/session").await.status, 200);
    drop(b);
    f.stop().await;
}

#[tokio::test]
async fn a_slow_head_closes_the_connection_after_the_key_7_10() {
    let f = fixture_with(|s| s.header_read_timeout = Duration::from_millis(300)).await;
    let mut stream = TcpStream::connect(f.addr).await.unwrap();
    stream.write_all(b"GET /api/session HTTP/1.1\r\nHost: localhost\r\n").await.unwrap();
    let mut out = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    assert!(read.is_ok(), "the connection is closed");
    assert!(out.is_empty(), "no answer for an incomplete head: {:?}", String::from_utf8_lossy(&out));
    f.stop().await;
}

#[tokio::test]
async fn an_idle_connection_closes_after_the_key_7_10() {
    let f = fixture_with(|s| s.idle_timeout = Duration::from_millis(300)).await;
    let mut stream = TcpStream::connect(f.addr).await.unwrap();
    stream.write_all(b"GET /api/session HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200 "));
    // The connection is kept for a second request.
    stream.write_all(b"GET /api/session HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200 "));
    // Then it is idle, and the server closes it.
    let n = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await.expect("closed in time").unwrap();
    assert_eq!(n, 0);
    f.stop().await;
}

#[tokio::test]
async fn the_page_size_is_the_key_5_6() {
    let f = fixture_with(|s| s.max_page_items = 2).await;
    for path in [
        "/api/environments?limit=3",
        "/api/environments/production/hosts?limit=3",
        "/api/runs?limit=3",
        "/api/environments/production/history?limit=3",
    ] {
        assert_eq!(f.get(path).await.status, 400, "{path}");
    }
    let r = f.get("/api/environments/production/hosts").await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 2, "the default is the bound");
    assert!(r.member("next").as_str().is_some());
    let next = r.member("next").as_str().unwrap().to_string();
    let r = f.get(&format!("/api/environments/production/hosts?after={next}")).await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 1);
    assert!(r.json().as_object().unwrap().get("next").is_none());
    assert_eq!(f.get("/api/session").await.member("limits").as_object().unwrap()["max_page_items"], Json::Int(2));
    f.stop().await;
}

// The write guard of 7.4.

fn put_env(f: &Fixture<AnyAccess>) -> Req {
    let _ = f;
    Req::post("/api/environments").json(r#"{"id":"staging","document":{"format":1,"label":"S"}}"#)
}

#[tokio::test]
async fn the_fetch_metadata_decides_a_write_first_7_4() {
    let f = open_fixture().await;
    for (value, ok) in [("same-origin", true), ("none", true), ("cross-site", false), ("same-site", false), ("", false)]
    {
        let r = f.send(put_env(&f).header("Sec-Fetch-Site", value).header("X-Index", value)).await;
        if ok {
            assert!(r.status == 201 || r.status == 409, "{value}: {} {}", r.status, r.text());
        } else {
            assert_eq!((r.status, r.error().as_str()), (403, "cross_site_request"), "{value:?}");
        }
    }
    // The guard covers each write method and no read.
    let r = f
        .send(
            Req::delete("/api/environments/production")
                .header("Sec-Fetch-Site", "cross-site")
                .if_match(&"a".repeat(64)),
        )
        .await;
    assert_eq!(r.error(), "cross_site_request");
    let r = f
        .send(
            Req::put("/api/environments/production/secrets/storage.password")
                .header("Sec-Fetch-Site", "cross-site")
                .json(r#"{"value":"12345678"}"#),
        )
        .await;
    assert_eq!(r.error(), "cross_site_request");
    let r = f
        .send(Req::post("/api/runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42/cancel").header("Sec-Fetch-Site", "cross-site"))
        .await;
    assert_eq!(r.error(), "cross_site_request");
    assert_eq!(f.send(Req::get("/api/session").header("Sec-Fetch-Site", "cross-site")).await.status, 200);
    // Nothing was written by a refused request.
    assert_eq!(f.get("/api/environments/production/secrets").await.member("items"), Json::Arr(vec![]));
    f.stop().await;
}

#[tokio::test]
async fn the_origin_must_equal_the_host_when_there_is_no_fetch_metadata_7_4() {
    let f = open_fixture().await;
    let host = f.addr.to_string();
    let body = |id: &str| format!(r#"{{"id":"{id}","document":{{"format":1,"label":"S"}}}}"#);
    let send = |origin: &str, id: &str| {
        let req = Req::post("/api/environments").header("Origin", origin).json(&body(id));
        f.send(req)
    };
    assert_eq!(send(&format!("http://{host}"), "one").await.status, 201);
    for bad in [
        "http://other.example.invalid",
        "https://127.0.0.1:1",
        "null",
        "http://127.0.0.1",
        &format!("http://{host}.evil.example.invalid"),
    ] {
        let r = send(bad, "bad").await;
        assert_eq!((r.status, r.error().as_str()), (403, "cross_site_request"), "{bad}");
    }
    assert_eq!(f.get("/api/environments/bad").await.status, 404);
    // Sec-Fetch-Site wins: a matching Origin does not save a cross-site request.
    let r = f
        .send(
            Req::post("/api/environments")
                .header("Origin", &format!("http://{host}"))
                .header("Sec-Fetch-Site", "cross-site")
                .json(&body("bad")),
        )
        .await;
    assert_eq!(r.error(), "cross_site_request");
    f.stop().await;
}

#[tokio::test]
async fn a_body_needs_the_media_type_json_7_4() {
    let f = open_fixture().await;
    let body = r#"{"id":"staging","document":{"format":1,"label":"S"}}"#;
    for ok in ["application/json", "application/json; charset=utf-8", "Application/JSON;charset=UTF-8"] {
        let r = f.send(Req::post("/api/environments").header("Content-Type", ok).raw_body(body.as_bytes())).await;
        assert!(r.status == 201 || r.status == 409, "{ok}: {}", r.text());
    }
    for bad in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "application/json; charset=latin1",
        "application/json; boundary=x",
        "application/vnd.api+json",
    ] {
        let r = f.send(Req::post("/api/environments").header("Content-Type", bad).raw_body(body.as_bytes())).await;
        assert_eq!((r.status, r.error().as_str()), (415, "unsupported_media_type"), "{bad}");
    }
    let r = f.send(Req::post("/api/environments").raw_body(body.as_bytes())).await;
    assert_eq!((r.status, r.error().as_str()), (415, "unsupported_media_type"), "no Content-Type");
    // A request with no body has no media type to check.
    let r = f.send(Req::post("/api/runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42/cancel")).await;
    assert_eq!(r.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn no_answer_has_a_cors_header_or_a_cookie_7_4() {
    let f = open_fixture().await;
    for r in [
        f.send(Req::get("/api/session").header("Origin", "http://other.example.invalid")).await,
        f.send(
            Req::method("/api/session", "OPTIONS")
                .header("Origin", "http://other.example.invalid")
                .header("Access-Control-Request-Method", "PUT"),
        )
        .await,
        f.send(Req::post("/api/environments").header("Origin", "http://other.example.invalid").json("{}")).await,
    ] {
        assert!(
            r.headers.iter().all(|(n, _)| !n.to_ascii_lowercase().starts_with("access-control-")),
            "{:?}",
            r.headers
        );
        assert!(r.header("set-cookie").is_none());
    }
    f.stop().await;
}
