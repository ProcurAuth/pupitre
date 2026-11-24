// SPDX-License-Identifier: MIT OR Apache-2.0
//! The command `import` of 6.24a: the output of `ansible-inventory --list --export` becomes the
//! documents of a new environment.
//!
//! The command has two parts. `build_plan` is a pure function: it reads the JSON of the
//! inventory and the catalog of the configuration, and it gives the documents and the report,
//! or the list of errors. `import` reads the file, calls `build_plan`, opens the store, checks
//! that the environment is new, prints the report, and with `--write` writes the documents.
//! The report never shows the value of a variable that the command does not import.
//!
//! Choices that 6.24a does not fix:
//! - The label of the document `environment` and the label of each unit are their ids.
//! - A host with no `ansible_host` is an error: Pupitre never makes up an address.
//! - A group that has child groups holds the hosts of its children, as Ansible does.
//! - The file of the inventory has 64 MiB at most.
//! - Without a block `playbook` in the configuration (`runner.enabled = false`), a unit gets the
//!   playbook `deploy-<id>` with no check (2.9).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::path::Path;

use pupitre_core::capability::{Capabilities, PlaybookKind};
use pupitre_core::doc::{CanonicalDocument, DocKey, DocKind, Document, Environment, Host, Unit, UnitPlaybooks, Values};
use pupitre_core::error::{FieldError, MAX_ERRORS};
use pupitre_core::ids::{EnvId, HostId, HostLabel, KeyPath, PlaybookId, Subject, UnitId};
use pupitre_core::json::{Json, MAX_SAFE_INTEGER};
use pupitre_core::time::UtcMillis;
use pupitre_core::validate::check_unit_values;
use pupitre_store::files::FilesStore;
use pupitre_store::postgres::PgStore;
use pupitre_store::{AnyStore, Message, Store, StoreError, WriteMeta};
use serde_json::{Map, Value};

use crate::config::{Config, LimitsConfig, LoadedConfig, StorageConfig};

/// The largest file of an inventory that the command reads.
const MAX_INVENTORY_BYTES: u64 = 64 << 20;
/// The most labels of a host (2.8).
const MAX_LABELS: usize = 16;
/// The message of each version that the command writes (2.18).
const IMPORT_MESSAGE: &str = "import of an Ansible inventory";

/// `import --config <path> --inventory-json <path> --environment <id> [--write]`.
pub fn import(loaded: &LoadedConfig, inventory_json: &Path, environment: &EnvId, write: bool) -> u8 {
    let bytes = match read_inventory(inventory_json) {
        Ok(b) => b,
        Err(message) => return super::failure(message),
    };
    let plan = match build_plan(&bytes, &loaded.config, environment) {
        Ok(plan) => plan,
        Err(errors) => return super::failure(format_errors(&errors)),
    };
    let runtime = match super::runtime() {
        Ok(rt) => rt,
        Err(e) => return super::failure(e),
    };
    match runtime.block_on(apply(&loaded.config, &plan, write)) {
        Ok(()) => super::EXIT_OK,
        Err(message) => super::failure(message),
    }
}

fn format_errors(errors: &[String]) -> String {
    let mut out = String::new();
    for e in errors.iter().take(MAX_ERRORS) {
        let _ = writeln!(out, "{e}");
    }
    if errors.len() > MAX_ERRORS {
        let _ = write!(out, "and {} more errors", errors.len() - MAX_ERRORS);
    }
    out.trim_end().to_string()
}

fn read_inventory(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("--inventory-json {}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| format!("--inventory-json {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("--inventory-json {}: expected a regular file", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_INVENTORY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("--inventory-json {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_INVENTORY_BYTES {
        return Err(format!(
            "--inventory-json {}: expected a file of {MAX_INVENTORY_BYTES} bytes at most",
            path.display()
        ));
    }
    Ok(bytes)
}

// The inventory.

#[derive(Debug, Default)]
struct Group {
    hosts: Vec<String>,
    children: Vec<String>,
    vars: Map<String, Value>,
}

#[derive(Debug, Default)]
struct Inventory {
    /// Each host with its variables.
    hosts: BTreeMap<String, Map<String, Value>>,
    groups: BTreeMap<String, Group>,
}

fn string_list(value: Option<&Value>, place: &str, errors: &mut Vec<String>) -> Vec<String> {
    match value {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| match i {
                Value::String(s) => Some(s.clone()),
                _ => {
                    errors.push(format!("{place}: expected a list of texts"));
                    None
                }
            })
            .collect(),
        Some(_) => {
            errors.push(format!("{place}: expected a list of texts"));
            Vec::new()
        }
    }
}

fn parse_inventory(bytes: &[u8]) -> Result<Inventory, Vec<String>> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| vec![format!("the inventory is not valid JSON: {e}")])?;
    let Value::Object(top) = value else {
        return Err(vec!["the inventory: expected an object, the output of ansible-inventory --list".to_string()]);
    };
    let mut errors = Vec::new();
    let mut inv = Inventory::default();
    for (name, value) in &top {
        if name == "_meta" {
            let hostvars = value.get("hostvars");
            match hostvars {
                Some(Value::Object(hosts)) => {
                    for (host, vars) in hosts {
                        match vars {
                            Value::Object(vars) => {
                                inv.hosts.insert(host.clone(), vars.clone());
                            }
                            _ => errors.push(format!("_meta.hostvars.{host}: expected an object")),
                        }
                    }
                }
                None => {}
                Some(_) => errors.push("_meta.hostvars: expected an object".to_string()),
            }
            continue;
        }
        let Value::Object(group) = value else {
            errors.push(format!("group {name}: expected an object"));
            continue;
        };
        let vars = match group.get("vars") {
            None => Map::new(),
            Some(Value::Object(vars)) => vars.clone(),
            Some(_) => {
                errors.push(format!("group {name}: the member vars is not an object"));
                Map::new()
            }
        };
        inv.groups.insert(
            name.clone(),
            Group {
                hosts: string_list(group.get("hosts"), &format!("group {name}: hosts"), &mut errors),
                children: string_list(group.get("children"), &format!("group {name}: children"), &mut errors),
                vars,
            },
        );
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // A host of a group has a place in the list of hosts, with no variable of its own.
    let named: Vec<String> = inv.groups.values().flat_map(|g| g.hosts.iter().cloned()).collect();
    for host in named {
        inv.hosts.entry(host).or_default();
    }
    Ok(inv)
}

impl Inventory {
    /// The hosts of a group and of its child groups, in byte order.
    fn group_hosts(&self, group: &str) -> BTreeSet<String> {
        let mut hosts = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut todo = vec![group.to_string()];
        while let Some(name) = todo.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(g) = self.groups.get(&name) {
                hosts.extend(g.hosts.iter().cloned());
                todo.extend(g.children.iter().cloned());
            }
        }
        hosts
    }

    /// The groups that hold a host, directly or through a child group.
    fn groups_of(&self, host: &str) -> BTreeSet<String> {
        self.groups.keys().filter(|g| self.group_hosts(g).contains(host)).cloned().collect()
    }
}

// The plan.

/// The documents to write and the lines of the report.
#[derive(Debug)]
struct Plan {
    env: EnvId,
    /// The documents in the order of the write: the parents first (2.16).
    docs: Vec<(DocKey, CanonicalDocument)>,
    /// The lines of the report about the hosts.
    hosts: Vec<String>,
    /// The lines of the report about the units.
    units: Vec<String>,
    /// The counts of the documents by kind, in the order of 2.2.
    counts: BTreeMap<&'static str, usize>,
    /// Each group that is not a host label, and each variable that the command leaves out.
    not_imported: Vec<String>,
    skipped_groups: Vec<String>,
}

struct Problems(Vec<String>);

impl Problems {
    fn add(&mut self, place: impl std::fmt::Display, what: impl std::fmt::Display) {
        self.0.push(format!("{place}: {what}"));
    }

    fn fields(&mut self, place: &str, errors: &[FieldError]) {
        for e in errors {
            self.add(place, e);
        }
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The text of a JSON value for a message.
fn shown(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 80 { format!("{}...", text.chars().take(80).collect::<String>()) } else { text }
}

/// Flatten a nested object of an Ansible variable into key paths (6.24a, item 4).
fn flatten(prefix: &str, value: &Value, place: &str, out: &mut BTreeMap<KeyPath, Json>, problems: &mut Problems) {
    match value {
        Value::Object(map) => {
            for (name, inner) in map {
                let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}.{name}") };
                flatten(&path, inner, place, out, problems);
            }
        }
        leaf => {
            let json = match leaf {
                Value::String(s) => Some(Json::Str(s.clone())),
                Value::Bool(b) => Some(Json::Bool(*b)),
                Value::Number(n) => n.as_u64().filter(|n| *n <= MAX_SAFE_INTEGER).map(Json::Int),
                Value::Array(items) => {
                    items.iter().map(|i| i.as_str().map(Json::str)).collect::<Option<Vec<_>>>().map(Json::Arr)
                }
                Value::Null | Value::Object(_) => None,
            };
            let key = KeyPath::parse(prefix);
            match (json, key) {
                (Some(json), Ok(key)) => {
                    if out.insert(key, json).is_some() {
                        problems.add(place, format!("{prefix}: the key path occurs two times"));
                    }
                }
                (_, Err(e)) => problems.add(place, format!("{prefix}: {e}")),
                (None, Ok(_)) => problems.add(
                    place,
                    format!(
                        "{prefix}: expected an integer from 0 to {MAX_SAFE_INTEGER}, a text, a boolean or a list of texts, found {}",
                        shown(leaf)
                    ),
                ),
            }
        }
    }
}

/// Read the nested variable of a schema into the document of values (6.24a, items 4 to 6).
/// A secret key stops the import. A value that fails 2.10 stops it, with its key path.
fn values_of(
    variable: &str,
    value: &Value,
    place: &str,
    schema: &pupitre_core::schema::UnitSchema,
    problems: &mut Problems,
) -> Option<Values> {
    let Value::Object(_) = value else {
        // The message names the type, not the value: the variable can hold a secret.
        problems.add(place, format!("{variable}: expected an object, found {}", type_name(value)));
        return None;
    };
    let mut leaves = BTreeMap::new();
    flatten("", value, place, &mut leaves, problems);
    let before = problems.0.len();
    let mut checked = Values::default();
    for (path, json) in leaves {
        if schema.key(&path).is_some_and(|k| k.secret) {
            problems.add(
                place,
                format!("{path}: a secret key; the command never imports a secret, write it through the API"),
            );
        } else {
            checked.values.insert(path, json);
        }
    }
    if checked.values.len() > pupitre_core::schema::MAX_KEYS {
        problems.add(place, format!("{variable}: expected 1 to {} keys", pupitre_core::schema::MAX_KEYS));
    }
    problems.fields(place, &check_unit_values(schema, &checked));
    (problems.0.len() == before && !checked.values.is_empty()).then_some(checked)
}

/// The playbook of a name when the configuration has it with the kind. `required` makes a
/// missing block an error.
fn playbook_of(
    caps: &Capabilities,
    name: &str,
    kind: PlaybookKind,
    required: bool,
    problems: &mut Problems,
    place: &str,
) -> Option<PlaybookId> {
    let id = PlaybookId::parse(name).ok()?;
    match caps.playbooks.get(&id) {
        None => {
            if required {
                problems.add(
                    place,
                    format!("the configuration has no block playbook \"{name}\" of the kind {}", kind.name()),
                );
            }
            None
        }
        Some(spec) if spec.kind == kind => Some(id),
        Some(spec) => {
            problems.add(
                place,
                format!("the playbook {name} has the kind {}, expected the kind {}", spec.kind.name(), kind.name()),
            );
            None
        }
    }
}

fn host_document(
    name: &str,
    vars: &Map<String, Value>,
    inv: &Inventory,
    caps: &Capabilities,
    problems: &mut Problems,
    skipped: &mut BTreeSet<String>,
) -> Option<(HostId, Host)> {
    let place = format!("host {name}");
    let id = match HostId::parse(name) {
        Ok(id) => id,
        Err(e) => {
            problems.add(&place, e);
            return None;
        }
    };
    let address = match vars.get("ansible_host") {
        Some(Value::String(a)) => Some(a.clone()),
        Some(other) => {
            problems.add(&place, format!("ansible_host: expected a text, found {}", shown(other)));
            None
        }
        None => {
            problems.add(&place, "ansible_host is missing; the document host needs an address");
            None
        }
    };
    let port = match vars.get("ansible_port") {
        None => Some(None),
        Some(v) => match v.as_u64().and_then(|n| u16::try_from(n).ok()).filter(|n| *n >= 1) {
            Some(p) => Some(Some(p)),
            None => {
                problems.add(&place, format!("ansible_port: expected an integer from 1 to 65535, found {}", shown(v)));
                None
            }
        },
    };
    let mut labels = BTreeSet::new();
    for group in inv.groups_of(name) {
        if group == "all" || group == "ungrouped" || caps.is_schema_group(&group) {
            continue;
        }
        match HostLabel::parse(&group) {
            Ok(label) => {
                labels.insert(label);
            }
            Err(_) => {
                skipped.insert(group);
            }
        }
    }
    if labels.len() > MAX_LABELS {
        problems.add(&place, format!("expected {MAX_LABELS} labels at most, found {}", labels.len()));
        return None;
    }
    let host = Host { address: address?, port: port?, labels: labels.into_iter().collect() };
    match Document::from_json(DocKind::Host, &Document::Host(host.clone()).to_json()) {
        Ok(_) => Some((id, host)),
        Err(errors) => {
            problems.fields(&place, &errors);
            None
        }
    }
}

/// Build the documents and the report from the inventory (6.24a).
fn build_plan(bytes: &[u8], config: &Config, env: &EnvId) -> Result<Plan, Vec<String>> {
    let inv = parse_inventory(bytes)?;
    let caps = &config.capabilities;
    let limits = &config.limits;
    let mut problems = Problems(Vec::new());
    let mut skipped = BTreeSet::new();
    let mut docs: Vec<(DocKey, Document)> = Vec::new();
    let mut hosts_report = Vec::new();

    // The documents `host`.
    let mut host_ids: BTreeSet<String> = BTreeSet::new();
    let mut host_docs: Vec<(HostId, Host)> = Vec::new();
    for (name, vars) in &inv.hosts {
        if let Some((id, host)) = host_document(name, vars, &inv, caps, &mut problems, &mut skipped) {
            host_ids.insert(name.clone());
            let port = host.port.map(|p| format!("  port={p}")).unwrap_or_default();
            let labels: Vec<&str> = host.labels.iter().map(HostLabel::as_str).collect();
            hosts_report.push(format!("{id}  address={}{port}  labels={}", host.address, labels.join(",")));
            host_docs.push((id, host));
        }
    }
    if host_docs.len() > usize::try_from(limits.max_hosts_per_environment).unwrap_or(usize::MAX) {
        problems.add(
            "limits.max_hosts_per_environment",
            format!("expected {} hosts at most, found {}", limits.max_hosts_per_environment, host_docs.len()),
        );
    }

    // The units, their values and the values of their hosts.
    let mut units_report = Vec::new();
    let mut unit_docs: Vec<(UnitId, Unit)> = Vec::new();
    let mut unit_values: Vec<(UnitId, Values)> = Vec::new();
    let mut host_values: Vec<(UnitId, HostId, Values)> = Vec::new();
    let mut used_vars: BTreeSet<(String, String)> = BTreeSet::new();
    let mut used_group_vars: BTreeSet<(String, String)> = BTreeSet::new();
    for (schema_id, spec) in &caps.schemas {
        let schema = &spec.schema;
        let group_name = schema.group.as_str();
        let Some(group) = inv.groups.get(group_name) else { continue };
        let place = format!("unit {schema_id}");
        let unit_id = match UnitId::parse(schema_id.as_str()) {
            Ok(id) => id,
            Err(e) => {
                problems.add(&place, e);
                continue;
            }
        };
        let deploy_name = format!("deploy-{schema_id}");
        // With no block `playbook` at all (`runner.enabled = false`), the name is not checked (2.9).
        let deploy = if caps.playbooks.is_empty() {
            PlaybookId::parse(&deploy_name).ok()
        } else {
            playbook_of(caps, &deploy_name, PlaybookKind::Deploy, true, &mut problems, &place)
        };
        let render =
            playbook_of(caps, &format!("render-{schema_id}"), PlaybookKind::Render, false, &mut problems, &place);
        let status =
            playbook_of(caps, &format!("status-{schema_id}"), PlaybookKind::Status, false, &mut problems, &place);
        let members: Vec<HostId> = inv
            .group_hosts(group_name)
            .iter()
            .filter(|h| host_ids.contains(*h))
            .filter_map(|h| HostId::parse(h).ok())
            .collect();
        if members.len() > usize::try_from(limits.max_hosts_per_environment).unwrap_or(usize::MAX) {
            problems.add(
                &place,
                format!("expected {} hosts at most, found {}", limits.max_hosts_per_environment, members.len()),
            );
        }
        // The variable in the vars of the group.
        let variable = schema.variable.as_str();
        if let Some(value) = group.vars.get(variable) {
            used_group_vars.insert((group_name.to_string(), variable.to_string()));
            if let Some(values) = values_of(variable, value, &format!("group {group_name}"), schema, &mut problems) {
                unit_values.push((unit_id.clone(), values));
            }
        }
        // The variable in the vars of each host of the unit.
        for host in &members {
            let Some(value) = inv.hosts.get(host.as_str()).and_then(|v| v.get(variable)) else { continue };
            used_vars.insert((host.as_str().to_string(), variable.to_string()));
            if let Some(values) = values_of(variable, value, &format!("host {host}"), schema, &mut problems) {
                host_values.push((unit_id.clone(), host.clone(), values));
            }
        }
        let Some(deploy) = deploy else { continue };
        let names: Vec<&str> = members.iter().map(HostId::as_str).collect();
        let mut pb = format!("deploy={deploy}");
        if let Some(r) = &render {
            let _ = write!(pb, "  render={r}");
        }
        if let Some(s) = &status {
            let _ = write!(pb, "  status={s}");
        }
        units_report.push(format!("{unit_id}  schema={schema_id}  hosts={}  {pb}", names.join(",")));
        unit_docs.push((
            unit_id.clone(),
            Unit {
                label: unit_id.as_str().to_string(),
                schema: schema_id.clone(),
                playbooks: UnitPlaybooks { deploy, render, status },
                hosts: members,
            },
        ));
    }
    if unit_docs.len() > usize::try_from(limits.max_units_per_environment).unwrap_or(usize::MAX) {
        problems.add(
            "limits.max_units_per_environment",
            format!("expected {} units at most, found {}", limits.max_units_per_environment, unit_docs.len()),
        );
    }

    // The variables that the command leaves out. The report gives names, never values.
    let mut not_imported = Vec::new();
    for (name, group) in &inv.groups {
        for variable in group.vars.keys() {
            if !used_group_vars.contains(&(name.clone(), variable.clone())) {
                not_imported.push(format!("group {name}: {variable}"));
            }
        }
    }
    for (name, vars) in &inv.hosts {
        for variable in vars.keys() {
            let used = used_vars.contains(&(name.clone(), variable.clone()));
            let consumed = (variable == "ansible_host" || variable == "ansible_port") && host_ids.contains(name);
            if !used && !consumed {
                not_imported.push(format!("host {name}: {variable}"));
            }
        }
    }

    // The documents, with the parents first.
    docs.push((
        DocKey::Environment(env.clone()),
        Document::Environment(Environment { label: env.as_str().to_string() }),
    ));
    for (id, host) in host_docs {
        docs.push((DocKey::Host(env.clone(), id), Document::Host(host)));
    }
    for (id, unit) in unit_docs {
        docs.push((DocKey::Unit(env.clone(), id), Document::Unit(unit)));
    }
    for (id, values) in unit_values {
        docs.push((DocKey::UnitValues(env.clone(), id), Document::UnitValues(values)));
    }
    for (unit, host, values) in host_values {
        docs.push((DocKey::HostValues(env.clone(), unit, host), Document::HostValues(values)));
    }
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut canonical = Vec::new();
    for (key, doc) in docs {
        let made = doc.canonical();
        *counts.entry(key.kind().name()).or_default() += 1;
        check_stored(&key, &made, limits, &mut problems);
        canonical.push((key, made));
    }
    if problems.0.is_empty() {
        Ok(Plan {
            env: env.clone(),
            docs: canonical,
            hosts: hosts_report,
            units: units_report,
            counts,
            not_imported,
            skipped_groups: skipped.into_iter().collect(),
        })
    } else {
        Err(problems.0)
    }
}

/// A document of the plan passes the rules of its kind and the size limit (2.7 to 2.10, 2.42).
fn check_stored(key: &DocKey, doc: &CanonicalDocument, limits: &LimitsConfig, problems: &mut Problems) {
    let max = usize::try_from(limits.max_document_bytes).unwrap_or(usize::MAX);
    if doc.bytes.len() > max {
        problems
            .add(key, format!("limits.max_document_bytes: expected {max} bytes at most, found {}", doc.bytes.len()));
    }
    if let Err(errors) = Document::from_stored(key.kind(), &doc.bytes, max) {
        problems.fields(&key.text(), &errors);
    }
}

// The report and the write.

fn report(plan: &Plan, written: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "import report for the environment {}", plan.env);
    let _ = writeln!(out, "mode: {}", if written { "write" } else { "report only, nothing is written (use --write)" });
    let _ = writeln!(out, "hosts ({}):", plan.hosts.len());
    for line in &plan.hosts {
        let _ = writeln!(out, "  {line}");
    }
    let _ = writeln!(out, "units ({}):", plan.units.len());
    for line in &plan.units {
        let _ = writeln!(out, "  {line}");
    }
    let counts: Vec<String> = ["environment", "host", "unit", "unit_values", "host_values"]
        .iter()
        .map(|k| format!("{} {k}", plan.counts.get(k).copied().unwrap_or(0)))
        .collect();
    let _ = writeln!(out, "documents ({}): {}", plan.docs.len(), counts.join(", "));
    if !plan.skipped_groups.is_empty() {
        let _ = writeln!(out, "groups that are not host labels, left out: {}", plan.skipped_groups.join(", "));
    }
    let _ = writeln!(out, "variables not imported ({}):", plan.not_imported.len());
    for line in &plan.not_imported {
        let _ = writeln!(out, "  {line}");
    }
    out
}

async fn open_store(storage: &StorageConfig) -> Result<AnyStore, StoreError> {
    match storage {
        StorageConfig::Files(options) => FilesStore::open(options.clone()).map(AnyStore::Files),
        StorageConfig::Postgres(options) => PgStore::open(options.clone()).await.map(AnyStore::Postgres),
    }
}

/// Write one new document with the version kind `import` (6.24a, item 8).
async fn put_new(store: &AnyStore, key: &DocKey, doc: &CanonicalDocument, meta: &WriteMeta) -> Result<(), StoreError> {
    store.put_import(key, doc, meta).await.map(|_| ())
}

async fn apply(config: &Config, plan: &Plan, write: bool) -> Result<(), String> {
    let store = open_store(&config.storage).await.map_err(|e| e.to_string())?;
    // Item 1: the environment MUST NOT exist.
    let listing = store.load().await.map_err(|e| e.to_string())?;
    let existing: BTreeSet<&EnvId> = listing.docs.iter().map(|d| d.key.environment()).collect();
    if existing.contains(&plan.env) {
        return Err(format!(
            "--environment {}: the environment exists in the store; import makes a new environment only",
            plan.env
        ));
    }
    let max_envs = usize::try_from(config.limits.max_environments).unwrap_or(usize::MAX);
    if existing.len() + 1 > max_envs {
        return Err(format!(
            "limits.max_environments: expected {max_envs} environments at most, the import makes the {}th",
            existing.len() + 1
        ));
    }
    let stored: u64 = listing.docs.iter().map(|d| d.bytes.len() as u64).sum();
    let added: u64 = plan.docs.iter().map(|(_, d)| d.bytes.len() as u64).sum();
    if stored + added > config.limits.max_store_bytes {
        return Err(format!(
            "limits.max_store_bytes: expected {} bytes at most, the import makes {}",
            config.limits.max_store_bytes,
            stored + added
        ));
    }
    let _ = write!(std::io::stdout().lock(), "{}", report(plan, write));
    if !write {
        return Ok(());
    }
    let meta = WriteMeta {
        saved_by: Subject::parse(Subject::IMPORT).map_err(|e| e.to_string())?,
        saved_at: UtcMillis::now(),
        message: Message::parse(IMPORT_MESSAGE).ok_or("the message of the import is not valid")?,
    };
    for (i, (key, doc)) in plan.docs.iter().enumerate() {
        put_new(&store, key, doc, &meta)
            .await
            .map_err(|e| format!("{key}: {e}; {i} of {} documents are written", plan.docs.len()))?;
    }
    let _ = writeln!(std::io::stdout().lock(), "written: {} documents with the subject import", plan.docs.len());
    Ok(())
}
