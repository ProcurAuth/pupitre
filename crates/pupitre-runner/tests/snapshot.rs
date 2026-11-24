// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the snapshot builder (4.9 to 4.16): golden files, the hash, the modes and the
//! checks of the inputs.

mod support;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use pupitre_core::doc::DocKey;
use pupitre_core::ids::{KeyPath, RunId};
use pupitre_core::json::{self, Json, ReadLimits};
use pupitre_core::node::SecretScope;
use pupitre_core::run::{RunKind, RunMode, RunRecord};
use pupitre_core::time::UtcMillis;
use pupitre_runner::child::RunFolders;
use pupitre_runner::events::PLUGIN;
use pupitre_runner::snapshot::{BuiltSnapshot, SnapshotError, SnapshotInput, build_snapshot, disk};
use pupitre_store::secret::SecretValue;
use pupitre_store::types::StoredDoc;
use support::fixture::{self, Node1};

/// The folder of the golden test. A fixed path makes the hash a fixed text (4.16).
const GOLDEN_ROOT: &str = "/tmp/pupitre-golden-fixture";

struct Ctx {
    node: Node1,
    docs: Vec<StoredDoc>,
    secrets: Vec<(SecretScope, KeyPath, Vec<u8>, String)>,
}

impl Ctx {
    fn new(root: &Path) -> Ctx {
        Ctx { node: fixture::node(root), docs: fixture::documents(), secrets: fixture::secrets() }
    }

    fn build(&self, run: &RunRecord) -> Result<BuiltSnapshot, SnapshotError> {
        let secrets: Vec<_> =
            self.secrets.iter().map(|(s, n, v, _)| (s.clone(), n.clone(), SecretValue::new(v.clone()))).collect();
        let fingerprints: BTreeMap<_, _> =
            self.secrets.iter().map(|(s, n, _, f)| ((s.clone(), n.clone()), f.clone())).collect();
        let input = SnapshotInput {
            run,
            documents: &self.docs,
            capabilities: &self.node.caps,
            secrets: &secrets,
            settings: &self.node.settings,
            fingerprints: &fingerprints,
        };
        build_snapshot(&input, &RunFolders::of(&self.node.settings, &run.id.to_string()))
    }

    fn folders(&self, run: &RunRecord) -> RunFolders {
        RunFolders::of(&self.node.settings, &run.id.to_string())
    }
}

fn check_run(id: RunId) -> RunRecord {
    fixture::run_record(
        id,
        RunKind::Deploy,
        RunMode::Check,
        "environment:production/unit:relay",
        &["host-a", "host-b"],
        "deploy-relay",
    )
}

fn files_below(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(base).unwrap().to_string_lossy().into_owned(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/snapshot")
}

/// 4.10 to 4.16: the exact bytes of each file of the fixture and its exact hash.
#[test]
fn golden_files_and_hash_of_the_fixture() {
    let root = PathBuf::from(GOLDEN_ROOT);
    let _ = disk::remove_tree(&root);
    std::fs::create_dir_all(&root).unwrap();
    let ctx = Ctx::new(&root);
    let run = check_run(fixture::run_id(1));
    let built = ctx.build(&run).unwrap();
    let folders = ctx.folders(&run);

    // The files that the golden folder holds: all of `snapshot/` but the plugin, then the files
    // of the run folder that depend on the inputs, and the private folder.
    let mut actual: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (name, bytes) in files_below(&folders.snapshot()) {
        if name != "callback_plugins/pupitre_events.py" {
            actual.insert(format!("snapshot/{name}"), bytes);
        }
    }
    for name in ["ansible.cfg", "run-vars.json"] {
        actual.insert(format!("run/{name}"), std::fs::read(folders.run.join(name)).unwrap());
    }
    for (name, bytes) in files_below(&folders.secrets) {
        actual.insert(format!("secrets/{name}"), bytes);
    }
    // The plugin is the plugin of the binary.
    assert_eq!(std::fs::read(folders.snapshot().join("callback_plugins/pupitre_events.py")).unwrap(), PLUGIN);

    let golden = golden_dir();
    // A missing golden folder is written, and the test fails: a person reviews the files and
    // keeps them. To refresh them, remove the folder `tests/golden/snapshot`.
    if !golden.exists() {
        for (name, bytes) in &actual {
            let name = name.replace("018bcfe5-6801-7101-8101-010101010101", "RUN_ID");
            let path = golden.join(&name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        panic!("the golden files were missing: they are written now; review them");
    }
    let expected = files_below(&golden);
    let actual_named: BTreeMap<String, Vec<u8>> =
        actual.into_iter().map(|(n, b)| (n.replace(&run.id.to_string(), "RUN_ID"), b)).collect();
    assert_eq!(actual_named.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>(), "the list of files");
    for (name, bytes) in &actual_named {
        assert_eq!(String::from_utf8_lossy(bytes), String::from_utf8_lossy(&expected[name]), "the bytes of {name}");
    }
    assert_eq!(built.hash.to_string(), "sha256:11d6b9cb8a54bfa97d576028ca858e69fb7634103c47a11bfea327cb8fa65576");
    // The manifest on disk is the manifest of the hash (4.16).
    let manifest_bytes = std::fs::read(folders.snapshot().join("manifest.json")).unwrap();
    assert_eq!(pupitre_core::manifest::SnapshotHash::of_bytes(&manifest_bytes), built.hash);
    let _ = disk::remove_tree(&root);
}

/// 4.16: each of these changes the hash: one value, one secret, one playbook byte, the known
/// hosts file and one setting. The mode, the run id, the subject and the time do not.
#[test]
fn the_hash_follows_the_inputs_and_only_them() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let base = ctx.build(&check_run(fixture::run_id(1))).unwrap().hash;

    // Not inputs: the run id, the time, the subject, the mode (an apply names its check).
    let mut other = check_run(fixture::run_id(2));
    other.created_at = UtcMillis(1_800_000_000_000);
    other.subject = pupitre_core::ids::Subject::parse("bob").unwrap();
    other.message = Some("another message".to_string());
    assert_eq!(ctx.build(&other).unwrap().hash, base, "run id, time, subject and message");
    let mut apply = check_run(fixture::run_id(3));
    apply.mode = RunMode::Apply;
    apply.check_run = Some(fixture::run_id(1));
    assert_eq!(ctx.build(&apply).unwrap().hash, base, "the mode");

    // One value.
    let mut c = Ctx::new(dir.path());
    let e = fixture::env_id();
    c.docs.retain(|d| d.key != DocKey::UnitValues(e.clone(), "relay".parse_unit()));
    c.docs.push(fixture::stored(
        DocKey::UnitValues(e.clone(), "relay".parse_unit()),
        r#"{"format": 1, "values": {"limits.max_clients": 301}}"#,
    ));
    assert_ne!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base, "one value");

    // One secret (its fingerprint).
    let mut c = Ctx::new(dir.path());
    c.secrets[0].3 = "3333333333333333".to_string();
    assert_ne!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base, "one secret");

    // One playbook byte.
    let c = Ctx::new(dir.path());
    let path = &c.node.caps.playbooks.values().find(|p| p.id.as_str() == "deploy-relay").unwrap().path;
    std::fs::write(path, "# playbook deploy-relaX\n").unwrap();
    assert_ne!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base, "one playbook byte");
    std::fs::write(path, "# playbook deploy-relay\n").unwrap();
    assert_eq!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base);

    // The render playbook is a playbook of the snapshot.
    let render = &c.node.caps.playbooks.values().find(|p| p.id.as_str() == "render-relay").unwrap().path;
    std::fs::write(render, "# changed\n").unwrap();
    assert_ne!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base, "the render playbook");
    std::fs::write(render, "# playbook render-relay\n").unwrap();

    // The known hosts file.
    let mut known = std::fs::read_to_string(&c.node.settings.known_hosts_file).unwrap();
    known.push_str("production.host-b ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOther\n");
    std::fs::write(&c.node.settings.known_hosts_file, known).unwrap();
    assert_ne!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base, "the known hosts file");

    // A setting of `ansible.cfg` that the manifest holds.
    let mut c = Ctx::new(dir.path());
    c.node.settings.forks = 6;
    assert_ne!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base, "forks");
    let mut c = Ctx::new(dir.path());
    c.node.settings.ssh_user = "other".to_string();
    assert_ne!(c.build(&check_run(fixture::run_id(1))).unwrap().hash, base, "ssh user");
}

trait ParseUnit {
    fn parse_unit(&self) -> pupitre_core::ids::UnitId;
}
impl ParseUnit for &str {
    fn parse_unit(&self) -> pupitre_core::ids::UnitId {
        pupitre_core::ids::UnitId::parse(self).unwrap()
    }
}

/// 4.10, 4.13: the modes of the snapshot and of the private folder.
#[test]
fn the_modes_of_4_10_and_4_13() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let run = check_run(fixture::run_id(1));
    ctx.build(&run).unwrap();
    let f = ctx.folders(&run);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    for entry in walk_all(&f.snapshot()) {
        let expected = if entry.is_dir() { 0o500 } else { 0o400 };
        assert_eq!(mode(&entry), expected, "{}", entry.display());
    }
    assert_eq!(mode(&f.snapshot()), 0o500);
    assert_eq!(mode(&f.secrets), 0o700);
    let values = f.secrets.join("values");
    for entry in walk_all(&values) {
        let expected = if entry.is_dir() { 0o700 } else { 0o600 };
        assert_eq!(mode(&entry), expected, "{}", entry.display());
    }
    // The value has no line end added (4.13).
    let password = values.join("environments/production/units/relay/storage.password");
    assert_eq!(std::fs::read(password).unwrap(), b"relay-pass-1234");
    for name in ["home", "tmp"] {
        assert!(f.run.join(name).is_dir(), "{name}");
    }
    // The render folder is in the private folder on tmpfs, not on the disk of the run (4.13, 4.25).
    assert!(!f.run.join("render").exists());
    assert_eq!(mode(&f.secrets.join("render")), 0o700);
    let run_vars = std::fs::read_to_string(f.run.join("run-vars.json")).unwrap();
    assert!(
        run_vars.contains(&format!("\"pupitre_render_dir\": \"{}\"", f.secrets.join("render").display())),
        "{run_vars}"
    );
    assert!(f.run.join("ansible.cfg").is_file() && f.run.join("run-vars.json").is_file());
}

fn walk_all(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        out.push(path.clone());
        if path.is_dir() {
            out.extend(walk_all(&path));
        }
    }
    out
}

/// 4.12, 4.13: no secret value in a file outside the private folder.
#[test]
fn no_secret_value_outside_the_private_folder() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let run = check_run(fixture::run_id(1));
    let built = ctx.build(&run).unwrap();
    let f = ctx.folders(&run);
    assert_eq!(built.secret_values.len(), 2);
    for (name, bytes) in files_below(&f.run) {
        for (_, _, value, _) in &ctx.secrets {
            assert!(!bytes.windows(value.len()).any(|w| w == value.as_slice()), "{name} holds a secret value");
        }
    }
}

/// 4.11, 4.12: the inventory and the variables of a host of the run and of another host.
#[test]
fn inventory_and_host_variables() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let run = check_run(fixture::run_id(1));
    ctx.build(&run).unwrap();
    let snap = ctx.folders(&run).snapshot();
    let read = |name: &str| json::parse(&std::fs::read(snap.join(name)).unwrap(), ReadLimits::bytes(1 << 20)).unwrap();
    let hosts = read("inventory/hosts.yml");
    let all = hosts.as_object().unwrap()["all"].as_object().unwrap();
    assert_eq!(all["hosts"].as_object().unwrap().keys().collect::<Vec<_>>(), ["host-a", "host-b", "host-c"]);
    assert_eq!(
        all["children"].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["canary", "feed_hosts", "pupitre_run", "relay_hosts"]
    );
    // `pupitre_secret_files` only for a host of the run (4.12, C6).
    let text = |name: &str| std::fs::read_to_string(snap.join(name)).unwrap();
    assert!(text("inventory/host_vars/host-a.yml").contains("\n  \"pupitre_secret_files\": {\n"));
    assert!(!text("inventory/host_vars/host-c.yml").contains("pupitre_secret_files"));
    // The group of the run holds the hosts of the run only (4.11, 4.17).
    let run_group = all["children"].as_object().unwrap()["pupitre_run"].as_object().unwrap();
    assert_eq!(run_group["hosts"].as_object().unwrap().keys().collect::<Vec<_>>(), ["host-a", "host-b"]);
    assert!(!snap.join("limit.txt").exists());
}

/// 4.11, 4.17: a host id that is also the name of a label does not widen the limit. The limit
/// names the group `pupitre_run`, which holds the hosts of the run only.
#[test]
fn a_host_id_that_is_also_a_label_does_not_widen_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = Ctx::new(dir.path());
    let e = fixture::env_id();
    // host-b has the label `canary`; a new host has the id `canary`.
    ctx.docs.retain(|d| d.key != DocKey::Host(e.clone(), "host-b".parse_host()));
    ctx.docs.push(fixture::stored(
        DocKey::Host(e.clone(), "host-b".parse_host()),
        r#"{"format": 1, "address": "203.0.113.11", "labels": ["canary"]}"#,
    ));
    ctx.docs.push(fixture::stored(
        DocKey::Host(e, "canary".parse_host()),
        r#"{"format": 1, "address": "203.0.113.20", "labels": []}"#,
    ));
    let run = fixture::run_record(
        fixture::run_id(7),
        RunKind::Command,
        RunMode::Apply,
        "environment:production",
        &["canary"],
        "flush-cache",
    );
    let mut run = run;
    run.command = Some(pupitre_core::ids::CommandId::parse("flush-cache").unwrap());
    run.params = Some(pupitre_core::command::CanonicalParams::default());
    let built = ctx.build(&run).unwrap();
    let snap = ctx.folders(&run).snapshot();
    let hosts =
        json::parse(&std::fs::read(snap.join("inventory/hosts.yml")).unwrap(), ReadLimits::bytes(1 << 20)).unwrap();
    let children = hosts.as_object().unwrap()["all"].as_object().unwrap()["children"].as_object().unwrap().clone();
    let members = |group: &str| {
        children[group].as_object().unwrap()["hosts"].as_object().unwrap().keys().cloned().collect::<Vec<_>>()
    };
    assert_eq!(members("canary"), ["host-a", "host-b"]);
    assert_eq!(members("pupitre_run"), ["canary"]);
    let args = pupitre_runner::child::command_args(&ctx.folders(&run), Path::new("/p.yml"), false, built.has_params);
    let at = args.iter().position(|a| a == "--limit").unwrap();
    assert_eq!(args[at + 1], "pupitre_run");
}

/// 4.14: a command has `vars/params.yml`, with a duration in milliseconds and the tag `!unsafe`
/// on each text.
#[test]
fn a_command_gets_its_parameters_file() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let mut run = fixture::run_record(
        fixture::run_id(5),
        RunKind::Command,
        RunMode::Check,
        "environment:production/unit:relay",
        &["host-a"],
        "restart-service",
    );
    run.command = Some(pupitre_core::ids::CommandId::parse("restart-service").unwrap());
    run.params = Some(pupitre_core::command::CanonicalParams(BTreeMap::from([
        ("grace".to_string(), Json::str("90s")),
        ("reason".to_string(), Json::str("disk {x} \"full\" \u{e9}")),
    ])));
    let built = ctx.build(&run).unwrap();
    assert!(built.has_params);
    let snap = ctx.folders(&run).snapshot();
    assert_eq!(
        std::fs::read_to_string(snap.join("vars/params.yml")).unwrap(),
        "{\n  \"pupitre_params\": !unsafe {\n    \"grace\": 90000,\n    \"reason\": \"disk {x} \\\"full\\\" \\xe9\"\n  }\n}\n"
    );
    assert!(built.manifest.files.contains_key("vars/params.yml"));
    let vars = std::fs::read_to_string(snap.join("vars/run.json")).unwrap();
    assert!(vars.contains("\"pupitre_command\": \"restart-service\""));
    assert!(built.manifest.playbooks.contains_key("command") && built.manifest.playbooks.len() == 1);
    // A deploy run has no parameters file.
    let deploy = ctx.build(&check_run(fixture::run_id(6))).unwrap();
    assert!(!deploy.has_params);
    assert!(!ctx.folders(&check_run(fixture::run_id(6))).snapshot().join("vars/params.yml").exists());
}

/// 4.12, 7.3: the object of each variable of a schema reaches Ansible with the tag `!unsafe`, and
/// each object and array in it too. The address, the ids and the paths of the secret files are
/// outside these objects.
#[test]
fn each_value_of_a_document_is_in_a_collection_with_the_tag_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let run = check_run(fixture::run_id(1));
    ctx.build(&run).unwrap();
    let snap = ctx.folders(&run).snapshot();
    let a = std::fs::read_to_string(snap.join("inventory/host_vars/host-a.yml")).unwrap();
    assert!(a.contains("  \"relay_config\": !unsafe {\n"), "{a}");
    assert!(a.contains("    \"limits\": !unsafe {\n") && a.contains("    \"log\": !unsafe {\n"), "{a}");
    assert!(a.contains("    \"banner\": \"Say \\\"hi\\\" \\\\ caf\\xe9 \\U0001f600 {x} }}\",\n"), "{a}");
    assert!(a.contains("  \"ansible_host\": \"host-a.example.invalid\",\n"), "{a}");
    assert!(a.contains("  \"ansible_ssh_common_args\": \"-o HostKeyAlias=production.host-a\",\n"), "{a}");
    assert!(a.is_ascii());
    // Each line that opens an object or an array below the level of the variables has the tag.
    for (name, bytes) in files_below(&snap.join("inventory/host_vars")) {
        let text = String::from_utf8(bytes).unwrap();
        let mut inside = false;
        for line in text.lines() {
            if line.starts_with("  \"") {
                inside = line.contains("_config\": ");
            }
            if inside && (line.ends_with('{') || line.ends_with('[')) {
                assert!(line.contains(": !unsafe "), "{name}: a collection with no tag: {line}");
            }
        }
    }
}

/// 4.12, 7.8: SSH finds the host key by the name `<environment>.<host>`, never by the address.
/// A host that takes the address of another host still needs the key of its own id: the key of
/// the other host does not match.
#[test]
fn the_host_key_of_a_host_is_found_by_its_environment_and_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let run = check_run(fixture::run_id(1));
    let alias = |c: &Ctx| {
        c.build(&run).unwrap();
        let text = std::fs::read_to_string(c.folders(&run).snapshot().join("inventory/host_vars/host-a.yml")).unwrap();
        let line = text.lines().find(|l| l.contains("ansible_ssh_common_args")).unwrap().to_string();
        let cfg = std::fs::read_to_string(c.folders(&run).run.join("ansible.cfg")).unwrap();
        assert!(cfg.contains(" -o CheckHostIP=no "), "{cfg}");
        line
    };
    let base = alias(&Ctx::new(dir.path()));
    assert_eq!(base, "  \"ansible_ssh_common_args\": \"-o HostKeyAlias=production.host-a\",");
    // host-a takes the address of host-b: the name of its key stays the same.
    let mut c = Ctx::new(dir.path());
    let e = fixture::env_id();
    c.docs.retain(|d| d.key != DocKey::Host(e.clone(), "host-a".parse_host()));
    c.docs.push(fixture::stored(
        DocKey::Host(e, "host-a".parse_host()),
        r#"{"format": 1, "address": "203.0.113.11", "labels": ["canary"]}"#,
    ));
    assert_eq!(alias(&c), base);
    // The same host id in another environment has another name.
    let mut c = Ctx::new(dir.path());
    c.docs = fixture::documents_in("staging");
    let mut staging = check_run(fixture::run_id(2));
    staging.environment = pupitre_core::ids::EnvId::parse("staging").unwrap();
    staging.node = pupitre_core::node::Node::parse_path("environment:staging/unit:relay").unwrap();
    c.build(&staging).unwrap();
    let text = std::fs::read_to_string(c.folders(&staging).snapshot().join("inventory/host_vars/host-a.yml")).unwrap();
    assert!(text.contains("\"-o HostKeyAlias=staging.host-a\""), "{text}");
}

/// 2.12, 4.9, 7.3: a value with a template marker that a person wrote in the store files, past
/// the checks of the server, stops the run before a file is written.
#[test]
fn a_template_marker_in_a_stored_value_stops_the_build() {
    let dir = tempfile::tempdir().unwrap();
    let e = fixture::env_id();
    let run = check_run(fixture::run_id(1));
    let mut c = Ctx::new(dir.path());
    let key = DocKey::HostValues(e.clone(), "relay".parse_unit(), "host-a".parse_host());
    let doc = c.docs.iter_mut().find(|d| d.key == key).unwrap();
    let text = "{\n  \"format\": 1,\n  \"values\": {\n    \"banner\": \"{{ lookup('pipe', 'touch M') }}\"\n  }\n}\n";
    doc.bytes = text.as_bytes().to_vec();
    let errors = errors_of(c.build(&run));
    assert!(errors.iter().any(|m| m.contains("banner") && m.contains("no template marker")), "{errors:?}");
    assert!(!c.folders(&run).snapshot().exists(), "no file before the checks pass");
}

fn errors_of(result: Result<BuiltSnapshot, SnapshotError>) -> Vec<String> {
    match result {
        Err(SnapshotError::InvalidValues(e)) => e.iter().map(|f| f.to_string()).collect(),
        other => panic!("expected InvalidValues, found {other:?}"),
    }
}

/// 4.9: the checks of the inputs stop the run before a file is written.
#[test]
fn invalid_inputs_stop_the_build() {
    let dir = tempfile::tempdir().unwrap();
    let e = fixture::env_id();
    let run = check_run(fixture::run_id(1));

    // A value that fails its kind in `unit_values` (after a change of a schema file).
    let mut c = Ctx::new(dir.path());
    c.docs.retain(|d| d.key != DocKey::UnitValues(e.clone(), "relay".parse_unit()));
    c.docs.push(fixture::stored(
        DocKey::UnitValues(e.clone(), "relay".parse_unit()),
        r#"{"format": 1, "values": {"log.level": "trace"}}"#,
    ));
    let errors = errors_of(c.build(&run));
    assert!(errors.iter().any(|m| m.contains("log.level")), "{errors:?}");
    assert!(!c.folders(&run).snapshot().exists(), "no file before the checks pass");

    // A value in `common` that fails (2.13): the layer is in the error.
    let mut c = Ctx::new(dir.path());
    c.docs.retain(|d| d.key != DocKey::Common(e.clone()));
    c.docs.push(fixture::stored(DocKey::Common(e.clone()), r#"{"format": 1, "values": {"log.level": "trace"}}"#));
    let found = match c.build(&run) {
        Err(SnapshotError::InvalidValues(errors)) => errors,
        other => panic!("{other:?}"),
    };
    assert!(found.iter().any(|f| f.layer == Some("environment") && f.path == "log.level"), "{found:?}");

    // A host without a document (2.16).
    let mut c = Ctx::new(dir.path());
    c.docs.retain(|d| d.key != DocKey::Host(e.clone(), "host-a".parse_host()));
    assert!(!errors_of(c.build(&run)).is_empty());

    // A unit whose schema is not in the catalog.
    let mut c = Ctx::new(dir.path());
    c.node.caps.schemas.remove(&pupitre_core::ids::SchemaId::parse("relay").unwrap());
    assert!(errors_of(c.build(&run)).iter().any(|m| m.contains("schema relay")));

    // A document that is not canonical.
    let mut c = Ctx::new(dir.path());
    let doc = c.docs.iter_mut().find(|d| d.key == DocKey::Common(e.clone())).unwrap();
    doc.bytes = br#"{"format":1,"values":{}}"#.to_vec();
    assert!(errors_of(c.build(&run)).iter().any(|m| m.contains("not_canonical")));

    // A playbook that is not in the catalog.
    let mut c = Ctx::new(dir.path());
    c.node.caps.playbooks.remove(&pupitre_core::ids::PlaybookId::parse("deploy-relay").unwrap());
    assert!(errors_of(c.build(&run)).iter().any(|m| m.contains("deploy-relay")));

    // A run whose playbook is not the playbook of its unit.
    let c = Ctx::new(dir.path());
    let mut wrong = check_run(fixture::run_id(1));
    wrong.playbook = pupitre_core::ids::PlaybookId::parse("deploy-feed").unwrap();
    assert!(!errors_of(c.build(&wrong)).is_empty());
}

trait ParseHost {
    fn parse_host(&self) -> pupitre_core::ids::HostId;
}
impl ParseHost for &str {
    fn parse_host(&self) -> pupitre_core::ids::HostId {
        pupitre_core::ids::HostId::parse(self).unwrap()
    }
}

/// A new build of the same run replaces the old files (a retry before the child starts).
#[test]
fn a_second_build_replaces_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let run = check_run(fixture::run_id(1));
    let first = ctx.build(&run).unwrap().hash;
    let second = ctx.build(&run).unwrap().hash;
    assert_eq!(first, second);
}

/// An unreadable playbook file is a failure of the snapshot, not of the values.
#[test]
fn a_missing_playbook_file_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let path = &ctx.node.caps.playbooks.values().find(|p| p.id.as_str() == "deploy-relay").unwrap().path;
    std::fs::remove_file(path).unwrap();
    assert!(matches!(ctx.build(&check_run(fixture::run_id(1))), Err(SnapshotError::Io(_))));
}

/// 4.34, 4.16: a command check and its apply have the same parameters file, so the same hash.
#[test]
fn a_command_check_and_its_apply_have_the_same_hash() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    let command = |n: u8, mode: RunMode| {
        let mut run = fixture::run_record(
            fixture::run_id(n),
            RunKind::Command,
            mode,
            "environment:production/unit:relay",
            &["host-a", "host-b"],
            "restart-service",
        );
        run.command = Some(pupitre_core::ids::CommandId::parse("restart-service").unwrap());
        run.params =
            Some(pupitre_core::command::CanonicalParams(BTreeMap::from([("grace".to_string(), Json::str("90s"))])));
        run
    };
    let check = ctx.build(&command(1, RunMode::Check)).unwrap().hash;
    assert_eq!(ctx.build(&command(2, RunMode::Apply)).unwrap().hash, check);
    // Another parameter value gives another hash.
    let mut other = command(3, RunMode::Check);
    other.params =
        Some(pupitre_core::command::CanonicalParams(BTreeMap::from([("grace".to_string(), Json::str("2m"))])));
    assert_ne!(ctx.build(&other).unwrap().hash, check);
    // Another command gives another hash.
    let mut flush = fixture::run_record(
        fixture::run_id(4),
        RunKind::Command,
        RunMode::Apply,
        "environment:production/unit:relay/host:host-a",
        &["host-a"],
        "flush-cache",
    );
    flush.command = Some(pupitre_core::ids::CommandId::parse("flush-cache").unwrap());
    flush.params = Some(pupitre_core::command::CanonicalParams::default());
    let built = ctx.build(&flush).unwrap();
    assert_ne!(built.hash, check);
    assert!(built.has_params, "a command has a parameters file, also when it has no parameter");
}

/// 4.2, 7.8: the snapshot of a run checks the folder of its playbooks again.
#[test]
fn a_playbook_folder_with_host_vars_stops_the_build() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new(dir.path());
    ctx.build(&check_run(fixture::run_id(1))).unwrap();
    std::fs::create_dir_all(dir.path().join("playbooks/host_vars")).unwrap();
    std::fs::write(dir.path().join("playbooks/host_vars/host-a.yml"), "x: 1\n").unwrap();
    match ctx.build(&check_run(fixture::run_id(2))) {
        Err(SnapshotError::Io(text)) => assert!(text.contains("host_vars") && text.contains("deploy-relay"), "{text}"),
        other => panic!("{other:?}"),
    }
}
