// SPDX-License-Identifier: MIT OR Apache-2.0
//! The start of `serve` from a configuration (6.18, 6.22): the typed settings of the server, the
//! start order and the fail-fast of a start that cannot finish.

use std::time::Duration;

use pupitre::config::{self, CapturedEnvironment, LoadedConfig, Profile, ReadMode, ReferenceFormat};
use pupitre::server::state::{ModeSettings, Settings};
use pupitre::server::{ServeError, serve, startup};

const SCHEMA_FILE: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
  {"default": "info", "description": "The level.", "kind": "enum", "path": "log.level", "values": ["info", "debug"]},
  {"description": "The password.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true}
]}"#;

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn load_local(mode: ReadMode) -> (tempfile::TempDir, LoadedConfig) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("relay.json"), SCHEMA_FILE).unwrap();
    std::fs::write(dir.path().join("store-key"), format!("{KEY}\n")).unwrap();
    let text = config::render_reference(ReferenceFormat::Hcl, Profile::Local)
        .replace("/srv/ansible/schemas/relay.json", &dir.path().join("relay.json").to_string_lossy())
        .replace("/run/secrets/pupitre/store-key", &dir.path().join("store-key").to_string_lossy());
    let file = dir.path().join("pupitre.hcl");
    std::fs::write(&file, text).unwrap();
    let loaded = config::load(&file, &CapturedEnvironment::from_pairs(Vec::<(String, String)>::new()), mode)
        .unwrap_or_else(|e| panic!("{e:?}"));
    (dir, loaded)
}

#[test]
fn the_settings_of_the_server_come_from_the_typed_configuration_6_2() {
    let (_dir, loaded) = load_local(ReadMode::All);
    let c = &loaded.config;
    let s = Settings::from_config(c);
    assert_eq!(s.base_path, c.server.base_path);
    assert_eq!(s.max_connections, c.server.max_connections);
    assert_eq!(s.max_request_bytes, c.server.max_request_bytes);
    assert_eq!(s.max_inflight_bodies, c.server.max_inflight_bodies);
    assert_eq!(s.max_page_items, c.server.max_page_items);
    assert_eq!(s.idle_timeout, c.server.idle_timeout);
    assert_eq!(s.header_read_timeout, c.server.header_read_timeout);
    assert_eq!(s.body_read_timeout, c.server.body_read_timeout);
    assert_eq!(s.shutdown_grace, c.server.shutdown_grace);
    // The profile local: the mode open, the runner on and the runner node.
    assert!(matches!(s.mode, ModeSettings::Open { .. }));
    assert!(s.is_open());
    assert_eq!(s.question_budget(), u32::MAX);
    let runner = s.runner.expect("the profile local has the runner");
    assert!(runner.execute);
    assert!(runner.max_hosts > 0 && runner.max_queued_runs > 0);
    // The limits of 2.42 and their defaults.
    assert_eq!(s.limits.max_environments, 64);
    assert_eq!(s.limits.max_units_per_environment, 1000);
    assert_eq!(s.limits.max_hosts_per_environment, 1000);
    assert_eq!(s.limits.max_document_bytes, 262_144);
    assert_eq!(s.limits.max_store_bytes, 268_435_456);
    assert_eq!(s.version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_poll_period_is_the_period_of_the_backend() {
    let (_dir, loaded) = load_local(ReadMode::All);
    let period = startup::poll_interval(&loaded.config.storage);
    match &loaded.config.storage {
        pupitre::config::StorageConfig::Files(f) => assert_eq!(period, f.rescan_interval),
        pupitre::config::StorageConfig::Postgres(p) => assert_eq!(period, p.poll_interval),
    }
    assert!(period > Duration::ZERO);
}

#[test]
fn the_access_layer_of_the_mode_open_is_built_from_the_configuration_3_2() {
    let (_dir, loaded) = load_local(ReadMode::All);
    let access = startup::build_access(&loaded.config).unwrap();
    assert!(matches!(access, pupitre::access::AnyAccess::Open(_)));
}

#[tokio::test]
async fn serve_refuses_a_configuration_that_did_not_read_each_value_6_18() {
    let (_dir, loaded) = load_local(ReadMode::Storage);
    let e = serve(loaded).await.unwrap_err();
    assert!(e.to_string().contains("6.18"), "{e}");
}

#[tokio::test]
async fn a_start_that_cannot_finish_stops_before_it_serves_6_2() {
    // The profile local points at folders and files that do not exist in the test: the store, the
    // catalog or the runner stops the start. The server never reaches its listeners.
    let (_dir, loaded) = load_local(ReadMode::All);
    let result = tokio::time::timeout(Duration::from_secs(10), serve(loaded)).await.expect("serve ends");
    let ServeError::Start(text) = result.unwrap_err();
    assert!(
        ["storage:", "catalog:", "access:", "runner:"].iter().any(|step| text.starts_with(step)),
        "the message names the step: {text}"
    );
}
