// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ops listener (5.20), the metrics (6.28), the catalog poll and the own writes (2.41), and
//! the base path (5.2).

mod server_common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pupitre::access::{AnyAccess, OpenAccess, Refusal};
use pupitre_core::doc::DocKey;
use pupitre_core::json::Json;
use server_common::*;

async fn eventually<F: Fn() -> bool>(what: &str, f: F) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what}");
}

fn env_bytes(label: &str) -> Vec<u8> {
    format!("{{\n  \"format\": 1,\n  \"label\": \"{label}\"\n}}\n").into_bytes()
}

#[tokio::test]
async fn livez_answers_ok_5_20() {
    let f = open_fixture().await;
    let r = f.ops_get("/livez").await;
    assert_eq!((r.status, r.text().as_str()), (200, "ok"));
    let r = f.ops_get("/readyz").await;
    assert_eq!((r.status, r.text().as_str()), (200, "ok"));
    // The routes of the ops listener have no base path and no token; other paths are 404.
    assert_eq!(f.ops_get("/api/session").await.status, 404);
    assert_eq!(f.ops_get("/").await.status, 404);
    let r = send(f.ops, Req::post("/livez")).await;
    assert_eq!(r.status, 405);
    let r = send(f.ops, Req::head("/livez")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.is_empty());
    // The main listener has no ops route.
    assert_eq!(f.get("/livez").await.status, 404);
    assert_eq!(f.get("/metrics").await.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn readyz_follows_the_three_checks_5_20() {
    let f = open_fixture().await;
    // The store check: a poll that fails makes the probe fail with the code of the check.
    f.store.down.store(true, Ordering::SeqCst);
    eventually("the probe fails", || !f.state.store_ok()).await;
    let r = f.ops_get("/readyz").await;
    assert_eq!((r.status, r.text().as_str()), (503, "store"));
    // The liveness probe is not affected.
    assert_eq!(f.ops_get("/livez").await.status, 200);
    f.store.down.store(false, Ordering::SeqCst);
    eventually("the probe recovers", || f.state.store_ok()).await;
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    // The runner node holds the lease `runner`.
    f.state.set_runner_lease(false);
    let r = f.ops_get("/readyz").await;
    assert_eq!((r.status, r.text().as_str()), (503, "runner_lease"));
    f.state.set_runner_lease(true);
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    f.stop().await;
}

#[tokio::test]
async fn the_lease_check_is_for_the_runner_node_only_5_20() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    let mut r = runner();
    r.execute = false;
    s.runner = Some(r);
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    f.state.set_runner_lease(false);
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    f.stop().await;
}

#[tokio::test]
async fn a_pdp_outage_never_changes_readyz_5_20() {
    let (f, fake) = fake_fixture().await;
    fake.decide_with(|_, _, _| Err(Refusal::PdpUnavailable));
    fake.set_access(Err(Refusal::PdpUnavailable));
    let r = f.send(Req::get("/api/session").token(TOKEN_ALICE)).await;
    assert_eq!(r.status, 503);
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    assert_eq!(f.ops_get("/livez").await.status, 200);
    // The action document still answers during an outage (3.18).
    assert_eq!(f.get("/.well-known/procurauth/actions").await.status, 200);
    f.stop().await;
}

#[tokio::test]
async fn a_load_above_a_limit_keeps_the_old_state_and_fails_the_probe_2_41() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    s.limits.max_environments = 2;
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    f.store.external_put(&DocKey::parse("environments/second").unwrap(), env_bytes("Second"));
    eventually("the second environment", || f.state.catalog().environments().len() == 2).await;
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    // A third environment from another process goes above the limit.
    f.store.external_put(&DocKey::parse("environments/third").unwrap(), env_bytes("Third"));
    eventually("the probe fails", || !f.state.catalog().is_ready()).await;
    let r = f.ops_get("/readyz").await;
    assert_eq!((r.status, r.text().as_str()), (503, "catalog"));
    // The page still shows the old state, and the event is in the log.
    let list = f.get("/api/environments").await;
    assert_eq!(list.member("items").as_array().unwrap().len(), 2);
    let lines = f.log.events("catalog.limit_exceeded");
    assert!(!lines.is_empty());
    assert_eq!(lines[0].as_object().unwrap()["limit"], Json::str("limits.max_environments"));
    assert_eq!(lines[0].as_object().unwrap()["level"], Json::str("error"));
    // The other process removes the document: the next poll is within the limits.
    f.store.external_remove(&DocKey::parse("environments/third").unwrap());
    eventually("the probe recovers", || f.state.catalog().is_ready()).await;
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    f.stop().await;
}

/// 2.41: a Git pull adds a host file above `max_hosts_per_environment`. The catalog keeps the old
/// state, and a write that adds a document gives `503 store_over_limit` with `retryable` false. A delete
/// of a visible document goes through, so the operator can reduce the count. The next poll then
/// loads the store again.
#[tokio::test]
async fn a_delete_reduces_a_store_above_a_limit_2_41() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    s.limits.max_hosts_per_environment = 3;
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    // A fourth host file arrives in the store, outside the API.
    let host_d = b"{\n  \"address\": \"203.0.113.13\",\n  \"format\": 1,\n  \"labels\": []\n}\n".to_vec();
    f.store.external_put(&DocKey::parse("environments/production/hosts/host-d").unwrap(), host_d);
    eventually("the probe fails", || !f.state.catalog().is_ready()).await;
    assert_eq!(f.ops_get("/readyz").await.status, 503);
    // The extra document is not visible, the old state stays.
    assert_eq!(f.get("/api/environments/production/hosts").await.member("items").as_array().unwrap().len(), 3);
    assert_eq!(f.get("/api/environments/production/hosts/host-d").await.status, 404);

    // A write that adds a document, or changes one, is refused. The error is not retryable and names the limit.
    let host = f.get("/api/environments/production/hosts/host-a").await;
    let body = r#"{"document":{"format":1,"address":"203.0.113.20","labels":[]}}"#;
    let r = f.send(Req::put("/api/environments/production/hosts/host-e").create().json(body)).await;
    assert_eq!((r.status, r.error().as_str()), (503, "store_over_limit"));
    assert_eq!(r.json().as_object().unwrap()["retryable"], Json::Bool(false));
    assert_eq!(r.json().as_object().unwrap()["limit"], Json::str("limits.max_hosts_per_environment"));
    let r = f.send(Req::put("/api/environments/production/hosts/host-a").if_match(&host.etag()).json(body)).await;
    assert_eq!((r.status, r.error().as_str()), (503, "store_over_limit"));
    let r =
        f.send(Req::post("/api/environments").json(r#"{"id":"staging","document":{"format":1,"label":"S"}}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (503, "store_over_limit"));
    assert!(f.store.document(&DocKey::parse("environments/staging").unwrap()).is_none());

    // A delete of a visible document goes through.
    let victim = f.get("/api/environments/production/hosts/host-c").await;
    let r = f.send(Req::delete("/api/environments/production/hosts/host-c").if_match(&victim.etag())).await;
    assert_eq!(r.status, 200, "{}", r.text());
    // The next poll loads the store again: the catalog is ready, and the extra host is visible.
    eventually("the probe recovers", || f.state.catalog().is_ready()).await;
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    let r = f.get("/api/environments/production/hosts/host-d").await;
    assert_eq!(r.status, 200, "{}", r.text());
    // Within the limit again, a write works.
    let r =
        f.send(Req::post("/api/environments").json(r#"{"id":"staging","document":{"format":1,"label":"S"}}"#)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    f.stop().await;
}

/// 2.41: a delete of a document is allowed only for a catalog that was loaded. A store above a limit at
/// start gives an empty catalog, and each write is refused with `store_over_limit`.
#[tokio::test]
async fn a_delete_is_refused_when_nothing_is_loaded_2_41() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    store.seed_doc(
        "environments/second",
        &pupitre_core::doc::Document::Environment(pupitre_core::doc::Environment { label: "Second".into() }),
    );
    let mut s = settings(false);
    s.limits.max_environments = 1;
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    assert_eq!(f.ops_get("/readyz").await.status, 503);
    let r = f.send(Req::delete("/api/environments/second").if_match(&"a".repeat(64))).await;
    assert_eq!((r.status, r.error().as_str()), (503, "store_over_limit"));
    f.stop().await;
}

#[tokio::test]
async fn a_store_above_a_limit_at_start_does_not_stop_the_start_2_41() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    for id in ["second", "third"] {
        store.seed_doc(
            &format!("environments/{id}"),
            &pupitre_core::doc::Document::Environment(pupitre_core::doc::Environment { label: id.into() }),
        );
    }
    let mut s = settings(false);
    s.limits.max_environments = 2;
    // The start works: the catalog is empty and not ready.
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    let r = f.ops_get("/readyz").await;
    assert_eq!((r.status, r.text().as_str()), (503, "catalog"));
    assert_eq!(f.ops_get("/livez").await.status, 200);
    assert_eq!(f.get("/api/environments").await.member("items").as_array().unwrap().len(), 0);
    // A write cannot hold the limits on an empty state: it is refused.
    let body = r#"{"id":"staging","document":{"format":1,"label":"S"}}"#;
    let r = f.send(Req::post("/api/environments").json(body)).await;
    assert_eq!((r.status, r.error().as_str()), (503, "store_over_limit"));
    assert!(f.store.document(&DocKey::parse("environments/staging").unwrap()).is_none());
    eventually("the poll writes the event", || !f.log.events("catalog.limit_exceeded").is_empty()).await;
    // The store goes back within the limit: the next round loads it.
    f.store.external_remove(&DocKey::parse("environments/third").unwrap());
    eventually("the probe recovers", || f.state.catalog().is_ready()).await;
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    assert_eq!(f.get("/api/environments").await.member("items").as_array().unwrap().len(), 2);
    f.stop().await;
}

#[tokio::test]
async fn the_metrics_of_6_28_have_the_text_format_0_0_4() {
    let f = open_fixture().await;
    f.get("/api/session").await;
    f.get("/api/session").await;
    f.get("/api/environments/production").await;
    f.get("/api/environments/nothing").await;
    f.get("/api/environments/production/units/relay/effective").await;
    let r = f.ops_get("/metrics").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("text/plain; version=0.0.4; charset=utf-8"));
    let text = r.text();
    // The label of the route is the pattern, never an id.
    assert!(text.contains("pupitre_http_requests_total{route=\"GET /api/session\",status=\"200\"} 2"), "{text}");
    assert!(text.contains("pupitre_http_requests_total{route=\"GET /api/environments/{e}\",status=\"200\"} 1"));
    assert!(text.contains("pupitre_http_requests_total{route=\"GET /api/environments/{e}\",status=\"404\"} 1"));
    assert!(!text.contains("production") && !text.contains("nothing"));
    assert!(text.contains("pupitre_http_request_seconds_count{route=\"GET /api/session\"} 2"));
    assert!(text.contains("pupitre_catalog_documents{kind=\"host\"} 3"));
    assert!(text.contains("pupitre_catalog_documents{kind=\"unit\"} 1"));
    assert!(text.contains("pupitre_catalog_documents{kind=\"environment\"} 1"));
    assert!(text.contains("\npupitre_runs_queued 0\n"));
    assert!(text.contains("\npupitre_runs_oldest_queued_seconds 0\n"));
    assert!(text.contains("\npupitre_telemetry_spans_dropped_total 0\n"));
    // Every sample of the main listener has a route label of the router or `unmatched`.
    f.get("/api/does-not-exist").await;
    let text = f.ops_get("/metrics").await.text();
    assert!(text.contains("route=\"unmatched\""));
    f.stop().await;
}

#[tokio::test]
async fn the_queued_gauges_follow_the_store_6_28() {
    let f = open_fixture().await;
    let mut old = run_record(
        1,
        pupitre_core::run::RunKind::Deploy,
        pupitre_core::run::RunMode::Check,
        pupitre_core::run::RunStatus::Queued,
        "environment:production/unit:relay",
        &["host-a"],
    );
    old.created_at = pupitre_core::time::UtcMillis(pupitre_core::time::UtcMillis::now().0 - 12_000);
    f.store.seed_run(old);
    let running = run_record(
        2,
        pupitre_core::run::RunKind::Deploy,
        pupitre_core::run::RunMode::Check,
        pupitre_core::run::RunStatus::Running,
        "environment:production/unit:relay",
        &["host-a"],
    );
    f.store.seed_run(running);
    let text = f.ops_get("/metrics").await.text();
    assert!(text.contains("\npupitre_runs_queued 1\n"), "{text}");
    assert!(text.contains("\npupitre_runs_running 1\n"));
    let line = text.lines().find(|l| l.starts_with("pupitre_runs_oldest_queued_seconds ")).unwrap();
    let secs: u64 = line.rsplit(' ').next().unwrap().parse().unwrap();
    assert!((12..=20).contains(&secs), "{line}");
    // A store that does not answer gives no sample for the gauges that it feeds.
    f.store.down.store(true, Ordering::SeqCst);
    let text = f.ops_get("/metrics").await.text();
    assert!(!text.contains("\npupitre_runs_queued "));
    assert!(text.contains("pupitre_store_errors_total{method=\"list_runs\"}"));
    f.store.down.store(false, Ordering::SeqCst);
    f.stop().await;
}

#[tokio::test]
async fn the_catalog_reads_a_change_of_another_process_at_each_poll_2_41() {
    let f = open_fixture().await;
    let key = DocKey::parse("environments/production/hosts/host-d").unwrap();
    assert_eq!(f.get("/api/environments/production/hosts/host-d").await.status, 404);
    f.store.external_put(
        &key,
        br#"{
  "address": "203.0.113.13",
  "format": 1,
  "labels": []
}
"#
        .to_vec(),
    );
    eventually("the new host", || f.state.catalog().get(&key).is_some()).await;
    let r = f.get("/api/environments/production/hosts/host-d").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains("203.0.113.13"));
    assert_eq!(f.get("/api/environments/production/hosts").await.member("items").as_array().unwrap().len(), 4);
    // The poll runs at its period, and a poll with no change leaves the entries as they are.
    let polls = f.store.polls.load(Ordering::SeqCst);
    eventually("more polls", || f.store.polls.load(Ordering::SeqCst) > polls + 2).await;
    assert_eq!(f.get("/api/environments/production/hosts/host-d").await.status, 200);
    // A removal.
    f.store.external_remove(&key);
    eventually("the removal", || f.state.catalog().get(&key).is_none()).await;
    assert_eq!(f.get("/api/environments/production/hosts/host-d").await.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn a_broken_reference_is_shown_as_an_error_and_stops_the_runs_2_41_and_2_16() {
    let f = open_fixture().await;
    // Another instance deleted the document of a host that the unit still holds.
    f.store.external_remove(&DocKey::parse("environments/production/hosts/host-a").unwrap());
    eventually("the reference breaks", || {
        !f.state.catalog().entry_errors(&DocKey::parse("environments/production/units/relay").unwrap()).is_empty()
    })
    .await;
    let list = f.get("/api/environments/production/units").await;
    let binding = list.member("items");
    let item = binding.as_array().unwrap()[0].as_object().unwrap().clone();
    assert!(item.contains_key("errors"), "{item:?}");
    assert!(item["errors"].as_array().unwrap()[0].as_object().unwrap()["message"].as_str().unwrap().contains("host-a"));
    // The unit is not used for a run: its node is not valid in the catalog (4.4, rule 2).
    let r = f
        .send(
            Req::post("/api/runs").json(
                r#"{"kind":"deploy","mode":"check","node":"environment:production/unit:relay","hosts":["host-b"]}"#,
            ),
        )
        .await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"), "{}", r.text());
    // The host node of the broken host does not exist.
    let r = f
        .send(Req::post("/api/runs").json(r#"{"kind":"deploy","mode":"check","node":"environment:production/unit:relay/host:host-a","hosts":["host-a"]}"#))
        .await;
    assert_eq!(r.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn an_own_write_is_in_the_catalog_at_once_2_41() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    // A period of one hour: only the own write can bring the document.
    let f = start_with(AnyAccess::Open(OpenAccess), settings(false), store, capabilities(), Duration::from_secs(3600))
        .await;
    let r = f
        .send(
            Req::put("/api/environments/production/hosts/host-d")
                .create()
                .json(r#"{"document":{"address":"203.0.113.13","format":1,"labels":[]}}"#),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let r = f.get("/api/environments/production/hosts/host-d").await;
    assert_eq!(r.status, 200);
    let r = f.get("/api/environments/production/hosts").await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 4);
    f.stop().await;
}

#[tokio::test]
async fn the_base_path_5_2() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    s.base_path = "/console/".to_string();
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    let r = f.get("/console/api/session").await;
    assert_eq!(r.status, 200, "{}", r.text());
    // A path outside the base path gives 404 not_found, also for a page and for an API path.
    for path in ["/api/session", "/", "/e/production", "/console", "/consolex/api/session", "/static/x/y"] {
        let r = f.get(path).await;
        assert_eq!((r.status, r.error().as_str()), (404, "not_found"), "{path}");
    }
    // The shell has the base path and the static files are below it.
    let page = f.get("/console/").await;
    assert_eq!(page.status, 200);
    assert!(page.text().contains("<base href=\"/console/\">"), "{}", page.text());
    let build = f.state.assets.build.clone();
    assert_eq!(f.get(&format!("/console/static/{build}/css/base.css")).await.status, 200);
    assert_eq!(f.get("/console/e/production").await.status, 200);
    // The Location of a run is below the base path.
    let r = f
        .send(
            Req::post("/console/api/runs").json(
                r#"{"kind":"deploy","mode":"check","node":"environment:production/unit:relay","hosts":["host-a"]}"#,
            ),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.member("id").as_str().unwrap().to_string();
    assert_eq!(r.header("location"), Some(format!("/console/api/runs/{id}").as_str()));
    // The ops listener has no base path.
    assert_eq!(f.ops_get("/livez").await.status, 200);
    f.stop().await;
}

#[tokio::test]
async fn a_graceful_stop_waits_for_the_open_connection_and_logs_the_reason_6_27() {
    let f = open_fixture().await;
    let log = f.log.clone();
    let addr = f.addr;
    assert_eq!(f.get("/api/session").await.status, 200);
    f.stop().await;
    let lines = log.events("server.stop");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].as_object().unwrap()["reason"], Json::str("test"));
    assert_eq!(log.events("server.start").len(), 1);
    assert_eq!(log.events("server.start")[0].as_object().unwrap()["version"], Json::str("0.1.0"));
    // The listener is closed.
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn a_request_makes_the_spans_of_6_29_and_a_full_queue_drops_the_new_one() {
    use pupitre::server::telemetry::Telemetry;
    let store = Arc::new(MemStore::new());
    seed(&store);
    let fake = FakeAccess::allow_all();
    let t = Telemetry::enabled("pupitre-test", 4);
    let f = start_full(fake, settings(true), store, capabilities(), Duration::from_secs(3600), t.clone()).await;
    // A write has the spans `http.request`, `access.decide` and `store.put`.
    let r = f
        .send(
            Req::put("/api/environments/production/hosts/host-d")
                .token(TOKEN_ALICE)
                .create()
                .json(r#"{"document":{"address":"203.0.113.13","format":1,"labels":[]}}"#),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    eventually("the spans", || t.queued() == 3).await;
    assert_eq!(t.dropped(), 0);
    // The queue has 4 places: the next request makes 2 spans, and the last one is dropped.
    assert_eq!(f.send(Req::get("/api/environments/production").token(TOKEN_ALICE)).await.status, 200);
    eventually("the spans", || t.queued() + t.dropped() as usize == 5).await;
    assert_eq!((t.queued(), t.dropped()), (4, 1));
    let text = f.ops_get("/metrics").await.text();
    assert!(text.contains("\npupitre_telemetry_spans_dropped_total 1\n"), "{text}");
    f.stop().await;
}
