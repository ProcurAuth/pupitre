// SPDX-License-Identifier: MIT OR Apache-2.0
//! A fixture environment: 2 units, 3 hosts, 2 schemas, 1 label, one common document, and the
//! catalog of capabilities with their playbook files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pupitre_core::capability::{Capabilities, PlaybookKind, PlaybookSpec, SchemaSpec};
use pupitre_core::command::{CommandSpec, ParamSpec, ParamType, Target};
use pupitre_core::doc::{DocKey, Document};
use pupitre_core::etag::Etag;
use pupitre_core::ids::{CommandId, EnvId, HostId, KeyPath, ParamName, PlaybookId, RunId, SchemaId, Subject, UnitId};
use pupitre_core::json::{self, ReadLimits};
use pupitre_core::node::{Node, SecretScope};
use pupitre_core::run::{RunKind, RunMode, RunRecord, RunStatus};
use pupitre_core::schema::UnitSchema;
use pupitre_core::time::UtcMillis;
use pupitre_runner::RunnerSettings;
use pupitre_store::types::StoredDoc;

pub const RELAY_SCHEMA: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
    {"description": "Banner.", "kind": "text", "max": 256, "min": 0, "path": "banner", "rule": "any"},
    {"default": 100, "description": "Clients.", "kind": "integer", "max": 100000, "min": 1, "path": "limits.max_clients"},
    {"default": "info", "description": "Level.", "kind": "enum", "path": "log.level", "values": ["info", "debug"]},
    {"description": "Password.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true}
]}"#;

pub const FEED_SCHEMA: &str = r#"{"format": 1, "group": "feed_hosts", "variable": "feed_config", "keys": [
    {"description": "Token.", "kind": "text", "max": 64, "min": 8, "path": "api.token", "rule": "any", "secret": true},
    {"default": "info", "description": "Level.", "kind": "enum", "path": "log.level", "values": ["info", "debug"]}
]}"#;

pub fn env_id() -> EnvId {
    EnvId::parse("production").unwrap()
}

/// A stored document from a JSON text.
pub fn stored(key: DocKey, text: &str) -> StoredDoc {
    let value = json::parse(text.as_bytes(), ReadLimits::bytes(1 << 20)).unwrap();
    let doc = Document::from_json(key.kind(), &value).unwrap();
    let canonical = doc.canonical();
    StoredDoc { key, etag: canonical.etag, bytes: canonical.bytes, version: 1 }
}

fn host(h: &str) -> HostId {
    HostId::parse(h).unwrap()
}

fn unit(u: &str) -> UnitId {
    UnitId::parse(u).unwrap()
}

/// The documents of the environment `production`.
pub fn documents() -> Vec<StoredDoc> {
    documents_in("production")
}

/// The documents of an environment: the same units and hosts.
pub fn documents_in(env: &str) -> Vec<StoredDoc> {
    let e = EnvId::parse(env).unwrap();
    vec![
        stored(DocKey::Environment(e.clone()), r#"{"format": 1, "label": "Production"}"#),
        stored(
            DocKey::Host(e.clone(), host("host-a")),
            r#"{"format": 1, "address": "host-a.example.invalid", "labels": ["canary"], "port": 22}"#,
        ),
        stored(DocKey::Host(e.clone(), host("host-b")), r#"{"format": 1, "address": "203.0.113.11", "labels": []}"#),
        stored(DocKey::Host(e.clone(), host("host-c")), r#"{"format": 1, "address": "203.0.113.12", "labels": []}"#),
        stored(
            DocKey::Unit(e.clone(), unit("relay")),
            r#"{"format": 1, "hosts": ["host-a", "host-b"], "label": "Relay", "playbooks": {"deploy": "deploy-relay", "render": "render-relay", "status": "status-relay"}, "schema": "relay"}"#,
        ),
        stored(
            DocKey::Unit(e.clone(), unit("feed")),
            r#"{"format": 1, "hosts": ["host-b", "host-c"], "label": "Feed", "playbooks": {"deploy": "deploy-feed"}, "schema": "feed"}"#,
        ),
        stored(DocKey::Common(e.clone()), r#"{"format": 1, "values": {"log.level": "debug"}}"#),
        stored(
            DocKey::UnitValues(e.clone(), unit("relay")),
            r#"{"format": 1, "values": {"banner": "Say \"hi\" \\ caf\u00e9 \ud83d\ude00 {x} }}", "limits.max_clients": 300}}"#,
        ),
        stored(
            DocKey::HostValues(e, unit("relay"), host("host-a")),
            r#"{"format": 1, "values": {"limits.max_clients": 500}, "notes": {"limits.max_clients": "Canary."}}"#,
        ),
    ]
}

/// The playbook files and the settings of a runner node, in a folder.
pub struct Node1 {
    pub root: PathBuf,
    pub settings: RunnerSettings,
    pub caps: Capabilities,
}

pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The settings of a runner node whose folders are below `root`.
pub fn settings(root: &Path) -> RunnerSettings {
    RunnerSettings {
        work_dir: root.join("work"),
        secrets_dir: root.join("secrets"),
        ansible_playbook: root.join("bin/ansible-playbook"),
        path: "/usr/bin:/bin".into(),
        ssh_user: "deploy".into(),
        ssh_key_file: root.join("ssh/id_ed25519"),
        known_hosts_file: root.join("ssh/known_hosts"),
        releases_dir: None,
        forks: 5,
        max_parallel_runs: 1,
        check_max_age: Duration::from_secs(1800),
        run_timeout: Duration::from_secs(7200),
        cancel_grace: Duration::from_millis(300),
        lease_ttl: Duration::from_secs(30),
        max_log_bytes: 16 << 20,
        max_events: 100_000,
        max_event_bytes: 65_536,
        runs_keep: 10_000,
        poll_interval: Duration::from_millis(50),
    }
}

fn playbook(root: &Path, id: &str, kind: PlaybookKind) -> PlaybookSpec {
    let path = root.join("playbooks").join(format!("{id}.yml"));
    write(&path, &format!("# playbook {id}\n"));
    PlaybookSpec { id: PlaybookId::parse(id).unwrap(), kind, path }
}

/// The catalog of capabilities: 2 schemas, 5 playbooks and 2 commands.
pub fn capabilities(root: &Path) -> Capabilities {
    let mut caps = Capabilities::default();
    for (id, text) in [("relay", RELAY_SCHEMA), ("feed", FEED_SCHEMA)] {
        let schema = UnitSchema::parse(text.as_bytes()).unwrap();
        let id = SchemaId::parse(id).unwrap();
        caps.schemas.insert(id.clone(), SchemaSpec { id, path: root.join("schema.json"), schema });
    }
    for (id, kind) in [
        ("deploy-relay", PlaybookKind::Deploy),
        ("render-relay", PlaybookKind::Render),
        ("status-relay", PlaybookKind::Status),
        ("deploy-feed", PlaybookKind::Deploy),
        ("restart-service", PlaybookKind::Command),
        ("flush-cache", PlaybookKind::Command),
    ] {
        let spec = playbook(root, id, kind);
        caps.playbooks.insert(spec.id.clone(), spec);
    }
    let restart = CommandSpec {
        id: CommandId::parse("restart-service").unwrap(),
        label: "Restart".into(),
        playbook: PlaybookId::parse("restart-service").unwrap(),
        target: Target::Unit,
        check: true,
        params: vec![
            ParamSpec {
                name: ParamName::parse("grace").unwrap(),
                label: "Grace".into(),
                required: true,
                ty: ParamType::Duration { min_ms: 1000, max_ms: 300_000 },
            },
            ParamSpec {
                name: ParamName::parse("reason").unwrap(),
                label: "Reason".into(),
                required: false,
                ty: ParamType::Text { min: 0, max: 200 },
            },
        ],
    };
    let flush = CommandSpec {
        id: CommandId::parse("flush-cache").unwrap(),
        label: "Flush".into(),
        playbook: PlaybookId::parse("flush-cache").unwrap(),
        target: Target::Host,
        check: false,
        params: vec![],
    };
    caps.commands.insert(restart.id.clone(), restart);
    caps.commands.insert(flush.id.clone(), flush);
    caps
}

/// Make the files that the checks of 7.8 need, and give the node.
pub fn node(root: &Path) -> Node1 {
    write(&root.join("ssh/known_hosts"), "production.host-a ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample\n");
    write(
        &root.join("ssh/id_ed25519"),
        "-----BEGIN OPENSSH PRIVATE KEY-----\nexample\n-----END OPENSSH PRIVATE KEY-----\n",
    );
    std::fs::set_permissions(root.join("ssh/id_ed25519"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    Node1 { root: root.to_path_buf(), settings: settings(root), caps: capabilities(root) }
}

pub fn run_id(n: u8) -> RunId {
    RunId::from_parts(1_700_000_000_000 + u64::from(n), [n; 10])
}

/// A queued run record.
pub fn run_record(id: RunId, kind: RunKind, mode: RunMode, node: &str, hosts: &[&str], playbook: &str) -> RunRecord {
    run_record_in("production", id, kind, mode, node, hosts, playbook)
}

/// A queued run record in an environment. The node path names the same environment.
pub fn run_record_in(
    env: &str,
    id: RunId,
    kind: RunKind,
    mode: RunMode,
    node: &str,
    hosts: &[&str],
    playbook: &str,
) -> RunRecord {
    RunRecord {
        id,
        kind,
        mode,
        environment: EnvId::parse(env).unwrap(),
        node: Node::parse_path(node).unwrap(),
        hosts: hosts.iter().map(|h| host(h)).collect(),
        playbook: PlaybookId::parse(playbook).unwrap(),
        command: None,
        params: None,
        check_run: None,
        applied_by: None,
        message: None,
        subject: Subject::parse("alice").unwrap(),
        created_at: UtcMillis(1_700_000_000_000),
        started_at: None,
        finished_at: None,
        status: RunStatus::Queued,
        reason: None,
        snapshot: None,
        runner: None,
        exit_code: None,
        stats: None,
        cancel: None,
        output_truncated: false,
        events_truncated: false,
    }
}

/// The secrets of the fixture: the password of `relay`, the token of `feed`.
pub fn secrets() -> Vec<(SecretScope, KeyPath, Vec<u8>, String)> {
    let e = env_id();
    vec![
        (
            SecretScope::Unit(e.clone(), unit("relay")),
            KeyPath::parse("storage.password").unwrap(),
            b"relay-pass-1234".to_vec(),
            "1111111111111111".to_string(),
        ),
        (
            SecretScope::Environment(e),
            KeyPath::parse("api.token").unwrap(),
            b"token-value-5678".to_vec(),
            "2222222222222222".to_string(),
        ),
    ]
}

pub fn fingerprints() -> BTreeMap<(SecretScope, KeyPath), String> {
    secrets().into_iter().map(|(s, n, _, f)| ((s, n), f)).collect()
}

pub fn etag(bytes: &[u8]) -> Etag {
    Etag::of(bytes)
}
