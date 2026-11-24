// SPDX-License-Identifier: MIT OR Apache-2.0
//! The catalog (2.41): the current documents in memory, checked, with the tree of 3.7, the
//! limits of 2.42 and the rules across documents of 2.16.
//!
//! The catalog reads a listing and a delta, checks each document with the rules of its kind,
//! and answers the lookups of the tree. The server calls `check_put` and `check_delete` before
//! each write (3.13, step 10). It calls `put_own` and `remove_own` after the write (2.41).
//! Each limit of 2.42 is a counter that a change updates, so a poll costs the size of its delta.

use std::collections::BTreeMap;
use std::sync::Arc;

use pupitre_core::capability::{Capabilities, PlaybookKind};
use pupitre_core::doc::{DocKey, DocKind, Document, Unit, Values};
use pupitre_core::error::FieldError;
use pupitre_core::etag::Etag;
use pupitre_core::ids::{EnvId, HostId, UnitId};
use pupitre_core::node::Node;
use pupitre_core::validate::{check_common_values, check_unit_values};

use crate::types::{Delta, Listing, Marks, StoredDoc};

/// The limits of 2.42.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogLimits {
    /// `limits.max_environments`
    pub max_environments: usize,
    /// `limits.max_units_per_environment`
    pub max_units_per_environment: usize,
    /// `limits.max_hosts_per_environment`
    pub max_hosts_per_environment: usize,
    /// `limits.max_document_bytes`
    pub max_document_bytes: usize,
    /// `limits.max_store_bytes`
    pub max_store_bytes: usize,
}

/// The state of one entry (2.41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    /// A valid document.
    Valid(Document),
    /// The errors of the stored bytes.
    Invalid(Vec<FieldError>),
}

/// One entry of the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The key.
    pub key: DocKey,
    /// The etag of the stored bytes.
    pub etag: Etag,
    /// The version.
    pub version: u64,
    /// The size of the stored bytes.
    pub size: usize,
    /// The parsed document or its errors.
    pub state: EntryState,
}

/// A load or a poll above a limit of 2.42. The catalog keeps its old state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitExceeded {
    /// The key of the configuration, for example `limits.max_store_bytes`.
    pub limit: &'static str,
}

/// What a check of a write needs besides the catalog (2.9, 2.10).
#[derive(Debug, Clone, Copy)]
pub struct CheckContext<'a> {
    /// The catalog of capabilities of the configuration file (4.2).
    pub capabilities: &'a Capabilities,
    /// The value of `runner.enabled`. With `false`, the playbooks of a unit are not checked (2.9).
    pub runner_enabled: bool,
}

/// The refusal of a write by a rule of 2.10, 2.16 or 2.42 (3.13, step 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteRefusal {
    /// `422 invalid_document`
    Invalid(Vec<FieldError>),
    /// `409 in_use`
    InUse(Vec<FieldError>),
    /// `409 not_empty`
    NotEmpty(Vec<FieldError>),
    /// `422 limit_exceeded` with the key of the limit.
    Limit(&'static str),
}

/// The entries with the counters of the limits.
#[derive(Debug, Clone, Default)]
struct State {
    entries: BTreeMap<DocKey, Arc<CatalogEntry>>,
    total_bytes: usize,
    environments: usize,
    units: BTreeMap<EnvId, usize>,
    hosts: BTreeMap<EnvId, usize>,
}

impl State {
    /// Put or remove an entry and update the counters. It gives the old entry.
    fn set(&mut self, key: &DocKey, entry: Option<Arc<CatalogEntry>>) -> Option<Arc<CatalogEntry>> {
        let old = match &entry {
            Some(new) => self.entries.insert(key.clone(), new.clone()),
            None => self.entries.remove(key),
        };
        if let Some(old) = &old {
            self.total_bytes = self.total_bytes.saturating_sub(old.size);
        }
        if let Some(new) = &entry {
            self.total_bytes = self.total_bytes.saturating_add(new.size);
        }
        match (old.is_some(), entry.is_some()) {
            (false, true) => self.count(key, true),
            (true, false) => self.count(key, false),
            _ => {}
        }
        old
    }

    fn count(&mut self, key: &DocKey, up: bool) {
        fn bump(n: &mut usize, up: bool) {
            *n = if up { n.saturating_add(1) } else { n.saturating_sub(1) };
        }
        match key {
            DocKey::Environment(_) => bump(&mut self.environments, up),
            DocKey::Unit(e, _) => bump(self.units.entry(e.clone()).or_default(), up),
            DocKey::Host(e, _) => bump(self.hosts.entry(e.clone()).or_default(), up),
            _ => {}
        }
    }

    fn exceeded(&self, limits: &CatalogLimits) -> Option<&'static str> {
        if self.total_bytes > limits.max_store_bytes {
            return Some("limits.max_store_bytes");
        }
        if self.environments > limits.max_environments {
            return Some("limits.max_environments");
        }
        if self.units.values().any(|n| *n > limits.max_units_per_environment) {
            return Some("limits.max_units_per_environment");
        }
        if self.hosts.values().any(|n| *n > limits.max_hosts_per_environment) {
            return Some("limits.max_hosts_per_environment");
        }
        None
    }
}

/// The catalog.
#[derive(Debug, Clone)]
pub struct Catalog {
    limits: CatalogLimits,
    state: State,
    marks: Marks,
    loaded: bool,
    limit_error: Option<&'static str>,
}

fn entry_of(doc: StoredDoc, max_bytes: usize) -> CatalogEntry {
    let size = doc.bytes.len();
    let state = match Document::from_stored(doc.key.kind(), &doc.bytes, max_bytes) {
        Ok(d) => EntryState::Valid(d),
        Err(errors) => EntryState::Invalid(errors),
    };
    CatalogEntry { key: doc.key, etag: doc.etag, version: doc.version, size, state }
}

/// The lowest id of 2.1. Each id of 2.1 is at or above the text `0`. Thus a range that starts
/// with it holds each id of the type.
fn lowest<T>(parse: fn(&str) -> Result<T, pupitre_core::ids::IdError>) -> Option<T> {
    parse("0").ok()
}

impl Catalog {
    /// An empty catalog. It is not loaded: `is_ready` gives `false` until a listing is applied.
    pub fn new(limits: CatalogLimits) -> Catalog {
        Catalog { limits, state: State::default(), marks: Marks::default(), loaded: false, limit_error: None }
    }

    /// The limits.
    pub fn limits(&self) -> &CatalogLimits {
        &self.limits
    }

    /// Replace the state with a listing (2.22). Above a limit, keep the old state.
    pub fn apply_listing(&mut self, listing: Listing) -> Result<(), LimitExceeded> {
        let mut state = State::default();
        for doc in listing.docs {
            let entry = Arc::new(entry_of(doc, self.limits.max_document_bytes));
            let key = entry.key.clone();
            state.set(&key, Some(entry));
        }
        if let Some(limit) = state.exceeded(&self.limits) {
            self.limit_error = Some(limit);
            return Err(LimitExceeded { limit });
        }
        self.state = state;
        self.marks = listing.marks;
        self.loaded = true;
        self.limit_error = None;
        Ok(())
    }

    /// Apply a delta (2.22). An entry with the same etag and version keeps its parsed document.
    /// A document with a lower version than its entry is a late read of an old state. The
    /// catalog keeps the entry. Above a limit, keep the old state.
    pub fn apply_delta(&mut self, delta: Delta) -> Result<(), LimitExceeded> {
        let mut undo: Vec<(DocKey, Option<Arc<CatalogEntry>>)> = Vec::new();
        for key in delta.removed {
            let old = self.state.set(&key, None);
            undo.push((key, old));
        }
        for doc in delta.changed {
            let skip = self
                .state
                .entries
                .get(&doc.key)
                .is_some_and(|e| (e.etag == doc.etag && e.version == doc.version) || e.version > doc.version);
            if skip {
                continue;
            }
            let entry = Arc::new(entry_of(doc, self.limits.max_document_bytes));
            let key = entry.key.clone();
            let old = self.state.set(&key, Some(entry));
            undo.push((key, old));
        }
        if let Some(limit) = self.state.exceeded(&self.limits) {
            for (key, old) in undo.into_iter().rev() {
                self.state.set(&key, old);
            }
            self.limit_error = Some(limit);
            return Err(LimitExceeded { limit });
        }
        self.marks = delta.marks;
        self.limit_error = None;
        Ok(())
    }

    /// Put a document that this instance wrote (2.41).
    pub fn put_own(&mut self, doc: StoredDoc) {
        let entry = Arc::new(entry_of(doc, self.limits.max_document_bytes));
        let key = entry.key.clone();
        self.state.set(&key, Some(entry));
    }

    /// Remove a document that this instance deleted.
    pub fn remove_own(&mut self, key: &DocKey) {
        self.state.set(key, None);
    }

    /// The marks of the last load or poll.
    pub fn marks(&self) -> &Marks {
        &self.marks
    }

    /// True after a load and when the last load or poll was within the limits (5.20).
    pub fn is_ready(&self) -> bool {
        self.loaded && self.limit_error.is_none()
    }

    /// True after a load within the limits (2.41). An instance whose store was above a limit at
    /// start has an empty catalog that is not loaded.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// The limit that the last load or poll exceeded, when it did (2.41).
    pub fn limit_error(&self) -> Option<&'static str> {
        self.limit_error
    }

    /// The count of the entries of each kind, for the metric `pupitre_catalog_documents`.
    pub fn document_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut out: BTreeMap<&'static str, usize> = BTreeMap::new();
        for kind in [
            DocKind::Environment,
            DocKind::Common,
            DocKind::Host,
            DocKind::Unit,
            DocKind::UnitValues,
            DocKind::HostValues,
        ] {
            out.insert(kind.name(), 0);
        }
        for key in self.state.entries.keys() {
            *out.entry(key.kind().name()).or_default() += 1;
        }
        out
    }

    /// The canonical bytes of all current documents (2.42).
    pub fn total_bytes(&self) -> usize {
        self.state.total_bytes
    }

    /// The entry of a key.
    pub fn get(&self, key: &DocKey) -> Option<&Arc<CatalogEntry>> {
        self.state.entries.get(key)
    }

    /// The valid document of a key.
    pub fn document(&self, key: &DocKey) -> Option<&Document> {
        match &self.state.entries.get(key)?.state {
            EntryState::Valid(d) => Some(d),
            EntryState::Invalid(_) => None,
        }
    }

    /// The ids of the environments that have a document `environment`, in byte order.
    pub fn environments(&self) -> Vec<&EnvId> {
        let Some(first) = lowest(EnvId::parse) else { return Vec::new() };
        self.state
            .entries
            .range(DocKey::Environment(first)..)
            .map_while(|(k, _)| match k {
                DocKey::Environment(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    /// The ids of the units of an environment, in byte order.
    pub fn units(&self, env: &EnvId) -> Vec<&UnitId> {
        let Some(first) = lowest(UnitId::parse) else { return Vec::new() };
        self.state
            .entries
            .range(DocKey::Unit(env.clone(), first)..)
            .map_while(|(k, _)| match k {
                DocKey::Unit(e, u) if e == env => Some(u),
                _ => None,
            })
            .collect()
    }

    /// The ids of the hosts of an environment, in byte order.
    pub fn hosts(&self, env: &EnvId) -> Vec<&HostId> {
        let Some(first) = lowest(HostId::parse) else { return Vec::new() };
        self.state
            .entries
            .range(DocKey::Host(env.clone(), first)..)
            .map_while(|(k, _)| match k {
                DocKey::Host(e, h) if e == env => Some(h),
                _ => None,
            })
            .collect()
    }

    /// The valid document `unit`.
    pub fn unit(&self, env: &EnvId, unit: &UnitId) -> Option<&Unit> {
        match self.document(&DocKey::Unit(env.clone(), unit.clone()))? {
            Document::Unit(u) => Some(u),
            _ => None,
        }
    }

    /// The host ids of the documents `host_values` of a unit, in byte order.
    fn host_values_of(&self, env: &EnvId, unit: &UnitId) -> Vec<&HostId> {
        let Some(first) = lowest(HostId::parse) else { return Vec::new() };
        self.state
            .entries
            .range(DocKey::HostValues(env.clone(), unit.clone(), first)..)
            .map_while(|(k, _)| match k {
                DocKey::HostValues(e, u, h) if e == env && u == unit => Some(h),
                _ => None,
            })
            .collect()
    }

    fn unit_values_exist(&self, env: &EnvId) -> bool {
        let Some(first) = lowest(UnitId::parse) else { return false };
        self.state
            .entries
            .range(DocKey::UnitValues(env.clone(), first)..)
            .next()
            .is_some_and(|(k, _)| matches!(k, DocKey::UnitValues(e, _) if e == env))
    }

    fn host_values_exist(&self, env: &EnvId) -> bool {
        let (Some(unit), Some(host)) = (lowest(UnitId::parse), lowest(HostId::parse)) else { return false };
        self.state
            .entries
            .range(DocKey::HostValues(env.clone(), unit, host)..)
            .next()
            .is_some_and(|(k, _)| matches!(k, DocKey::HostValues(e, _, _) if e == env))
    }

    /// True when the node is in the tree of 3.7. An environment and a unit need their document.
    /// A host needs its document, and the `hosts` of its unit must hold it.
    pub fn node_exists(&self, node: &Node) -> bool {
        match node {
            Node::Service => true,
            Node::Environment(e) => self.state.entries.contains_key(&DocKey::Environment(e.clone())),
            Node::Unit(e, u) => self.state.entries.contains_key(&DocKey::Unit(e.clone(), u.clone())),
            Node::Host(e, u, h) => {
                self.unit(e, u).is_some_and(|unit| unit.hosts.contains(h))
                    && self.state.entries.contains_key(&DocKey::Host(e.clone(), h.clone()))
            }
        }
    }

    /// The errors of an entry for the list routes (2.41, 5.8 to 5.10): the errors of its
    /// bytes, or the references that no document satisfies. An empty list means no error.
    pub fn entry_errors(&self, key: &DocKey) -> Vec<FieldError> {
        match self.state.entries.get(key).map(|e| &e.state) {
            Some(EntryState::Invalid(errors)) => errors.clone(),
            Some(EntryState::Valid(doc)) => self.reference_errors(key, doc),
            None => Vec::new(),
        }
    }

    fn reference_errors(&self, key: &DocKey, doc: &Document) -> Vec<FieldError> {
        let mut errors = Vec::new();
        let env = key.environment();
        if !matches!(key, DocKey::Environment(_)) && !self.state.entries.contains_key(&DocKey::Environment(env.clone()))
        {
            errors.push(FieldError::new("", "the environment of the document has no document"));
        }
        match (key, doc) {
            (DocKey::Unit(..), Document::Unit(unit)) => {
                for host in &unit.hosts {
                    if !self.state.entries.contains_key(&DocKey::Host(env.clone(), host.clone())) {
                        errors.push(FieldError::new("hosts", format!("the host {host} has no document")));
                    }
                }
            }
            (DocKey::UnitValues(_, u), _) => {
                if !self.state.entries.contains_key(&DocKey::Unit(env.clone(), u.clone())) {
                    errors.push(FieldError::new("", format!("the unit {u} has no document")));
                }
            }
            (DocKey::HostValues(_, u, h), _) => match self.unit(env, u) {
                None => errors.push(FieldError::new("", format!("the unit {u} has no valid document"))),
                Some(unit) if !unit.hosts.contains(h) => {
                    errors.push(FieldError::new("", format!("the unit {u} does not hold the host {h}")));
                }
                Some(_) => {}
            },
            _ => {}
        }
        errors
    }

    /// Check a write of a document against the rules of 2.8 to 2.10, 2.15, 2.16 and 2.42
    /// (3.13, step 10). `canonical_len` is the length of the canonical bytes of `doc`.
    pub fn check_put(
        &self,
        ctx: &CheckContext<'_>,
        key: &DocKey,
        doc: &Document,
        canonical_len: usize,
    ) -> Result<(), WriteRefusal> {
        if key.kind() != doc.kind() {
            return Err(WriteRefusal::Invalid(vec![FieldError::new("", "the document has another kind than its key")]));
        }
        let env = key.environment();
        if !matches!(key, DocKey::Environment(_)) && !self.state.entries.contains_key(&DocKey::Environment(env.clone()))
        {
            return Err(WriteRefusal::Invalid(vec![FieldError::new(
                "",
                "the environment of the document has no document",
            )]));
        }
        let mut errors = Vec::new();
        let mut in_use = Vec::new();
        match (key, doc) {
            (DocKey::Host(..), Document::Host(host)) => {
                for label in &host.labels {
                    if ctx.capabilities.is_schema_group(label.as_str()) {
                        errors.push(FieldError::new("labels", format!("the label {label} is the group of a schema")));
                    }
                }
            }
            (DocKey::Unit(_, uid), Document::Unit(unit)) => {
                self.check_unit(ctx, env, uid, unit, &mut errors, &mut in_use)
            }
            (DocKey::Common(_), Document::Common(values)) => {
                errors.extend(check_common_values(ctx.capabilities.schema_set(), values));
            }
            (DocKey::UnitValues(_, uid), Document::UnitValues(values)) => match self.unit(env, uid) {
                None => errors.push(FieldError::new("", format!("the unit {uid} has no valid document"))),
                Some(unit) => self.check_values(ctx, unit, values, &mut errors),
            },
            (DocKey::HostValues(_, uid, hid), Document::HostValues(values)) => match self.unit(env, uid) {
                None => errors.push(FieldError::new("", format!("the unit {uid} has no valid document"))),
                Some(unit) if !unit.hosts.contains(hid) => {
                    errors.push(FieldError::new("", format!("the unit {uid} does not hold the host {hid}")));
                }
                Some(unit) => self.check_values(ctx, unit, values, &mut errors),
            },
            _ => {}
        }
        if !errors.is_empty() {
            return Err(WriteRefusal::Invalid(errors));
        }
        if !in_use.is_empty() {
            return Err(WriteRefusal::InUse(in_use));
        }
        if let Document::Unit(unit) = doc
            && unit.hosts.len() > self.limits.max_hosts_per_environment
        {
            return Err(WriteRefusal::Limit("limits.max_hosts_per_environment"));
        }
        self.check_limits_of_put(key, canonical_len)
    }

    fn check_values(&self, ctx: &CheckContext<'_>, unit: &Unit, values: &Values, errors: &mut Vec<FieldError>) {
        match ctx.capabilities.schemas.get(&unit.schema) {
            Some(spec) => errors.extend(check_unit_values(&spec.schema, values)),
            None => errors
                .push(FieldError::new("", format!("the configuration has no schema {} for the unit", unit.schema))),
        }
    }

    fn check_unit(
        &self,
        ctx: &CheckContext<'_>,
        env: &EnvId,
        uid: &UnitId,
        unit: &Unit,
        errors: &mut Vec<FieldError>,
        in_use: &mut Vec<FieldError>,
    ) {
        let caps = ctx.capabilities;
        if !caps.schemas.contains_key(&unit.schema) {
            errors.push(FieldError::new("schema", format!("the configuration has no schema {}", unit.schema)));
        }
        if ctx.runner_enabled {
            let named = [
                ("playbooks.deploy", Some(&unit.playbooks.deploy), PlaybookKind::Deploy),
                ("playbooks.render", unit.playbooks.render.as_ref(), PlaybookKind::Render),
                ("playbooks.status", unit.playbooks.status.as_ref(), PlaybookKind::Status),
            ];
            for (path, id, kind) in named {
                if let Some(id) = id
                    && caps.playbook_of_kind(id, kind).is_none()
                {
                    errors.push(FieldError::new(
                        path,
                        format!("the configuration has no playbook {id} with the kind {}", kind.name()),
                    ));
                }
            }
        }
        for host in &unit.hosts {
            if !self.state.entries.contains_key(&DocKey::Host(env.clone(), host.clone())) {
                errors.push(FieldError::new("hosts", format!("the host {host} has no document")));
            }
        }
        // A host in two units of one environment with the same schema (2.9).
        for other_id in self.units(env) {
            if other_id == uid {
                continue;
            }
            let Some(other) = self.unit(env, other_id) else { continue };
            if other.schema != unit.schema {
                continue;
            }
            for host in unit.hosts.iter().filter(|h| other.hosts.contains(h)) {
                errors.push(FieldError::new(
                    "hosts",
                    format!("the host {host} is also in the unit {other_id}, which has the same schema"),
                ));
            }
        }
        // A host that leaves the unit has no document `host_values` for it (2.16).
        for host in self.host_values_of(env, uid) {
            if !unit.hosts.contains(host) {
                in_use.push(FieldError::new(
                    "hosts",
                    format!("the host {host} has a document host_values for this unit"),
                ));
            }
        }
    }

    /// The limits of 2.42 for a write (2.16, last line).
    fn check_limits_of_put(&self, key: &DocKey, canonical_len: usize) -> Result<(), WriteRefusal> {
        if canonical_len > self.limits.max_document_bytes {
            return Err(WriteRefusal::Limit("limits.max_document_bytes"));
        }
        let old = self.state.entries.get(key);
        let old_size = old.map_or(0, |e| e.size);
        if self.state.total_bytes.saturating_sub(old_size).saturating_add(canonical_len) > self.limits.max_store_bytes {
            return Err(WriteRefusal::Limit("limits.max_store_bytes"));
        }
        if old.is_none() {
            let env = key.environment();
            match key {
                DocKey::Environment(_) if self.state.environments >= self.limits.max_environments => {
                    return Err(WriteRefusal::Limit("limits.max_environments"));
                }
                DocKey::Unit(..)
                    if self.state.units.get(env).copied().unwrap_or(0) >= self.limits.max_units_per_environment =>
                {
                    return Err(WriteRefusal::Limit("limits.max_units_per_environment"));
                }
                DocKey::Host(..)
                    if self.state.hosts.get(env).copied().unwrap_or(0) >= self.limits.max_hosts_per_environment =>
                {
                    return Err(WriteRefusal::Limit("limits.max_hosts_per_environment"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Check a delete against the rules of 2.16 (3.13, step 10).
    pub fn check_delete(&self, key: &DocKey) -> Result<(), WriteRefusal> {
        let env = key.environment();
        match key {
            DocKey::Host(_, host) => {
                let holders: Vec<FieldError> = self
                    .units(env)
                    .into_iter()
                    .filter(|u| self.unit(env, u).is_some_and(|unit| unit.hosts.contains(host)))
                    .map(|u| FieldError::new("", format!("the unit {u} holds the host")))
                    .collect();
                if holders.is_empty() { Ok(()) } else { Err(WriteRefusal::InUse(holders)) }
            }
            DocKey::Unit(_, uid) => {
                let mut errors = Vec::new();
                if self.state.entries.contains_key(&DocKey::UnitValues(env.clone(), uid.clone())) {
                    errors.push(FieldError::new("", "the unit has a document unit_values"));
                }
                if !self.host_values_of(env, uid).is_empty() {
                    errors.push(FieldError::new("", "the unit has a document host_values"));
                }
                if errors.is_empty() { Ok(()) } else { Err(WriteRefusal::NotEmpty(errors)) }
            }
            DocKey::Environment(_) => {
                let mut errors = Vec::new();
                if self.state.entries.contains_key(&DocKey::Common(env.clone())) {
                    errors.push(FieldError::new("", "the environment has a document common"));
                }
                if !self.hosts(env).is_empty() {
                    errors.push(FieldError::new("", "the environment has a document host"));
                }
                if !self.units(env).is_empty() {
                    errors.push(FieldError::new("", "the environment has a document unit"));
                }
                if self.unit_values_exist(env) {
                    errors.push(FieldError::new("", "the environment has a document unit_values"));
                }
                if self.host_values_exist(env) {
                    errors.push(FieldError::new("", "the environment has a document host_values"));
                }
                if errors.is_empty() { Ok(()) } else { Err(WriteRefusal::NotEmpty(errors)) }
            }
            DocKey::Common(_) | DocKey::UnitValues(..) | DocKey::HostValues(..) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> CatalogLimits {
        CatalogLimits {
            max_environments: 2,
            max_units_per_environment: 10,
            max_hosts_per_environment: 10,
            max_document_bytes: 4096,
            max_store_bytes: 1 << 20,
        }
    }

    fn stored(key: &str, doc: &Document) -> StoredDoc {
        let c = doc.canonical();
        StoredDoc { key: DocKey::parse(key).unwrap(), bytes: c.bytes, etag: c.etag, version: 1 }
    }

    fn env_doc() -> Document {
        Document::Environment(pupitre_core::doc::Environment { label: "Production".into() })
    }

    fn unit_doc(hosts: &[&str]) -> Document {
        Document::Unit(Unit {
            label: "Relay".into(),
            schema: pupitre_core::ids::SchemaId::parse("relay").unwrap(),
            playbooks: pupitre_core::doc::UnitPlaybooks {
                deploy: pupitre_core::ids::PlaybookId::parse("deploy-relay").unwrap(),
                render: None,
                status: None,
            },
            hosts: hosts.iter().map(|h| HostId::parse(h).unwrap()).collect(),
        })
    }

    fn host_doc() -> Document {
        Document::Host(pupitre_core::doc::Host { address: "203.0.113.10".into(), port: None, labels: vec![] })
    }

    #[test]
    fn the_tree_of_3_7() {
        let mut c = Catalog::new(limits());
        let listing = Listing {
            docs: vec![
                stored("environments/p", &env_doc()),
                stored("environments/p/units/relay", &unit_doc(&["host-a"])),
                stored("environments/p/hosts/host-a", &host_doc()),
                stored("environments/p/hosts/host-b", &host_doc()),
            ],
            marks: Marks::default(),
        };
        c.apply_listing(listing).unwrap();
        assert!(c.node_exists(&Node::parse_path("environment:p/unit:relay/host:host-a").unwrap()));
        assert!(!c.node_exists(&Node::parse_path("environment:p/unit:relay/host:host-b").unwrap()));
        assert!(!c.node_exists(&Node::parse_path("environment:q").unwrap()));
        assert_eq!(c.units(&EnvId::parse("p").unwrap()).len(), 1);
        assert_eq!(c.hosts(&EnvId::parse("p").unwrap()).len(), 2);
    }

    #[test]
    fn an_invalid_entry_stays_with_its_errors() {
        let mut c = Catalog::new(limits());
        let bad = StoredDoc {
            key: DocKey::parse("environments/p").unwrap(),
            bytes: b"{\"format\":1,\"label\":\"P\"}".to_vec(),
            etag: Etag::of(b"{\"format\":1,\"label\":\"P\"}"),
            version: 1,
        };
        c.apply_listing(Listing { docs: vec![bad], marks: Marks::default() }).unwrap();
        let entry = c.get(&DocKey::parse("environments/p").unwrap()).unwrap();
        assert!(matches!(entry.state, EntryState::Invalid(_)));
        assert!(c.document(&entry.key).is_none());
    }

    use pupitre_core::capability::{PlaybookKind, PlaybookSpec, SchemaSpec};
    use pupitre_core::doc::Values;
    use pupitre_core::ids::{KeyPath, PlaybookId, SchemaId};
    use pupitre_core::json::Json;
    use pupitre_core::schema::UnitSchema;

    const SCHEMA: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
        {"default": 100, "description": "C.", "kind": "integer", "max": 1000, "min": 1, "path": "limits.max_clients"},
        {"description": "P.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true}
    ]}"#;

    fn caps() -> Capabilities {
        let mut c = Capabilities::default();
        let id = SchemaId::parse("relay").unwrap();
        c.schemas.insert(
            id.clone(),
            SchemaSpec { id, path: "/x".into(), schema: UnitSchema::parse(SCHEMA.as_bytes()).unwrap() },
        );
        for (name, kind) in [("deploy-relay", PlaybookKind::Deploy), ("status-relay", PlaybookKind::Status)] {
            let id = PlaybookId::parse(name).unwrap();
            c.playbooks.insert(id.clone(), PlaybookSpec { id, kind, path: "/p".into() });
        }
        c
    }

    fn ctx(c: &Capabilities) -> CheckContext<'_> {
        CheckContext { capabilities: c, runner_enabled: true }
    }

    fn big_limits() -> CatalogLimits {
        CatalogLimits {
            max_environments: 2,
            max_units_per_environment: 2,
            max_hosts_per_environment: 3,
            max_document_bytes: 4096,
            max_store_bytes: 1 << 20,
        }
    }

    fn values_doc(entries: &[(&str, Json)]) -> Values {
        let mut v = Values::default();
        for (k, j) in entries {
            v.values.insert(KeyPath::parse(k).unwrap(), j.clone());
        }
        v
    }

    fn base() -> Catalog {
        let mut c = Catalog::new(big_limits());
        c.apply_listing(Listing {
            docs: vec![
                stored("environments/p", &env_doc()),
                stored("environments/p/units/relay", &unit_doc(&["host-a"])),
                stored("environments/p/hosts/host-a", &host_doc()),
                stored("environments/p/hosts/host-b", &host_doc()),
                stored(
                    "environments/p/units/relay/hosts/host-a/values",
                    &Document::HostValues(values_doc(&[("limits.max_clients", Json::Int(5))])),
                ),
            ],
            marks: Marks::default(),
        })
        .unwrap();
        c
    }

    fn put(c: &Catalog, key: &str, doc: &Document) -> Result<(), WriteRefusal> {
        let caps = caps();
        let key = DocKey::parse(key).unwrap();
        c.check_put(&ctx(&caps), &key, doc, doc.canonical().bytes.len())
    }

    #[test]
    fn a_document_needs_the_environment_document_2_16() {
        let c = base();
        let r = put(&c, "environments/q/hosts/host-a", &host_doc());
        assert!(matches!(r, Err(WriteRefusal::Invalid(_))));
        assert!(put(&c, "environments/p/hosts/host-c", &host_doc()).is_ok());
    }

    #[test]
    fn a_unit_needs_the_host_documents_2_16() {
        let c = base();
        let r = put(&c, "environments/p/units/relay", &unit_doc(&["host-a", "host-z"]));
        let Err(WriteRefusal::Invalid(errors)) = r else { panic!("{r:?}") };
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "hosts");
        assert!(errors[0].message.contains("host-z"));
    }

    #[test]
    fn a_host_cannot_leave_a_unit_that_has_its_host_values_2_16() {
        let c = base();
        let r = put(&c, "environments/p/units/relay", &unit_doc(&["host-b"]));
        assert!(matches!(r, Err(WriteRefusal::InUse(_))), "{r:?}");
        assert!(put(&c, "environments/p/units/relay", &unit_doc(&["host-a", "host-b"])).is_ok());
    }

    #[test]
    fn a_host_is_in_one_unit_for_one_schema_2_9() {
        let c = base();
        let r = put(&c, "environments/p/units/second", &unit_doc(&["host-a"]));
        let Err(WriteRefusal::Invalid(errors)) = r else { panic!("{r:?}") };
        assert!(errors[0].message.contains("same schema"));
        assert!(put(&c, "environments/p/units/second", &unit_doc(&["host-b"])).is_ok());
    }

    #[test]
    fn host_values_need_the_unit_and_the_host_2_16() {
        let c = base();
        let doc = Document::HostValues(values_doc(&[("limits.max_clients", Json::Int(7))]));
        assert!(put(&c, "environments/p/units/relay/hosts/host-a/values", &doc).is_ok());
        assert!(matches!(
            put(&c, "environments/p/units/relay/hosts/host-b/values", &doc),
            Err(WriteRefusal::Invalid(_))
        ));
        assert!(matches!(
            put(&c, "environments/p/units/other/hosts/host-a/values", &doc),
            Err(WriteRefusal::Invalid(_))
        ));
    }

    #[test]
    fn values_follow_the_schema_2_10_and_2_15() {
        let c = base();
        let bad = Document::UnitValues(values_doc(&[("limits.max_clients", Json::Int(5000))]));
        assert!(matches!(put(&c, "environments/p/units/relay/values", &bad), Err(WriteRefusal::Invalid(_))));
        let secret = Document::UnitValues(values_doc(&[("storage.password", Json::str("12345678"))]));
        assert!(matches!(put(&c, "environments/p/units/relay/values", &secret), Err(WriteRefusal::Invalid(_))));
        let ok = Document::Common(values_doc(&[("limits.max_clients", Json::Int(7))]));
        assert!(put(&c, "environments/p/common", &ok).is_ok());
        let unknown = Document::Common(values_doc(&[("other.key", Json::Int(7))]));
        assert!(matches!(put(&c, "environments/p/common", &unknown), Err(WriteRefusal::Invalid(_))));
    }

    #[test]
    fn a_host_label_is_not_a_schema_group_2_8() {
        let c = base();
        let doc = Document::Host(pupitre_core::doc::Host {
            address: "203.0.113.11".into(),
            port: None,
            labels: vec![pupitre_core::ids::HostLabel::parse("relay_hosts").unwrap()],
        });
        assert!(matches!(put(&c, "environments/p/hosts/host-c", &doc), Err(WriteRefusal::Invalid(_))));
    }

    #[test]
    fn the_playbooks_follow_the_catalog_of_capabilities_when_the_runner_is_on() {
        let c = base();
        let caps = caps();
        let mut unit = unit_doc(&["host-a"]);
        if let Document::Unit(u) = &mut unit {
            u.playbooks.render = Some(PlaybookId::parse("render-relay").unwrap());
        }
        let key = DocKey::parse("environments/p/units/relay").unwrap();
        let len = unit.canonical().bytes.len();
        assert!(matches!(c.check_put(&ctx(&caps), &key, &unit, len), Err(WriteRefusal::Invalid(_))));
        let off = CheckContext { capabilities: &caps, runner_enabled: false };
        assert!(c.check_put(&off, &key, &unit, len).is_ok());
    }

    #[test]
    fn the_deletes_of_2_16() {
        let c = base();
        let key = |t: &str| DocKey::parse(t).unwrap();
        assert!(matches!(c.check_delete(&key("environments/p/hosts/host-a")), Err(WriteRefusal::InUse(_))));
        assert!(c.check_delete(&key("environments/p/hosts/host-b")).is_ok());
        assert!(matches!(c.check_delete(&key("environments/p/units/relay")), Err(WriteRefusal::NotEmpty(_))));
        assert!(matches!(c.check_delete(&key("environments/p")), Err(WriteRefusal::NotEmpty(_))));
        assert!(c.check_delete(&key("environments/p/units/relay/hosts/host-a/values")).is_ok());
    }

    #[test]
    fn the_counts_of_2_42_refuse_a_write() {
        let mut c = base();
        c.put_own(stored("environments/q", &env_doc()));
        assert_eq!(put(&c, "environments/r", &env_doc()), Err(WriteRefusal::Limit("limits.max_environments")));
        c.put_own(stored("environments/p/hosts/host-c", &host_doc()));
        assert_eq!(
            put(&c, "environments/p/hosts/host-d", &host_doc()),
            Err(WriteRefusal::Limit("limits.max_hosts_per_environment"))
        );
        // A save of an existing key is not a new document.
        assert!(put(&c, "environments/p/hosts/host-c", &host_doc()).is_ok());
        let long = Document::Environment(pupitre_core::doc::Environment { label: "x".repeat(60) });
        let caps = caps();
        let key = DocKey::parse("environments/p").unwrap();
        assert_eq!(c.check_put(&ctx(&caps), &key, &long, 4097), Err(WriteRefusal::Limit("limits.max_document_bytes")));
    }

    #[test]
    fn the_bytes_of_the_store_are_bounded() {
        let mut limits = big_limits();
        limits.max_store_bytes = 100;
        let mut c = Catalog::new(limits);
        c.apply_listing(Listing { docs: vec![stored("environments/p", &env_doc())], marks: Marks::default() }).unwrap();
        let caps = caps();
        let key = DocKey::parse("environments/q").unwrap();
        assert_eq!(c.check_put(&ctx(&caps), &key, &env_doc(), 90), Err(WriteRefusal::Limit("limits.max_store_bytes")));
    }

    #[test]
    fn a_broken_reference_is_an_error_of_the_entry_2_41() {
        let mut c = base();
        let key = DocKey::parse("environments/p/units/relay").unwrap();
        assert!(c.entry_errors(&key).is_empty());
        // Another instance removed the document of the host.
        c.apply_delta(Delta {
            changed: vec![],
            removed: vec![DocKey::parse("environments/p/hosts/host-a").unwrap()],
            marks: Marks::default(),
        })
        .unwrap();
        let errors = c.entry_errors(&key);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("host-a"));
        assert!(!c.node_exists(&Node::parse_path("environment:p/unit:relay/host:host-a").unwrap()));
    }

    #[test]
    fn a_delta_with_the_same_etag_keeps_the_parsed_document() {
        let mut c = base();
        let again = stored("environments/p", &env_doc());
        let before = c.get(&again.key).unwrap().clone();
        c.apply_delta(Delta { changed: vec![again.clone()], removed: vec![], marks: Marks::default() }).unwrap();
        assert!(Arc::ptr_eq(&before, c.get(&again.key).unwrap()));
        // A newer version replaces the entry.
        let mut newer =
            stored("environments/p", &Document::Environment(pupitre_core::doc::Environment { label: "Other".into() }));
        newer.version = 2;
        c.apply_delta(Delta { changed: vec![newer], removed: vec![], marks: Marks::default() }).unwrap();
        assert_eq!(c.get(&again.key).unwrap().version, 2);
        // A late read of version 1 does not bring the old state back.
        c.apply_delta(Delta { changed: vec![again.clone()], removed: vec![], marks: Marks::default() }).unwrap();
        assert_eq!(c.get(&again.key).unwrap().version, 2);
    }

    #[test]
    fn a_delta_above_a_limit_keeps_the_old_state_and_the_readiness_fails() {
        let mut c = base();
        assert!(c.is_ready());
        let docs = ["x", "y"].iter().map(|e| stored(&format!("environments/{e}"), &env_doc())).collect();
        let r = c.apply_delta(Delta { changed: docs, removed: vec![], marks: Marks::default() });
        assert_eq!(r, Err(LimitExceeded { limit: "limits.max_environments" }));
        assert_eq!(c.environments().len(), 1);
        assert_eq!(c.limit_error(), Some("limits.max_environments"));
        assert!(!c.is_ready());
        c.apply_delta(Delta { changed: vec![], removed: vec![], marks: Marks::default() }).unwrap();
        assert!(c.is_ready());
        assert_eq!(
            c.total_bytes(),
            c.get(&DocKey::parse("environments/p").unwrap()).map_or(0, |e| e.size)
                + [
                    "environments/p/units/relay",
                    "environments/p/hosts/host-a",
                    "environments/p/hosts/host-b",
                    "environments/p/units/relay/hosts/host-a/values"
                ]
                .iter()
                .map(|k| c.get(&DocKey::parse(k).unwrap()).map_or(0, |e| e.size))
                .sum::<usize>()
        );
    }

    #[test]
    fn a_catalog_that_never_loaded_is_not_ready() {
        let mut c = Catalog::new(big_limits());
        assert!(!c.is_ready());
        let docs = ["a", "b", "c"].iter().map(|e| stored(&format!("environments/{e}"), &env_doc())).collect();
        assert!(c.apply_listing(Listing { docs, marks: Marks::default() }).is_err());
        assert!(!c.is_ready());
    }

    #[test]
    fn the_lists_come_in_byte_order_and_the_counts_by_kind() {
        let c = base();
        let env = EnvId::parse("p").unwrap();
        let hosts: Vec<&str> = c.hosts(&env).iter().map(|h| h.as_str()).collect();
        assert_eq!(hosts, vec!["host-a", "host-b"]);
        let counts = c.document_counts();
        assert_eq!(counts["host"], 2);
        assert_eq!(counts["common"], 0);
        assert_eq!(counts["host_values"], 1);
    }

    #[test]
    fn a_limit_keeps_the_old_state() {
        let mut c = Catalog::new(limits());
        let docs = ["a", "b", "c"].iter().map(|e| stored(&format!("environments/{e}"), &env_doc())).collect();
        assert_eq!(
            c.apply_listing(Listing { docs, marks: Marks::default() }),
            Err(LimitExceeded { limit: "limits.max_environments" })
        );
        assert!(c.environments().is_empty());
    }
}
