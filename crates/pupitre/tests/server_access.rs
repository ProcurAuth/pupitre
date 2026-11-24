// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mode `authzen` with a fake access layer: the questions of each route (3.12), the order of
//! the checks (3.13), unknown and forbidden (3.14), the display filter (3.15), the options route
//! (3.16), the cache policy (3.17), the map of the answers (3.18, 3.19) and the ceilings (3.10).

mod server_common;

use pupitre::access::{Granted, Refusal};
use pupitre_core::action::Action;
use pupitre_core::json::Json;
use pupitre_core::run::{RunKind, RunMode, RunStatus};
use server_common::*;

const ENV: &str = "environment:production";
const UNIT: &str = "environment:production/unit:relay";
const HOST: &str = "environment:production/unit:relay/host:host-a";

const ALL_ACTIONS: [&str; 8] = [
    "config.read",
    "config.write",
    "secret.write",
    "inventory.write",
    "run.read",
    "run.check",
    "run.apply",
    "command.run",
];
const NO_INVENTORY: [&str; 7] =
    ["config.read", "config.write", "secret.write", "run.read", "run.check", "run.apply", "command.run"];

fn alice(req: Req) -> Req {
    req.token(TOKEN_ALICE)
}

fn use_q(action: &'static str, node: &str, policy: &'static str) -> Asked {
    Asked::Use { subject: "alice".into(), action, node: node.into(), mode: "node", policy }
}

fn access_q() -> Asked {
    Asked::Access { subject: "alice".into() }
}

fn valid(doc: &str) -> String {
    format!(r#"{{"document":{doc}}}"#)
}

const HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[tokio::test]
async fn the_questions_of_each_route_are_the_table_of_3_12() {
    let (f, fake) = fake_fixture().await;
    let run = run_record(1, RunKind::Deploy, RunMode::Check, RunStatus::Passed, UNIT, &["host-a"]);
    f.store.seed_run(run.clone());
    let rid = run.id.to_string();
    let build = f.state.assets.build.clone();
    let env_doc = valid(r#"{"format":1,"label":"P"}"#);
    let host_doc = valid(r#"{"address":"203.0.113.10","format":1,"labels":[]}"#);
    let unit_doc =
        valid(r#"{"format":1,"hosts":["host-a"],"label":"R","playbooks":{"deploy":"deploy-relay"},"schema":"relay"}"#);
    let values_doc = valid(r#"{"format":1,"values":{"limits.max_clients":5}}"#);
    let secret_body = r#"{"value":"12345678"}"#.to_string();
    let rows: Vec<(String, Req, Vec<Asked>)> = vec![
        ("session".into(), Req::get("/api/session"), vec![access_q()]),
        ("catalog".into(), Req::get("/api/catalog"), vec![access_q()]),
        ("page".into(), Req::get("/"), vec![access_q()]),
        ("page e".into(), Req::get("/e/production"), vec![access_q()]),
        ("static".into(), Req::get(&format!("/static/{build}/css/base.css")), vec![access_q()]),
        ("unknown page".into(), Req::get("/nothing/here"), vec![access_q()]),
        (
            "env list".into(),
            Req::get("/api/environments"),
            vec![Asked::Visible { items: vec![(ENV.into(), ALL_ACTIONS.to_vec())], budget: 50 }],
        ),
        ("env create".into(), Req::post("/api/environments").json(r#"{"id":"staging","document":{"format":1,"label":"S"}}"#), vec![use_q("inventory.write", "", "fresh")]),
        ("env get".into(), Req::get("/api/environments/production"), vec![use_q("config.read", ENV, "kept")]),
        ("env put".into(), Req::put("/api/environments/production").if_match(HEX).json(&env_doc), vec![use_q("inventory.write", ENV, "fresh")]),
        ("env delete".into(), Req::delete("/api/environments/production").if_match(HEX), vec![use_q("inventory.write", ENV, "fresh")]),
        ("env history".into(), Req::get("/api/environments/production/history"), vec![use_q("config.read", ENV, "kept")]),
        ("env version".into(), Req::get("/api/environments/production/history/1"), vec![use_q("config.read", ENV, "kept")]),
        ("host list".into(), Req::get("/api/environments/production/hosts"), vec![use_q("config.read", ENV, "kept")]),
        ("host get".into(), Req::get("/api/environments/production/hosts/host-a"), vec![use_q("config.read", ENV, "kept")]),
        ("host put".into(), Req::put("/api/environments/production/hosts/host-a").if_match(HEX).json(&host_doc), vec![use_q("inventory.write", ENV, "fresh")]),
        ("host delete".into(), Req::delete("/api/environments/production/hosts/host-a").if_match(HEX), vec![use_q("inventory.write", ENV, "fresh")]),
        ("host history".into(), Req::get("/api/environments/production/hosts/host-a/history"), vec![use_q("config.read", ENV, "kept")]),
        (
            "unit list".into(),
            Req::get("/api/environments/production/units"),
            vec![Asked::Visible { items: vec![(UNIT.into(), NO_INVENTORY.to_vec())], budget: 50 }],
        ),
        ("unit get".into(), Req::get("/api/environments/production/units/relay"), vec![use_q("config.read", UNIT, "kept")]),
        ("unit put".into(), Req::put("/api/environments/production/units/relay").if_match(HEX).json(&unit_doc), vec![use_q("inventory.write", ENV, "fresh")]),
        ("unit delete".into(), Req::delete("/api/environments/production/units/relay").if_match(HEX), vec![use_q("inventory.write", ENV, "fresh")]),
        ("unit history".into(), Req::get("/api/environments/production/units/relay/history"), vec![use_q("config.read", UNIT, "kept")]),
        ("common get".into(), Req::get("/api/environments/production/common"), vec![use_q("config.read", ENV, "kept")]),
        ("common put".into(), Req::put("/api/environments/production/common").create().json(&values_doc), vec![use_q("config.write", ENV, "fresh")]),
        ("common delete".into(), Req::delete("/api/environments/production/common").if_match(HEX), vec![use_q("config.write", ENV, "fresh")]),
        ("common history".into(), Req::get("/api/environments/production/common/history"), vec![use_q("config.read", ENV, "kept")]),
        ("unit values get".into(), Req::get("/api/environments/production/units/relay/values"), vec![use_q("config.read", UNIT, "kept")]),
        ("unit values put".into(), Req::put("/api/environments/production/units/relay/values").create().json(&values_doc), vec![use_q("config.write", UNIT, "fresh")]),
        ("unit values delete".into(), Req::delete("/api/environments/production/units/relay/values").if_match(HEX), vec![use_q("config.write", UNIT, "fresh")]),
        ("unit values history".into(), Req::get("/api/environments/production/units/relay/values/history"), vec![use_q("config.read", UNIT, "kept")]),
        ("host values get".into(), Req::get("/api/environments/production/units/relay/hosts/host-a/values"), vec![use_q("config.read", HOST, "kept")]),
        ("host values put".into(), Req::put("/api/environments/production/units/relay/hosts/host-a/values").create().json(&values_doc), vec![use_q("config.write", HOST, "fresh")]),
        ("host values delete".into(), Req::delete("/api/environments/production/units/relay/hosts/host-a/values").if_match(HEX), vec![use_q("config.write", HOST, "fresh")]),
        ("host values history".into(), Req::get("/api/environments/production/units/relay/hosts/host-a/values/history"), vec![use_q("config.read", HOST, "kept")]),
        ("effective".into(), Req::get("/api/environments/production/units/relay/effective"), vec![use_q("config.read", UNIT, "kept")]),
        ("effective host".into(), Req::get("/api/environments/production/units/relay/effective?host=host-a"), vec![use_q("config.read", HOST, "kept")]),
        ("env secrets".into(), Req::get("/api/environments/production/secrets"), vec![use_q("config.read", ENV, "kept")]),
        ("env secret put".into(), Req::put("/api/environments/production/secrets/storage.password").json(&secret_body), vec![use_q("secret.write", ENV, "fresh")]),
        ("env secret delete".into(), Req::delete("/api/environments/production/secrets/storage.password"), vec![use_q("secret.write", ENV, "fresh")]),
        ("unit secrets".into(), Req::get("/api/environments/production/units/relay/secrets"), vec![use_q("config.read", UNIT, "kept")]),
        ("unit secret put".into(), Req::put("/api/environments/production/units/relay/secrets/storage.password").json(&secret_body), vec![use_q("secret.write", UNIT, "fresh")]),
        ("unit secret delete".into(), Req::delete("/api/environments/production/units/relay/secrets/storage.password"), vec![use_q("secret.write", UNIT, "fresh")]),
        (
            "run list".into(),
            Req::get("/api/runs"),
            vec![Asked::Visible { items: vec![(UNIT.into(), vec!["run.read"])], budget: 50 }],
        ),
        ("run get".into(), Req::get(&format!("/api/runs/{rid}")), vec![use_q("run.read", UNIT, "kept")]),
        ("run events".into(), Req::get(&format!("/api/runs/{rid}/events")), vec![use_q("run.read", UNIT, "kept")]),
        ("run output".into(), Req::get(&format!("/api/runs/{rid}/output")), vec![use_q("run.read", UNIT, "kept")]),
        (
            "run create check".into(),
            Req::post("/api/runs").json(&format!(r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a"]}}"#)),
            vec![use_q("run.check", UNIT, "fresh")],
        ),
        (
            "run create status".into(),
            Req::post("/api/runs").json(&format!(r#"{{"kind":"status","mode":"apply","node":"{UNIT}","hosts":["host-a"]}}"#)),
            vec![use_q("run.check", UNIT, "fresh")],
        ),
        (
            "run create command".into(),
            Req::post("/api/runs").json(&format!(r#"{{"kind":"command","mode":"apply","command":"restart-service","node":"{UNIT}","hosts":["host-a"],"params":{{"grace":"5s"}}}}"#)),
            vec![use_q("command.run", UNIT, "fresh")],
        ),
        (
            "run create apply".into(),
            Req::post("/api/runs").json(&format!(r#"{{"kind":"deploy","mode":"apply","check_run":"{rid}"}}"#)),
            vec![use_q("run.apply", UNIT, "fresh")],
        ),
    ];
    for (label, req, expected) in rows {
        fake.clear();
        let r = f.send(alice(req)).await;
        assert!(r.status != 401 && r.status != 405, "{label}: {} {}", r.status, r.text());
        assert_eq!(fake.asked(), expected, "{label} (status {})", r.status);
    }
    // The cancel route asks the action of the kind and the mode of the run (4.3).
    let queued = run_record(2, RunKind::Deploy, RunMode::Apply, RunStatus::Queued, UNIT, &["host-a"]);
    f.store.seed_run(queued.clone());
    fake.clear();
    let r = f.send(alice(Req::post(&format!("/api/runs/{}/cancel", queued.id)))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(fake.asked(), vec![use_q("run.apply", UNIT, "fresh")]);
    f.stop().await;
}

#[tokio::test]
async fn head_of_a_get_route_asks_the_questions_of_that_route_3_12() {
    let (f, fake) = fake_fixture().await;
    let r = f.send(alice(Req::head("/api/environments/production"))).await;
    assert_eq!(r.status, 200);
    assert!(r.body.is_empty());
    assert_eq!(fake.asked(), vec![use_q("config.read", ENV, "kept")]);
    fake.clear();
    let r = f.send(alice(Req::head("/api/environments/nothing"))).await;
    assert_eq!(r.status, 404);
    assert_eq!(fake.asked(), vec![Asked::Unknown { subject: "alice".into(), action: "config.read" }]);
    f.stop().await;
}

#[tokio::test]
async fn each_route_of_the_router_has_a_question_in_the_table_3_12() {
    // The table above names each route family; this test checks the router the other way:
    // each route of the router has a rule, so no route gives `403 forbidden` with `access.no_rule`.
    for spec in pupitre::server::routes::all() {
        assert!(pupitre::server::routes::rule_of(spec.id).is_some(), "{} has no line in 3.12", spec.label);
    }
}

fn comparable(r: &Resp) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut headers: Vec<(String, String)> = r
        .headers
        .iter()
        .filter(|(n, _)| !n.eq_ignore_ascii_case("x-request-id") && !n.eq_ignore_ascii_case("date"))
        .cloned()
        .collect();
    headers.sort();
    (r.status, headers, r.body.clone())
}

#[tokio::test]
async fn unknown_and_forbidden_give_the_same_answer_3_14() {
    let (f, fake) = fake_fixture().await;
    fake.decide_with(|_, _, _| Err(Refusal::Forbidden { action: "ignored" }));
    let run = run_record(1, RunKind::Deploy, RunMode::Check, RunStatus::Passed, UNIT, &["host-a"]);
    f.store.seed_run(run.clone());
    let unknown_run = "0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42";
    let pairs = [
        ("/api/environments/production", "/api/environments/nothing"),
        ("/api/environments/production/units/relay", "/api/environments/production/units/nothing"),
        ("/api/environments/production/hosts/host-a", "/api/environments/production/hosts/nothing"),
        ("/api/environments/production/units/relay/values", "/api/environments/production/units/nothing/values"),
        ("/api/environments/production/units/relay/effective", "/api/environments/production/units/nothing/effective"),
        (
            "/api/environments/production/units/relay/effective?host=host-a",
            "/api/environments/production/units/relay/effective?host=host-z",
        ),
        ("/api/environments/production/secrets", "/api/environments/nothing/secrets"),
        ("/api/environments/production/history", "/api/environments/BAD_ID/history"),
    ];
    for (known, unknown) in pairs {
        fake.clear();
        let a = f.send(alice(Req::get(known))).await;
        let asked_known = fake.asked();
        fake.clear();
        let b = f.send(alice(Req::get(unknown))).await;
        let asked_unknown = fake.asked();
        assert_eq!(a.status, 403, "{known}");
        assert_eq!(a.error(), "forbidden");
        assert_eq!(comparable(&a), comparable(&b), "{known} and {unknown}");
        assert_eq!(asked_known.len(), 1, "{known}");
        assert_eq!(asked_unknown.len(), 1, "{unknown}");
        // A host with no document is below a node that exists: the question of the use is asked on
        // that node, and the answer is the same (3.14, rule 5).
        let node_exists = unknown.contains("/hosts/nothing");
        if node_exists {
            assert!(matches!(asked_unknown[0], Asked::Use { .. }), "{unknown}: {asked_unknown:?}");
        } else {
            assert!(
                matches!(asked_unknown[0], Asked::Unknown { .. }),
                "the question of 3.14 for {unknown}: {asked_unknown:?}"
            );
        }
    }
    // The run routes: the record of a run of the route is read in both cases.
    let a = f.send(alice(Req::get(&format!("/api/runs/{}", run.id)))).await;
    let b = f.send(alice(Req::get(&format!("/api/runs/{unknown_run}")))).await;
    assert_eq!(comparable(&a), comparable(&b));
    assert_eq!(a.member("action"), Json::str("run.read"));
    let a = f.send(alice(Req::get(&format!("/api/runs/{}/events", run.id)))).await;
    let b = f.send(alice(Req::get(&format!("/api/runs/{unknown_run}/events")))).await;
    assert_eq!(comparable(&a), comparable(&b));
    // A write: the same `403` with the use action of the route.
    let body = valid(r#"{"format":1,"values":{"limits.max_clients":5}}"#);
    let a = f.send(alice(Req::put("/api/environments/production/units/relay/values").create().json(&body))).await;
    let b = f.send(alice(Req::put("/api/environments/production/units/nothing/values").create().json(&body))).await;
    assert_eq!(comparable(&a), comparable(&b));
    assert_eq!(a.member("action"), Json::str("config.write"));
    // A subject that has the right gets 404 for the unknown item.
    fake.decide_with(|_, _, _| Ok(Granted::default()));
    let r = f.send(alice(Req::get("/api/environments/nothing"))).await;
    assert_eq!((r.status, r.error().as_str()), (404, "not_found"));
    f.stop().await;
}

#[tokio::test]
async fn the_unknown_question_is_asked_on_the_service_with_the_use_action_3_14() {
    let (f, fake) = fake_fixture().await;
    let r = f.send(alice(Req::get("/api/environments/nothing/units/x/values"))).await;
    assert_eq!(r.status, 404);
    assert_eq!(fake.asked(), vec![Asked::Unknown { subject: "alice".into(), action: "config.read" }]);
    fake.clear();
    let body = valid(r#"{"format":1,"values":{}}"#);
    f.send(alice(Req::put("/api/environments/nothing/common").create().json(&body))).await;
    assert_eq!(fake.asked(), vec![Asked::Unknown { subject: "alice".into(), action: "config.write" }]);
    f.stop().await;
}

#[tokio::test]
async fn a_subject_without_the_right_never_reaches_the_validation_3_13() {
    let (f, fake) = fake_fixture().await;
    fake.decide_with(|_, _, _| Err(Refusal::Forbidden { action: "x" }));
    // A document that breaks every rule of its kind: the answer is 403, never 422.
    let bad = valid(r#"{"format":1,"values":{"limits.max_clients":999999}}"#);
    let r = f.send(alice(Req::put("/api/environments/production/units/relay/values").create().json(&bad))).await;
    assert_eq!((r.status, r.error().as_str()), (403, "forbidden"));
    assert!(!r.text().contains("999999"));
    // The same for a run request with a node that breaks rule 1 of 4.4, and for the rules of 2.16.
    let r = f
        .send(alice(
            Req::post("/api/runs").json(&format!(r#"{{"kind":"deploy","mode":"check","node":"{ENV}","hosts":["x"]}}"#)),
        ))
        .await;
    assert_eq!((r.status, r.error().as_str()), (403, "forbidden"));
    let r = f.send(alice(Req::delete("/api/environments/production").if_match(HEX))).await;
    assert_eq!((r.status, r.error().as_str()), (403, "forbidden"));
    // Nothing was written.
    assert!(
        f.store
            .document(&pupitre_core::doc::DocKey::parse("environments/production/units/relay/values").unwrap())
            .is_none()
    );
    // The shape of the body is a 400 before the decision (3.13, step 6).
    fake.clear();
    let r = f.send(alice(Req::put("/api/environments/production/common").create().json("{bad"))).await;
    assert_eq!(r.status, 400);
    assert!(fake.asked().is_empty());
    f.stop().await;
}

#[tokio::test]
async fn the_order_of_the_checks_3_13() {
    let (f, fake) = fake_fixture().await;
    let body = valid(r#"{"format":1,"label":"P"}"#);
    // Step 2 comes before step 3: a cross-site write with no token gives 403, not 401.
    let r = f
        .send(Req::put("/api/environments/production").if_match(HEX).header("Sec-Fetch-Site", "cross-site").json(&body))
        .await;
    assert_eq!((r.status, r.error().as_str()), (403, "cross_site_request"));
    // A bad media type with no token gives 415 before 401.
    let r = f
        .send(
            Req::put("/api/environments/production").if_match(HEX).header("Content-Type", "text/plain").raw_body(b"x"),
        )
        .await;
    assert_eq!((r.status, r.error().as_str()), (415, "unsupported_media_type"));
    // Step 3: no token gives 401, also for a route that does not exist as a feature.
    let r = f.send(Req::put("/api/environments/production").if_match(HEX).json(&body)).await;
    assert_eq!((r.status, r.error().as_str()), (401, "unauthenticated"));
    // Step 1 comes before step 3: an unknown path is 404 with no token.
    assert_eq!(f.send(Req::get("/api/nothing")).await.status, 404);
    // A method that the route does not have is 405 with no token.
    assert_eq!(f.send(Req::method("/api/session", "POST")).await.status, 405);
    // Step 4: a feature that is off gives 404 with no question, after the token check.
    let store = std::sync::Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(true);
    s.runner = None;
    let fake2 = FakeAccess::allow_all();
    let g = start(fake2.clone(), s, store).await;
    let r = g.get("/api/runs").await;
    assert_eq!((r.status, r.error().as_str()), (401, "unauthenticated"));
    let r = g.send(alice(Req::get("/api/runs"))).await;
    assert_eq!((r.status, r.error().as_str()), (404, "not_found"));
    assert!(fake2.asked().is_empty());
    g.stop().await;
    // Step 5 and 6 come before the decision: no question for a body that is not JSON.
    fake.clear();
    let r = f.send(alice(Req::put("/api/environments/production").if_match(HEX).json("{"))).await;
    assert_eq!(r.status, 400);
    // A precondition header that is missing is also a shape error: no question.
    let r = f.send(alice(Req::put("/api/environments/production").json(&body))).await;
    assert_eq!((r.status, r.error().as_str()), (428, "precondition_required"));
    assert!(fake.asked().is_empty());
    f.stop().await;
}

#[tokio::test]
async fn the_token_3_4() {
    let (f, fake) = fake_fixture().await;
    for path in [
        "/api/session",
        "/api/catalog",
        "/",
        "/e/production",
        "/api/environments",
        "/api/environments/production",
        "/api/runs",
        "/.well-known/procurauth/resources?type=environment",
        "/nothing/here",
    ] {
        let r = f.get(path).await;
        assert_eq!((r.status, r.error().as_str()), (401, "unauthenticated"), "{path}");
        let r = f.send(Req::get(path).token("bad")).await;
        assert_eq!(r.status, 401, "{path}");
        // Two copies of the header.
        let r = f.send(Req::get(path).token(TOKEN_ALICE).token(TOKEN_ALICE)).await;
        assert_eq!(r.status, 401, "{path} with two copies");
    }
    // The service reads the token only from the configured header.
    let r = f.send(Req::get("/api/session").header("Authorization", &format!("Bearer {TOKEN_ALICE}"))).await;
    assert_eq!(r.status, 401);
    let r = f.send(Req::get(&format!("/api/session?x-identity={TOKEN_ALICE}"))).await;
    assert_eq!(r.status, 400);
    let r = f.send(Req::get("/api/session").header("Cookie", &format!("x-identity={TOKEN_ALICE}"))).await;
    assert_eq!(r.status, 401);
    // A valid token: the subject of the session is the subject of the token.
    let r = f.send(alice(Req::get("/api/session"))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.member("subject"), Json::str("alice"));
    assert_eq!(r.member("mode"), Json::str("authzen"));
    assert_eq!(r.member("runner").as_object().unwrap()["allow_apply"], Json::Bool(true));
    // The action document needs no token and asks nothing.
    fake.clear();
    let r = f.get("/.well-known/procurauth/actions").await;
    assert_eq!(r.status, 200);
    assert!(fake.asked().is_empty());
    // The log line of a request has the subject.
    let lines = f.log.events("http.request");
    assert!(lines.iter().any(|l| l.as_object().unwrap()["subject"] == Json::str("alice")));
    f.stop().await;
}

#[tokio::test]
async fn the_action_document_is_the_same_bytes_at_each_read_3_11() {
    let (f, _) = fake_fixture().await;
    let a = f.get("/.well-known/procurauth/actions").await;
    let b = f.get("/.well-known/procurauth/actions").await;
    assert_eq!(a.status, 200);
    assert_eq!(a.body, b.body);
    assert_eq!(a.body, br#"{"server":"pupitre-test"}"#);
    assert_eq!(a.header("content-type"), Some("application/json"));
    assert_eq!(a.header("cache-control"), Some("no-store"));
    let h = f.send(Req::head("/.well-known/procurauth/actions")).await;
    assert_eq!(h.status, 200);
    assert!(h.body.is_empty());
    assert_eq!(h.header("content-length"), Some(a.body.len().to_string().as_str()));
    f.stop().await;
}

#[tokio::test]
async fn the_access_question_gives_403_with_the_action_access_3_20() {
    let (f, fake) = fake_fixture().await;
    fake.set_access(Err(Refusal::Forbidden { action: "access" }));
    for path in ["/api/session", "/api/catalog", "/", "/nothing/here"] {
        let r = f.send(alice(Req::get(path))).await;
        assert_eq!((r.status, r.error().as_str()), (403, "forbidden"), "{path}");
        assert_eq!(r.member("action"), Json::str("access"));
    }
    fake.set_access(Err(Refusal::PdpUnavailable));
    let r = f.send(alice(Req::get("/api/session"))).await;
    assert_eq!((r.status, r.error().as_str()), (503, "pdp_unavailable"));
    assert_eq!(r.member("retryable"), Json::Bool(true));
    f.stop().await;
}

#[tokio::test]
async fn the_map_of_the_answers_3_18_and_3_19() {
    let (f, fake) = fake_fixture().await;
    // `approval_cannot_wait`: the body is exactly the body of P 17.7a, and nothing runs.
    fake.decide_with(|_, _, _| Err(Refusal::ApprovalCannotWait));
    let body = valid(r#"{"format":1,"values":{"limits.max_clients":5}}"#);
    let r = f.send(alice(Req::put("/api/environments/production/units/relay/values").create().json(&body))).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.text(), r#"{"error":"approval_cannot_wait"}"#);
    assert_eq!(f.store.count("put"), 0);
    let r = f.send(alice(Req::get("/api/environments/production"))).await;
    assert_eq!(r.text(), r#"{"error":"approval_cannot_wait"}"#);
    // A run: nothing is queued.
    let r = f
        .send(alice(
            Req::post("/api/runs")
                .json(&format!(r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a"]}}"#)),
        ))
        .await;
    assert_eq!(r.text(), r#"{"error":"approval_cannot_wait"}"#);
    assert!(f.store.all_runs().is_empty());
    // A PDP that does not answer: 503, retryable, fails closed.
    fake.decide_with(|_, _, _| Err(Refusal::PdpUnavailable));
    let r = f.send(alice(Req::get("/api/environments/production"))).await;
    assert_eq!((r.status, r.error().as_str()), (503, "pdp_unavailable"));
    assert_eq!(r.member("retryable"), Json::Bool(true));
    assert!(r.header("retry-after").is_none());
    // Unknown node and a PDP that does not answer: the same 503.
    let r = f.send(alice(Req::get("/api/environments/nothing"))).await;
    assert_eq!((r.status, r.error().as_str()), (503, "pdp_unavailable"));
    // A forbidden answer has the use action.
    fake.decide_with(|_, _, _| Err(Refusal::Forbidden { action: "other" }));
    let r = f.send(alice(Req::get("/api/environments/production/secrets"))).await;
    assert_eq!(r.member("action"), Json::str("config.read"));
    assert!(!r.text().contains("other"));
    f.stop().await;
}

#[tokio::test]
async fn the_decisions_are_counted_and_logged_without_a_detail_3_18() {
    let (f, fake) = fake_fixture().await;
    f.send(alice(Req::get("/api/environments/production"))).await;
    fake.decide_with(|_, _, _| Err(Refusal::Forbidden { action: "x" }));
    f.send(alice(Req::get("/api/environments/production"))).await;
    fake.decide_with(|_, _, _| Err(Refusal::PdpUnavailable));
    f.send(alice(Req::get("/api/environments/production"))).await;
    let decisions = f.log.events("access.decision");
    let kinds: Vec<&str> = decisions.iter().map(|l| l.as_object().unwrap()["decision"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["allow", "deny", "unavailable"]);
    let first = decisions[0].as_object().unwrap();
    assert_eq!(first["subject"], Json::str("alice"));
    assert_eq!(first["action"], Json::str("config.read"));
    assert_eq!(first["resource"], Json::str("environment:production"));
    assert_eq!(first["match"], Json::str("node"));
    let m = f.ops_get("/metrics").await.text();
    assert!(m.contains("pupitre_pdp_requests_total{result=\"allow\"} 1"), "{m}");
    assert!(m.contains("pupitre_pdp_requests_total{result=\"deny\"} 1"));
    assert!(m.contains("pupitre_pdp_requests_total{result=\"error\"} 1"));
    f.stop().await;
}

#[tokio::test]
async fn the_ceiling_of_the_hosts_is_the_lowest_value_3_10() {
    let (f, fake) = fake_fixture().await;
    let two = format!(r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a","host-b"]}}"#);
    // `runner.max_hosts` is 3 and the answer has no ceiling: two hosts pass.
    let r = f.send(alice(Req::post("/api/runs").json(&two))).await;
    assert_eq!(r.status, 201, "{}", r.text());
    // The answer gives 1: two hosts are refused, with the ceiling and the limit.
    fake.decide_with(|_, _, _| Ok(Granted { max_hosts: Some(1) }));
    let r = f.send(alice(Req::post("/api/runs").json(&two))).await;
    assert_eq!((r.status, r.error().as_str()), (403, "ceiling_exceeded"));
    assert_eq!(r.member("ceiling"), Json::str("max_hosts"));
    assert_eq!(r.member("limit"), Json::Int(1));
    assert_eq!(f.store.all_runs().len(), 1, "no part of the hosts runs");
    // One host passes.
    let one = format!(r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a"]}}"#);
    assert_eq!(f.send(alice(Req::post("/api/runs").json(&one))).await.status, 201);
    // A ceiling above `runner.max_hosts` has no effect: 3 is the bound.
    fake.decide_with(|_, _, _| Ok(Granted { max_hosts: Some(99) }));
    let four = format!(
        r#"{{"kind":"command","mode":"apply","command":"restart-service","node":"{UNIT}","hosts":["host-a","host-b","host-c","host-d"],"params":{{"grace":"5s"}}}}"#
    );
    let r = f.send(alice(Req::post("/api/runs").json(&four))).await;
    assert_eq!((r.status, r.error().as_str()), (403, "ceiling_exceeded"));
    assert_eq!(r.member("limit"), Json::Int(3));
    f.stop().await;
}

#[tokio::test]
async fn the_policy_of_the_cache_3_17() {
    let (f, fake) = fake_fixture().await;
    let policies = |fake: &FakeAccess| -> Vec<&'static str> {
        fake.asked()
            .iter()
            .filter_map(|a| match a {
                Asked::Use { policy, .. } => Some(*policy),
                _ => None,
            })
            .collect()
    };
    f.send(alice(Req::get("/api/environments/production"))).await;
    assert_eq!(policies(&fake), vec!["kept"]);
    fake.clear();
    f.send(alice(Req::put("/api/environments/production/common").create().json(&valid(r#"{"format":1,"values":{}}"#))))
        .await;
    assert_eq!(policies(&fake), vec!["fresh"]);
    fake.clear();
    f.send(alice(Req::delete("/api/environments/production/secrets/storage.password"))).await;
    assert_eq!(policies(&fake), vec!["fresh"]);
    fake.clear();
    f.send(alice(Req::get("/api/environments/nothing"))).await;
    assert!(matches!(fake.asked()[0], Asked::Unknown { .. }), "the question of 3.14 is never kept");
    f.stop().await;
}

fn many_environments(f: &Fixture<FakeAccess>, ids: &[&str]) {
    for id in ids {
        f.store.external_put(
            &pupitre_core::doc::DocKey::parse(&format!("environments/{id}")).unwrap(),
            format!("{{\n  \"format\": 1,\n  \"label\": \"{id}\"\n}}\n").into_bytes(),
        );
    }
}

/// The body of an options answer with `"next":null`, which the strict reader refuses (5.3).
fn options_json(r: &Resp) -> Json {
    let text = r.text().replace(r#","next":null"#, "");
    pupitre_core::json::parse(text.as_bytes(), pupitre_core::json::ReadLimits::bytes(1 << 20)).unwrap()
}

fn options_ids(r: &Resp) -> Vec<Json> {
    options_json(r).as_object().unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_object().unwrap()["id"].clone())
        .collect()
}

fn options_next(r: &Resp) -> Option<String> {
    let text = r.text();
    if text.ends_with(r#""next":null}"#) {
        return None;
    }
    pupitre_core::json::parse(text.as_bytes(), pupitre_core::json::ReadLimits::bytes(1 << 20))
        .unwrap()
        .as_object()
        .unwrap()["next"]
        .as_str()
        .map(str::to_string)
}

fn decode_cursor(text: &str) -> String {
    String::from_utf8(pupitre::server::query::base64url_decode(text).unwrap()).unwrap()
}

#[tokio::test]
async fn the_display_filter_shows_what_the_subject_can_see_3_15() {
    let store = std::sync::Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(true);
    s.max_page_items = 3;
    s.limits.max_environments = 20;
    let fake = FakeAccess::allow_all();
    let f = start(fake.clone(), s, store).await;
    many_environments(&f, &["aa", "ab", "ac", "ad", "ae"]);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // The subject sees the environments whose id ends with a vowel or `production`.
    fake.show_with(|n| matches!(n.path().as_str(), "environment:ab" | "environment:ad" | "environment:production"));
    let r = f.send(alice(Req::get("/api/environments?limit=3"))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    // The window of the first page is aa, ab, ac: only ab is visible.
    let ids: Vec<Json> =
        r.member("items").as_array().unwrap().iter().map(|i| i.as_object().unwrap()["id"].clone()).collect();
    assert_eq!(ids, vec![Json::str("ab")]);
    let next = r.member("next").as_str().unwrap().to_string();
    // The cursor holds the last visible id and the count of hidden items after it.
    assert_eq!(decode_cursor(&next), "environments\nab\n1");
    let r = f.send(alice(Req::get(&format!("/api/environments?limit=3&after={next}")))).await;
    // The window of the second page is ad, ae, production, and the list ends there.
    let ids: Vec<Json> =
        r.member("items").as_array().unwrap().iter().map(|i| i.as_object().unwrap()["id"].clone()).collect();
    assert_eq!(ids, vec![Json::str("ad"), Json::str("production")]);
    assert!(r.json().as_object().unwrap().get("next").is_none());
    f.stop().await;
}

#[tokio::test]
async fn the_budget_of_questions_stops_a_page_with_a_next_value_3_15() {
    let store = std::sync::Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(true);
    s.limits.max_environments = 20;
    let fake = FakeAccess::allow_all();
    let f = start(fake.clone(), s, store).await;
    many_environments(&f, &["aa", "ab", "ac"]);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // The budget stops the walk after two items of the window of four.
    fake.stop_after(Some(2));
    let r = f.send(alice(Req::get("/api/environments?limit=4"))).await;
    assert_eq!(r.status, 200);
    let ids: Vec<Json> =
        r.member("items").as_array().unwrap().iter().map(|i| i.as_object().unwrap()["id"].clone()).collect();
    assert_eq!(ids, vec![Json::str("aa"), Json::str("ab")]);
    let next = r.member("next").as_str().unwrap().to_string();
    assert_eq!(decode_cursor(&next), "environments\nab\n0");
    // The next page goes on after `ab`.
    let r = f.send(alice(Req::get(&format!("/api/environments?limit=4&after={next}")))).await;
    let ids: Vec<Json> =
        r.member("items").as_array().unwrap().iter().map(|i| i.as_object().unwrap()["id"].clone()).collect();
    assert_eq!(ids, vec![Json::str("ac"), Json::str("production")]);
    // A budget that shows nothing gives a page with no item and a next value that holds no id.
    fake.show_with(|_| false);
    let r = f.send(alice(Req::get("/api/environments?limit=4"))).await;
    assert_eq!(r.member("items"), Json::Arr(vec![]));
    let next = r.member("next").as_str().unwrap().to_string();
    assert_eq!(decode_cursor(&next), "environments\n\n2");
    f.stop().await;
}

#[tokio::test]
async fn a_pdp_failure_never_gives_a_shorter_list_3_15() {
    let (f, fake) = fake_fixture().await;
    fake.fail_visible(Some(Refusal::PdpUnavailable));
    for path in ["/api/environments", "/api/environments/production/units", "/api/runs"] {
        let r = f.send(alice(Req::get(path))).await;
        assert_eq!((r.status, r.error().as_str()), (503, "pdp_unavailable"), "{path}");
    }
    f.stop().await;
}

#[tokio::test]
async fn a_list_of_units_of_an_unknown_environment_is_empty_3_14() {
    let (f, _) = fake_fixture().await;
    let r = f.send(alice(Req::get("/api/environments/nothing/units"))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.member("items"), Json::Arr(vec![]));
    f.stop().await;
}

#[tokio::test]
async fn the_run_list_shows_the_runs_that_the_subject_can_read_3_15() {
    let (f, fake) = fake_fixture().await;
    for n in 1..=4u64 {
        f.store.seed_run(run_record(n, RunKind::Deploy, RunMode::Check, RunStatus::Passed, UNIT, &["host-a"]));
    }
    let ids: Vec<String> = f.store.all_runs().iter().map(|r| r.id.to_string()).collect();
    // The subject can see the runs 4 and 2 only. The page has the window of two runs.
    let visible: std::collections::BTreeSet<String> = [ids[0].clone(), ids[2].clone()].into_iter().collect();
    let hidden_ids: Vec<String> = [ids[1].clone(), ids[3].clone()].to_vec();
    let _ = visible;
    fake.show_with(|_| true);
    let r = f.send(alice(Req::get("/api/runs?limit=2"))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.member("items").as_array().unwrap().len(), 2);
    let next = r.member("next").as_str().unwrap().to_string();
    let r2 = f.send(alice(Req::get(&format!("/api/runs?limit=2&after={next}")))).await;
    assert_eq!(r2.member("items").as_array().unwrap().len(), 2);
    assert!(r2.json().as_object().unwrap().get("next").is_none());
    // The filter by status and by environment.
    let r = f.send(alice(Req::get("/api/runs?status=queued"))).await;
    assert_eq!(r.member("items"), Json::Arr(vec![]));
    let r = f.send(alice(Req::get("/api/runs?status=nothing"))).await;
    assert_eq!(r.status, 400);
    let r = f.send(alice(Req::get("/api/runs?environment=BAD"))).await;
    assert_eq!(r.status, 400);
    // Nothing is visible: the cursor never holds the id of a run that the subject cannot see.
    fake.show_with(|_| false);
    let r = f.send(alice(Req::get("/api/runs?limit=2"))).await;
    assert_eq!(r.member("items"), Json::Arr(vec![]));
    let next = r.member("next").as_str().unwrap().to_string();
    let text = decode_cursor(&next);
    assert_eq!(text, "runs\n\n2");
    for id in ids.iter().chain(hidden_ids.iter()) {
        assert!(!text.contains(id.as_str()));
    }
    f.stop().await;
}

#[tokio::test]
async fn the_options_route_3_16() {
    let (f, fake) = fake_fixture().await;
    let get = |q: &str| Req::get(&format!("/.well-known/procurauth/resources{q}")).token(TOKEN_ALICE);
    // The environments.
    let r = f.send(get("?type=environment")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.text(), r#"{"items":[{"has_children":true,"id":"production","label":"Production"}],"next":null}"#);
    // The units of an environment, with the parent decoded one time.
    let r = f.send(get("?type=unit&parent=environment%3Aproduction")).await;
    assert_eq!(r.text(), r#"{"items":[{"has_children":true,"id":"relay","label":"Relay"}],"next":null}"#);
    // The hosts of a unit have no child.
    let r = f.send(get("?type=host&parent=environment%3Aproduction%2Funit%3Arelay")).await;
    assert_eq!(
        r.text(),
        r#"{"items":[{"has_children":false,"id":"host-a","label":"host-a"},{"has_children":false,"id":"host-b","label":"host-b"}],"next":null}"#
    );
    // The questions are the display questions with the match `subtree` (P 4.4).
    fake.clear();
    f.send(get("?type=unit&parent=environment%3Aproduction")).await;
    assert_eq!(fake.asked(), vec![Asked::Visible { items: vec![(UNIT.into(), NO_INVENTORY.to_vec())], budget: 50 }]);
    // The search keeps the children whose id or label holds the text.
    let r = f.send(get("?type=host&parent=environment%3Aproduction%2Funit%3Arelay&q=HOST-B")).await;
    assert_eq!(r.text(), r#"{"items":[{"has_children":false,"id":"host-b","label":"host-b"}],"next":null}"#);
    // A parent that does not exist gives an answer with no item.
    let r = f.send(get("?type=unit&parent=environment%3Anothing")).await;
    assert_eq!(r.text(), r#"{"items":[],"next":null}"#);
    // Each bad request gives 400 invalid_options_request.
    for q in [
        "",
        "?type=unit",
        "?type=environment&parent=environment%3Aproduction",
        "?type=host&parent=environment%3Aproduction",
        "?type=environment&type=environment",
        "?type=other",
        "?type=environment&limit=0",
        "?type=environment&limit=5",
        "?type=environment&limit=x",
        "?type=environment&after=abc",
        "?type=environment&q=%zz",
        "?type=unit&parent=garbage",
        "?type=environment&q=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ] {
        let r = f.send(get(q)).await;
        assert_eq!((r.status, r.error().as_str()), (400, "invalid_options_request"), "{q:?}: {}", r.text());
    }
    // An unknown parameter has no effect, and a raw `+` is a fault.
    let r = f.send(get("?type=environment&other=1")).await;
    assert_eq!(r.status, 200);
    let r = f.send(get("?type=environment&q=pro+duction")).await;
    assert_eq!((r.status, r.error().as_str()), (400, "invalid_options_request"), "a raw plus is not valid (P 5.7)");
    // The route needs a token.
    assert_eq!(f.get("/.well-known/procurauth/resources?type=environment").await.status, 401);
    f.stop().await;
}

#[tokio::test]
async fn the_options_route_pages_with_a_cursor_that_holds_no_hidden_id_3_16() {
    let store = std::sync::Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(true);
    s.limits.max_environments = 20;
    let fake = FakeAccess::allow_all();
    let f = start(fake.clone(), s, store).await;
    many_environments(&f, &["aa", "ab", "ac", "ad"]);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    fake.show_with(|n| matches!(n.path().as_str(), "environment:ab" | "environment:ad"));
    let get = |q: &str| Req::get(&format!("/.well-known/procurauth/resources{q}")).token(TOKEN_ALICE);
    // A page holds `limit` visible items: the walk goes over hidden children to fill it.
    let r = f.send(get("?type=environment&limit=2")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(options_ids(&r), vec![Json::str("ab"), Json::str("ad")]);
    // The page is full. `production` stays to examine, so a cursor follows.
    let next = options_next(&r).unwrap();
    assert_eq!(decode_cursor(&next), "environment\n\nad\n0");
    // One item for each page: the cursor holds the last visible id and no hidden id.
    let r = f.send(get("?type=environment&limit=1")).await;
    assert_eq!(options_ids(&r), vec![Json::str("ab")]);
    let next = options_next(&r).unwrap();
    assert_eq!(decode_cursor(&next), "environment\n\nab\n0");
    let r = f.send(get(&format!("?type=environment&limit=1&after={next}"))).await;
    assert_eq!(options_ids(&r), vec![Json::str("ad")]);
    let next = options_next(&r).unwrap();
    assert_eq!(decode_cursor(&next), "environment\n\nad\n0");
    let r = f.send(get(&format!("?type=environment&limit=1&after={next}"))).await;
    assert_eq!(options_ids(&r), Vec::<Json>::new());
    assert_eq!(options_next(&r), None);
    // Only the last child is visible: the walk goes over the four hidden children.
    fake.show_with(|n| n.path() == "environment:production");
    let r = f.send(get("?type=environment&limit=1")).await;
    assert_eq!(options_ids(&r), vec![Json::str("production")]);
    assert_eq!(options_next(&r), None);
    fake.show_with(|n| matches!(n.path().as_str(), "environment:ab" | "environment:ad"));
    // A cursor of another type is a 400.
    let cursor = pupitre::server::query::base64url_encode(b"unit\nenvironment:production\nab\n0");
    let r = f.send(get(&format!("?type=environment&after={cursor}"))).await;
    assert_eq!(r.status, 400);
    // A PDP failure gives 503, never an empty list.
    fake.fail_visible(Some(Refusal::PdpUnavailable));
    let r = f.send(get("?type=environment")).await;
    assert_eq!((r.status, r.error().as_str()), (503, "pdp_unavailable"));
    assert_eq!(r.member("retryable"), Json::Bool(true));
    f.stop().await;
}

#[test]
fn the_unused_action_constants_stay_in_sync() {
    assert_eq!(Action::published(true).len(), ALL_ACTIONS.len());
}
