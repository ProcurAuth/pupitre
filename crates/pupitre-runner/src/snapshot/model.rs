// SPDX-License-Identifier: MIT OR Apache-2.0
//! The plan of a snapshot (4.9 to 4.14): the checks of the inputs and the content of each
//! file, with no file written. The content depends on the inputs only.

use std::collections::{BTreeMap, BTreeSet};

use pupitre_core::capability::{PlaybookKind, PlaybookSpec};
use pupitre_core::command::CommandSpec;
use pupitre_core::doc::{DocKey, Document, Host, Unit, Values};
use pupitre_core::error::FieldError;
use pupitre_core::ids::{EnvId, HostId, KeyPath, UnitId};
use pupitre_core::json::Json;
use pupitre_core::layers::{LayerInputs, SecretLayers, resolve};
use pupitre_core::node::{Node, SecretScope};
use pupitre_core::run::RunKind;
use pupitre_core::validate::check_unit_values;
use pupitre_store::secret::SecretValue;

use super::yaml::{Tag, Yaml};
use super::{SnapshotError, SnapshotInput, secret_file_text};

/// The facts of one host for its variable file (4.12).
pub(super) struct HostFacts<'a> {
    /// The environment of the run.
    pub env: &'a EnvId,
    /// The id of the host.
    pub id: &'a HostId,
    /// The document `host`.
    pub host: &'a Host,
    /// The units that hold the host, in byte order.
    pub units: &'a [&'a UnitId],
}

/// The file `inventory/host_vars/<host>.yml` (4.12). `variables` holds the effective values of
/// each unit of the host, by the variable of its schema. `secret_files` holds the paths of the
/// secret files, by unit and key path: it is empty for a host that is not in the run.
///
/// The object of each variable has the tag `!unsafe`, at each depth (`yaml`). The address and
/// the ids stay plain: their characters (2.1, 2.8) cannot make a template marker. The paths of
/// the secret files stay plain, because Ansible must evaluate `{{ pupitre_secrets_dir }}` in them.
pub(super) fn host_vars_file(
    facts: &HostFacts<'_>,
    variables: &BTreeMap<String, BTreeMap<String, Json>>,
    secret_files: &BTreeMap<String, BTreeMap<String, String>>,
) -> Yaml {
    let mut vars: BTreeMap<String, Yaml> = variables.iter().map(|(k, v)| (k.clone(), Yaml::untrusted(v))).collect();
    vars.insert("ansible_host".to_string(), Yaml::Str(facts.host.address.clone()));
    if let Some(port) = facts.host.port {
        vars.insert("ansible_port".to_string(), Yaml::Int(u64::from(port)));
    }
    // The host key of the host id, not of the address (4.15, 7.8): a new address finds no key.
    vars.insert("ansible_ssh_common_args".to_string(), Yaml::Str(format!("-o HostKeyAlias={}", host_key_alias(facts))));
    vars.insert("pupitre_environment".to_string(), Yaml::Str(facts.env.as_str().to_string()));
    vars.insert(
        "pupitre_units".to_string(),
        Yaml::Seq(Tag::Plain, facts.units.iter().map(|u| Yaml::Str(u.as_str().to_string())).collect()),
    );
    if !secret_files.is_empty() {
        let files = secret_files.iter().map(|(unit, paths)| {
            (unit.clone(), Yaml::map(paths.iter().map(|(k, v)| (k.clone(), Yaml::Str(v.clone())))))
        });
        vars.insert("pupitre_secret_files".to_string(), Yaml::map(files));
    }
    Yaml::Map(Tag::Plain, vars)
}

/// The name of the host key of a host in `runner.known_hosts_file`: `<environment>.<host>` (4.12).
/// The ids have no `.`, so each pair gives one name.
pub(super) fn host_key_alias(facts: &HostFacts<'_>) -> String {
    format!("{}.{}", facts.env, facts.id)
}

/// The file `vars/params.yml` of a command (4.14): `pupitre_params`, with the tag `!unsafe`.
pub(super) fn params_file(params: &BTreeMap<String, Json>) -> Yaml {
    Yaml::map([("pupitre_params", Yaml::untrusted(params))])
}

/// The content of a snapshot before any write.
pub(super) struct Plan {
    /// `inventory/hosts.yml` (4.11).
    pub inventory: Json,
    /// `inventory/host_vars/<host>.yml` of each host of the environment (4.12).
    pub host_vars: BTreeMap<HostId, Yaml>,
    /// `vars/run.json` (4.14).
    pub run_vars: Json,
    /// `vars/params.yml` (4.14), for a command.
    pub params_vars: Option<Yaml>,
    /// The playbooks of the snapshot, by role (4.16).
    pub playbooks: BTreeMap<&'static str, PlaybookSpec>,
    /// The files of the private folder (4.13).
    pub private_files: Vec<(SecretScope, KeyPath, SecretValue)>,
}

/// The documents of one environment, parsed.
#[derive(Default)]
struct Environment {
    hosts: BTreeMap<HostId, Host>,
    units: BTreeMap<UnitId, Unit>,
    common: Option<Values>,
    unit_values: BTreeMap<UnitId, Values>,
    host_values: BTreeMap<(UnitId, HostId), Values>,
}

fn read_documents(input: &SnapshotInput<'_>, errors: &mut Vec<FieldError>) -> Environment {
    let mut env = Environment::default();
    for stored in input.documents {
        if stored.key.environment() != &input.run.environment {
            continue;
        }
        let doc = match Document::from_stored(stored.key.kind(), &stored.bytes, stored.bytes.len()) {
            Ok(doc) => doc,
            Err(found) => {
                for e in found {
                    let message = if e.path.is_empty() { e.message } else { format!("{}: {}", e.path, e.message) };
                    errors.push(FieldError::new(stored.key.text(), message));
                }
                continue;
            }
        };
        match (&stored.key, doc) {
            (DocKey::Host(_, h), Document::Host(d)) => {
                env.hosts.insert(h.clone(), d);
            }
            (DocKey::Unit(_, u), Document::Unit(d)) => {
                env.units.insert(u.clone(), d);
            }
            (DocKey::Common(_), Document::Common(v)) => env.common = Some(v),
            (DocKey::UnitValues(_, u), Document::UnitValues(v)) => {
                env.unit_values.insert(u.clone(), v);
            }
            (DocKey::HostValues(_, u, h), Document::HostValues(v)) => {
                env.host_values.insert((u.clone(), h.clone()), v);
            }
            _ => {}
        }
    }
    env
}

/// The command spec of a command run.
fn command_spec<'a>(input: &'a SnapshotInput<'_>, errors: &mut Vec<FieldError>) -> Option<&'a CommandSpec> {
    let Some(id) = &input.run.command else {
        errors.push(FieldError::new("command", "a command run names its command"));
        return None;
    };
    let spec = input.capabilities.commands.get(id);
    if spec.is_none() {
        errors.push(FieldError::new("command", format!("the command {id} is not in the catalog")));
    }
    spec
}

/// Find the playbooks of the run, by role (4.3, 4.16).
fn playbooks_of(
    input: &SnapshotInput<'_>,
    env: &Environment,
    errors: &mut Vec<FieldError>,
) -> BTreeMap<&'static str, PlaybookSpec> {
    let mut out = BTreeMap::new();
    let run = input.run;
    let caps = input.capabilities;
    let mut add =
        |role: &'static str, kind: PlaybookKind, id: &pupitre_core::ids::PlaybookId, errors: &mut Vec<_>| match caps
            .playbook_of_kind(id, kind)
        {
            Some(spec) => {
                out.insert(role, spec.clone());
            }
            None => errors.push(FieldError::new(
                "playbook",
                format!("the catalog has no playbook {id} of the kind {}", kind.name()),
            )),
        };
    match run.kind {
        RunKind::Command => {
            if let Some(spec) = command_spec(input, errors) {
                if spec.playbook != run.playbook {
                    errors.push(FieldError::new(
                        "playbook",
                        "the playbook of the run is not the playbook of the command",
                    ));
                }
                add("command", PlaybookKind::Command, &spec.playbook, errors);
            }
        }
        RunKind::Deploy | RunKind::Status => {
            let Some(unit_id) = super::node_unit(&run.node) else {
                errors.push(FieldError::new("node", "a deploy run and a status run have a unit node or a host node"));
                return out;
            };
            let Some(unit) = env.units.get(unit_id) else {
                errors.push(FieldError::new("node", format!("the unit {unit_id} has no document")));
                return out;
            };
            if run.kind == RunKind::Deploy {
                if unit.playbooks.deploy != run.playbook {
                    errors.push(FieldError::new(
                        "playbook",
                        "the playbook of the run is not the deploy playbook of the unit",
                    ));
                }
                add("deploy", PlaybookKind::Deploy, &unit.playbooks.deploy, errors);
                // The render playbook is in the snapshot of a check and of its apply, so that
                // the two have the same hash (4.16). Only a check starts it (4.25).
                if let Some(render) = &unit.playbooks.render {
                    add("render", PlaybookKind::Render, render, errors);
                }
            } else {
                match &unit.playbooks.status {
                    Some(status) if *status == run.playbook => add("status", PlaybookKind::Status, status, errors),
                    _ => errors.push(FieldError::new(
                        "playbook",
                        "the playbook of the run is not the status playbook of the unit",
                    )),
                }
            }
        }
    }
    out
}

fn resolve_layers(input: &SnapshotInput<'_>, unit_id: &UnitId) -> SecretLayers {
    let env = &input.run.environment;
    let mut layers = SecretLayers::default();
    for ((scope, name), fingerprint) in input.fingerprints {
        match scope {
            SecretScope::Environment(e) if e == env => {
                layers.environment.insert(name.clone(), fingerprint.clone());
            }
            SecretScope::Unit(e, u) if e == env && u == unit_id => {
                layers.unit.insert(name.clone(), fingerprint.clone());
            }
            _ => {}
        }
    }
    layers
}

pub(super) fn plan(input: &SnapshotInput<'_>) -> Result<Plan, SnapshotError> {
    let run = input.run;
    let caps = input.capabilities;
    let mut errors: Vec<FieldError> = Vec::new();
    let env = read_documents(input, &mut errors);

    // The hosts of the run have a document (2.16).
    for host in &run.hosts {
        if !env.hosts.contains_key(host) {
            errors.push(FieldError::new("hosts", format!("the host {host} has no document")));
        }
    }
    // The units of the run: the units that hold a host of the run (4.13).
    let run_hosts = super::host_set(run);
    let run_units: BTreeSet<&UnitId> =
        env.units.iter().filter(|(_, u)| u.hosts.iter().any(|h| run_hosts.contains(h))).map(|(id, _)| id).collect();
    if let Some(unit_id) = super::node_unit(&run.node) {
        match env.units.get(unit_id) {
            Some(unit) => {
                for host in &run.hosts {
                    if !unit.hosts.contains(host) {
                        errors.push(FieldError::new(
                            "hosts",
                            format!("the unit {unit_id} does not hold the host {host}"),
                        ));
                    }
                }
            }
            None => errors.push(FieldError::new("node", format!("the unit {unit_id} has no document"))),
        }
    } else if run.kind != RunKind::Command || !matches!(run.node, Node::Environment(_)) {
        errors.push(FieldError::new("node", "the node of the run is not valid for its kind"));
    }

    let playbooks = playbooks_of(input, &env, &mut errors);

    // The checks of each unit of the run, and the effective values of each host.
    let mut host_variables: BTreeMap<HostId, BTreeMap<String, BTreeMap<String, Json>>> = BTreeMap::new();
    for (unit_id, unit) in &env.units {
        let is_run_unit = run_units.contains(unit_id);
        let Some(schema_spec) = caps.schemas.get(&unit.schema) else {
            if is_run_unit {
                errors.push(FieldError::new(
                    unit_id.as_str(),
                    format!("the schema {} of the unit is not in the catalog", unit.schema),
                ));
            }
            continue;
        };
        let schema = &schema_spec.schema;
        if is_run_unit {
            if let Some(values) = env.unit_values.get(unit_id) {
                for e in check_unit_values(schema, values) {
                    errors.push(FieldError::new(format!("{unit_id}/values: {}", e.path), e.message));
                }
            }
            for ((u, h), values) in &env.host_values {
                if u != unit_id {
                    continue;
                }
                if !unit.hosts.contains(h) {
                    errors.push(FieldError::new(
                        format!("{unit_id}/hosts/{h}/values"),
                        format!("the unit {unit_id} does not hold the host {h}"),
                    ));
                }
                for e in check_unit_values(schema, values) {
                    errors.push(FieldError::new(format!("{unit_id}/hosts/{h}/values: {}", e.path), e.message));
                }
            }
        }
        let secret_layers = resolve_layers(input, unit_id);
        for host in &unit.hosts {
            if !env.hosts.contains_key(host) {
                if is_run_unit {
                    errors.push(FieldError::new(
                        unit_id.as_str(),
                        format!("the unit holds the host {host}, which has no document"),
                    ));
                }
                continue;
            }
            let resolution = resolve(&LayerInputs {
                schema,
                common: env.common.as_ref(),
                unit: env.unit_values.get(unit_id),
                host: env.host_values.get(&(unit_id.clone(), host.clone())),
                secrets: &secret_layers,
            });
            if is_run_unit {
                for key in &resolution.keys {
                    if let Some(e) = &key.error {
                        errors.push(FieldError {
                            path: key.path.as_str().to_string(),
                            message: format!("unit {unit_id}, host {host}: {}", e.message),
                            layer: Some(e.layer.name()),
                        });
                    }
                }
            }
            let vars = host_variables.entry(host.clone()).or_default();
            if vars.insert(schema.variable.as_str().to_string(), resolution.nested_members()).is_some() {
                errors.push(FieldError::new(
                    host.as_str(),
                    format!("two units give the variable {} on the host", schema.variable),
                ));
            }
        }
    }

    // The private folder: for each secret key of each unit of the run, the value that wins
    // (the unit scope before the environment scope, 2.13).
    let by_name: BTreeMap<(&SecretScope, &KeyPath), &SecretValue> =
        input.secrets.iter().map(|(s, n, v)| ((s, n), v)).collect();
    let env_scope = SecretScope::Environment(run.environment.clone());
    let mut secret_paths: BTreeMap<&UnitId, BTreeMap<String, String>> = BTreeMap::new();
    let mut private: BTreeMap<(SecretScope, KeyPath), &SecretValue> = BTreeMap::new();
    for unit_id in &run_units {
        let Some(unit) = env.units.get(*unit_id) else { continue };
        let Some(schema_spec) = caps.schemas.get(&unit.schema) else { continue };
        let unit_scope = SecretScope::Unit(run.environment.clone(), (*unit_id).clone());
        for key in schema_spec.schema.keys.iter().filter(|k| k.secret) {
            let winner = [&unit_scope, &env_scope].into_iter().find(|s| by_name.contains_key(&(*s, &key.path)));
            if let Some(scope) = winner {
                if let Some(value) = by_name.get(&(scope, &key.path)) {
                    private.insert((scope.clone(), key.path.clone()), *value);
                }
                secret_paths
                    .entry(*unit_id)
                    .or_default()
                    .insert(key.path.as_str().to_string(), secret_file_text(scope, &key.path));
            }
        }
    }

    if !errors.is_empty() {
        return Err(SnapshotError::InvalidValues(errors));
    }

    // The variables of each host (4.12).
    let mut units_of_host: BTreeMap<&HostId, Vec<&UnitId>> = BTreeMap::new();
    for (unit_id, unit) in &env.units {
        for host in &unit.hosts {
            units_of_host.entry(host).or_default().push(unit_id);
        }
    }
    let mut host_vars = BTreeMap::new();
    for (host_id, host) in &env.hosts {
        let held = units_of_host.get(host_id).cloned().unwrap_or_default();
        let files: BTreeMap<String, BTreeMap<String, String>> = if run_hosts.contains(host_id) {
            held.iter().filter_map(|u| Some((u.as_str().to_string(), secret_paths.get(u)?.clone()))).collect()
        } else {
            BTreeMap::new()
        };
        let variables = host_variables.remove(host_id).unwrap_or_default();
        host_vars.insert(
            host_id.clone(),
            host_vars_file(&HostFacts { env: &run.environment, id: host_id, host, units: &held }, &variables, &files),
        );
    }

    // The inventory (4.11): the group of each schema and one group for each label.
    let mut groups: BTreeMap<String, BTreeSet<&HostId>> = BTreeMap::new();
    for unit in env.units.values() {
        let Some(schema_spec) = caps.schemas.get(&unit.schema) else { continue };
        for host in unit.hosts.iter().filter(|h| env.hosts.contains_key(*h)) {
            groups.entry(schema_spec.schema.group.as_str().to_string()).or_default().insert(host);
        }
    }
    for (host_id, host) in &env.hosts {
        for label in &host.labels {
            groups.entry(label.as_str().to_string()).or_default().insert(host_id);
        }
    }
    // The hosts of the run: the limit of the command line names this group (4.17). A host id
    // that is also the name of a label or of a group of a schema thus adds no host to the limit.
    groups.insert(crate::child::RUN_GROUP.to_string(), run.hosts.iter().collect());
    let empty = || Json::Obj(BTreeMap::new());
    let hosts_object =
        |ids: &mut dyn Iterator<Item = &HostId>| Json::Obj(ids.map(|h| (h.as_str().to_string(), empty())).collect());
    let mut all = BTreeMap::new();
    if !groups.is_empty() {
        let children = groups
            .iter()
            .map(|(name, ids)| (name.clone(), Json::object([("hosts", hosts_object(&mut ids.iter().copied()))])))
            .collect();
        all.insert("children".to_string(), Json::Obj(children));
    }
    all.insert("hosts".to_string(), hosts_object(&mut env.hosts.keys()));
    let inventory = Json::object([("all", Json::Obj(all))]);

    // The variable files (4.14).
    let mut run_vars = BTreeMap::new();
    run_vars.insert("pupitre_kind".to_string(), Json::str(run.kind.name()));
    run_vars.insert("pupitre_node".to_string(), Json::str(run.node.path()));
    run_vars.insert("pupitre_playbook".to_string(), Json::str(run.playbook.as_str()));
    if let Some(command) = &run.command {
        run_vars.insert("pupitre_command".to_string(), Json::str(command.as_str()));
    }
    if let Some(dir) = &input.settings.releases_dir {
        run_vars.insert("pupitre_releases_dir".to_string(), Json::str(dir.to_string_lossy().into_owned()));
    }
    let params_vars = if run.kind == RunKind::Command {
        let spec = input.capabilities.commands.get(run.command.as_ref().ok_or_else(|| {
            SnapshotError::InvalidValues(vec![FieldError::new("command", "a command run names its command")])
        })?);
        let params = match (&run.params, spec) {
            (Some(p), Some(spec)) => p.ansible_members(spec),
            _ => BTreeMap::new(),
        };
        Some(params_file(&params))
    } else {
        None
    };

    Ok(Plan {
        inventory,
        host_vars,
        run_vars: Json::Obj(run_vars),
        params_vars,
        playbooks,
        private_files: private.into_iter().map(|((s, n), v)| (s, n, v.clone())).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folder of the files that `tests/ansible/unsafe_test.py` reads. It is below `out/`,
    /// which Git ignores.
    fn out_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ansible/out/unsafe")
    }

    /// The JSON form of a value of a variable file: the value that a reader of YAML must give.
    fn as_json(value: &Yaml) -> Json {
        match value {
            Yaml::Null => Json::Null,
            Yaml::Bool(b) => Json::Bool(*b),
            Yaml::Int(n) => Json::Int(*n),
            Yaml::Str(s) => Json::str(s.clone()),
            Yaml::Seq(_, items) => Json::Arr(items.iter().map(as_json).collect()),
            Yaml::Map(_, map) => Json::Obj(map.iter().map(|(k, v)| (k.clone(), as_json(v))).collect()),
        }
    }

    /// The texts that a reader of YAML must give back with no change (4.12).
    fn hard_texts() -> Vec<Json> {
        let long: String = "ab\"\\\u{e9}\u{1F600}\n".repeat(3200);
        [
            "",
            " leading and trailing spaces ",
            "quote \" apostrophe ' backslash \\ slash /",
            "line 1\nline 2\r\nend\ttab",
            "\u{1}\u{8}\u{b}\u{c}\u{1b}\u{1f}\u{7f}",
            "\u{80}\u{85}\u{9f}\u{a0}\u{ad}",
            "\u{2028}\u{2029}\u{feff}\u{fffe}\u{fffd}",
            "\u{202e}rtl\u{202c} \u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{65e5}\u{672c}",
            "\u{1F600}\u{10FFFF}\u{10000}",
            "null",
            "true",
            "yes",
            "~",
            "0x10",
            "1e3",
            "- item",
            "key: value # comment",
            "!unsafe tag",
            "&anchor *alias",
            "{a: b}, [c]",
            "{{ 7 * 6 }}",
            "{% raw %}x{% endraw %}",
            "{# comment #}",
            "}} {{",
            &long,
        ]
        .into_iter()
        .map(Json::str)
        .collect()
    }

    /// 4.12, 4.14, 7.3: the variable files of the test with a real Ansible. A value with a
    /// template marker never passes the checks of 2.12, so the test builds the files from the
    /// functions of the snapshot directly: the test covers the tag `!unsafe` alone.
    #[test]
    fn the_files_of_the_test_with_a_real_ansible() {
        let touch = |name: &str| format!("{{{{ lookup('pipe', 'touch /tmp/pupitre-unsafe/{name}') }}}}");
        let env = EnvId::parse("production").unwrap();
        let unit = UnitId::parse("demo").unwrap();
        let id = HostId::parse("host-a").unwrap();
        // The address stays plain: 2.8 permits no template marker in it.
        let host = Host { address: "host-a.example.invalid".to_string(), port: Some(22), labels: vec![] };
        let values = BTreeMap::from([
            ("banner", Json::str(touch("M-value"))),
            ("cases", Json::Arr(hard_texts())),
            (
                "names",
                Json::Arr(vec![Json::str("a"), Json::str(format!("{{% if true %}}{}{{% endif %}}", touch("M-list")))]),
            ),
            ("nested", Json::object([("deep", Json::str(format!("{{# c #}}{}", touch("M-nested"))))])),
            ("port", Json::Int(8080)),
            ("enabled", Json::Bool(true)),
        ])
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let variables = BTreeMap::from([("demo_config".to_string(), values)]);
        let files = BTreeMap::from([(
            "demo".to_string(),
            BTreeMap::from([(
                "api.token".to_string(),
                "{{ pupitre_secrets_dir }}/environments/production/api.token".to_string(),
            )]),
        )]);
        let host_vars =
            host_vars_file(&HostFacts { env: &env, id: &id, host: &host, units: &[&unit] }, &variables, &files);
        let params = params_file(&BTreeMap::from([
            ("grace".to_string(), Json::Int(90_000)),
            ("reason".to_string(), Json::str(touch("M-param"))),
            ("verbose".to_string(), Json::Bool(false)),
        ]));

        // Each collection of a document has the tag. The path of a secret file is plain.
        let host_text = String::from_utf8(host_vars.to_bytes()).unwrap();
        for opening in
            ["\"demo_config\": !unsafe {", "\"names\": !unsafe [", "\"nested\": !unsafe {", "\"cases\": !unsafe ["]
        {
            assert!(host_text.contains(opening), "{opening}");
        }
        assert!(host_text.contains("\"api.token\": \"{{ pupitre_secrets_dir }}/environments/production/api.token\""));
        assert!(String::from_utf8(params.to_bytes()).unwrap().starts_with("{\n  \"pupitre_params\": !unsafe {\n"));

        let dir = out_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("host-a.yml"), host_vars.to_bytes()).unwrap();
        std::fs::write(dir.join("params.yml"), params.to_bytes()).unwrap();
        let expected = Json::object([("host_vars", as_json(&host_vars)), ("params", as_json(&params))]);
        std::fs::write(dir.join("expected.json"), pupitre_core::json::to_canonical(&expected)).unwrap();
    }
}
