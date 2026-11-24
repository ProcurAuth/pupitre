// SPDX-License-Identifier: MIT OR Apache-2.0
//! The routes of section 5 in the mode `open`: status, headers, body, `ETag`, `If-Match` and
//! `If-None-Match` (5.5, 5.8 to 5.15).

mod server_common;

use pupitre_core::json::Json;
use server_common::*;

fn doc(inner: &str) -> String {
    format!(r#"{{"document":{inner},"message":"test"}}"#)
}

const ENV_DOC: &str = r#"{"format":1,"label":"Staging"}"#;

#[tokio::test]
async fn the_session_of_5_7() {
    let f = open_fixture().await;
    let r = f.get("/api/session").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.text(),
        r#"{"limits":{"max_document_bytes":4096,"max_hosts":3,"max_page_items":5},"mode":"open","runner":{"allow_apply":true,"enabled":true},"subject":"-","version":"0.1.0"}"#
    );
    assert_eq!(r.header("content-type"), Some("application/json; charset=utf-8"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    f.stop().await;
}

#[tokio::test]
async fn the_session_without_runner_5_7() {
    let store = std::sync::Arc::new(MemStore::new());
    let mut s = settings(false);
    s.runner = None;
    let f = start(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess), s, store).await;
    let r = f.get("/api/session").await;
    assert_eq!(r.member("runner").as_object().unwrap().len(), 2);
    assert_eq!(r.member("runner"), Json::object([("allow_apply", Json::Bool(false)), ("enabled", Json::Bool(false))]));
    f.stop().await;
}

#[tokio::test]
async fn the_catalog_of_5_15_has_no_path() {
    let f = open_fixture().await;
    let r = f.get("/api/catalog").await;
    assert_eq!(r.status, 200);
    let body = r.json();
    let m = body.as_object().unwrap();
    assert_eq!(m["schemas"].as_array().unwrap().len(), 2);
    assert_eq!(m["playbooks"].as_array().unwrap().len(), 6);
    assert_eq!(m["commands"].as_array().unwrap().len(), 2);
    assert!(!r.text().contains("/playbooks/"), "a path in the catalog: {}", r.text());
    assert!(!r.text().contains("/schemas/"));
    f.stop().await;
}

#[tokio::test]
async fn the_catalog_without_runner_has_no_commands_and_no_playbooks_5_15() {
    let store = std::sync::Arc::new(MemStore::new());
    let mut s = settings(false);
    s.runner = None;
    let f = start(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess), s, store).await;
    let r = f.get("/api/catalog").await;
    let body = r.json();
    let m = body.as_object().unwrap();
    assert!(m.contains_key("schemas"));
    assert!(!m.contains_key("commands"));
    assert!(!m.contains_key("playbooks"));
    f.stop().await;
}

#[tokio::test]
async fn the_environments_list_5_8_and_5_6() {
    let f = open_fixture().await;
    let r = f.get("/api/environments").await;
    assert_eq!(r.status, 200);
    let items = r.member("items");
    let item = &items.as_array().unwrap()[0];
    let m = item.as_object().unwrap();
    assert_eq!(m["id"], Json::str("production"));
    assert_eq!(m["label"], Json::str("Production"));
    assert_eq!(m["version"], Json::Int(1));
    assert_eq!(m["etag"].as_str().unwrap().len(), 64);
    assert!(!m.contains_key("errors"));
    assert!(!r.json().as_object().unwrap().contains_key("next"), "the last page has no next");
    f.stop().await;
}

#[tokio::test]
async fn the_post_of_5_8_creates_an_environment() {
    let f = open_fixture().await;
    let r = f
        .send(
            Req::post("/api/environments")
                .json(&format!(r#"{{"id":"staging","document":{ENV_DOC},"message":"first"}}"#)),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.member("version"), Json::Int(1));
    let etag = r.member("etag").as_str().unwrap().to_string();
    assert_eq!(r.etag(), etag);
    // A second post with the same id.
    let again =
        f.send(Req::post("/api/environments").json(&format!(r#"{{"id":"staging","document":{ENV_DOC}}}"#))).await;
    assert_eq!((again.status, again.error().as_str()), (409, "already_exists"));
    // The document is readable at once, with the same etag (2.41, own writes).
    let got = f.get("/api/environments/staging").await;
    assert_eq!(got.status, 200);
    assert_eq!(got.etag(), etag);
    assert_eq!(got.text(), "{\n  \"format\": 1,\n  \"label\": \"Staging\"\n}\n");
    f.stop().await;
}

#[tokio::test]
async fn a_bad_id_or_document_gives_422_with_errors_5_4() {
    let f = open_fixture().await;
    let r = f.send(Req::post("/api/environments").json(&format!(r#"{{"id":"Bad_Id","document":{ENV_DOC}}}"#))).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    let errors = r.member("errors");
    assert_eq!(errors.as_array().unwrap()[0].as_object().unwrap()["path"], Json::str("id"));
    let r =
        f.send(Req::post("/api/environments").json(r#"{"id":"ok","document":{"format":1,"label":"A","x":1}}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    assert_eq!(r.member("errors").as_array().unwrap()[0].as_object().unwrap()["path"], Json::str("x"));
    // The shape of the body: a member that the route does not know is a 400.
    let r =
        f.send(Req::post("/api/environments").json(&format!(r#"{{"id":"ok","document":{ENV_DOC},"extra":1}}"#))).await;
    assert_eq!((r.status, r.error().as_str()), (400, "bad_request"));
    let r = f.send(Req::post("/api/environments").json("not json")).await;
    assert_eq!((r.status, r.error().as_str()), (400, "bad_request"));
    let r = f.send(Req::post("/api/environments").json(r#"{"id":"ok","document":[1]}"#)).await;
    assert_eq!(r.status, 400);
    f.stop().await;
}

#[tokio::test]
async fn the_environments_are_bounded_by_the_limit_2_42() {
    let f = open_fixture().await;
    for id in ["a1", "a2", "a3"] {
        let r = f.send(Req::post("/api/environments").json(&format!(r#"{{"id":"{id}","document":{ENV_DOC}}}"#))).await;
        assert_eq!(r.status, 201, "{id}: {}", r.text());
    }
    let r = f.send(Req::post("/api/environments").json(&format!(r#"{{"id":"a4","document":{ENV_DOC}}}"#))).await;
    assert_eq!((r.status, r.error().as_str()), (422, "limit_exceeded"));
    assert_eq!(r.member("limit"), Json::str("limits.max_environments"));
    f.stop().await;
}

#[tokio::test]
async fn a_save_needs_the_current_etag_5_5() {
    let f = open_fixture().await;
    let got = f.get("/api/environments/production").await;
    let etag = got.etag();
    let new = doc(r#"{"format":1,"label":"Prod 2"}"#);
    // No precondition.
    let r = f.send(Req::put("/api/environments/production").json(&new)).await;
    assert_eq!((r.status, r.error().as_str()), (428, "precondition_required"));
    // Two preconditions, a weak tag and a star in If-Match.
    let r = f.send(Req::put("/api/environments/production").if_match(&etag).create().json(&new)).await;
    assert_eq!((r.status, r.error().as_str()), (400, "bad_request"));
    let r = f.send(Req::put("/api/environments/production").header("If-Match", "*").json(&new)).await;
    assert_eq!(r.status, 400);
    let r =
        f.send(Req::put("/api/environments/production").header("If-Match", &format!("W/\"{etag}\"")).json(&new)).await;
    assert_eq!(r.status, 400);
    // The save.
    let r = f.send(Req::put("/api/environments/production").if_match(&etag).json(&new)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.member("version"), Json::Int(2));
    let new_etag = r.member("etag").as_str().unwrap().to_string();
    assert_ne!(new_etag, etag);
    assert_eq!(r.etag(), new_etag);
    // The old etag now gives 409 with the current etag.
    let r = f.send(Req::put("/api/environments/production").if_match(&etag).json(&new)).await;
    assert_eq!((r.status, r.error().as_str()), (409, "conflict"));
    assert_eq!(r.member("etag"), Json::str(new_etag.clone()));
    assert_eq!(r.member("retryable"), Json::Bool(false));
    // A create of an existing document.
    let r = f.send(Req::put("/api/environments/production").create().json(&new)).await;
    assert_eq!((r.status, r.error().as_str()), (409, "already_exists"));
    f.stop().await;
}

#[tokio::test]
async fn a_save_of_a_missing_document_is_404_and_a_put_can_create_5_5() {
    let f = open_fixture().await;
    // The unit has no document `unit_values`: a save with If-Match gives 404.
    let body = doc(r#"{"format":1,"values":{"limits.max_clients":5}}"#);
    let r =
        f.send(Req::put("/api/environments/production/units/relay/values").if_match(&"a".repeat(64)).json(&body)).await;
    assert_eq!((r.status, r.error().as_str()), (404, "not_found"));
    // A create with If-None-Match: * gives 201.
    let r = f.send(Req::put("/api/environments/production/units/relay/values").create().json(&body)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.member("version"), Json::Int(1));
    f.stop().await;
}

#[tokio::test]
async fn a_delete_needs_if_match_and_follows_2_16() {
    let f = open_fixture().await;
    let etag = f.get("/api/environments/production").await.etag();
    let r = f.send(Req::delete("/api/environments/production")).await;
    assert_eq!((r.status, r.error().as_str()), (428, "precondition_required"));
    // The environment has other documents.
    let r = f.send(Req::delete("/api/environments/production").if_match(&etag)).await;
    assert_eq!((r.status, r.error().as_str()), (409, "not_empty"));
    assert!(!r.member("errors").as_array().unwrap().is_empty());
    // A host that a unit holds.
    let host = f.get("/api/environments/production/hosts/host-a").await;
    let r = f.send(Req::delete("/api/environments/production/hosts/host-a").if_match(&host.etag())).await;
    assert_eq!((r.status, r.error().as_str()), (409, "in_use"));
    // A host that no unit holds.
    let host = f.get("/api/environments/production/hosts/host-c").await;
    let r = f.send(Req::delete("/api/environments/production/hosts/host-c").if_match(&host.etag())).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.text(), r#"{"version":2}"#);
    assert_eq!(f.get("/api/environments/production/hosts/host-c").await.status, 404);
    // A delete with a body is a 400.
    let r = f.send(Req::delete("/api/environments/production/hosts/host-b").if_match(&"a".repeat(64)).json("{}")).await;
    assert_eq!((r.status, r.error().as_str()), (400, "bad_request"));
    // The rules of 2.16 come first: host-b is held by the unit.
    let r = f.send(Req::delete("/api/environments/production/hosts/host-b").if_match(&"a".repeat(64))).await;
    assert_eq!((r.status, r.error().as_str()), (409, "in_use"));
    // A stale etag of a host that no unit holds gives 409 conflict with the current etag.
    let new = doc(r#"{"address":"host-d.example.invalid","format":1,"labels":[]}"#);
    let made = f.send(Req::put("/api/environments/production/hosts/host-d").create().json(&new)).await;
    let r = f.send(Req::delete("/api/environments/production/hosts/host-d").if_match(&"a".repeat(64))).await;
    assert_eq!((r.status, r.error().as_str()), (409, "conflict"));
    assert_eq!(r.member("etag"), made.member("etag"));
    f.stop().await;
}

#[tokio::test]
async fn the_deletes_of_units_and_environments_with_nothing_below() {
    let f = open_fixture().await;
    let unit = f.get("/api/environments/production/units/relay").await;
    let r = f.send(Req::delete("/api/environments/production/units/relay").if_match(&unit.etag())).await;
    assert_eq!(r.status, 200, "{}", r.text());
    for host in ["host-a", "host-b", "host-c"] {
        let h = f.get(&format!("/api/environments/production/hosts/{host}")).await;
        let r = f.send(Req::delete(&format!("/api/environments/production/hosts/{host}")).if_match(&h.etag())).await;
        assert_eq!(r.status, 200, "{host}: {}", r.text());
    }
    let env = f.get("/api/environments/production").await;
    let r = f.send(Req::delete("/api/environments/production").if_match(&env.etag())).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(f.get("/api/environments").await.member("items"), Json::Arr(vec![]));
    f.stop().await;
}

#[tokio::test]
async fn an_environment_with_secrets_is_not_deleted() {
    let f = open_fixture().await;
    // Keep one secret, then delete every document of the environment.
    let r = f
        .send(Req::put("/api/environments/production/secrets/storage.password").json(r#"{"value":"correct horse"}"#))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let u = f.get("/api/environments/production/units/relay").await;
    f.send(Req::delete("/api/environments/production/units/relay").if_match(&u.etag())).await;
    for host in ["host-a", "host-b", "host-c"] {
        let h = f.get(&format!("/api/environments/production/hosts/{host}")).await;
        f.send(Req::delete(&format!("/api/environments/production/hosts/{host}")).if_match(&h.etag())).await;
    }
    let env = f.get("/api/environments/production").await;
    let r = f.send(Req::delete("/api/environments/production").if_match(&env.etag())).await;
    assert_eq!((r.status, r.error().as_str()), (409, "not_empty"));
    f.stop().await;
}

#[tokio::test]
async fn the_host_documents_5_9() {
    let f = open_fixture().await;
    let list = f.get("/api/environments/production/hosts").await;
    assert_eq!(list.status, 200);
    let binding = list.member("items");
    let items = binding.as_array().unwrap();
    assert_eq!(items.len(), 3);
    let first = items[0].as_object().unwrap();
    assert_eq!(first["id"], Json::str("host-a"));
    assert_eq!(first["document"].as_object().unwrap()["address"], Json::str("203.0.113.10"));
    // A save of a host with a label that is the group of a schema (2.8).
    let got = f.get("/api/environments/production/hosts/host-a").await;
    let bad = doc(r#"{"address":"203.0.113.10","format":1,"labels":["relay_hosts"],"port":22}"#);
    let r = f.send(Req::put("/api/environments/production/hosts/host-a").if_match(&got.etag()).json(&bad)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    // A new host.
    let new = doc(r#"{"address":"host-d.example.invalid","format":1,"labels":[]}"#);
    let r = f.send(Req::put("/api/environments/production/hosts/host-d").create().json(&new)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    f.stop().await;
}

#[tokio::test]
async fn the_unit_documents_and_the_rules_across_documents_5_10_and_2_16() {
    let f = open_fixture().await;
    let list = f.get("/api/environments/production/units").await;
    let binding = list.member("items");
    let item = binding.as_array().unwrap()[0].as_object().unwrap().clone();
    assert_eq!(item["id"], Json::str("relay"));
    assert_eq!(item["schema"], Json::str("relay"));
    assert_eq!(item["hosts"], Json::Arr(vec![Json::str("host-a"), Json::str("host-b")]));
    // A unit whose host has no document.
    let bad = doc(
        r#"{"format":1,"hosts":["host-a","host-z"],"label":"Relay","playbooks":{"deploy":"deploy-relay"},"schema":"relay"}"#,
    );
    let got = f.get("/api/environments/production/units/relay").await;
    let r = f.send(Req::put("/api/environments/production/units/relay").if_match(&got.etag()).json(&bad)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    // A unit with an unknown schema.
    let bad = doc(r#"{"format":1,"hosts":[],"label":"X","playbooks":{"deploy":"deploy-relay"},"schema":"nothing"}"#);
    let r = f.send(Req::put("/api/environments/production/units/other").create().json(&bad)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    // A unit that names a playbook with the wrong kind.
    let bad = doc(r#"{"format":1,"hosts":[],"label":"X","playbooks":{"deploy":"status-relay"},"schema":"relay"}"#);
    let r = f.send(Req::put("/api/environments/production/units/other").create().json(&bad)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    // A second unit with the same schema on host-a (2.9).
    let bad =
        doc(r#"{"format":1,"hosts":["host-a"],"label":"X","playbooks":{"deploy":"deploy-relay"},"schema":"relay"}"#);
    let r = f.send(Req::put("/api/environments/production/units/other").create().json(&bad)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    // A second unit with another schema on host-a is valid.
    let ok =
        doc(r#"{"format":1,"hosts":["host-a"],"label":"Feed","playbooks":{"deploy":"deploy-feed"},"schema":"feed"}"#);
    let r = f.send(Req::put("/api/environments/production/units/feed").create().json(&ok)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    // A host that leaves a unit that has its host_values (2.16).
    let values = doc(r#"{"format":1,"values":{"limits.max_clients":7}}"#);
    let r =
        f.send(Req::put("/api/environments/production/units/relay/hosts/host-b/values").create().json(&values)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let one = doc(
        r#"{"format":1,"hosts":["host-a"],"label":"Relay","playbooks":{"deploy":"deploy-relay"},"schema":"relay"}"#,
    );
    let got = f.get("/api/environments/production/units/relay").await;
    let r = f.send(Req::put("/api/environments/production/units/relay").if_match(&got.etag()).json(&one)).await;
    assert_eq!((r.status, r.error().as_str()), (409, "in_use"));
    f.stop().await;
}

#[tokio::test]
async fn the_units_are_bounded_by_the_limit_2_42() {
    let f = open_fixture().await;
    for id in ["u1", "u2", "u3"] {
        let ok = doc(r#"{"format":1,"hosts":[],"label":"X","playbooks":{"deploy":"deploy-relay"},"schema":"relay"}"#);
        let r = f.send(Req::put(&format!("/api/environments/production/units/{id}")).create().json(&ok)).await;
        assert_eq!(r.status, 201, "{id}: {}", r.text());
    }
    let ok = doc(r#"{"format":1,"hosts":[],"label":"X","playbooks":{"deploy":"deploy-relay"},"schema":"relay"}"#);
    let r = f.send(Req::put("/api/environments/production/units/u4").create().json(&ok)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "limit_exceeded"));
    assert_eq!(r.member("limit"), Json::str("limits.max_units_per_environment"));
    f.stop().await;
}

#[tokio::test]
async fn a_document_above_the_size_limit_is_refused_2_42() {
    let store = std::sync::Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    // The canonical bytes of the seeded documents are below 200. A unit with long labels is above.
    s.limits.max_document_bytes = 200;
    let f = start(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess), s, store).await;
    let small = doc(r#"{"address":"host-d.example.invalid","format":1,"labels":[]}"#);
    assert_eq!(f.send(Req::put("/api/environments/production/hosts/host-d").create().json(&small)).await.status, 201);
    let label = "x".repeat(60);
    let big = doc(&format!(
        r#"{{"format":1,"hosts":["host-c"],"label":"{label}","playbooks":{{"deploy":"deploy-relay","render":"render-relay","status":"status-relay"}},"schema":"relay"}}"#
    ));
    let r = f.send(Req::put("/api/environments/production/units/other").create().json(&big)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "limit_exceeded"), "{}", r.text());
    assert_eq!(r.member("limit"), Json::str("limits.max_document_bytes"));
    // Nothing was written.
    assert_eq!(f.get("/api/environments/production/units/other").await.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn the_bytes_of_the_store_are_bounded_2_42() {
    let store = std::sync::Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    // A write that goes above the total is refused.
    let probe = pupitre::server::startup::load_catalog(&*store, &s).await.unwrap();
    s.limits.max_store_bytes = probe.total_bytes() + 20;
    let f = start(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess), s, store).await;
    let r = f
        .send(
            Req::put("/api/environments/production/hosts/host-d")
                .create()
                .json(&doc(r#"{"address":"host-d.example.invalid","format":1,"labels":[]}"#)),
        )
        .await;
    assert_eq!((r.status, r.error().as_str()), (422, "limit_exceeded"), "{}", r.text());
    assert_eq!(r.member("limit"), Json::str("limits.max_store_bytes"));
    f.stop().await;
}

#[tokio::test]
async fn the_values_of_a_layer_follow_the_schema_2_10() {
    let f = open_fixture().await;
    let put = |path: &str, v: &str| Req::put(path).create().json(&doc(&format!(r#"{{"format":1,"values":{v}}}"#)));
    // A value out of range: the error has the key path, the range and the value.
    let r = f.send(put("/api/environments/production/units/relay/values", r#"{"limits.max_clients":5000}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    let errors = r.member("errors");
    let e = errors.as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(e["path"], Json::str("limits.max_clients"));
    let message = e["message"].as_str().unwrap();
    assert!(message.contains("5000"), "{message}");
    // A secret key is not a value.
    let r = f.send(put("/api/environments/production/units/relay/values", r#"{"storage.password":"12345678"}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    // A key that the schema does not have.
    let r = f.send(put("/api/environments/production/units/relay/values", r#"{"other.key":1}"#)).await;
    assert_eq!(r.status, 422);
    // A common value must pass each schema that declares the key (2.15): `trace` is in no list.
    let r = f.send(put("/api/environments/production/common", r#"{"log.level":"debug"}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    let r = f.send(put("/api/environments/production/common", r#"{"log.level":"warn"}"#)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    // A host_values document of a host that the unit does not hold: the node does not exist.
    let r = f
        .send(put("/api/environments/production/units/relay/hosts/host-c/values", r#"{"limits.max_clients":3}"#))
        .await;
    assert_eq!((r.status, r.error().as_str()), (404, "not_found"));
    f.stop().await;
}

#[tokio::test]
async fn a_get_of_a_document_that_does_not_exist_is_404() {
    let f = open_fixture().await;
    for path in [
        "/api/environments/nothing",
        "/api/environments/production/hosts/nothing",
        "/api/environments/production/units/nothing",
        "/api/environments/production/common",
        "/api/environments/production/units/relay/values",
        "/api/environments/production/units/relay/hosts/host-a/values",
        "/api/environments/BAD/units/relay",
        "/api/environments/production/units/relay/hosts/host-c/values",
    ] {
        let r = f.get(path).await;
        assert_eq!((r.status, r.error().as_str()), (404, "not_found"), "{path}");
    }
    f.stop().await;
}

#[tokio::test]
async fn the_effective_values_5_12() {
    let f = open_fixture().await;
    let put = |p: &str, v: &str| Req::put(p).create().json(&doc(&format!(r#"{{"format":1,"values":{v}}}"#)));
    assert_eq!(
        f.send(put("/api/environments/production/common", r#"{"limits.max_clients":200,"log.level":"warn"}"#))
            .await
            .status,
        201
    );
    assert_eq!(
        f.send(put("/api/environments/production/units/relay/values", r#"{"limits.max_clients":300}"#)).await.status,
        201
    );
    assert_eq!(
        f.send(put("/api/environments/production/units/relay/hosts/host-b/values", r#"{"limits.max_clients":500}"#))
            .await
            .status,
        201
    );
    let r = f
        .send(
            Req::put("/api/environments/production/units/relay/secrets/storage.password")
                .json(r#"{"value":"correct horse"}"#),
        )
        .await;
    let fingerprint = r.member("fingerprint").as_str().unwrap().to_string();
    // The view of the unit: the layers 0 to 2.
    let r = f.get("/api/environments/production/units/relay/effective").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let m = body.as_object().unwrap();
    assert_eq!(m["environment"], Json::str("production"));
    assert_eq!(m["schema"], Json::str("relay"));
    assert_eq!(m["unit"], Json::str("relay"));
    assert!(!m.contains_key("host"));
    let keys = m["keys"].as_array().unwrap();
    let key = |p: &str| {
        keys.iter().find(|k| k.as_object().unwrap()["path"] == Json::str(p)).unwrap().as_object().unwrap().clone()
    };
    assert_eq!(key("limits.max_clients")["value"], Json::Int(300));
    assert_eq!(key("limits.max_clients")["layer"], Json::str("unit"));
    let layers = key("limits.max_clients")["layers"].as_object().unwrap().clone();
    assert_eq!(layers.len(), 3);
    assert_eq!(layers["default"], Json::Int(100));
    assert_eq!(layers["environment"], Json::Int(200));
    assert_eq!(key("log.level")["layer"], Json::str("environment"));
    // The secret has a fingerprint and no value.
    let pw = key("storage.password");
    assert_eq!(pw["secret"], Json::Bool(true));
    assert!(!pw.contains_key("value"));
    assert_eq!(pw["layers"].as_object().unwrap()["unit"], Json::object([("fingerprint", Json::str(fingerprint))]));
    assert!(!r.text().contains("correct horse"));
    // The view of the unit on a host: the layers 0 to 3.
    let r = f.get("/api/environments/production/units/relay/effective?host=host-b").await;
    let body = r.json();
    let m = body.as_object().unwrap();
    assert_eq!(m["host"], Json::str("host-b"));
    let keys = m["keys"].as_array().unwrap();
    let k = keys
        .iter()
        .find(|k| k.as_object().unwrap()["path"] == Json::str("limits.max_clients"))
        .unwrap()
        .as_object()
        .unwrap();
    assert_eq!(k["value"], Json::Int(500));
    assert_eq!(k["layer"], Json::str("host"));
    // A host that is not in the unit.
    assert_eq!(f.get("/api/environments/production/units/relay/effective?host=host-c").await.status, 404);
    assert_eq!(f.get("/api/environments/production/units/relay/effective?host=BAD").await.status, 404);
    assert_eq!(f.get("/api/environments/production/units/nothing/effective").await.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn the_history_5_13() {
    let f = open_fixture().await;
    let mut etag = f.get("/api/environments/production").await.etag();
    for i in 0..3 {
        let body = format!(r#"{{"document":{{"format":1,"label":"Prod {i}"}},"message":"change {i}"}}"#);
        let r = f.send(Req::put("/api/environments/production").if_match(&etag).json(&body)).await;
        assert_eq!(r.status, 200, "{}", r.text());
        etag = r.member("etag").as_str().unwrap().to_string();
    }
    let r = f.get("/api/environments/production/history?limit=2").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let binding = r.member("items");
    let items = binding.as_array().unwrap();
    assert_eq!(items.len(), 2);
    let first = items[0].as_object().unwrap();
    assert_eq!(first["version"], Json::Int(4));
    assert_eq!(first["kind"], Json::str("save"));
    assert_eq!(first["saved_by"], Json::str("-"));
    assert_eq!(first["message"], Json::str("change 2"));
    assert_eq!(first["etag"], Json::str(etag.clone()));
    assert!(pupitre_core::time::UtcMillis::parse(first["saved_at"].as_str().unwrap()).is_some());
    assert_eq!(r.member("next"), Json::Int(3));
    let r = f.get("/api/environments/production/history?limit=2&before=3").await;
    let binding = r.member("items");
    let items = binding.as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].as_object().unwrap()["version"], Json::Int(1));
    assert!(!r.json().as_object().unwrap().contains_key("next"));
    // The bytes of a version, with its ETag.
    let v = f.get("/api/environments/production/history/2").await;
    assert_eq!(v.status, 200);
    assert!(v.text().contains("Prod 0"));
    assert_eq!(v.etag().len(), 64);
    assert_eq!(f.get("/api/environments/production/history/9").await.status, 404);
    assert_eq!(f.get("/api/environments/production/history/0").await.status, 400);
    assert_eq!(f.get("/api/environments/production/history/x").await.status, 400);
    // A key that never had a document.
    assert_eq!(f.get("/api/environments/production/common/history").await.status, 404);
    // A bound above the page size is a 400, and a bad before is a 400.
    assert_eq!(f.get("/api/environments/production/history?limit=6").await.status, 400);
    assert_eq!(f.get("/api/environments/production/history?before=0").await.status, 400);
    // A deleted version has no bytes.
    let host = f.get("/api/environments/production/hosts/host-c").await;
    f.send(Req::delete("/api/environments/production/hosts/host-c").if_match(&host.etag())).await;
    let h = f.get("/api/environments/production/hosts/host-c/history").await;
    assert_eq!(
        h.json().as_object().unwrap()["items"].as_array().unwrap()[0].as_object().unwrap()["kind"],
        Json::str("delete")
    );
    assert!(!h.json().as_object().unwrap()["items"].as_array().unwrap()[0].as_object().unwrap().contains_key("etag"));
    assert_eq!(f.get("/api/environments/production/hosts/host-c/history/2").await.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn the_secrets_5_14() {
    let f = open_fixture().await;
    let path = "/api/environments/production/units/relay/secrets/storage.password";
    let r = f.send(Req::put(path).json(r#"{"value":"hunter2hunter2"}"#)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let fp = r.member("fingerprint").as_str().unwrap().to_string();
    assert_eq!(fp.len(), 16);
    assert!(!r.text().contains("hunter2"));
    let list = f.get("/api/environments/production/units/relay/secrets").await;
    assert_eq!(list.status, 200);
    let binding = list.member("items");
    let item = binding.as_array().unwrap()[0].as_object().unwrap().clone();
    assert_eq!(item["name"], Json::str("storage.password"));
    assert_eq!(item["fingerprint"], Json::str(fp));
    assert_eq!(item["updated_by"], Json::str("-"));
    assert!(!list.text().contains("hunter2"));
    // A new value replaces the old one with no precondition.
    let r = f.send(Req::put(path).json(r#"{"value":"another value"}"#)).await;
    assert_eq!(r.status, 200);
    // The log has the scope, the name, the subject and the fingerprint, and never the value.
    let lines = f.log.events("secret.write");
    assert_eq!(lines.len(), 2);
    let m = lines[1].as_object().unwrap();
    assert_eq!(m["scope"], Json::str("environment:production/unit:relay"));
    assert_eq!(m["name"], Json::str("storage.password"));
    assert_eq!(m["subject"], Json::str("-"));
    assert!(m.contains_key("fingerprint"));
    assert!(!f.log.text().contains("hunter2") && !f.log.text().contains("another value"));
    // A short value, a name that is not a secret key, a name that is not a key path.
    let r = f.send(Req::put(path).json(r#"{"value":"short"}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_secret"));
    assert!(!r.text().contains("short\""), "the value must not be in the message");
    let r = f
        .send(
            Req::put("/api/environments/production/units/relay/secrets/limits.max_clients")
                .json(r#"{"value":"12345678"}"#),
        )
        .await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_secret"));
    let r = f
        .send(Req::put("/api/environments/production/units/relay/secrets/Bad-Name").json(r#"{"value":"12345678"}"#))
        .await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_secret"));
    // The name `units` is the folder of the unit scopes in the layout of 2.31.
    let r = f.send(Req::put("/api/environments/production/secrets/units").json(r#"{"value":"12345678"}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_secret"));
    assert!(r.text().contains("reserved"), "{}", r.text());
    // The body has the member `value` only.
    let r = f.send(Req::put(path).json(r#"{"value":"12345678","x":1}"#)).await;
    assert_eq!(r.status, 400);
    let r = f.send(Req::put(path).json(r#"{"value":5}"#)).await;
    assert_eq!(r.status, 400);
    // The scope of an environment needs a secret key of at least one schema.
    let r =
        f.send(Req::put("/api/environments/production/secrets/storage.password").json(r#"{"value":"12345678"}"#)).await;
    assert_eq!(r.status, 200);
    // The secrets of an unknown unit.
    let r = f
        .send(
            Req::put("/api/environments/production/units/nothing/secrets/storage.password")
                .json(r#"{"value":"12345678"}"#),
        )
        .await;
    assert_eq!(r.status, 404);
    // The delete.
    let r = f.send(Req::delete(path)).await;
    assert_eq!((r.status, r.text().as_str()), (200, "{}"));
    let r = f.send(Req::delete(path)).await;
    assert_eq!((r.status, r.error().as_str()), (404, "not_found"));
    let r = f.send(Req::delete("/api/environments/production/units/relay/secrets/Bad-Name")).await;
    assert_eq!(r.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn the_pages_and_the_static_files_5_21_and_5_23() {
    let f = open_fixture().await;
    let build = f.state.assets.build.clone();
    assert_eq!(build.len(), 12);
    for page in [
        "/",
        "/e/production",
        "/e/production/units/relay",
        "/e/production/units/relay/hosts/host-a",
        "/e/production/hosts/host-a",
        "/e/production/units/relay/history",
        "/e/production/units/relay/run",
        "/e/production/commands",
        "/e/production/commands/restart-service",
        "/runs",
        "/runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42",
    ] {
        let r = f.get(page).await;
        assert_eq!(r.status, 200, "{page}");
        assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
        assert_eq!(r.header("cache-control"), Some("no-cache"), "{page}");
        assert!(r.text().contains(&format!("static/{build}/js/app.js")), "the shell has the build id");
        assert!(r.text().contains("<base href=\"/\">"));
        assert!(!r.text().contains("{{"));
    }
    // A path that no page names: the shell with the status 404.
    let r = f.get("/nothing/here").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    // The files below /static/<build>/.
    let r = f.get(&format!("/static/{build}/css/base.css")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("cache-control"), Some("private, max-age=31536000, immutable"));
    assert_eq!(r.header("content-type"), Some("text/css; charset=utf-8"));
    assert_eq!(r.text(), "body { margin: 0; }\n");
    assert_eq!(f.get(&format!("/static/{build}/css/none.css")).await.status, 404);
    assert_eq!(f.get("/static/000000000000/css/base.css").await.status, 404);
    // The static route never reads a file outside the list: no `..` and no absolute path.
    assert_eq!(f.get(&format!("/static/{build}/../Cargo.toml")).await.status, 404);
    // The shell has no data and the run pages go with the runner.
    let r = f.get("/api/nothing").await;
    assert_eq!((r.status, r.error().as_str()), (404, "not_found"));
    assert!(r.header("content-type").unwrap().starts_with("application/json"));
    f.stop().await;
}

#[tokio::test]
async fn a_page_with_a_query_string_gives_the_shell_5_21() {
    // The page keeps its state in the address (`?tab=`, `?check=`, `?new=1`), so a reload must
    // give the shell and never a JSON error.
    let f = open_fixture().await;
    for page in [
        "/?new=1",
        "/e/production?tab=hosts",
        "/e/production?tab=units&new=1",
        "/e/production/units/relay/run?check=0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42",
        "/e/production/units/relay/history?doc=unit",
        "/runs?environment=production&status=failed",
        "/e/production?",
        "/e/production?x=1&x=2&%zz",
    ] {
        for r in [f.get(page).await, f.send(Req::head(page)).await] {
            assert_eq!(r.status, 200, "{page}: {}", r.text());
            assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"), "{page}");
        }
    }
    // A path that no page names keeps the status 404 and the shell, with or without a query.
    for page in ["/nothing/here", "/nothing/here?tab=x"] {
        let r = f.get(page).await;
        assert_eq!((r.status, r.header("content-type")), (404, Some("text/html; charset=utf-8")), "{page}");
    }
    // An API route and the static files keep the strict query of 5.3.
    assert_eq!(f.get("/api/session?tab=x").await.status, 400);
    let build = f.state.assets.build.clone();
    assert_eq!(f.get(&format!("/static/{build}/css/base.css?x=1")).await.status, 400);
    f.stop().await;
}

#[tokio::test]
async fn the_run_pages_are_off_without_runner_3_3() {
    let store = std::sync::Arc::new(MemStore::new());
    let mut s = settings(false);
    s.runner = None;
    let f = start(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess), s, store).await;
    for page in [
        "/runs",
        "/runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42",
        "/e/production/commands",
        "/e/production/commands/x",
        "/e/production/units/relay/run",
    ] {
        let r = f.get(page).await;
        assert_eq!((r.status, r.error().as_str()), (404, "not_found"), "{page}");
    }
    assert_eq!(f.get("/").await.status, 200);
    for path in
        ["/api/runs", "/api/runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42", "/api/runs/x/events", "/api/runs/x/output"]
    {
        assert_eq!(f.get(path).await.status, 404, "{path}");
    }
    assert_eq!(f.send(Req::post("/api/runs").json("{}")).await.status, 404);
    assert_eq!(f.send(Req::post("/api/runs/x/cancel")).await.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn the_two_procurauth_routes_are_404_in_the_mode_open_5_19() {
    let f = open_fixture().await;
    for path in ["/.well-known/procurauth/actions", "/.well-known/procurauth/resources?type=environment"] {
        let r = f.get(path).await;
        assert_eq!((r.status, r.error().as_str()), (404, "not_found"), "{path}");
    }
    f.stop().await;
}

#[tokio::test]
async fn the_common_rules_5_3() {
    let f = open_fixture().await;
    // A known path with another method: 405 and Allow.
    let r = f.send(Req::method("/api/session", "POST")).await;
    assert_eq!((r.status, r.error().as_str()), (405, "method_not_allowed"));
    assert_eq!(r.header("allow"), Some("GET, HEAD"));
    let r = f.send(Req::method("/api/environments/production", "PATCH")).await;
    assert_eq!(r.header("allow"), Some("DELETE, GET, HEAD, PUT"));
    // OPTIONS gives 405 with no CORS header.
    let r = f.send(Req::method("/api/session", "OPTIONS").header("Origin", "http://other.example.invalid")).await;
    assert_eq!(r.status, 405);
    assert!(r.headers.iter().all(|(n, _)| !n.to_ascii_lowercase().starts_with("access-control-")));
    // HEAD of a GET route: the headers of the GET and no body.
    let get = f.get("/api/session").await;
    let head = f.send(Req::head("/api/session")).await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.header("content-length"), Some(get.body.len().to_string().as_str()));
    assert_eq!(head.header("content-type"), get.header("content-type"));
    // A parameter that the route does not name, and a parameter two times.
    assert_eq!(f.get("/api/session?x=1").await.status, 400);
    assert_eq!(f.get("/api/environments?limit=1&limit=2").await.error(), "bad_request");
    assert_eq!(f.get("/api/environments?limit=1&other=2").await.status, 400);
    assert_eq!(f.get("/api/environments?after=%zz").await.status, 400);
    // A bad limit is never changed.
    for bad in ["0", "6", "-1", "x", "1.5", ""] {
        assert_eq!(f.get(&format!("/api/environments?limit={bad}")).await.status, 400, "limit={bad:?}");
    }
    assert_eq!(f.get("/api/environments?limit=5").await.status, 200);
    // A cursor that the route did not make.
    assert_eq!(f.get("/api/environments?after=abc").await.status, 400);
    assert_eq!(f.get("/api/environments/production/hosts?after=abc").await.status, 400);
    // A body on a route that has none.
    let r = f.send(Req::method("/api/session", "GET").json("{}")).await;
    assert_eq!(r.status, 400);
    f.stop().await;
}

#[tokio::test]
async fn each_answer_has_the_headers_of_7_5_and_a_request_id_5_3() {
    let f = open_fixture().await;
    let mut ids = std::collections::BTreeSet::new();
    for r in [
        f.get("/api/session").await,
        f.get("/api/environments/nothing").await,
        f.get("/").await,
        f.get("/api/nothing").await,
        f.send(Req::method("/api/session", "POST")).await,
        f.send(Req::put("/api/environments/production").json("{}")).await,
    ] {
        for (name, want) in [
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "same-origin"),
            ("cross-origin-resource-policy", "same-origin"),
            ("cross-origin-opener-policy", "same-origin"),
            ("permissions-policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
        ] {
            assert_eq!(r.header(name), Some(want), "{name} on {}", r.status);
        }
        let csp = r.header("content-security-policy").unwrap();
        assert!(csp.starts_with("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:;"));
        assert!(!csp.contains("unsafe-inline"));
        assert!(r.header("set-cookie").is_none());
        assert!(r.headers.iter().all(|(n, _)| !n.to_ascii_lowercase().starts_with("access-control-")));
        let id = r.header("x-request-id").unwrap().to_string();
        assert_eq!(id.len(), 32);
        assert!(ids.insert(id));
    }
    // A client cannot choose its request id.
    let r = f.send(Req::get("/api/session").header("X-Request-Id", "my-own-id")).await;
    assert_ne!(r.header("x-request-id"), Some("my-own-id"));
    f.stop().await;
}

#[tokio::test]
async fn the_log_lines_of_a_request_and_a_write_6_26_and_7_9() {
    let f = open_fixture().await;
    let etag = f.get("/api/environments/production").await.etag();
    let r = f
        .send(
            Req::put("/api/environments/production")
                .if_match(&etag)
                .json(&doc(r#"{"format":1,"label":"Secret label"}"#)),
        )
        .await;
    assert_eq!(r.status, 200);
    let id = r.header("x-request-id").unwrap().to_string();
    let lines = f.log.events("http.request");
    let line = lines
        .iter()
        .find(|l| l.as_object().unwrap()["request_id"] == Json::str(id.clone()))
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(line["method"], Json::str("PUT"));
    assert_eq!(line["route"], Json::str("PUT /api/environments/{e}"));
    assert_eq!(line["status"], Json::Int(200));
    assert_eq!(line["subject"], Json::str("-"));
    assert!(line.contains_key("duration_ms"));
    let w = f.log.events("document.write");
    let w = w[0].as_object().unwrap();
    assert_eq!(w["key"], Json::str("environments/production"));
    assert_eq!(w["kind"], Json::str("environment"));
    assert_eq!(w["version"], Json::Int(2));
    assert_eq!(w["etag_before"], Json::str(etag));
    assert_eq!(w["etag_after"], r.member("etag"));
    assert_eq!(w["request_id"], Json::str(id));
    assert!(!f.log.text().contains("Secret label"), "a log line never holds a value of a document");
    f.stop().await;
}

#[tokio::test]
async fn the_changed_key_paths_of_a_write_are_logged_without_values_7_9() {
    let f = open_fixture().await;
    let r = f
        .send(
            Req::put("/api/environments/production/common")
                .create()
                .json(&doc(r#"{"format":1,"values":{"limits.max_clients":11,"log.level":"warn"}}"#)),
        )
        .await;
    assert_eq!(r.status, 201);
    let etag = r.member("etag").as_str().unwrap().to_string();
    let r = f
        .send(
            Req::put("/api/environments/production/common")
                .if_match(&etag)
                .json(&doc(r#"{"format":1,"values":{"limits.max_clients":12,"log.level":"warn"}}"#)),
        )
        .await;
    assert_eq!(r.status, 200);
    let lines = f.log.events("document.write");
    assert_eq!(
        lines[0].as_object().unwrap()["paths"],
        Json::Arr(vec![Json::str("limits.max_clients"), Json::str("log.level")])
    );
    assert_eq!(lines[1].as_object().unwrap()["paths"], Json::Arr(vec![Json::str("limits.max_clients")]));
    assert!(!f.log.text().contains("\"value\""));
    f.stop().await;
}

#[tokio::test]
async fn the_body_of_an_answer_is_compact_json_5_3() {
    let f = open_fixture().await;
    for path in ["/api/session", "/api/environments", "/api/catalog", "/api/environments/production/hosts"] {
        let r = f.get(path).await;
        let text = r.text();
        assert!(!text.contains('\n') && !text.contains(": ") && !text.contains(", "), "{path}: {text}");
    }
    let r = f.get("/api/environments/nothing").await;
    assert_eq!(r.text(), r#"{"error":"not_found","message":"No such item.","retryable":false}"#);
    f.stop().await;
}

#[tokio::test]
async fn two_saves_with_one_base_etag_make_one_winner_2_20() {
    let f = open_fixture().await;
    let etag = f.get("/api/environments/production").await.etag();
    let a = Req::put("/api/environments/production").if_match(&etag).json(&doc(r#"{"format":1,"label":"A"}"#));
    let b = Req::put("/api/environments/production").if_match(&etag).json(&doc(r#"{"format":1,"label":"B"}"#));
    let (ra, rb) = tokio::join!(f.send(a), f.send(b));
    let mut statuses = [ra.status, rb.status];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 409]);
    let loser = if ra.status == 409 { &ra } else { &rb };
    assert_eq!(loser.error(), "conflict");
    f.stop().await;
}

#[tokio::test]
async fn a_delete_of_a_unit_that_has_secrets_is_refused() {
    let f = open_fixture().await;
    let r = f
        .send(
            Req::put("/api/environments/production/units/relay/secrets/storage.password")
                .json(r#"{"value":"12345678"}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    let u = f.get("/api/environments/production/units/relay").await;
    let r = f.send(Req::delete("/api/environments/production/units/relay").if_match(&u.etag())).await;
    assert_eq!((r.status, r.error().as_str()), (409, "not_empty"));
    f.stop().await;
}

#[tokio::test]
async fn a_document_that_fails_its_checks_is_shown_with_its_errors_2_41() {
    let f = open_fixture().await;
    let key = pupitre_core::doc::DocKey::parse("environments/production/hosts/host-c").unwrap();
    // A person edited the file by hand and broke the canonical form.
    f.store.external_put(&key, br#"{"address":"203.0.113.12","format":1,"labels":[]}"#.to_vec());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let list = f.get("/api/environments/production/hosts").await;
    let binding = list.member("items");
    let item = binding
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i.as_object().unwrap()["id"] == Json::str("host-c"))
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    assert!(item.contains_key("errors"), "{item:?}");
    assert!(!item.contains_key("document"));
    assert_eq!(item["version"], Json::Int(2));
    // The document itself gives its errors, and a save with its etag repairs it.
    let r = f.get("/api/environments/production/hosts/host-c").await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_document"));
    let etag = item["etag"].as_str().unwrap().to_string();
    let fixed = doc(r#"{"address":"203.0.113.12","format":1,"labels":[]}"#);
    let r = f.send(Req::put("/api/environments/production/hosts/host-c").if_match(&etag).json(&fixed)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(f.get("/api/environments/production/hosts/host-c").await.status, 200);
    f.stop().await;
}
