// SPDX-License-Identifier: MIT OR Apache-2.0
//! The snapshot builder (4.9 to 4.16): the checks of the inputs, the immutable folder, the
//! private folder of the secrets and the hash.
//!
//! `build_snapshot` is a function of its inputs. The mode, the run id and the times are not in
//! any file of `snapshot/`. Thus a check and its apply with the same inputs have the same
//! hash (4.16).

pub mod disk;
mod model;
pub mod yaml;

/// The file of the parameters of a command, below `snapshot/` (4.14).
pub const PARAMS_FILE: &str = "vars/params.yml";

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use pupitre_core::capability::Capabilities;
use pupitre_core::error::FieldError;
use pupitre_core::etag::sha256_hex;
use pupitre_core::ids::{HostId, KeyPath, UnitId};
use pupitre_core::json::{self, Json};
use pupitre_core::manifest::{Manifest, ManifestPlaybook, ManifestSettings, SnapshotHash};
use pupitre_core::node::{Node, SecretScope};
use pupitre_core::run::RunRecord;
use pupitre_store::secret::SecretValue;
use pupitre_store::types::StoredDoc;

use crate::child::{RunFolders, ansible_cfg};
use crate::events::PLUGIN;
use crate::settings::RunnerSettings;

/// The inputs of one snapshot (4.9).
#[derive(Debug)]
pub struct SnapshotInput<'a> {
    /// The run record: the kind, the node, the hosts, the command and the parameters.
    pub run: &'a RunRecord,
    /// The documents of the environment, from `read_environment` (2.23).
    pub documents: &'a [StoredDoc],
    /// The catalog of capabilities (4.2).
    pub capabilities: &'a Capabilities,
    /// The secret values of the units of the run (4.13), from `read_secrets`.
    pub secrets: &'a [(SecretScope, KeyPath, SecretValue)],
    /// The settings of the runner node.
    pub settings: &'a RunnerSettings,
    /// The fingerprints of the secrets, by scope and name (2.26). The store keys give them.
    pub fingerprints: &'a BTreeMap<(SecretScope, KeyPath), String>,
}

/// A snapshot on disk.
#[derive(Debug)]
pub struct BuiltSnapshot {
    /// The manifest.
    pub manifest: Manifest,
    /// The hash of the manifest (4.16).
    pub hash: SnapshotHash,
    /// True when the run has `vars/params.yml`.
    pub has_params: bool,
    /// The secret values of the private folder of the run (4.13). The scrubber uses them (4.21).
    pub secret_values: Vec<SecretValue>,
}

/// A failure of the build (4.23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// A check of 4.9 failed: the reason `invalid_values`, with the errors for the output.
    InvalidValues(Vec<FieldError>),
    /// A file could not be written or read: the reason `snapshot_failed`.
    Io(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::InvalidValues(errors) => write!(f, "{} input errors", errors.len()),
            SnapshotError::Io(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for SnapshotError {}

fn io_error(what: &str, path: &Path, error: std::io::Error) -> SnapshotError {
    SnapshotError::Io(format!("{what} {}: {error}", path.display()))
}

/// The text of a secret file in a variable file (4.12): Ansible replaces `pupitre_secrets_dir`
/// with the value of `run-vars.json`.
fn secret_file_text(scope: &SecretScope, name: &KeyPath) -> String {
    format!("{{{{ pupitre_secrets_dir }}}}/{}/{}", scope.folder(), name)
}

/// The path of a secret file below the folder `values` (4.13).
fn secret_relative_path(scope: &SecretScope, name: &KeyPath) -> String {
    format!("{}/{}", scope.folder(), name)
}

fn canonical(value: &Json) -> Vec<u8> {
    json::to_canonical(value)
}

/// Build the snapshot of a run in its folders (4.9 to 4.16), and write the private folder of
/// the secrets (4.13).
pub fn build_snapshot(input: &SnapshotInput<'_>, folders: &RunFolders) -> Result<BuiltSnapshot, SnapshotError> {
    let plan = model::plan(input)?;

    // A new build replaces the folders of an old build of the same run (a retry before the
    // child starts). No child runs at this point.
    let snapshot = folders.snapshot();
    for path in [snapshot.clone(), folders.secrets.clone()] {
        disk::remove_tree(&path).map_err(|e| io_error("remove", &path, e))?;
    }
    for path in [&folders.run, &folders.run.join("home"), &folders.run.join("tmp")] {
        disk::make_dir(path).map_err(|e| io_error("make the folder", path, e))?;
    }

    // The files of `snapshot/`. The inventory and `vars/run.json` hold no text of a document:
    // they are in the canonical form of 2.4. The files with values of documents or parameters
    // are in the YAML form of `yaml`, with the tag `!unsafe` (4.10).
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert("inventory/hosts.yml".to_string(), canonical(&plan.inventory));
    for (host, vars) in &plan.host_vars {
        files.insert(format!("inventory/host_vars/{host}.yml"), vars.to_bytes());
    }
    files.insert("vars/run.json".to_string(), canonical(&plan.run_vars));
    if let Some(params) = &plan.params_vars {
        files.insert(PARAMS_FILE.to_string(), params.to_bytes());
    }
    let has_params = plan.params_vars.is_some();

    let mut manifest_files = BTreeMap::new();
    for (name, bytes) in &files {
        let path = snapshot.join(name);
        disk::write_file(&path, bytes, 0o600).map_err(|e| io_error("write", &path, e))?;
        manifest_files.insert(name.clone(), sha256_hex(bytes));
    }
    let plugin_path = snapshot.join("callback_plugins/pupitre_events.py");
    disk::write_file(&plugin_path, PLUGIN, 0o600).map_err(|e| io_error("write", &plugin_path, e))?;

    // The playbooks and the file of the host keys: the hash covers their bytes.
    let mut playbooks = BTreeMap::new();
    for (role, spec) in &plan.playbooks {
        // The folder can change after the start: check it again for each run (4.2, 7.8).
        crate::executor::checks::check_playbook_folder(spec).map_err(|e| SnapshotError::Io(e.to_string()))?;
        let bytes = std::fs::read(&spec.path).map_err(|e| io_error("read the playbook", &spec.path, e))?;
        playbooks.insert(
            role.to_string(),
            ManifestPlaybook {
                id: spec.id.clone(),
                path: spec.path.to_string_lossy().into_owned(),
                sha256: sha256_hex(&bytes),
            },
        );
    }
    let known_hosts = std::fs::read(&input.settings.known_hosts_file)
        .map_err(|e| io_error("read the known hosts file", &input.settings.known_hosts_file, e))?;

    let mut secrets: BTreeMap<String, BTreeMap<KeyPath, String>> = BTreeMap::new();
    for (scope, name, _) in &plan.private_files {
        let fingerprint = input
            .fingerprints
            .get(&(scope.clone(), name.clone()))
            .ok_or_else(|| SnapshotError::Io(format!("no fingerprint for the secret {} of {}", name, scope.text())))?;
        secrets.entry(scope.text()).or_default().insert(name.clone(), fingerprint.clone());
    }

    let manifest = Manifest {
        environment: input.run.environment.clone(),
        kind: input.run.kind,
        files: manifest_files,
        callback_plugin: sha256_hex(PLUGIN),
        playbooks,
        secrets,
        settings: ManifestSettings {
            forks: u64::from(input.settings.forks),
            known_hosts_sha256: sha256_hex(&known_hosts),
            ssh_key_file: input.settings.ssh_key_file.to_string_lossy().into_owned(),
            ssh_user: input.settings.ssh_user.clone(),
        },
    };
    let manifest_path = snapshot.join("manifest.json");
    disk::write_file(&manifest_path, &manifest.canonical(), 0o600).map_err(|e| io_error("write", &manifest_path, e))?;
    // The modes change after the last file (4.10).
    disk::make_read_only(&snapshot).map_err(|e| io_error("change the modes of", &snapshot, e))?;

    // Outside the snapshot: the settings of Ansible, the variables that change with each run,
    // and the private folder of the secrets.
    let cfg = folders.run.join("ansible.cfg");
    disk::write_file(&cfg, ansible_cfg(input.settings, folders).as_bytes(), 0o600)
        .map_err(|e| io_error("write", &cfg, e))?;
    let secrets_values = folders.secrets.join("values");
    let run_vars = Json::object([
        ("pupitre_render_dir", Json::str(folders.render().to_string_lossy().into_owned())),
        ("pupitre_run_id", Json::str(input.run.id.to_string())),
        ("pupitre_secrets_dir", Json::str(secrets_values.to_string_lossy().into_owned())),
    ]);
    let run_vars_path = folders.run.join("run-vars.json");
    disk::write_file(&run_vars_path, &canonical(&run_vars), 0o600).map_err(|e| io_error("write", &run_vars_path, e))?;
    disk::make_dir(&secrets_values).map_err(|e| io_error("make the folder", &secrets_values, e))?;
    let render = folders.render();
    disk::make_dir(&render).map_err(|e| io_error("make the folder", &render, e))?;
    for (scope, name, value) in &plan.private_files {
        let path = secrets_values.join(secret_relative_path(scope, name));
        disk::write_file(&path, value.expose(), 0o600).map_err(|e| io_error("write the secret file", &path, e))?;
    }

    let hash = manifest.hash();
    let secret_values = plan.private_files.into_iter().map(|(_, _, v)| v).collect();
    Ok(BuiltSnapshot { manifest, hash, has_params, secret_values })
}

/// The unit of a unit node or a host node. `None` for another node.
pub(crate) fn node_unit(node: &Node) -> Option<&UnitId> {
    match node {
        Node::Unit(_, u) | Node::Host(_, u, _) => Some(u),
        _ => None,
    }
}

/// The hosts of the run as a set.
pub(crate) fn host_set(run: &RunRecord) -> BTreeSet<&HostId> {
    run.hosts.iter().collect()
}
