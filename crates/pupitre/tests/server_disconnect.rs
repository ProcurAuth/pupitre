// SPDX-License-Identifier: MIT OR Apache-2.0
//! A client that closes the connection does not cancel a request (3.13): the write, the catalog,
//! the log and the limits stay in step with the store (2.41, 2.42).

mod server_common;

use std::sync::Arc;
use std::time::Duration;

use pupitre::access::{AnyAccess, OpenAccess};
use pupitre_core::doc::DocKey;
use server_common::*;

fn environment_body(id: &str) -> String {
    format!(r#"{{"id":"{id}","document":{{"format":1,"label":"{id}"}}}}"#)
}

fn stored_environments(store: &MemStore) -> usize {
    ["staging", "qa", "dev", "lab", "test"]
        .iter()
        .chain(["production"].iter())
        .filter(|id| store.document(&DocKey::parse(&format!("environments/{id}")).unwrap()).is_some())
        .count()
}

#[tokio::test]
async fn a_closed_connection_does_not_cancel_a_write_3_13() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    // The store commits, then it takes a short time to answer: the client is gone by then.
    *store.write_delay.lock().unwrap() = Duration::from_millis(60);
    let mut s = settings(false);
    s.limits.max_environments = 2;
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    for id in ["staging", "qa", "dev"] {
        send_and_half_close(f.addr, Req::post("/api/environments").json(&environment_body(id))).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    // The limit holds: `production` and one more environment.
    assert_eq!(stored_environments(&f.store), 2, "the limit max_environments is 2");
    // Each stored document has its log line and its place in the catalog.
    let lines = f.log.events("document.write");
    assert_eq!(lines.len(), 1, "one write above the seed: {lines:?}");
    assert_eq!(f.state.catalog().environments().len(), 2);
    // The probe stays ready and the other writes were refused with `limit_exceeded`.
    assert_eq!(f.ops_get("/readyz").await.status, 200);
    let refused: Vec<_> = f
        .log
        .events("http.request")
        .into_iter()
        .filter(|l| l.as_object().unwrap()["status"] == pupitre_core::json::Json::Int(422))
        .collect();
    assert_eq!(refused.len(), 2, "two writes above the limit: {:?}", f.log.events("http.request"));
    f.stop().await;
}
#[tokio::test]
async fn the_requests_of_a_closed_connection_count_as_open_connections_7_10() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    *store.write_delay.lock().unwrap() = Duration::from_millis(300);
    let f = start(AnyAccess::Open(OpenAccess), settings(false), store).await;
    send_and_half_close(f.addr, Req::post("/api/environments").json(&environment_body("staging"))).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(f.state.inflight_requests(), 1, "the handler still runs");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(f.state.inflight_requests(), 0);
    assert_eq!(f.log.events("document.write").len(), 1);
    f.stop().await;
}
