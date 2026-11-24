// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sizes of 1.8: 1,000 units, 1,000 hosts and 100 hosts in one unit in one environment, and
//! 64 environments. The catalog loads them, and the lists page through them.

mod server_common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use pupitre::access::{AnyAccess, OpenAccess};
use pupitre_core::doc::{Document, Environment, Host};
use pupitre_core::ids::{HostId, PlaybookId, SchemaId};
use pupitre_core::json::Json;
use server_common::*;

fn big_store() -> Arc<MemStore> {
    let store = Arc::new(MemStore::new());
    for e in 0..64 {
        store.seed_doc(
            &format!("environments/env-{e:02}"),
            &Document::Environment(Environment { label: format!("Environment {e}") }),
        );
    }
    // The environment with the large sizes.
    for h in 0..1000 {
        store.seed_doc(
            &format!("environments/env-00/hosts/host-{h:04}"),
            &Document::Host(Host { address: format!("host-{h:04}.example.invalid"), port: Some(22), labels: vec![] }),
        );
    }
    for u in 0..1000 {
        let hosts: Vec<HostId> =
            (0..100).map(|i| HostId::parse(&format!("host-{:04}", (u + i) % 1000)).unwrap()).collect();
        let mut hosts = hosts;
        hosts.sort();
        store.seed_doc(
            &format!("environments/env-00/units/unit-{u:04}"),
            &Document::Unit(pupitre_core::doc::Unit {
                label: format!("Unit {u}"),
                schema: SchemaId::parse("relay").unwrap(),
                playbooks: pupitre_core::doc::UnitPlaybooks {
                    deploy: PlaybookId::parse("deploy-relay").unwrap(),
                    render: None,
                    status: None,
                },
                hosts,
            }),
        );
    }
    store
}

fn limits(s: &mut pupitre::server::Settings) {
    s.limits.max_environments = 64;
    s.limits.max_units_per_environment = 1000;
    s.limits.max_hosts_per_environment = 1000;
    s.limits.max_document_bytes = 262_144;
    s.limits.max_store_bytes = 268_435_456;
    s.max_page_items = 200;
}

#[tokio::test]
async fn the_catalog_loads_and_lists_the_sizes_of_1_8() {
    let store = big_store();
    let mut s = settings(false);
    limits(&mut s);
    let started = Instant::now();
    let f = start_with(AnyAccess::Open(OpenAccess), s, store, capabilities(), Duration::from_secs(3600)).await;
    let load = started.elapsed();
    assert!(load < Duration::from_secs(10), "the load took {load:?}");
    assert!(f.state.catalog().is_ready());
    let counts = f.state.catalog().document_counts();
    assert_eq!((counts["environment"], counts["host"], counts["unit"]), (64, 1000, 1000));
    // The lists page through the whole content with the cursor.
    let started = Instant::now();
    let mut seen = 0;
    let mut after = String::new();
    loop {
        let r = f.get(&format!("/api/environments/env-00/units?limit=200{after}")).await;
        assert_eq!(r.status, 200);
        seen += r.member("items").as_array().unwrap().len();
        match r.json().as_object().unwrap().get("next") {
            Some(Json::Str(n)) => after = format!("&after={n}"),
            _ => break,
        }
    }
    assert_eq!(seen, 1000);
    let mut hosts = 0;
    let mut after = String::new();
    loop {
        let r = f.get(&format!("/api/environments/env-00/hosts?limit=200{after}")).await;
        hosts += r.member("items").as_array().unwrap().len();
        match r.json().as_object().unwrap().get("next") {
            Some(Json::Str(n)) => after = format!("&after={n}"),
            _ => break,
        }
    }
    assert_eq!(hosts, 1000);
    let r = f.get("/api/environments?limit=200").await;
    assert_eq!(r.member("items").as_array().unwrap().len(), 64);
    // A write checks the rules across 1,000 units in a short time.
    let one = f.get("/api/environments/env-00/hosts/host-0001").await;
    let new = r#"{"document":{"address":"host-0001.example.invalid","format":1,"labels":["canary"],"port":22}}"#;
    let r = f.send(Req::put("/api/environments/env-00/hosts/host-0001").if_match(&one.etag()).json(new)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(10), "the lists and the write took {elapsed:?}");
    f.stop().await;
}

#[tokio::test]
async fn a_unit_of_100_hosts_is_written_in_the_limits_1_8() {
    let store = Arc::new(MemStore::new());
    let mut s = settings(false);
    limits(&mut s);
    store.seed_doc("environments/p", &Document::Environment(Environment { label: "P".into() }));
    for h in 0..100 {
        store.seed_doc(
            &format!("environments/p/hosts/host-{h:03}"),
            &Document::Host(Host { address: format!("host-{h:03}.example.invalid"), port: None, labels: vec![] }),
        );
    }
    let f = start_with(AnyAccess::Open(OpenAccess), s, store, capabilities(), Duration::from_secs(3600)).await;
    let hosts: Vec<String> = (0..100).map(|h| format!("\"host-{h:03}\"")).collect();
    let body = format!(
        r#"{{"document":{{"format":1,"hosts":[{}],"label":"Big","playbooks":{{"deploy":"deploy-relay"}},"schema":"relay"}}}}"#,
        hosts.join(",")
    );
    let r = f.send(Req::put("/api/environments/p/units/big").create().json(&body)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    // The run request names 100 hosts: the ceiling `runner.max_hosts` of the test is 3.
    let r = f
        .send(Req::post("/api/runs").json(&format!(
            r#"{{"kind":"deploy","mode":"check","node":"environment:p/unit:big","hosts":[{}]}}"#,
            hosts.join(",")
        )))
        .await;
    assert_eq!((r.status, r.error().as_str()), (403, "ceiling_exceeded"));
    assert_eq!(r.member("limit"), Json::Int(3));
    f.stop().await;
}
