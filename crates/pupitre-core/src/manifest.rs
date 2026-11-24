// SPDX-License-Identifier: MIT OR Apache-2.0
//! The manifest of a snapshot and the snapshot hash (4.16).

use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::etag::{parse_hex32, to_hex};
use crate::ids::{EnvId, KeyPath, PlaybookId};
use crate::json::{self, Json};
use crate::run::RunKind;

/// The snapshot hash: `sha256:` and 64 lower-case hexadecimal characters (4.16).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotHash([u8; 32]);

impl SnapshotHash {
    /// The SHA-256 of bytes.
    pub fn of_bytes(bytes: &[u8]) -> SnapshotHash {
        let mut out = [0u8; 32];
        out.copy_from_slice(&Sha256::digest(bytes));
        SnapshotHash(out)
    }

    /// Read the text form.
    pub fn parse(text: &str) -> Option<SnapshotHash> {
        text.strip_prefix("sha256:").and_then(parse_hex32).map(SnapshotHash)
    }
}

impl fmt::Display for SnapshotHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", to_hex(&self.0))
    }
}

impl fmt::Debug for SnapshotHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SnapshotHash({self})")
    }
}

/// One playbook of a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestPlaybook {
    /// The id.
    pub id: PlaybookId,
    /// The absolute path, as the configuration gives it.
    pub path: String,
    /// The SHA-256 of the file, 64 hexadecimal characters.
    pub sha256: String,
}

/// The settings of `ansible.cfg` that are not paths of the run folder (4.15, 4.16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSettings {
    /// `runner.forks`
    pub forks: u64,
    /// The SHA-256 of `runner.known_hosts_file`.
    pub known_hosts_sha256: String,
    /// `runner.ssh_key_file`
    pub ssh_key_file: String,
    /// `runner.ssh_user`
    pub ssh_user: String,
}

/// The manifest of a snapshot (4.16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The environment.
    pub environment: EnvId,
    /// The kind of the run.
    pub kind: RunKind,
    /// The SHA-256 of each file of `snapshot/`, except the manifest and the plugin.
    pub files: BTreeMap<String, String>,
    /// The SHA-256 of the callback plugin.
    pub callback_plugin: String,
    /// The playbooks, by role: `render`, `deploy`, `status` or `command`.
    pub playbooks: BTreeMap<String, ManifestPlaybook>,
    /// The fingerprints of the secrets, by scope and by key path.
    pub secrets: BTreeMap<String, BTreeMap<KeyPath, String>>,
    /// The settings.
    pub settings: ManifestSettings,
}

impl Manifest {
    /// The manifest as JSON (4.16).
    pub fn to_json(&self) -> Json {
        let playbooks = self
            .playbooks
            .iter()
            .map(|(role, p)| {
                (
                    role.clone(),
                    Json::object([
                        ("id", Json::str(p.id.as_str())),
                        ("path", Json::str(p.path.clone())),
                        ("sha256", Json::str(p.sha256.clone())),
                    ]),
                )
            })
            .collect();
        let secrets = self
            .secrets
            .iter()
            .map(|(scope, keys)| {
                (
                    scope.clone(),
                    Json::Obj(keys.iter().map(|(k, fp)| (k.as_str().to_string(), Json::str(fp.clone()))).collect()),
                )
            })
            .collect();
        Json::object([
            ("callback_plugin", Json::str(self.callback_plugin.clone())),
            ("environment", Json::str(self.environment.as_str())),
            ("files", Json::Obj(self.files.iter().map(|(k, v)| (k.clone(), Json::str(v.clone()))).collect())),
            ("format", Json::Int(1)),
            ("kind", Json::str(self.kind.name())),
            ("playbooks", Json::Obj(playbooks)),
            ("secrets", Json::Obj(secrets)),
            (
                "settings",
                Json::object([
                    ("forks", Json::Int(self.settings.forks)),
                    ("known_hosts_sha256", Json::str(self.settings.known_hosts_sha256.clone())),
                    ("ssh_key_file", Json::str(self.settings.ssh_key_file.clone())),
                    ("ssh_user", Json::str(self.settings.ssh_user.clone())),
                ]),
            ),
        ])
    }

    /// The canonical bytes of `manifest.json`.
    pub fn canonical(&self) -> Vec<u8> {
        json::to_canonical(&self.to_json())
    }

    /// The snapshot hash: the SHA-256 of the canonical bytes (4.16).
    pub fn hash(&self) -> SnapshotHash {
        SnapshotHash::of_bytes(&self.canonical())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etag::sha256_hex;

    fn manifest() -> Manifest {
        let mut files = BTreeMap::new();
        files.insert("inventory/hosts.yml".to_string(), sha256_hex(b"{}"));
        let mut playbooks = BTreeMap::new();
        playbooks.insert(
            "deploy".to_string(),
            ManifestPlaybook {
                id: PlaybookId::parse("deploy-relay").unwrap(),
                path: "/srv/d.yml".into(),
                sha256: sha256_hex(b"d"),
            },
        );
        Manifest {
            environment: EnvId::parse("production").unwrap(),
            kind: RunKind::Deploy,
            files,
            callback_plugin: sha256_hex(b"plugin"),
            playbooks,
            secrets: BTreeMap::new(),
            settings: ManifestSettings {
                forks: 5,
                known_hosts_sha256: sha256_hex(b"kh"),
                ssh_key_file: "/run/secrets/ssh/id_ed25519".into(),
                ssh_user: "deploy".into(),
            },
        }
    }

    #[test]
    fn the_hash_is_stable_and_follows_each_input() {
        let a = manifest();
        assert_eq!(a.hash(), manifest().hash());
        let text = a.hash().to_string();
        assert!(text.starts_with("sha256:") && text.len() == 71);
        assert_eq!(SnapshotHash::parse(&text), Some(a.hash()));
        let mut b = manifest();
        b.settings.forks = 6;
        assert_ne!(a.hash(), b.hash());
        let mut c = manifest();
        c.secrets
            .entry("environment:production".into())
            .or_default()
            .insert(KeyPath::parse("pw").unwrap(), "0123456789abcdef".into());
        assert_ne!(a.hash(), c.hash());
    }
}
