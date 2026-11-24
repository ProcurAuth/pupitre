// SPDX-License-Identifier: MIT OR Apache-2.0
//! The command `import` (6.24a), through the binary: the report, the write, and each rule
//! that stops the import.

mod import_support;

use import_support::*;
use pupitre_core::doc::{DocKind, Document};
use pupitre_core::json::{self, ReadLimits};
use pupitre_store::{Store, VersionKind};

fn doc(kind: DocKind, text: &str) -> Vec<u8> {
    let value = json::parse(text.as_bytes(), ReadLimits::bytes(1 << 20)).unwrap();
    Document::from_json(kind, &value).unwrap().canonical().bytes
}

#[tokio::test]
async fn the_report_without_write_shows_the_plan_and_writes_nothing() {
    let fx = Fixture::new();
    fx.migrate();
    let inventory = fx.write_inventory(INVENTORY);
    let out = fx.import(&inventory, "production", false);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("import report for the environment production"), "{text}");
    assert!(text.contains("report only, nothing is written"), "{text}");
    assert!(text.contains("hosts (3):"), "{text}");
    assert!(text.contains("host-a  address=203.0.113.10  port=2222  labels=canary"), "{text}");
    assert!(
        text.contains("host-b  address=relay-b.example.invalid  labels=\n")
            || text.contains("host-b  address=relay-b.example.invalid  labels="),
        "{text}"
    );
    assert!(text.contains("units (2):"), "{text}");
    assert!(text.contains("feed  schema=feed  hosts=host-c  deploy=deploy-feed"), "{text}");
    assert!(
        text.contains("relay  schema=relay  hosts=host-a,host-b  deploy=deploy-relay  render=render-relay"),
        "{text}"
    );
    assert!(text.contains("documents (9): 1 environment, 3 host, 2 unit, 2 unit_values, 1 host_values"), "{text}");
    // The variables that the command leaves out: names, no value.
    assert!(text.contains("variables not imported (2):"), "{text}");
    assert!(text.contains("group all: ansible_python_interpreter"), "{text}");
    assert!(text.contains("host host-a: ansible_user"), "{text}");
    assert!(!text.contains("/usr/bin/python3") && !text.contains("deploy\n"), "{text}");
    // Nothing is written.
    let store = fx.open();
    assert!(store.load().await.unwrap().docs.is_empty());
    assert!(std::fs::read_dir(fx.path("docs/.history")).map_or(true, |mut d| d.next().is_none()));
}

#[tokio::test]
async fn the_write_makes_the_documents_with_the_subject_import_and_the_kind_import() {
    let fx = Fixture::new();
    fx.migrate();
    let inventory = fx.write_inventory(INVENTORY);
    let out = fx.import(&inventory, "production", true);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("written: 9 documents with the subject import"));
    let store = fx.open();
    let docs = documents(&store).await;
    let keys: Vec<&str> = docs.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "environments/production",
            "environments/production/hosts/host-a",
            "environments/production/hosts/host-b",
            "environments/production/hosts/host-c",
            "environments/production/units/feed",
            "environments/production/units/feed/values",
            "environments/production/units/relay",
            "environments/production/units/relay/hosts/host-a/values",
            "environments/production/units/relay/values",
        ]
    );
    let bytes = |k: &str| docs[k].bytes.clone();
    assert_eq!(bytes("environments/production"), doc(DocKind::Environment, r#"{"format": 1, "label": "production"}"#));
    assert_eq!(
        bytes("environments/production/hosts/host-a"),
        doc(DocKind::Host, r#"{"format": 1, "address": "203.0.113.10", "port": 2222, "labels": ["canary"]}"#)
    );
    assert_eq!(
        bytes("environments/production/hosts/host-b"),
        doc(DocKind::Host, r#"{"format": 1, "address": "relay-b.example.invalid", "labels": []}"#)
    );
    assert_eq!(
        bytes("environments/production/units/relay"),
        doc(
            DocKind::Unit,
            r#"{"format": 1, "label": "relay", "schema": "relay", "hosts": ["host-a", "host-b"],
                "playbooks": {"deploy": "deploy-relay", "render": "render-relay"}}"#
        )
    );
    assert_eq!(
        bytes("environments/production/units/feed"),
        doc(
            DocKind::Unit,
            r#"{"format": 1, "label": "feed", "schema": "feed", "hosts": ["host-c"], "playbooks": {"deploy": "deploy-feed"}}"#
        )
    );
    // Each member of the nested object is a key path.
    assert_eq!(
        bytes("environments/production/units/relay/values"),
        doc(
            DocKind::UnitValues,
            r#"{"format": 1, "values": {"limits.max_clients": 500, "log.level": "info", "tags": ["blue", "eu"]}}"#
        )
    );
    assert_eq!(
        bytes("environments/production/units/relay/hosts/host-a/values"),
        doc(DocKind::HostValues, r#"{"format": 1, "values": {"log.level": "debug"}}"#)
    );
    assert_eq!(
        bytes("environments/production/units/feed/values"),
        doc(
            DocKind::UnitValues,
            r#"{"format": 1, "values": {"feed.enabled": true, "feed.url": "https://feed.example.invalid/v1"}}"#
        )
    );
    // Each version has the subject `import` and the kind `import`.
    for (text, stored) in &docs {
        let versions = store.history(&stored.key, None, 10).await.unwrap();
        assert_eq!(versions.len(), 1, "{text}");
        assert_eq!(versions[0].kind, VersionKind::Import, "{text}");
        assert_eq!(versions[0].meta.saved_by.as_str(), "import", "{text}");
        assert_eq!(stored.version, 1);
    }
    // No secret, and no run.
    assert!(!fx.path("secrets/values").exists());
    // The nested variable of the group and its hosts has the form of 2.10: no unknown key.
    for stored in docs.values() {
        Document::from_stored(stored.key.kind(), &stored.bytes, 1 << 20).unwrap();
    }
}

#[tokio::test]
async fn an_environment_that_exists_stops_the_import() {
    let fx = Fixture::new();
    fx.migrate();
    let inventory = fx.write_inventory(INVENTORY);
    {
        let store = fx.open();
        let d = pupitre_core::doc::Document::Environment(pupitre_core::doc::Environment { label: "Production".into() })
            .canonical();
        store.put(&key("environments/production"), &d, &pupitre_store::Precondition::Create, &meta()).await.unwrap();
    }
    for write in [false, true] {
        let out = fx.import(&inventory, "production", write);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("the environment exists in the store"), "{}", stderr(&out));
        assert!(stdout(&out).is_empty());
    }
    let store = fx.open();
    assert_eq!(store.load().await.unwrap().docs.len(), 1);
    // A second import of the same inventory under a new name works; one environment of the store stays.
    drop(store);
    let out = fx.import(&inventory, "staging", true);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // An import of the same environment again stops.
    let again = fx.import(&inventory, "staging", true);
    assert_eq!(again.status.code(), Some(1));
}

fn meta() -> pupitre_store::WriteMeta {
    pupitre_store::WriteMeta {
        saved_by: pupitre_core::ids::Subject::anonymous(),
        saved_at: pupitre_core::time::UtcMillis::now(),
        message: Default::default(),
    }
}

async fn stopped(fx: &Fixture, inventory: &str, needles: &[&str]) {
    fx.migrate();
    let path = fx.write_inventory(inventory);
    for write in [false, true] {
        let out = fx.import(&path, "production", write);
        assert_eq!(out.status.code(), Some(1), "write={write}");
        let err = stderr(&out);
        for n in needles {
            assert!(err.contains(n), "missing {n:?} in:\n{err}");
        }
        assert!(stdout(&out).is_empty(), "no report when the import stops");
    }
    // The store has no document.
    let store = fx.open();
    assert!(store.load().await.unwrap().docs.is_empty());
}

#[tokio::test]
async fn a_secret_key_in_the_inventory_stops_the_import() {
    let fx = Fixture::new();
    let inv = INVENTORY.replace(
        r#""tags": ["blue", "eu"]"#,
        r#""tags": ["blue", "eu"], "storage": {"password": "s3cret-value-1234"}"#,
    );
    stopped(&fx, &inv, &["group relay_hosts: storage.password: a secret key", "never imports a secret"]).await;
    // The value never shows.
    let out = fx.import(&fx.path("inventory.json"), "production", false);
    assert!(!stderr(&out).contains("s3cret-value-1234"));
    // A secret key in the vars of a host stops it too.
    let fx = Fixture::new();
    let inv = INVENTORY.replace(
        r#""relay_config": {"log": {"level": "debug"}}"#,
        r#""relay_config": {"storage": {"password": "host-s3cret-1234"}}"#,
    );
    stopped(&fx, &inv, &["host host-a: storage.password: a secret key"]).await;
}

#[tokio::test]
async fn a_value_that_fails_2_10_stops_the_import_with_each_key() {
    let fx = Fixture::new();
    let inv = INVENTORY.replace(
        r#"{"limits": {"max_clients": 500}, "log": {"level": "info"}, "tags": ["blue", "eu"]}"#,
        r#"{"limits": {"max_clients": 0}, "log": {"level": "trace"}, "unknown": {"key": 1}, "tags": ["a", ""]}"#,
    );
    stopped(
        &fx,
        &inv,
        &[
            "group relay_hosts: limits.max_clients:",
            "group relay_hosts: log.level:",
            "group relay_hosts: unknown.key: the schema of the unit has no such key",
            "group relay_hosts: tags:",
        ],
    )
    .await;
}

#[tokio::test]
async fn a_value_with_a_wrong_form_is_an_error_and_never_changed() {
    for (bad, needle) in [
        (r#""max_clients": 1.5"#, "limits.max_clients: expected an integer from 0 to 9007199254740991"),
        (r#""max_clients": -3"#, "limits.max_clients: expected an integer"),
        (r#""max_clients": null"#, "limits.max_clients: expected an integer"),
        (r#""max_clients": "500""#, "limits.max_clients:"),
        (r#""max_clients": 9007199254740992"#, "limits.max_clients: expected an integer"),
        (r#""max_clients": [1, 2]"#, "limits.max_clients: expected an integer"),
    ] {
        let fx = Fixture::new();
        let inv = INVENTORY.replace(r#""max_clients": 500"#, bad);
        stopped(&fx, &inv, &[needle]).await;
    }
    // A key path that is not valid is not repaired.
    let fx = Fixture::new();
    let inv = INVENTORY.replace(r#""limits": {"#, r#""Limits-X": {"#);
    stopped(&fx, &inv, &["Limits-X.max_clients: expected a key path"]).await;
    // The variable is not an object.
    let fx = Fixture::new();
    let inv = INVENTORY.replace(r#""relay_config": {"log": {"level": "debug"}}"#, r#""relay_config": "text value""#);
    stopped(&fx, &inv, &["host host-a: relay_config: expected an object, found a string"]).await;
}

#[tokio::test]
async fn the_hosts_and_the_units_are_checked() {
    for (from, to, needle) in [
        (r#""ansible_host": "203.0.113.12""#, r#""ansible_user": "x""#, "host host-c: ansible_host is missing"),
        (
            r#""ansible_port": 2222"#,
            r#""ansible_port": "2222""#,
            "host host-a: ansible_port: expected an integer from 1 to 65535",
        ),
        (
            r#""ansible_port": 2222"#,
            r#""ansible_port": 70000"#,
            "host host-a: ansible_port: expected an integer from 1 to 65535",
        ),
        (r#""ansible_host": "203.0.113.10""#, r#""ansible_host": 5"#, "host host-a: ansible_host: expected a text"),
        (r#""ansible_host": "203.0.113.10""#, r#""ansible_host": "203.0.113.010""#, "host host-a:"),
        (r#""host-c": {"#, r#""Host_C": {"#, "host Host_C: expected a host id"),
    ] {
        let fx = Fixture::new();
        let inv = INVENTORY.replace(from, to);
        stopped(&fx, &inv, &[needle]).await;
    }
}

#[tokio::test]
async fn a_schema_with_no_deploy_playbook_stops_the_import() {
    let fx = Fixture::new();
    let text = fx.config_text(true).replace("playbook \"deploy-feed\"", "playbook \"other-feed\"");
    std::fs::write(fx.config(), text).unwrap();
    stopped(&fx, INVENTORY, &["unit feed: the configuration has no block playbook \"deploy-feed\" of the kind deploy"])
        .await;
    // A playbook with the id but another kind is also refused.
    let fx = Fixture::new();
    let text = fx
        .config_text(true)
        .replace("\"render-relay\" {\n  kind = \"render\"", "\"render-relay\" {\n  kind = \"status\"");
    std::fs::write(fx.config(), text).unwrap();
    stopped(&fx, INVENTORY, &["unit relay: the playbook render-relay has the kind status, expected the kind render"])
        .await;
}

#[tokio::test]
async fn a_schema_with_no_group_in_the_inventory_makes_no_unit_and_an_empty_group_makes_a_unit_with_no_host() {
    let fx = Fixture::new();
    fx.migrate();
    let inv = INVENTORY.replace(r#""feed_hosts": {"hosts": ["host-c"], "vars": {"feed_config": {"feed": {"enabled": true, "url": "https://feed.example.invalid/v1"}}}},"#, "");
    let path = fx.write_inventory(&inv);
    let out = fx.import(&path, "production", true);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let docs = documents(&fx.open()).await;
    assert!(!docs.keys().any(|k| k.contains("units/feed")));
    assert!(docs.contains_key("environments/production/hosts/host-c"));
    // An empty group.
    let fx = Fixture::new();
    fx.migrate();
    let inv = INVENTORY.replace(r#""feed_hosts": {"hosts": ["host-c"],"#, r#""feed_hosts": {"hosts": [],"#);
    let path = fx.write_inventory(&inv);
    let out = fx.import(&path, "production", true);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let docs = documents(&fx.open()).await;
    assert_eq!(
        docs["environments/production/units/feed"].bytes,
        doc(
            DocKind::Unit,
            r#"{"format": 1, "label": "feed", "schema": "feed", "hosts": [], "playbooks": {"deploy": "deploy-feed"}}"#
        )
    );
}

#[tokio::test]
async fn a_group_with_children_has_the_hosts_of_its_children_and_a_label_is_not_a_schema_group() {
    let fx = Fixture::new();
    fx.migrate();
    let inv = r#"{
      "_meta": {"hostvars": {
        "host-a": {"ansible_host": "203.0.113.10"},
        "host-b": {"ansible_host": "203.0.113.11"}
      }},
      "all": {"children": ["ungrouped", "region_eu", "relay_hosts"]},
      "region_eu": {"children": ["eu_west"]},
      "eu_west": {"hosts": ["host-a"]},
      "relay_hosts": {"hosts": ["host-a", "host-b"], "children": ["eu_west"]},
      "Not-A-Label": {"hosts": ["host-b"]},
      "ungrouped": {}
    }"#;
    let path = fx.write_inventory(inv);
    let out = fx.import(&path, "production", true);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("host-a  address=203.0.113.10  labels=eu_west,region_eu"), "{text}");
    // `relay_hosts` is the group of a schema: it is not a label.
    assert!(!text.contains("relay_hosts,") && !text.contains(",relay_hosts"), "{text}");
    assert!(text.contains("groups that are not host labels, left out: Not-A-Label"), "{text}");
    let docs = documents(&fx.open()).await;
    assert_eq!(docs.len(), 1 + 2 + 1);
}

#[tokio::test]
async fn an_inventory_that_is_not_valid_stops_the_command() {
    let fx = Fixture::new();
    fx.migrate();
    for (text, needle) in [
        ("not json", "the inventory is not valid JSON"),
        ("[]", "expected an object"),
        (r#"{"relay_hosts": 5}"#, "group relay_hosts: expected an object"),
        (r#"{"relay_hosts": {"hosts": "host-a"}}"#, "group relay_hosts: hosts: expected a list of texts"),
        (r#"{"_meta": {"hostvars": {"host-a": 1}}}"#, "_meta.hostvars.host-a: expected an object"),
    ] {
        let path = fx.write_inventory(text);
        let out = fx.import(&path, "production", true);
        assert_eq!(out.status.code(), Some(1), "{text}");
        assert!(stderr(&out).contains(needle), "{text}: {}", stderr(&out));
    }
    let out = fx.import(&fx.path("absent.json"), "production", false);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--inventory-json"));
    // A folder is not an inventory.
    let out = fx.import(fx.dir.path(), "production", false);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("expected a regular file"));
}

#[tokio::test]
async fn the_limits_of_2_42_stop_the_import() {
    let fx = Fixture::new();
    fx.migrate();
    let text = fx.config_text(true) + "limits {\n  max_hosts_per_environment = 2\n}\n";
    std::fs::write(fx.config(), text).unwrap();
    let path = fx.write_inventory(INVENTORY);
    let out = fx.import(&path, "production", true);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("limits.max_hosts_per_environment: expected 2 hosts at most, found 3"),
        "{}",
        stderr(&out)
    );
    // A unit with 400 hosts has a document of more than 4096 bytes, the smallest limit.
    let text = fx.config_text(true) + "limits {\n  max_document_bytes = 4096\n}\n";
    std::fs::write(fx.config(), text).unwrap();
    let hosts: Vec<String> = (0..400).map(|i| format!("host-{i:04}")).collect();
    let hostvars: Vec<String> =
        hosts.iter().map(|h| format!("\"{h}\": {{\"ansible_host\": \"{h}.example.invalid\"}}")).collect();
    let big = format!(
        "{{\"_meta\": {{\"hostvars\": {{{}}}}}, \"relay_hosts\": {{\"hosts\": [{}]}}}}",
        hostvars.join(","),
        hosts.iter().map(|h| format!("\"{h}\"")).collect::<Vec<_>>().join(",")
    );
    let big_path = fx.write_inventory(&big);
    let out = fx.import(&big_path, "production", true);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("limits.max_document_bytes: expected 4096 bytes at most"), "{}", stderr(&out));
    let path = fx.write_inventory(INVENTORY);
    let text = fx.config_text(true) + "limits {\n  max_environments = 1\n}\n";
    std::fs::write(fx.config(), text).unwrap();
    assert_eq!(fx.import(&path, "production", true).status.code(), Some(0));
    let out = fx.import(&path, "staging", true);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("limits.max_environments"), "{}", stderr(&out));
}

#[tokio::test]
async fn a_store_that_a_server_holds_stops_the_import() {
    let fx = Fixture::new();
    fx.migrate();
    let path = fx.write_inventory(INVENTORY);
    let _held = fx.open();
    let out = fx.import(&path, "production", false);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("is locked by another process"), "{}", stderr(&out));
}

#[tokio::test]
async fn a_store_with_no_store_schema_stops_the_import() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.path("docs")).unwrap();
    std::fs::create_dir_all(fx.path("runs")).unwrap();
    std::fs::create_dir_all(fx.path("secrets")).unwrap();
    std::fs::set_permissions(fx.path("secrets"), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let path = fx.write_inventory(INVENTORY);
    let out = fx.import(&path, "production", false);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("run pupitre migrate"), "{}", stderr(&out));
}

#[tokio::test]
async fn the_command_line_refuses_a_relative_path_and_a_bad_environment() {
    let fx = Fixture::new();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_pupitre")).env_clear().args(args).output().unwrap()
    };
    let cfg = fx.config().display().to_string();
    assert_eq!(
        run(&["import", "--config", &cfg, "--inventory-json", "inventory.json", "--environment", "p"]).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&["import", "--config", &cfg, "--inventory-json", "/x.json", "--environment", "Bad"]).status.code(),
        Some(2)
    );
    assert_eq!(run(&["import", "--config", &cfg, "--inventory-json", "/x.json"]).status.code(), Some(2));
}

#[tokio::test]
async fn without_a_runner_the_units_name_the_deploy_playbook_with_no_check() {
    let fx = Fixture::new();
    fx.migrate();
    let text = fx.config_text(false);
    let start = text.find("runner {").unwrap();
    let end = start + text[start..].find("\n}\n").unwrap() + 3;
    let text = format!("{}runner {{\n  enabled = false\n}}\n{}", &text[..start], &text[end..]);
    std::fs::write(fx.config(), text).unwrap();
    let path = fx.write_inventory(INVENTORY);
    let out = fx.import(&path, "production", true);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let docs = documents(&fx.open()).await;
    assert_eq!(
        docs["environments/production/units/relay"].bytes,
        doc(
            DocKind::Unit,
            r#"{"format": 1, "label": "relay", "schema": "relay", "hosts": ["host-a", "host-b"], "playbooks": {"deploy": "deploy-relay"}}"#
        )
    );
}
