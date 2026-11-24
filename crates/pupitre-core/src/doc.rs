// SPDX-License-Identifier: MIT OR Apache-2.0
//! The documents of the store (2.2 to 2.10), their keys (2.3), their canonical bytes (2.4)
//! and their etags (2.6).

use std::collections::BTreeMap;
use std::fmt;

use crate::error::FieldError;
use crate::etag::Etag;
use crate::ids::{EnvId, HostId, HostLabel, IdError, KeyPath, PlaybookId, SchemaId, UnitId};
use crate::json::{self, Json, Members, ReadLimits};
use crate::node::Node;
use crate::schema::MAX_KEYS;
use crate::text::has_hidden;

/// The kind of a document (2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocKind {
    /// `environment`
    Environment,
    /// `common`: the layer `environment`.
    Common,
    /// `host`
    Host,
    /// `unit`
    Unit,
    /// `unit_values`: the layer `unit`.
    UnitValues,
    /// `host_values`: the layer `host`.
    HostValues,
}

impl DocKind {
    /// The name of the kind.
    pub fn name(self) -> &'static str {
        match self {
            DocKind::Environment => "environment",
            DocKind::Common => "common",
            DocKind::Host => "host",
            DocKind::Unit => "unit",
            DocKind::UnitValues => "unit_values",
            DocKind::HostValues => "host_values",
        }
    }

    /// True for the three kinds of values (2.10).
    pub fn is_values(self) -> bool {
        matches!(self, DocKind::Common | DocKind::UnitValues | DocKind::HostValues)
    }
}

/// The key of a document (2.3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocKey {
    /// `environments/<env>`
    Environment(EnvId),
    /// `environments/<env>/common`
    Common(EnvId),
    /// `environments/<env>/hosts/<host>`
    Host(EnvId, HostId),
    /// `environments/<env>/units/<unit>`
    Unit(EnvId, UnitId),
    /// `environments/<env>/units/<unit>/values`
    UnitValues(EnvId, UnitId),
    /// `environments/<env>/units/<unit>/hosts/<host>/values`
    HostValues(EnvId, UnitId, HostId),
}

/// A text that is not a document key (2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The text does not have one of the six forms.
    Form,
    /// An id breaks its rule.
    Id(IdError),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Form => f.write_str("expected a document key of 2.3"),
            KeyError::Id(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for KeyError {}

impl DocKey {
    /// The kind of the document of the key.
    pub fn kind(&self) -> DocKind {
        match self {
            DocKey::Environment(_) => DocKind::Environment,
            DocKey::Common(_) => DocKind::Common,
            DocKey::Host(..) => DocKind::Host,
            DocKey::Unit(..) => DocKind::Unit,
            DocKey::UnitValues(..) => DocKind::UnitValues,
            DocKey::HostValues(..) => DocKind::HostValues,
        }
    }

    /// The environment of the key.
    pub fn environment(&self) -> &EnvId {
        match self {
            DocKey::Environment(e)
            | DocKey::Common(e)
            | DocKey::Host(e, _)
            | DocKey::Unit(e, _)
            | DocKey::UnitValues(e, _)
            | DocKey::HostValues(e, _, _) => e,
        }
    }

    /// The text of the key (2.3).
    pub fn text(&self) -> String {
        match self {
            DocKey::Environment(e) => format!("environments/{e}"),
            DocKey::Common(e) => format!("environments/{e}/common"),
            DocKey::Host(e, h) => format!("environments/{e}/hosts/{h}"),
            DocKey::Unit(e, u) => format!("environments/{e}/units/{u}"),
            DocKey::UnitValues(e, u) => format!("environments/{e}/units/{u}/values"),
            DocKey::HostValues(e, u, h) => format!("environments/{e}/units/{u}/hosts/{h}/values"),
        }
    }

    /// Read the text of a key.
    pub fn parse(text: &str) -> Result<DocKey, KeyError> {
        let parts: Vec<&str> = text.split('/').collect();
        let env = |s: &str| EnvId::parse(s).map_err(KeyError::Id);
        let unit = |s: &str| UnitId::parse(s).map_err(KeyError::Id);
        let host = |s: &str| HostId::parse(s).map_err(KeyError::Id);
        match parts.as_slice() {
            ["environments", e] => Ok(DocKey::Environment(env(e)?)),
            ["environments", e, "common"] => Ok(DocKey::Common(env(e)?)),
            ["environments", e, "hosts", h] => Ok(DocKey::Host(env(e)?, host(h)?)),
            ["environments", e, "units", u] => Ok(DocKey::Unit(env(e)?, unit(u)?)),
            ["environments", e, "units", u, "values"] => Ok(DocKey::UnitValues(env(e)?, unit(u)?)),
            ["environments", e, "units", u, "hosts", h, "values"] => {
                Ok(DocKey::HostValues(env(e)?, unit(u)?, host(h)?))
            }
            _ => Err(KeyError::Form),
        }
    }

    /// The path of the current file in the layout of the backend `files` (2.31), relative
    /// to `storage.files.path`.
    pub fn file_path(&self) -> String {
        match self {
            DocKey::Environment(e) => format!("environments/{e}/environment.json"),
            DocKey::Common(e) => format!("environments/{e}/common.json"),
            DocKey::Host(e, h) => format!("environments/{e}/hosts/{h}.json"),
            DocKey::Unit(e, u) => format!("environments/{e}/units/{u}/unit.json"),
            DocKey::UnitValues(e, u) => format!("environments/{e}/units/{u}/values.json"),
            DocKey::HostValues(e, u, h) => format!("environments/{e}/units/{u}/hosts/{h}/values.json"),
        }
    }

    /// The node whose actions apply to the document (3.12).
    pub fn node(&self) -> Node {
        match self {
            DocKey::Environment(e) | DocKey::Common(e) | DocKey::Host(e, _) | DocKey::Unit(e, _) => {
                Node::Environment(e.clone())
            }
            DocKey::UnitValues(e, u) => Node::Unit(e.clone(), u.clone()),
            DocKey::HostValues(e, u, h) => Node::Host(e.clone(), u.clone(), h.clone()),
        }
    }
}

impl fmt::Display for DocKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// The document `environment` (2.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// The label: 1 to 64 characters with no control character and no format character.
    pub label: String,
}

/// The document `host` (2.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// A DNS name or an IP address in canonical form.
    pub address: String,
    /// The SSH port, when the document has one.
    pub port: Option<u16>,
    /// 0 to 16 host labels, in byte order.
    pub labels: Vec<HostLabel>,
}

/// The playbooks of a unit (2.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitPlaybooks {
    /// The playbook of kind `deploy`.
    pub deploy: PlaybookId,
    /// The playbook of kind `render`.
    pub render: Option<PlaybookId>,
    /// The playbook of kind `status`.
    pub status: Option<PlaybookId>,
}

/// The document `unit` (2.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The label.
    pub label: String,
    /// The schema id.
    pub schema: SchemaId,
    /// The playbook ids.
    pub playbooks: UnitPlaybooks,
    /// The host ids, in byte order.
    pub hosts: Vec<HostId>,
}

/// A document of values (2.10).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Values {
    /// The values, by key path.
    pub values: BTreeMap<KeyPath, Json>,
    /// The notes, by key path.
    pub notes: BTreeMap<KeyPath, String>,
}

/// A parsed document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Document {
    /// `environment`
    Environment(Environment),
    /// `common`
    Common(Values),
    /// `host`
    Host(Host),
    /// `unit`
    Unit(Unit),
    /// `unit_values`
    UnitValues(Values),
    /// `host_values`
    HostValues(Values),
}

/// The canonical bytes of a document with their etag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalDocument {
    /// The canonical bytes (2.4).
    pub bytes: Vec<u8>,
    /// The SHA-256 of the bytes (2.6).
    pub etag: Etag,
}

impl CanonicalDocument {
    /// Wrap canonical bytes and compute the etag.
    pub fn from_bytes(bytes: Vec<u8>) -> CanonicalDocument {
        let etag = Etag::of(&bytes);
        CanonicalDocument { bytes, etag }
    }
}

/// A label of 2.7 and 2.9: 1 to 64 characters with no control character and no format
/// character (2.1).
fn label_ok(text: &str) -> bool {
    let n = text.chars().count();
    (1..=64).contains(&n) && !has_hidden(text, &[])
}

struct Reader {
    m: Members,
    errors: Vec<FieldError>,
}

impl Reader {
    fn new(value: &Json) -> Result<Reader, Vec<FieldError>> {
        let m = Members::of(value).map_err(|e| vec![FieldError::new("", e)])?;
        let mut r = Reader { m, errors: Vec::new() };
        match r.m.take("format") {
            Some(Json::Int(1)) => {}
            _ => r.errors.push(FieldError::new("format", "expected the integer 1")),
        }
        Ok(r)
    }

    fn label(&mut self, name: &str) -> Option<String> {
        match self.m.take(name) {
            Some(Json::Str(t)) if label_ok(&t) => Some(t),
            Some(other) => {
                let shown = String::from_utf8_lossy(&json::to_compact(&other)).into_owned();
                self.errors.push(FieldError::new(
                    name,
                    format!(
                        "expected a text of 1 to 64 characters with no control character and no format character, found {shown}"
                    ),
                ));
                None
            }
            None => {
                self.errors.push(FieldError::new(name, "missing member"));
                None
            }
        }
    }

    fn id<T>(&mut self, name: &str, value: Option<Json>, parse: fn(&str) -> Result<T, IdError>) -> Option<T> {
        match value {
            Some(Json::Str(t)) => parse(&t).map_err(|e| self.errors.push(FieldError::new(name, e.to_string()))).ok(),
            Some(other) => {
                self.errors.push(FieldError::new(name, format!("expected a string, found {}", other.type_name())));
                None
            }
            None => {
                self.errors.push(FieldError::new(name, "missing member"));
                None
            }
        }
    }

    fn sorted_ids<T: Ord>(&mut self, name: &str, max: usize, parse: fn(&str) -> Result<T, IdError>) -> Option<Vec<T>> {
        let Some(Json::Arr(items)) = self.m.take(name) else {
            self.errors.push(FieldError::new(name, "expected an array of ids"));
            return None;
        };
        if items.len() > max {
            self.errors.push(FieldError::new(name, format!("expected 0 to {max} items, found {}", items.len())));
            return None;
        }
        let mut out: Vec<T> = Vec::with_capacity(items.len());
        for item in items {
            let id = self.id(name, Some(item), parse)?;
            if out.last().is_some_and(|last| *last >= id) {
                self.errors.push(FieldError::new(name, "expected the items in byte order, each one time"));
                return None;
            }
            out.push(id);
        }
        Some(out)
    }

    fn finish<T>(mut self, value: Option<T>) -> Result<T, Vec<FieldError>> {
        for unknown in self.m.finish() {
            self.errors.push(FieldError::new(unknown, "unknown member"));
        }
        match value {
            Some(v) if self.errors.is_empty() => Ok(v),
            _ => Err(self.errors),
        }
    }
}

fn parse_values(value: &Json) -> Result<Values, Vec<FieldError>> {
    let mut r = Reader::new(value)?;
    let mut out = Values::default();
    match r.m.take("values") {
        Some(Json::Obj(map)) => {
            if map.len() > MAX_KEYS {
                r.errors
                    .push(FieldError::new("values", format!("expected 0 to {MAX_KEYS} members, found {}", map.len())));
            }
            for (name, v) in map {
                match KeyPath::parse(&name) {
                    Ok(path) => {
                        out.values.insert(path, v);
                    }
                    Err(e) => r.errors.push(FieldError::new(format!("values.{name}"), e.to_string())),
                }
            }
        }
        _ => r.errors.push(FieldError::new("values", "expected an object")),
    }
    match r.m.take("notes") {
        None => {}
        Some(Json::Obj(map)) => {
            for (name, v) in map {
                let Ok(path) = KeyPath::parse(&name) else {
                    r.errors.push(FieldError::new(format!("notes.{name}"), "expected a key path"));
                    continue;
                };
                if !out.values.contains_key(&path) {
                    r.errors
                        .push(FieldError::new(format!("notes.{name}"), "a note needs a value of the same key path"));
                }
                match v {
                    Json::Str(t) if note_ok(&t) => {
                        out.notes.insert(path, t);
                    }
                    _ => r.errors.push(FieldError::new(
                        format!("notes.{name}"),
                        "expected a text of 1 to 500 characters with no control character except U+000A and no format character",
                    )),
                }
            }
        }
        Some(_) => r.errors.push(FieldError::new("notes", "expected an object")),
    }
    r.finish(Some(out))
}

/// A note of 2.10: 1 to 500 characters with no control character except U+000A and no format
/// character (2.1).
fn note_ok(text: &str) -> bool {
    let n = text.chars().count();
    (1..=500).contains(&n) && !has_hidden(text, &['\n'])
}

/// An address of 2.8: an IP address in canonical form, or a DNS name whose last label is not
/// all digits (so that `203.0.113.010` is not a name).
fn address_ok(text: &str) -> bool {
    match text.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_string() == text,
        Err(_) => {
            hclconf::kinds::is_dns_name(text)
                && !text.rsplit('.').next().is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
        }
    }
}

impl Document {
    /// The kind of the document.
    pub fn kind(&self) -> DocKind {
        match self {
            Document::Environment(_) => DocKind::Environment,
            Document::Common(_) => DocKind::Common,
            Document::Host(_) => DocKind::Host,
            Document::Unit(_) => DocKind::Unit,
            Document::UnitValues(_) => DocKind::UnitValues,
            Document::HostValues(_) => DocKind::HostValues,
        }
    }

    /// Read a document of a kind from a JSON value, with the rules of its kind (2.7 to
    /// 2.10). The checks against a schema are in `crate::validate`.
    pub fn from_json(kind: DocKind, value: &Json) -> Result<Document, Vec<FieldError>> {
        match kind {
            DocKind::Common => parse_values(value).map(Document::Common),
            DocKind::UnitValues => parse_values(value).map(Document::UnitValues),
            DocKind::HostValues => parse_values(value).map(Document::HostValues),
            DocKind::Environment => {
                let mut r = Reader::new(value)?;
                let label = r.label("label");
                r.finish(label.map(|label| Document::Environment(Environment { label })))
            }
            DocKind::Host => {
                let mut r = Reader::new(value)?;
                let address = match r.m.take("address") {
                    Some(Json::Str(t)) if address_ok(&t) => Some(t),
                    _ => {
                        r.errors.push(FieldError::new(
                            "address",
                            "expected a DNS name of 1 to 253 bytes, or an IP address in canonical form",
                        ));
                        None
                    }
                };
                let port = match r.m.take("port") {
                    None => Some(None),
                    Some(Json::Int(n)) if (1..=65535).contains(&n) => Some(u16::try_from(n).ok()),
                    Some(_) => {
                        r.errors.push(FieldError::new("port", "expected an integer from 1 to 65535"));
                        None
                    }
                };
                let labels = r.sorted_ids("labels", 16, HostLabel::parse);
                let host = match (address, port, labels) {
                    (Some(address), Some(port), Some(labels)) => Some(Document::Host(Host { address, port, labels })),
                    _ => None,
                };
                r.finish(host)
            }
            DocKind::Unit => {
                let mut r = Reader::new(value)?;
                let label = r.label("label");
                let schema_value = r.m.take("schema");
                let schema = r.id("schema", schema_value, SchemaId::parse);
                let playbooks = match r.m.take("playbooks") {
                    Some(v @ Json::Obj(_)) => {
                        let mut pm = Members::of(&v).map_err(|e| vec![FieldError::new("playbooks", e)])?;
                        let deploy_value = pm.take("deploy");
                        let render_value = pm.take("render");
                        let status_value = pm.take("status");
                        for unknown in pm.finish() {
                            r.errors.push(FieldError::new(format!("playbooks.{unknown}"), "unknown member"));
                        }
                        let deploy = r.id("playbooks.deploy", deploy_value, PlaybookId::parse);
                        let render = match render_value {
                            None => Some(None),
                            Some(v) => r.id("playbooks.render", Some(v), PlaybookId::parse).map(Some),
                        };
                        let status = match status_value {
                            None => Some(None),
                            Some(v) => r.id("playbooks.status", Some(v), PlaybookId::parse).map(Some),
                        };
                        match (deploy, render, status) {
                            (Some(deploy), Some(render), Some(status)) => {
                                Some(UnitPlaybooks { deploy, render, status })
                            }
                            _ => None,
                        }
                    }
                    _ => {
                        r.errors.push(FieldError::new("playbooks", "expected an object"));
                        None
                    }
                };
                let hosts = r.sorted_ids("hosts", usize::MAX, HostId::parse);
                let unit = match (label, schema, playbooks, hosts) {
                    (Some(label), Some(schema), Some(playbooks), Some(hosts)) => {
                        Some(Document::Unit(Unit { label, schema, playbooks, hosts }))
                    }
                    _ => None,
                };
                r.finish(unit)
            }
        }
    }

    /// The document as JSON.
    pub fn to_json(&self) -> Json {
        match self {
            Document::Environment(e) => Json::object([("format", Json::Int(1)), ("label", Json::str(e.label.clone()))]),
            Document::Host(h) => {
                let mut pairs = vec![
                    ("address", Json::str(h.address.clone())),
                    ("format", Json::Int(1)),
                    ("labels", Json::Arr(h.labels.iter().map(|l| Json::str(l.as_str())).collect())),
                ];
                if let Some(port) = h.port {
                    pairs.push(("port", Json::Int(u64::from(port))));
                }
                Json::object(pairs)
            }
            Document::Unit(u) => {
                let mut playbooks = vec![("deploy", Json::str(u.playbooks.deploy.as_str()))];
                if let Some(r) = &u.playbooks.render {
                    playbooks.push(("render", Json::str(r.as_str())));
                }
                if let Some(s) = &u.playbooks.status {
                    playbooks.push(("status", Json::str(s.as_str())));
                }
                Json::object([
                    ("format", Json::Int(1)),
                    ("hosts", Json::Arr(u.hosts.iter().map(|h| Json::str(h.as_str())).collect())),
                    ("label", Json::str(u.label.clone())),
                    ("playbooks", Json::object(playbooks)),
                    ("schema", Json::str(u.schema.as_str())),
                ])
            }
            Document::Common(v) | Document::UnitValues(v) | Document::HostValues(v) => {
                let mut pairs = vec![
                    ("format", Json::Int(1)),
                    ("values", Json::Obj(v.values.iter().map(|(k, j)| (k.as_str().to_string(), j.clone())).collect())),
                ];
                if !v.notes.is_empty() {
                    pairs.push((
                        "notes",
                        Json::Obj(
                            v.notes.iter().map(|(k, t)| (k.as_str().to_string(), Json::str(t.clone()))).collect(),
                        ),
                    ));
                }
                Json::object(pairs)
            }
        }
    }

    /// The canonical bytes and the etag of the document.
    pub fn canonical(&self) -> CanonicalDocument {
        CanonicalDocument::from_bytes(json::to_canonical(&self.to_json()))
    }

    /// Read stored bytes (2.5): the strict reader, the rules of the kind, and the canonical
    /// form. `max_bytes` is `limits.max_document_bytes`.
    pub fn from_stored(kind: DocKind, bytes: &[u8], max_bytes: usize) -> Result<Document, Vec<FieldError>> {
        let value =
            json::parse(bytes, ReadLimits::bytes(max_bytes)).map_err(|e| vec![FieldError::new("", e.to_string())])?;
        let doc = Document::from_json(kind, &value)?;
        if let Some(offset) = json::canonical_difference(bytes, &doc.to_json()) {
            return Err(vec![FieldError::new(
                "",
                format!("not_canonical: the bytes differ from the canonical form at byte {offset}"),
            )]);
        }
        Ok(doc)
    }

    /// The values of a document of values.
    pub fn values(&self) -> Option<&Values> {
        match self {
            Document::Common(v) | Document::UnitValues(v) | Document::HostValues(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(kind: DocKind, text: &str) -> Result<Document, Vec<FieldError>> {
        Document::from_json(kind, &json::parse(text.as_bytes(), ReadLimits::bytes(1 << 20)).unwrap())
    }

    #[test]
    fn keys_round_trip() {
        for text in [
            "environments/production",
            "environments/production/common",
            "environments/production/hosts/host-a",
            "environments/production/units/relay",
            "environments/production/units/relay/values",
            "environments/production/units/relay/hosts/host-a/values",
        ] {
            assert_eq!(DocKey::parse(text).unwrap().text(), text);
        }
        assert_eq!(DocKey::parse("environments/p/units/r/x"), Err(KeyError::Form));
        assert!(DocKey::parse("environments/P").is_err());
        let key = DocKey::parse("environments/p/units/r/hosts/a/values").unwrap();
        assert_eq!(key.file_path(), "environments/p/units/r/hosts/a/values.json");
        assert_eq!(key.node().path(), "environment:p/unit:r/host:a");
    }

    #[test]
    fn the_example_unit_of_2_9() {
        let doc = read(
            DocKind::Unit,
            r#"{"format": 1, "hosts": ["host-a", "host-b"], "label": "Relay",
                "playbooks": {"deploy": "deploy-relay", "render": "render-relay", "status": "status-relay"},
                "schema": "relay"}"#,
        )
        .unwrap();
        let canonical = doc.canonical();
        let again = Document::from_stored(DocKind::Unit, &canonical.bytes, 1 << 20).unwrap();
        assert_eq!(again, doc);
    }

    #[test]
    fn closed_documents() {
        assert!(
            read(DocKind::Environment, r#"{"format": 1, "label": "P", "x": 1}"#)
                .unwrap_err()
                .iter()
                .any(|e| e.path == "x")
        );
        assert!(read(DocKind::Environment, r#"{"format": 2, "label": "P"}"#).is_err());
        assert!(read(DocKind::Host, r#"{"format": 1, "address": "203.0.113.010", "labels": []}"#).is_err());
        assert!(read(DocKind::Host, r#"{"format": 1, "address": "host.example.org", "labels": ["b", "a"]}"#).is_err());
        assert!(
            read(DocKind::Host, r#"{"format": 1, "address": "host.example.org", "labels": [], "port": 0}"#).is_err()
        );
        assert!(
            read(
                DocKind::Unit,
                r#"{"format": 1, "hosts": ["b", "a"], "label": "R", "playbooks": {"deploy": "d"}, "schema": "s"}"#
            )
            .is_err()
        );
        assert!(
            read(
                DocKind::Unit,
                r#"{"format": 1, "hosts": [], "label": "R", "playbooks": {"deploy": "d", "x": "y"}, "schema": "s"}"#
            )
            .is_err()
        );
        assert!(read(DocKind::Common, r#"{"format": 1, "values": {"a": 1}, "notes": {"b": "no value"}}"#).is_err());
        assert!(read(DocKind::Common, r#"{"format": 1, "values": {"A": 1}}"#).is_err());
    }

    /// 2.1, 2.7, 2.9, 2.10: a label or a note with a bidirectional control or another format
    /// character is refused. Text of a right-to-left script stays valid.
    #[test]
    fn a_label_or_a_note_with_a_format_character_is_refused() {
        for hidden in
            ['\u{202E}', '\u{202A}', '\u{2066}', '\u{2069}', '\u{200E}', '\u{200F}', '\u{061C}', '\u{200B}', '\u{FEFF}']
        {
            let label = Json::object([("format", Json::Int(1)), ("label", Json::str(format!("Prod{hidden}uction")))]);
            let errors = Document::from_json(DocKind::Environment, &label).unwrap_err();
            assert_eq!(errors[0].path, "label");
            assert!(errors[0].message.contains("no format character"), "{}", errors[0].message);
            let unit = Json::object([
                ("format", Json::Int(1)),
                ("hosts", Json::Arr(vec![])),
                ("label", Json::str(format!("Relay{hidden}"))),
                ("playbooks", Json::object([("deploy", Json::str("d"))])),
                ("schema", Json::str("s")),
            ]);
            assert!(Document::from_json(DocKind::Unit, &unit).is_err());
            let note = Json::object([
                ("format", Json::Int(1)),
                ("notes", Json::object([("a", Json::str(format!("line 1\nline{hidden} 2")))])),
                ("values", Json::object([("a", Json::Int(1))])),
            ]);
            assert!(Document::from_json(DocKind::Common, &note).is_err());
        }
        assert!(read(DocKind::Environment, r#"{"format": 1, "label": "סביבת ייצור"}"#).is_ok());
        assert!(
            read(DocKind::Common, r#"{"format": 1, "notes": {"a": "line 1\nline 2"}, "values": {"a": 1}}"#).is_ok()
        );
    }

    #[test]
    fn a_stored_document_must_be_canonical() {
        let errors =
            Document::from_stored(DocKind::Environment, b"{\"format\":1,\"label\":\"P\"}", 1 << 20).unwrap_err();
        assert!(errors[0].message.starts_with("not_canonical"));
        let good = b"{\n  \"format\": 1,\n  \"label\": \"P\"\n}\n";
        assert!(Document::from_stored(DocKind::Environment, good, 1 << 20).is_ok());
    }
}
