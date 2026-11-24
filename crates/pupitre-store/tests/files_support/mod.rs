// SPDX-License-Identifier: MIT OR Apache-2.0
//! Helpers of the tests of the backend `files`: a fixture with the three folders, documents,
//! write metadata and run records.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use pupitre_core::doc::{CanonicalDocument, DocKey, Document, Environment, Host};
use pupitre_core::ids::{EnvId, HostId, KeyPath, PlaybookId, RunId, Subject};
use pupitre_core::node::{Node, SecretScope};
use pupitre_core::run::{RunKind, RunMode, RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;
use pupitre_store::files::{FilesOptions, FilesStore};
use pupitre_store::secret::{SecretKey, SecretValue};
use pupitre_store::{Message, WriteMeta};
use tempfile::TempDir;

/// The key of the vectors of 2.26 in `secret.rs`.
pub const KEY_A: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
/// Another key.
pub const KEY_B: &str = "abababababababababababababababababababababababababababababababab";

/// Three folders in one temporary folder, and the options of a store on them.
pub struct Fixture {
    pub dir: TempDir,
    pub history_keep: u32,
    pub key: &'static str,
}

impl Fixture {
    pub fn new() -> Fixture {
        Fixture { dir: tempfile::tempdir().unwrap(), history_keep: 100, key: KEY_A }
    }

    pub fn with_history_keep(history_keep: u32) -> Fixture {
        Fixture { history_keep, ..Fixture::new() }
    }

    pub fn docs(&self) -> PathBuf {
        self.dir.path().join("docs")
    }
    pub fn secrets(&self) -> PathBuf {
        self.dir.path().join("secrets")
    }
    pub fn runs(&self) -> PathBuf {
        self.dir.path().join("runs")
    }

    pub fn options(&self) -> FilesOptions {
        FilesOptions {
            path: self.docs(),
            secrets_path: self.secrets(),
            runs_path: self.runs(),
            rescan_interval: Duration::from_secs(5),
            history_keep: self.history_keep,
            secret_key: SecretKey::parse(self.key).unwrap(),
            max_document_bytes: 262_144,
        }
    }

    /// Make the store schema and open the store.
    pub fn open(&self) -> FilesStore {
        FilesStore::migrate(&self.options()).unwrap();
        FilesStore::open(self.options()).unwrap()
    }

    /// Open a store that `migrate` made before.
    pub fn reopen(&self) -> FilesStore {
        FilesStore::open(self.options()).unwrap()
    }
}

pub fn key(text: &str) -> DocKey {
    DocKey::parse(text).unwrap()
}

pub fn env_doc(label: &str) -> CanonicalDocument {
    Document::Environment(Environment { label: label.to_string() }).canonical()
}

pub fn host_doc(address: &str) -> CanonicalDocument {
    Document::Host(Host { address: address.to_string(), port: None, labels: Vec::new() }).canonical()
}

pub fn meta(by: &str, message: &str) -> WriteMeta {
    WriteMeta {
        saved_by: Subject::parse(by).unwrap(),
        saved_at: UtcMillis::parse("2026-11-24T10:00:00.000Z").unwrap(),
        message: Message::parse(message).unwrap(),
    }
}

pub fn env(id: &str) -> EnvId {
    EnvId::parse(id).unwrap()
}

pub fn host(id: &str) -> HostId {
    HostId::parse(id).unwrap()
}

pub fn name(path: &str) -> KeyPath {
    KeyPath::parse(path).unwrap()
}

pub fn value(text: &str) -> SecretValue {
    SecretValue::new(text.as_bytes().to_vec())
}

pub fn unit_scope(e: &str, u: &str) -> SecretScope {
    SecretScope::Unit(env(e), pupitre_core::ids::UnitId::parse(u).unwrap())
}

/// A run id with a time and a distinguishing byte.
pub fn run_id(ms: u64, n: u8) -> RunId {
    RunId::from_parts(1_795_514_400_000 + ms, [n; 10])
}

pub fn run_record(id: RunId, environment: &str, status: RunStatus) -> RunRecord {
    RunRecord {
        id,
        kind: RunKind::Deploy,
        mode: RunMode::Check,
        environment: env(environment),
        node: Node::Environment(env(environment)),
        hosts: vec![host("host-a")],
        playbook: PlaybookId::parse("deploy-relay").unwrap(),
        command: None,
        params: None,
        check_run: None,
        applied_by: None,
        message: None,
        subject: Subject::anonymous(),
        created_at: UtcMillis::parse("2026-11-24T10:00:00.000Z").unwrap(),
        started_at: None,
        finished_at: None,
        status,
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

/// Each file below a folder, with its path relative to the folder, in byte order.
pub fn files_below(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for p in entries {
            let meta = std::fs::symlink_metadata(&p).unwrap();
            if meta.is_dir() {
                walk(&p, root, out);
            } else {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

/// A document of a kind from JSON text, in the canonical form.
pub fn doc_from(kind: pupitre_core::doc::DocKind, text: &str) -> CanonicalDocument {
    let value = pupitre_core::json::parse(text.as_bytes(), pupitre_core::json::ReadLimits::bytes(1 << 20)).unwrap();
    Document::from_json(kind, &value).unwrap().canonical()
}

/// The unit `relay` with the schema `relay`.
pub fn unit_doc() -> CanonicalDocument {
    doc_from(
        pupitre_core::doc::DocKind::Unit,
        r#"{"format": 1, "hosts": ["host-a"], "label": "Relay",
            "playbooks": {"deploy": "deploy-relay"}, "schema": "relay"}"#,
    )
}

/// A document of values.
pub fn values_doc(kind: pupitre_core::doc::DocKind, text: &str) -> CanonicalDocument {
    doc_from(kind, &format!(r#"{{"format": 1, "values": {text}}}"#))
}

/// The mode bits of a path.
pub fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Each file and folder below a folder, with the folder itself.
pub fn entries_below(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            out.push(p.clone());
            if std::fs::symlink_metadata(&p).unwrap().is_dir() {
                walk(&p, out);
            }
        }
    }
    let mut out = vec![root.to_path_buf()];
    walk(root, &mut out);
    out.sort();
    out
}
