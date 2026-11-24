// SPDX-License-Identifier: MIT OR Apache-2.0
//! The routes of the runs (5.16 to 5.18): the run request of 4.4, the guard of 4.24, the queue
//! of 4.5, the events, the output and the cancel of 4.27.

mod server_common;

use std::sync::Arc;
use std::time::Duration;

use pupitre::access::{AnyAccess, OpenAccess};
use pupitre_core::json::Json;
use pupitre_core::run::{RunKind, RunMode, RunStatus};
use pupitre_core::time::UtcMillis;
use server_common::*;

const UNIT: &str = "environment:production/unit:relay";
const HOST: &str = "environment:production/unit:relay/host:host-a";

fn deploy_check(node: &str, hosts: &str) -> String {
    format!(r#"{{"kind":"deploy","mode":"check","node":"{node}","hosts":{hosts}}}"#)
}

fn command(mode: &str, name: &str, node: &str, hosts: &str, params: &str) -> String {
    format!(
        r#"{{"kind":"command","mode":"{mode}","command":"{name}","node":"{node}","hosts":{hosts},"params":{params}}}"#
    )
}

fn passed_check(n: u64, kind: RunKind, node: &str, hosts: &[&str]) -> pupitre_core::run::RunRecord {
    run_record(n, kind, RunMode::Check, RunStatus::Passed, node, hosts)
}

async fn post(f: &Fixture<AnyAccess>, body: &str) -> Resp {
    f.send(Req::post("/api/runs").json(body)).await
}

#[tokio::test]
async fn a_run_request_goes_into_the_store_queued_4_4_and_4_5() {
    let f = open_fixture().await;
    let r = post(&f, &deploy_check(UNIT, r#"["host-a","host-b"]"#)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let body = r.json();
    let m = body.as_object().unwrap();
    assert_eq!(m["status"], Json::str("queued"));
    assert_eq!(m["kind"], Json::str("deploy"));
    assert_eq!(m["mode"], Json::str("check"));
    assert_eq!(m["node"], Json::str(UNIT));
    assert_eq!(m["environment"], Json::str("production"));
    assert_eq!(m["hosts"], Json::Arr(vec![Json::str("host-a"), Json::str("host-b")]));
    assert_eq!(m["playbook"], Json::str("deploy-relay"));
    assert_eq!(m["subject"], Json::str("-"));
    assert_eq!(m["format"], Json::Int(1));
    assert!(!m.contains_key("started_at") && !m.contains_key("check_run") && !m.contains_key("snapshot"));
    let id = m["id"].as_str().unwrap().to_string();
    assert_eq!(r.header("location"), Some(format!("/api/runs/{id}").as_str()));
    assert!(pupitre_core::time::UtcMillis::parse(m["created_at"].as_str().unwrap()).is_some());
    // The store holds the record, and the runner loop is woken (4.6).
    let rid = pupitre_core::ids::RunId::parse(&id).unwrap();
    assert_eq!(f.store.run(&rid).unwrap().status, RunStatus::Queued);
    tokio::time::timeout(Duration::from_millis(500), f.wake.wait()).await.expect("the runner loop is woken");
    // The log line and the metric.
    let lines = f.log.events("run.queued");
    let l = lines[0].as_object().unwrap();
    assert_eq!(l["run"], Json::str(id.clone()));
    assert_eq!(l["kind"], Json::str("deploy"));
    assert_eq!(l["node"], Json::str(UNIT));
    assert_eq!(l["status"], Json::str("queued"));
    assert_eq!(l["subject"], Json::str("-"));
    let metrics = f.ops_get("/metrics").await.text();
    assert!(metrics.contains(r#"pupitre_runs_total{kind="deploy",mode="check",status="queued"} 1"#), "{metrics}");
    assert!(metrics.contains("\npupitre_runs_queued 1\n"));
    assert!(metrics.contains("\npupitre_runs_running 0\n"));
    // The record reads back with the same bytes.
    let got = f.get(&format!("/api/runs/{id}")).await;
    assert_eq!(got.body, r.body);
    f.stop().await;
}

#[tokio::test]
async fn a_status_run_uses_the_status_playbook_4_3() {
    let f = open_fixture().await;
    let r = post(&f, &format!(r#"{{"kind":"status","mode":"apply","node":"{UNIT}","hosts":["host-a"]}}"#)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.member("playbook"), Json::str("status-relay"));
    // A status run has the mode apply only.
    let r = post(&f, &format!(r#"{{"kind":"status","mode":"check","node":"{UNIT}","hosts":["host-a"]}}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (400, "bad_request"));
    // A unit with no status playbook.
    let ok = r#"{"document":{"format":1,"hosts":["host-c"],"label":"F","playbooks":{"deploy":"deploy-feed"},"schema":"feed"}}"#;
    assert_eq!(f.send(Req::put("/api/environments/production/units/feed").create().json(ok)).await.status, 201);
    let r =
        post(&f, r#"{"kind":"status","mode":"apply","node":"environment:production/unit:feed","hosts":["host-c"]}"#)
            .await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    f.stop().await;
}

#[tokio::test]
async fn the_rules_of_4_4_give_422_with_one_error_for_each_problem() {
    let f = open_fixture().await;
    let err = |r: &Resp| -> Vec<(String, String)> {
        r.member("errors")
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let m = e.as_object().unwrap();
                (m["path"].as_str().unwrap().to_string(), m["message"].as_str().unwrap().to_string())
            })
            .collect()
    };
    // Rule 3: a host that the unit does not hold.
    let r = post(&f, &deploy_check(UNIT, r#"["host-a","host-c"]"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    let e = err(&r);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].0, "hosts");
    assert!(e[0].1.contains("host-c"));
    // Rule 3: a host node takes exactly its host.
    let r = post(&f, &deploy_check(HOST, r#"["host-a","host-b"]"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    let r = post(&f, &deploy_check(HOST, r#"["host-a"]"#)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.member("node"), Json::str(HOST));
    // Rule 1: the node of a deploy is a unit or a host.
    let r = post(&f, &deploy_check("environment:production", r#"["host-a"]"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    assert_eq!(err(&r)[0].0, "node");
    // The hosts are in byte order, each one time, and at least one (shape: 400).
    for hosts in [r#"["host-b","host-a"]"#, r#"["host-a","host-a"]"#, "[]", r#"["Host"]"#, "\"host-a\""] {
        let r = post(&f, &deploy_check(UNIT, hosts)).await;
        assert_eq!((r.status, r.error().as_str()), (400, "bad_request"), "{hosts}");
    }
    // A node path that is not a path is a 400; a node that does not exist is a 404.
    assert_eq!(post(&f, &deploy_check("production", r#"["host-a"]"#)).await.status, 400);
    assert_eq!(post(&f, &deploy_check("environment:production/unit:nothing", r#"["host-a"]"#)).await.status, 404);
    // The message: 0 to 500 characters with no control character except U+000A.
    let long = "m".repeat(501);
    let r = post(
        &f,
        &format!(r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a"],"message":"{long}"}}"#),
    )
    .await;
    assert_eq!(r.status, 400);
    let r = post(
        &f,
        &format!(r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a"],"message":"line\nline"}}"#),
    )
    .await;
    assert_eq!(r.status, 201);
    assert_eq!(r.member("message"), Json::str("line\nline"));
    // The body has the members of its form only.
    for body in [
        r#"{"kind":"deploy","mode":"check","node":"X","hosts":["host-a"],"extra":1}"#.replace('X', UNIT),
        format!(r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a"],"command":"restart-service"}}"#),
        format!(
            r#"{{"kind":"deploy","mode":"check","node":"{UNIT}","hosts":["host-a"],"check_run":"0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42"}}"#
        ),
        r#"{"kind":"deploy","mode":"apply","check_run":"0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42","hosts":["host-a"]}"#
            .to_string(),
        r#"{"kind":"deploy","mode":"apply"}"#.to_string(),
        r#"{"kind":"nothing","mode":"check"}"#.to_string(),
        "[]".to_string(),
        "{}".to_string(),
    ] {
        let r = post(&f, &body).await;
        assert_eq!(r.status, 400, "{body}: {}", r.text());
    }
    f.stop().await;
}

#[tokio::test]
async fn a_unit_that_is_not_valid_in_the_catalog_gives_422_4_4() {
    let f = open_fixture().await;
    let key = pupitre_core::doc::DocKey::parse("environments/production/units/relay").unwrap();
    f.store.external_put(&key, b"{\"format\": 1}".to_vec());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = post(&f, &deploy_check(UNIT, r#"["host-a"]"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"), "{}", r.text());
    assert!(f.store.all_runs().is_empty());
    f.stop().await;
}

#[tokio::test]
async fn a_command_follows_4_32_to_4_35() {
    let f = open_fixture().await;
    // `check = false`: the mode apply runs after the confirmation of the person.
    let r = post(&f, &command("apply", "restart-service", UNIT, r#"["host-a"]"#, r#"{"grace":"90s"}"#)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.member("command"), Json::str("restart-service"));
    assert_eq!(r.member("params"), Json::object([("grace", Json::str("90s"))]));
    assert_eq!(r.member("playbook"), Json::str("restart-service"));
    assert_eq!(r.member("kind"), Json::str("command"));
    // The mode check of a command with `check = false` is refused.
    let r = post(&f, &command("check", "restart-service", UNIT, r#"["host-a"]"#, r#"{"grace":"90s"}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    // A command with `check = true`: the mode check, then the apply with the check run.
    let r = post(&f, &command("check", "reload-service", HOST, r#"["host-a"]"#, "{}")).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let r = post(&f, &command("apply", "reload-service", HOST, r#"["host-a"]"#, "{}")).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    // The target of the command gives the type of the node.
    let r = post(&f, &command("apply", "restart-service", HOST, r#"["host-a"]"#, r#"{"grace":"90s"}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    let r = post(&f, &command("check", "reload-service", UNIT, r#"["host-a"]"#, "{}")).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    // A command that the configuration does not have.
    let r = post(&f, &command("apply", "nothing", UNIT, r#"["host-a"]"#, "{}")).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    f.stop().await;
}

#[tokio::test]
async fn the_parameters_of_a_command_follow_4_33_with_the_name_the_range_and_the_value() {
    let f = open_fixture().await;
    let r = post(&f, &command("apply", "restart-service", UNIT, r#"["host-a"]"#, r#"{"grace":"10m","other":1}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    let errors = r.member("errors");
    let list = errors.as_array().unwrap();
    assert_eq!(list.len(), 2);
    let messages: Vec<String> =
        list.iter().map(|e| e.as_object().unwrap()["message"].as_str().unwrap().to_string()).collect();
    assert!(messages.iter().any(|m| m == "expected a canonical duration from 1s to 5m, found \"10m\""), "{messages:?}");
    // A missing required parameter, and a parameter that is not an object.
    let r = post(&f, &command("apply", "restart-service", UNIT, r#"["host-a"]"#, "{}")).await;
    assert_eq!(r.member("errors").as_array().unwrap()[0].as_object().unwrap()["path"], Json::str("params.grace"));
    let r = post(&f, &command("apply", "restart-service", UNIT, r#"["host-a"]"#, "[]")).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"));
    // A parameter that is not canonical: `120s` is `2m` (2.12).
    let r = post(&f, &command("apply", "restart-service", UNIT, r#"["host-a"]"#, r#"{"grace":"120s"}"#)).await;
    assert_eq!(r.status, 422);
    f.stop().await;
}

#[tokio::test]
async fn an_apply_needs_a_check_run_of_the_same_kind_that_passed_4_24() {
    let f = open_fixture().await;
    let check = passed_check(1, RunKind::Deploy, UNIT, &["host-a", "host-b"]);
    f.store.seed_run(check.clone());
    let body = format!(r#"{{"kind":"deploy","mode":"apply","check_run":"{}","message":"go"}}"#, check.id);
    let r = post(&f, &body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    // The apply takes the node and the hosts of the check; the request cannot change them.
    assert_eq!(r.member("node"), Json::str(UNIT));
    assert_eq!(r.member("hosts"), Json::Arr(vec![Json::str("host-a"), Json::str("host-b")]));
    assert_eq!(r.member("check_run"), Json::str(check.id.to_string()));
    assert_eq!(r.member("mode"), Json::str("apply"));
    assert_eq!(r.member("playbook"), Json::str("deploy-relay"));
    // One check gives one apply at most: the check has `applied_by` (2.27).
    let new_id = r.member("id").as_str().unwrap().to_string();
    assert_eq!(f.store.run(&check.id).unwrap().applied_by.map(|i| i.to_string()), Some(new_id));
    let r = post(&f, &body).await;
    assert_eq!((r.status, r.error().as_str()), (409, "check_used"));
    f.stop().await;
}

#[tokio::test]
async fn each_rule_of_the_guard_has_its_code_4_24() {
    let f = open_fixture().await;
    let apply = |id: &pupitre_core::ids::RunId| format!(r#"{{"kind":"deploy","mode":"apply","check_run":"{id}"}}"#);
    // A check run that failed, one that is queued, one that ran in the mode apply, one of another kind.
    let mut failed = passed_check(1, RunKind::Deploy, UNIT, &["host-a"]);
    failed.status = RunStatus::Failed;
    let queued = run_record(2, RunKind::Deploy, RunMode::Check, RunStatus::Queued, UNIT, &["host-a"]);
    let applied = run_record(3, RunKind::Deploy, RunMode::Apply, RunStatus::Passed, UNIT, &["host-a"]);
    let status_run = run_record(4, RunKind::Status, RunMode::Apply, RunStatus::Passed, UNIT, &["host-a"]);
    let command_check = passed_check(5, RunKind::Command, HOST, &["host-a"]);
    for r in [&failed, &queued, &applied, &status_run, &command_check] {
        f.store.seed_run(r.clone());
    }
    for r in [&failed, &queued, &applied, &status_run, &command_check] {
        let answer = post(&f, &apply(&r.id)).await;
        assert_eq!((answer.status, answer.error().as_str()), (409, "check_required"), "{:?}", r.id);
    }
    // A check that ended more than `runner.check_max_age` ago (600 seconds).
    let mut old = passed_check(6, RunKind::Deploy, UNIT, &["host-a"]);
    old.finished_at = Some(UtcMillis(UtcMillis::now().0 - 601_000));
    f.store.seed_run(old.clone());
    let answer = post(&f, &apply(&old.id)).await;
    assert_eq!((answer.status, answer.error().as_str()), (409, "check_too_old"));
    let mut young = passed_check(7, RunKind::Deploy, UNIT, &["host-a"]);
    young.finished_at = Some(UtcMillis(UtcMillis::now().0 - 590_000));
    f.store.seed_run(young.clone());
    assert_eq!(post(&f, &apply(&young.id)).await.status, 201);
    // A check run that does not exist is a 404 in the mode open (3.14); a bad id is a 400.
    let answer = post(&f, &apply(&pupitre_core::ids::RunId::from_parts(5, [1; 10]))).await;
    assert_eq!((answer.status, answer.error().as_str()), (404, "not_found"));
    let answer = post(&f, r#"{"kind":"deploy","mode":"apply","check_run":"nope"}"#).await;
    assert_eq!(answer.status, 400);
    // Nothing was queued by a refusal: one run (the apply of `young`).
    assert_eq!(f.store.all_runs().iter().filter(|r| r.status == RunStatus::Queued).count(), 2);
    f.stop().await;
}

#[tokio::test]
async fn the_apply_of_a_command_with_a_check_takes_the_command_and_the_parameters_4_24() {
    let f = open_fixture().await;
    let check = passed_check(1, RunKind::Command, HOST, &["host-a"]);
    f.store.seed_run(check.clone());
    let r = post(&f, &format!(r#"{{"kind":"command","mode":"apply","check_run":"{}"}}"#, check.id)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.member("command"), Json::str("reload-service"));
    assert_eq!(r.member("node"), Json::str(HOST));
    assert_eq!(r.member("check_run"), Json::str(check.id.to_string()));
    f.stop().await;
}

#[tokio::test]
async fn an_apply_whose_unit_is_gone_gives_422_4_24() {
    let f = open_fixture().await;
    let check = passed_check(1, RunKind::Deploy, "environment:production/unit:gone", &["host-a"]);
    f.store.seed_run(check.clone());
    let r = post(&f, &format!(r#"{{"kind":"deploy","mode":"apply","check_run":"{}"}}"#, check.id)).await;
    assert_eq!((r.status, r.error().as_str()), (422, "invalid_run_request"), "{}", r.text());
    // The check stays free.
    assert!(f.store.run(&check.id).unwrap().applied_by.is_none());
    f.stop().await;
}

#[tokio::test]
async fn the_apply_and_the_commands_are_off_in_the_mode_open_without_the_key_3_22() {
    let store = Arc::new(MemStore::new());
    seed(&store);
    let mut s = settings(false);
    s.mode = pupitre::server::state::ModeSettings::Open { allow_apply: false };
    let f = start(AnyAccess::Open(OpenAccess), s, store).await;
    let session = f.get("/api/session").await;
    assert_eq!(session.member("runner").as_object().unwrap()["allow_apply"], Json::Bool(false));
    // A check run and a status run still run.
    assert_eq!(post(&f, &deploy_check(UNIT, r#"["host-a"]"#)).await.status, 201);
    assert_eq!(
        post(&f, &format!(r#"{{"kind":"status","mode":"apply","node":"{UNIT}","hosts":["host-a"]}}"#)).await.status,
        201
    );
    // An apply and each command request give 403 apply_disabled.
    let check = passed_check(1, RunKind::Deploy, UNIT, &["host-a"]);
    f.store.seed_run(check.clone());
    let r = post(&f, &format!(r#"{{"kind":"deploy","mode":"apply","check_run":"{}"}}"#, check.id)).await;
    assert_eq!((r.status, r.error().as_str()), (403, "apply_disabled"));
    let r = post(&f, &command("apply", "restart-service", UNIT, r#"["host-a"]"#, r#"{"grace":"90s"}"#)).await;
    assert_eq!((r.status, r.error().as_str()), (403, "apply_disabled"));
    let r = post(&f, &command("check", "reload-service", HOST, r#"["host-a"]"#, "{}")).await;
    assert_eq!((r.status, r.error().as_str()), (403, "apply_disabled"));
    // The refusal comes before the rules of 4.4: a bad request of a command is the same 403.
    let r = post(&f, &command("apply", "nothing", UNIT, r#"["host-a"]"#, "{}")).await;
    assert_eq!(r.error(), "apply_disabled");
    // The check was not used.
    assert!(f.store.run(&check.id).unwrap().applied_by.is_none());
    f.stop().await;
}

#[tokio::test]
async fn a_queue_at_its_bound_refuses_with_503_and_retry_after_4_5() {
    let f = open_fixture().await;
    for _ in 0..3 {
        assert_eq!(post(&f, &deploy_check(UNIT, r#"["host-a"]"#)).await.status, 201);
    }
    let r = post(&f, &deploy_check(UNIT, r#"["host-a"]"#)).await;
    assert_eq!((r.status, r.error().as_str()), (503, "run_queue_full"));
    assert_eq!(r.header("retry-after"), Some("5"));
    assert_eq!(r.member("retryable"), Json::Bool(true));
    assert_eq!(f.store.all_runs().len(), 3);
    f.stop().await;
}

#[tokio::test]
async fn a_failure_of_the_store_gives_503_store_unavailable() {
    let f = open_fixture().await;
    f.store.down.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = post(&f, &deploy_check(UNIT, r#"["host-a"]"#)).await;
    assert_eq!((r.status, r.error().as_str()), (503, "store_unavailable"));
    assert_eq!(r.member("retryable"), Json::Bool(true));
    let r = f.get("/api/runs").await;
    assert_eq!(r.error(), "store_unavailable");
    let m = f.ops_get("/metrics").await.text();
    assert!(m.contains("pupitre_store_errors_total{method=\"create_run\"} 1"), "{m}");
    f.stop().await;
}

#[tokio::test]
async fn the_run_list_5_16() {
    let f = open_fixture().await;
    for n in 1..=7u64 {
        let status = if n % 2 == 0 { RunStatus::Passed } else { RunStatus::Queued };
        f.store.seed_run(run_record(n, RunKind::Deploy, RunMode::Check, status, UNIT, &["host-a"]));
    }
    f.store.seed_run(run_record(
        8,
        RunKind::Deploy,
        RunMode::Check,
        RunStatus::Passed,
        "environment:other/unit:relay",
        &["host-a"],
    ));
    let r = f.get("/api/runs").await;
    assert_eq!(r.status, 200);
    let binding = r.member("items");
    let items = binding.as_array().unwrap();
    assert_eq!(items.len(), 5, "the page has max_page_items = 5");
    let ids: Vec<String> = items.iter().map(|i| i.as_object().unwrap()["id"].as_str().unwrap().to_string()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.reverse();
    assert_eq!(ids, sorted, "newest first");
    let next = r.member("next").as_str().unwrap().to_string();
    let r2 = f.get(&format!("/api/runs?after={next}")).await;
    let binding = r2.member("items");
    let items2 = binding.as_array().unwrap();
    assert_eq!(items2.len(), 3);
    assert!(items2.iter().all(|i| !ids.contains(&i.as_object().unwrap()["id"].as_str().unwrap().to_string())));
    assert!(r2.json().as_object().unwrap().get("next").is_none());
    let r = f.get("/api/runs?status=queued").await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 4);
    let r = f.get("/api/runs?environment=other").await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 1);
    let r = f.get("/api/runs?environment=other&status=queued").await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 0);
    f.stop().await;
}

#[tokio::test]
async fn the_events_and_the_output_of_a_run_5_17() {
    let f = open_fixture().await;
    let mut run = run_record(1, RunKind::Deploy, RunMode::Check, RunStatus::Running, UNIT, &["host-a"]);
    run.started_at = Some(UtcMillis::now());
    f.store.seed_run(run.clone());
    for seq in 1..=3 {
        f.store.seed_event(&run.id, &format!(r#"{{"kind":"task_start","phase":"main","seq":{seq},"t":"2026-11-24T10:00:00.000Z","task":"t{seq}","v":1}}"#));
    }
    f.store.seed_output(&run.id, b"PLAY [all] ****\nok: [host-a]\n");
    let path = format!("/api/runs/{}", run.id);
    // The events after a number, with the last number and `complete`.
    let r = f.get(&format!("{path}/events")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.member("last_seq"), Json::Int(3));
    assert_eq!(r.member("complete"), Json::Bool(false));
    let binding = r.member("items");
    let items = binding.as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].as_object().unwrap()["task"], Json::str("t1"));
    let r = f.get(&format!("{path}/events?after=1&limit=1")).await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 1);
    assert_eq!(r.member("last_seq"), Json::Int(2));
    let r = f.get(&format!("{path}/events?after=3")).await;
    assert_eq!(r.member("items"), Json::Arr(vec![]));
    assert_eq!(r.member("last_seq"), Json::Int(3));
    assert_eq!(f.get(&format!("{path}/events?limit=6")).await.status, 400);
    assert_eq!(f.get(&format!("{path}/events?after=-1")).await.status, 400);
    // The output: raw bytes, the length and the completion in two headers.
    let r = f.get(&format!("{path}/output")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/octet-stream"));
    assert_eq!(r.header("x-pupitre-output-length"), Some("29"));
    assert_eq!(r.header("x-pupitre-run-complete"), Some("false"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(r.body, b"PLAY [all] ****\nok: [host-a]\n");
    let r = f.get(&format!("{path}/output?offset=16&limit=4")).await;
    assert_eq!(r.body, b"ok: ");
    assert_eq!(r.header("x-pupitre-output-length"), Some("29"));
    assert_eq!(f.get(&format!("{path}/output?limit=0")).await.status, 400);
    assert_eq!(f.get(&format!("{path}/output?limit=1048577")).await.status, 400);
    assert_eq!(f.get(&format!("{path}/output?limit=1048576")).await.status, 200);
    assert_eq!(f.get(&format!("{path}/output?offset=x")).await.status, 400);
    // The run ends: the headers and `complete` follow.
    let mut done = run.clone();
    done.status = RunStatus::Passed;
    done.finished_at = Some(UtcMillis::now());
    f.store.seed_run(done);
    assert_eq!(f.get(&format!("{path}/output")).await.header("x-pupitre-run-complete"), Some("true"));
    let r = f.get(&format!("{path}/events")).await;
    assert_eq!(r.member("complete"), Json::Bool(true));
    // With `limit` equal to the lines left, the answer cannot tell that no line follows.
    let r = f.get(&format!("{path}/events?limit=3")).await;
    assert_eq!(r.member("complete"), Json::Bool(false));
    // An unknown run, a bad id.
    assert_eq!(f.get("/api/runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42/events").await.status, 404);
    assert_eq!(f.get("/api/runs/nothing/output").await.status, 404);
    f.stop().await;
}

#[tokio::test]
async fn a_line_of_events_that_is_not_json_is_shown_as_invalid_4_20() {
    let f = open_fixture().await;
    let run = run_record(1, RunKind::Deploy, RunMode::Check, RunStatus::Running, UNIT, &["host-a"]);
    f.store.seed_run(run.clone());
    f.store.seed_event(&run.id, "not json at all");
    let r = f.get(&format!("/api/runs/{}/events", run.id)).await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.member("items").as_array().unwrap()[0],
        Json::object([("kind", Json::str("invalid")), ("seq", Json::Int(1))])
    );
    f.stop().await;
}

#[tokio::test]
async fn a_cancel_follows_4_27() {
    let f = open_fixture().await;
    // A queued run gets the status canceled at once.
    let queued = run_record(1, RunKind::Deploy, RunMode::Check, RunStatus::Queued, UNIT, &["host-a"]);
    f.store.seed_run(queued.clone());
    let r = f.send(Req::post(&format!("/api/runs/{}/cancel", queued.id))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.member("status"), Json::str("canceled"));
    assert_eq!(r.member("cancel").as_object().unwrap()["by"], Json::str("-"));
    assert!(r.member("finished_at").as_str().is_some());
    assert_eq!(f.store.run(&queued.id).unwrap().status, RunStatus::Canceled);
    // A finished run gives 409 run_finished.
    let r = f.send(Req::post(&format!("/api/runs/{}/cancel", queued.id))).await;
    assert_eq!((r.status, r.error().as_str()), (409, "run_finished"));
    // A running run gets the member `cancel` and keeps its status for the runner node.
    let mut running = run_record(2, RunKind::Deploy, RunMode::Apply, RunStatus::Running, UNIT, &["host-a"]);
    running.started_at = Some(UtcMillis::now());
    f.store.seed_run(running.clone());
    let r = f.send(Req::post(&format!("/api/runs/{}/cancel", running.id))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.member("status"), Json::str("running"));
    assert!(r.member("cancel").as_object().is_some());
    tokio::time::timeout(Duration::from_millis(500), f.wake.wait()).await.expect("the runner loop is woken");
    let stored = f.store.run(&running.id).unwrap();
    assert!(stored.cancel.is_some());
    assert_eq!(stored.status, RunStatus::Running);
    // A second cancel changes nothing.
    let before = f.store.run(&running.id).unwrap().cancel.clone();
    let r = f.send(Req::post(&format!("/api/runs/{}/cancel", running.id))).await;
    assert_eq!(r.status, 200);
    assert_eq!(f.store.run(&running.id).unwrap().cancel, before);
    // An unknown run, and a body.
    let r = f.send(Req::post("/api/runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42/cancel")).await;
    assert_eq!(r.status, 404);
    let r = f.send(Req::post(&format!("/api/runs/{}/cancel", running.id)).json("{}")).await;
    assert_eq!(r.status, 400);
    assert!(f.log.events("run.cancel").len() >= 2);
    f.stop().await;
}
