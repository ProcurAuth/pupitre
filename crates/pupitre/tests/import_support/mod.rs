// SPDX-License-Identifier: MIT OR Apache-2.0
//! Helpers of the tests of the command `import`: a configuration with two schemas and a
//! `files` store in a temporary folder, and a runner of the binary.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pupitre::config::{self, CapturedEnvironment, LoadedConfig, ReadMode, StorageConfig};
use pupitre_core::doc::DocKey;
use pupitre_store::files::FilesStore;
use pupitre_store::{Store, StoredDoc};
use tempfile::TempDir;

pub const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

const RELAY_SCHEMA: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
  {"description": "The most clients.", "kind": "integer", "max": 100000, "min": 1, "path": "limits.max_clients"},
  {"default": "info", "description": "The level.", "kind": "enum", "path": "log.level", "values": ["info", "debug"]},
  {"description": "The password.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true},
  {"description": "The tags.", "kind": "text_list", "item_max": 20, "item_min": 1, "max_items": 5, "min_items": 0, "path": "tags", "rule": "any"}
]}"#;

const FEED_SCHEMA: &str = r#"{"format": 1, "group": "feed_hosts", "variable": "feed_config", "keys": [
  {"description": "The switch.", "kind": "bool", "path": "feed.enabled"},
  {"description": "The URL.", "kind": "text", "max": 200, "min": 1, "path": "feed.url", "rule": "any"}
]}"#;

/// The inventory of the specification of the tests: 3 hosts, 2 groups of schemas, 1 label.
pub const INVENTORY: &str = r#"{
  "_meta": {"hostvars": {
    "host-a": {"ansible_host": "203.0.113.10", "ansible_port": 2222, "ansible_user": "deploy",
               "relay_config": {"log": {"level": "debug"}}},
    "host-b": {"ansible_host": "relay-b.example.invalid"},
    "host-c": {"ansible_host": "203.0.113.12"}
  }},
  "all": {"children": ["ungrouped", "relay_hosts", "feed_hosts", "canary"],
          "vars": {"ansible_python_interpreter": "/usr/bin/python3"}},
  "relay_hosts": {"hosts": ["host-a", "host-b"],
                  "vars": {"relay_config": {"limits": {"max_clients": 500}, "log": {"level": "info"}, "tags": ["blue", "eu"]}}},
  "feed_hosts": {"hosts": ["host-c"], "vars": {"feed_config": {"feed": {"enabled": true, "url": "https://feed.example.invalid/v1"}}}},
  "canary": {"hosts": ["host-a"]},
  "ungrouped": {"hosts": []}
}"#;

pub struct Fixture {
    pub dir: TempDir,
}

impl Fixture {
    pub fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let f = Fixture { dir };
        std::fs::write(f.path("relay.json"), RELAY_SCHEMA).unwrap();
        std::fs::write(f.path("feed.json"), FEED_SCHEMA).unwrap();
        std::fs::write(f.path("store-key"), format!("{KEY}\n")).unwrap();
        std::fs::write(f.path("pupitre.hcl"), f.config_text(true)).unwrap();
        f
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The configuration. With `playbooks`, it has `deploy-relay`, `render-relay` and `deploy-feed`.
    pub fn config_text(&self, playbooks: bool) -> String {
        let mut text = format!(
            r#"server {{
  listen     = "127.0.0.1:8080"
  ops_listen = "127.0.0.1:9090"
  base_path  = "/"
}}
log {{
  level = "info"
}}
telemetry {{
  enabled = false
}}
storage {{
  backend         = "files"
  secret_key_file = "{key}"
  files {{
    path         = "{docs}"
    secrets_path = "{secrets}"
    runs_path    = "{runs}"
  }}
}}
access {{
  mode = "open"
}}
runner {{
  enabled                  = true
  execute                  = true
  allow_apply_in_open_mode = false
  check_max_age            = "30m"
  max_hosts                = 100
  work_dir                 = "{work}"
  secrets_dir              = "/dev/shm/pupitre"
  ansible_playbook         = "/usr/bin/ansible-playbook"
  path                     = "/usr/local/bin:/usr/bin:/bin"
  ssh_user                 = "deploy"
  ssh_key_file             = "/run/secrets/ssh/id_ed25519"
  known_hosts_file         = "/run/secrets/ssh/known_hosts"
  run_timeout              = "2h"
}}
schema "relay" {{
  path = "{relay}"
}}
schema "feed" {{
  path = "{feed}"
}}
"#,
            key = self.path("store-key").display(),
            docs = self.path("docs").display(),
            secrets = self.path("secrets").display(),
            runs = self.path("runs").display(),
            work = self.path("work").display(),
            relay = self.path("relay.json").display(),
            feed = self.path("feed.json").display(),
        );
        if playbooks {
            for (id, kind) in [("deploy-relay", "deploy"), ("render-relay", "render"), ("deploy-feed", "deploy")] {
                text.push_str(&format!(
                    "playbook \"{id}\" {{\n  kind = \"{kind}\"\n  path = \"/srv/ansible/playbooks/{id}.yml\"\n}}\n"
                ));
            }
        }
        text
    }

    pub fn config(&self) -> PathBuf {
        self.path("pupitre.hcl")
    }

    pub fn loaded(&self) -> LoadedConfig {
        config::load(
            &self.config(),
            &CapturedEnvironment::from_pairs(Vec::<(String, String)>::new()),
            ReadMode::Storage,
        )
        .unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// Make the store schema of the `files` store of the configuration.
    pub fn migrate(&self) {
        let StorageConfig::Files(options) = self.loaded().config.storage else { panic!("files") };
        FilesStore::migrate(&options).unwrap();
    }

    pub fn open(&self) -> FilesStore {
        let StorageConfig::Files(options) = self.loaded().config.storage else { panic!("files") };
        FilesStore::open(options).unwrap()
    }

    pub fn write_inventory(&self, text: &str) -> PathBuf {
        let p = self.path("inventory.json");
        std::fs::write(&p, text).unwrap();
        p
    }

    /// Run `pupitre import` with the configuration of the fixture.
    pub fn import(&self, inventory: &Path, environment: &str, write: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pupitre"));
        cmd.env_clear().arg("import").arg("--config").arg(self.config()).arg("--inventory-json").arg(inventory);
        cmd.arg("--environment").arg(environment);
        if write {
            cmd.arg("--write");
        }
        cmd.output().unwrap()
    }
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

/// Each document of the store, by key text.
pub async fn documents(store: &FilesStore) -> std::collections::BTreeMap<String, StoredDoc> {
    store.load().await.unwrap().docs.into_iter().map(|d| (d.key.text(), d)).collect()
}

pub fn key(text: &str) -> DocKey {
    DocKey::parse(text).unwrap()
}
