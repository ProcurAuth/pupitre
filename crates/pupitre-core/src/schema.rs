// SPDX-License-Identifier: MIT OR Apache-2.0
//! The schema of a unit (2.11) and the value kinds with their checks (2.12).

use std::collections::BTreeMap;

use hclconf::value::{format_duration, parse_duration};
use hclconf::{Kind, TextRule as EngineRule};

use crate::error::FieldError;
use crate::ids::{AnsibleGroup, AnsibleVariable, KeyPath, SchemaId};
use crate::json::{self, Json, Members, ReadLimits};
use crate::text::{NO_TEMPLATE_MARKER, template_marker};

/// The largest schema file (2.11).
pub const MAX_SCHEMA_FILE_BYTES: usize = 1_048_576;
/// The most keys of one schema (2.11) and of one document of values (2.10).
pub const MAX_KEYS: usize = 1024;

/// A rule of a text (2.12). The configuration engine gives the meaning (6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRule {
    /// `any`
    Any,
    /// `no_control`
    NoControl,
    /// `printable_ascii`
    PrintableAscii,
    /// `no_control_no_space`
    NoControlNoSpace,
    /// `dns_name`
    DnsName,
    /// `host_or_ip`
    HostOrIp,
}

impl TextRule {
    const ALL: [(TextRule, &'static str); 6] = [
        (TextRule::Any, "any"),
        (TextRule::NoControl, "no_control"),
        (TextRule::PrintableAscii, "printable_ascii"),
        (TextRule::NoControlNoSpace, "no_control_no_space"),
        (TextRule::DnsName, "dns_name"),
        (TextRule::HostOrIp, "host_or_ip"),
    ];

    fn parse(text: &str) -> Option<TextRule> {
        TextRule::ALL.iter().find(|(_, n)| *n == text).map(|(r, _)| *r)
    }

    /// The name of the rule in a schema file.
    pub fn name(self) -> &'static str {
        TextRule::ALL.iter().find(|(r, _)| *r == self).map_or("any", |(_, n)| n)
    }

    fn engine(self) -> EngineRule {
        match self {
            TextRule::Any => EngineRule::Any,
            TextRule::NoControl => EngineRule::NoControl,
            TextRule::PrintableAscii => EngineRule::PrintableAscii,
            TextRule::NoControlNoSpace => EngineRule::NoControlNoSpace,
            TextRule::DnsName => EngineRule::DnsName,
            TextRule::HostOrIp => EngineRule::HostOrIp,
        }
    }
}

/// A value kind with its range (2.12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueKind {
    /// `text`: a length in bytes and a rule.
    Text {
        /// The shortest length in bytes.
        min: usize,
        /// The longest length in bytes.
        max: usize,
        /// The rule of the characters.
        rule: TextRule,
    },
    /// `text_list`
    TextList {
        /// The fewest items.
        min_items: usize,
        /// The most items.
        max_items: usize,
        /// The shortest item in bytes.
        item_min: usize,
        /// The longest item in bytes.
        item_max: usize,
        /// The rule of each item.
        rule: TextRule,
    },
    /// `enum`: 1 to 64 texts.
    Enum(Vec<String>),
    /// `integer`
    Integer {
        /// The lowest value.
        min: u64,
        /// The highest value.
        max: u64,
    },
    /// `size`, in bytes.
    Size {
        /// The lowest value.
        min: u64,
        /// The highest value.
        max: u64,
    },
    /// `duration`, in milliseconds.
    Duration {
        /// The lowest value.
        min_ms: u64,
        /// The highest value.
        max_ms: u64,
    },
    /// `bool`
    Bool,
    /// `path`
    Path,
    /// `socket_addr`
    SocketAddr,
}

impl ValueKind {
    /// The name of the kind in a schema file.
    pub fn name(&self) -> &'static str {
        match self {
            ValueKind::Text { .. } => "text",
            ValueKind::TextList { .. } => "text_list",
            ValueKind::Enum(_) => "enum",
            ValueKind::Integer { .. } => "integer",
            ValueKind::Size { .. } => "size",
            ValueKind::Duration { .. } => "duration",
            ValueKind::Bool => "bool",
            ValueKind::Path => "path",
            ValueKind::SocketAddr => "socket_addr",
        }
    }

    /// The text "type and range" of an error message.
    pub fn expected(&self) -> String {
        match self {
            ValueKind::Enum(values) => {
                let quoted: Vec<String> = values.iter().map(|v| json::string_form(v)).collect();
                format!("one of {}", quoted.join(", "))
            }
            ValueKind::Duration { min_ms, max_ms } => {
                format!("a canonical duration from {} to {}", format_duration(*min_ms), format_duration(*max_ms))
            }
            other => other.engine_kind().map_or_else(|| "a value".to_string(), |k| k.expected()),
        }
    }

    fn engine_kind(&self) -> Option<Kind> {
        Some(match self {
            ValueKind::Text { min, max, rule } => Kind::Text { min: *min, max: *max, rule: rule.engine() },
            ValueKind::TextList { min_items, max_items, item_min, item_max, rule } => Kind::TextList {
                min_items: *min_items,
                max_items: *max_items,
                item_min: *item_min,
                item_max: *item_max,
                rule: rule.engine(),
            },
            ValueKind::Integer { min, max } => Kind::Integer { min: *min, max: *max },
            ValueKind::Size { min, max } => Kind::Size { min: *min, max: *max },
            ValueKind::Duration { min_ms, max_ms } => Kind::Duration { min_ms: *min_ms, max_ms: *max_ms },
            ValueKind::Bool => Kind::Bool,
            ValueKind::Path => Kind::Path,
            ValueKind::SocketAddr => Kind::SocketAddr,
            ValueKind::Enum(_) => return None,
        })
    }

    /// Check a value (2.12). The message names the expected type and range, and the value. A
    /// secret value never shows: the caller gives `secret = true`.
    ///
    /// A value that is not a secret never holds a template marker of Ansible (2.12, 7.3): a text,
    /// a path and each item of a list. A secret value can hold one: it reaches Ansible as the
    /// content of a file (4.13), never as a variable.
    pub fn check(&self, value: &Json, secret: bool) -> Result<(), String> {
        self.check_kind(value, secret)?;
        if secret {
            return Ok(());
        }
        let marker = match value {
            Json::Str(text) => template_marker(text),
            Json::Arr(items) => items.iter().find_map(|i| i.as_str().and_then(template_marker)),
            _ => None,
        };
        match marker {
            None => Ok(()),
            Some(_) => Err(format!(
                "expected {} with {NO_TEMPLATE_MARKER}, found {}",
                self.expected(),
                String::from_utf8_lossy(&json::to_compact(value))
            )),
        }
    }

    fn check_kind(&self, value: &Json, secret: bool) -> Result<(), String> {
        let shown = || {
            if secret {
                "(value hidden)".to_string()
            } else {
                String::from_utf8_lossy(&json::to_compact(value)).into_owned()
            }
        };
        let bad = || format!("expected {}, found {}", self.expected(), shown());
        let text_ok = |kind: &Kind, text: &str| -> bool {
            !text.contains('\0') && hclconf::kinds::check_string(kind, secret, text).is_ok()
        };
        let ok = match (self, value) {
            (ValueKind::Text { .. } | ValueKind::Path | ValueKind::SocketAddr, Json::Str(text)) => {
                self.engine_kind().is_some_and(|k| text_ok(&k, text))
            }
            (ValueKind::TextList { min_items, max_items, item_min, item_max, rule }, Json::Arr(items)) => {
                let item_kind = Kind::Text { min: *item_min, max: *item_max, rule: rule.engine() };
                items.len() >= *min_items
                    && items.len() <= *max_items
                    && items.iter().all(|i| i.as_str().is_some_and(|t| text_ok(&item_kind, t)))
            }
            (ValueKind::Enum(values), Json::Str(text)) => values.iter().any(|v| v == text),
            (ValueKind::Integer { min, max } | ValueKind::Size { min, max }, Json::Int(n)) => n >= min && n <= max,
            (ValueKind::Duration { min_ms, max_ms }, Json::Str(text)) => match parse_duration(text) {
                Some(ms) if format_duration(ms) != *text => {
                    return Err(format!(
                        "expected the canonical duration {}, found {}",
                        json::string_form(&format_duration(ms)),
                        shown()
                    ));
                }
                Some(ms) => ms >= *min_ms && ms <= *max_ms,
                None => false,
            },
            (ValueKind::Bool, Json::Bool(_)) => true,
            _ => false,
        };
        if ok { Ok(()) } else { Err(bad()) }
    }
}

/// One key of a schema (2.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaKey {
    /// The key path.
    pub path: KeyPath,
    /// The kind and its range.
    pub kind: ValueKind,
    /// The description, 1 to 300 characters.
    pub description: String,
    /// True for a secret key (2.24).
    pub secret: bool,
    /// The layer `default` (2.13).
    pub default: Option<Json>,
}

/// The schema of a unit (2.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitSchema {
    /// The Ansible variable of the effective values (4.12).
    pub variable: AnsibleVariable,
    /// The inventory group of the hosts (4.11).
    pub group: AnsibleGroup,
    /// The keys, in the byte order of their paths.
    pub keys: Vec<SchemaKey>,
}

fn text_member(m: &mut Members, name: &str, errors: &mut Vec<FieldError>, at: &str) -> Option<String> {
    match m.take(name) {
        Some(Json::Str(s)) => Some(s),
        Some(other) => {
            errors.push(FieldError::new(
                format!("{at}{name}"),
                format!("expected a string, found {}", other.type_name()),
            ));
            None
        }
        None => {
            errors.push(FieldError::new(format!("{at}{name}"), "missing member"));
            None
        }
    }
}

fn int_member(m: &mut Members, name: &str, errors: &mut Vec<FieldError>, at: &str) -> Option<u64> {
    match m.take(name) {
        Some(Json::Int(n)) => Some(n),
        Some(other) => {
            errors.push(FieldError::new(
                format!("{at}{name}"),
                format!("expected an integer, found {}", other.type_name()),
            ));
            None
        }
        None => {
            errors.push(FieldError::new(format!("{at}{name}"), "missing member"));
            None
        }
    }
}

fn usize_member(m: &mut Members, name: &str, errors: &mut Vec<FieldError>, at: &str) -> Option<usize> {
    int_member(m, name, errors, at).and_then(|n| usize::try_from(n).ok())
}

fn rule_member(m: &mut Members, errors: &mut Vec<FieldError>, at: &str) -> Option<TextRule> {
    let text = text_member(m, "rule", errors, at)?;
    let rule = TextRule::parse(&text);
    if rule.is_none() {
        errors.push(FieldError::new(
            format!("{at}rule"),
            format!("expected a rule of 2.12, found {}", json::string_form(&text)),
        ));
    }
    rule
}

fn duration_member(m: &mut Members, name: &str, errors: &mut Vec<FieldError>, at: &str) -> Option<u64> {
    let text = text_member(m, name, errors, at)?;
    match parse_duration(&text) {
        Some(ms) if format_duration(ms) == text => Some(ms),
        _ => {
            errors.push(FieldError::new(
                format!("{at}{name}"),
                format!("expected a canonical duration, found {}", json::string_form(&text)),
            ));
            None
        }
    }
}

fn parse_kind(kind: &str, m: &mut Members, errors: &mut Vec<FieldError>, at: &str) -> Option<ValueKind> {
    let range_error =
        |errors: &mut Vec<FieldError>| errors.push(FieldError::new(format!("{at}min"), "expected min not above max"));
    match kind {
        "text" => {
            let (min, max, rule) =
                (usize_member(m, "min", errors, at), usize_member(m, "max", errors, at), rule_member(m, errors, at));
            let (min, max, rule) = (min?, max?, rule?);
            if min > max {
                range_error(errors);
                return None;
            }
            Some(ValueKind::Text { min, max, rule })
        }
        "text_list" => {
            let min_items = usize_member(m, "min_items", errors, at);
            let max_items = usize_member(m, "max_items", errors, at);
            let item_min = usize_member(m, "item_min", errors, at);
            let item_max = usize_member(m, "item_max", errors, at);
            let rule = rule_member(m, errors, at);
            let (min_items, max_items, item_min, item_max, rule) =
                (min_items?, max_items?, item_min?, item_max?, rule?);
            if min_items > max_items || item_min > item_max {
                range_error(errors);
                return None;
            }
            Some(ValueKind::TextList { min_items, max_items, item_min, item_max, rule })
        }
        "enum" => match m.take("values") {
            Some(Json::Arr(items)) => {
                let mut values = Vec::new();
                for item in &items {
                    match item.as_str() {
                        Some(t) if !t.is_empty() && t.len() <= 64 && !values.iter().any(|v: &String| v == t) => {
                            values.push(t.to_string());
                        }
                        _ => {
                            errors.push(FieldError::new(
                                format!("{at}values"),
                                "expected 1 to 64 distinct texts of 1 to 64 bytes",
                            ));
                            return None;
                        }
                    }
                }
                if values.is_empty() || values.len() > 64 {
                    errors.push(FieldError::new(
                        format!("{at}values"),
                        "expected 1 to 64 distinct texts of 1 to 64 bytes",
                    ));
                    return None;
                }
                Some(ValueKind::Enum(values))
            }
            _ => {
                errors.push(FieldError::new(format!("{at}values"), "expected an array of texts"));
                None
            }
        },
        "integer" | "size" => {
            let (min, max) = (int_member(m, "min", errors, at), int_member(m, "max", errors, at));
            let (min, max) = (min?, max?);
            if min > max {
                range_error(errors);
                return None;
            }
            Some(if kind == "integer" { ValueKind::Integer { min, max } } else { ValueKind::Size { min, max } })
        }
        "duration" => {
            let (min_ms, max_ms) = (duration_member(m, "min", errors, at), duration_member(m, "max", errors, at));
            let (min_ms, max_ms) = (min_ms?, max_ms?);
            if min_ms > max_ms {
                range_error(errors);
                return None;
            }
            Some(ValueKind::Duration { min_ms, max_ms })
        }
        "bool" => Some(ValueKind::Bool),
        "path" => Some(ValueKind::Path),
        "socket_addr" => Some(ValueKind::SocketAddr),
        other => {
            errors.push(FieldError::new(
                format!("{at}kind"),
                format!("expected a kind of 2.12, found {}", json::string_form(other)),
            ));
            None
        }
    }
}

fn parse_key(index: usize, value: &Json, errors: &mut Vec<FieldError>) -> Option<SchemaKey> {
    let at = format!("keys[{index}].");
    let mut m = match Members::of(value) {
        Ok(m) => m,
        Err(e) => {
            errors.push(FieldError::new(format!("keys[{index}]"), e));
            return None;
        }
    };
    let path = text_member(&mut m, "path", errors, &at).and_then(|t| match KeyPath::parse(&t) {
        Ok(p) => Some(p),
        Err(e) => {
            errors.push(FieldError::new(format!("{at}path"), e.to_string()));
            None
        }
    });
    let kind_name = text_member(&mut m, "kind", errors, &at);
    let kind = kind_name.and_then(|k| parse_kind(&k, &mut m, errors, &at));
    let description = text_member(&mut m, "description", errors, &at);
    if let Some(d) = &description {
        let n = d.chars().count();
        if n == 0 || n > 300 {
            errors
                .push(FieldError::new(format!("{at}description"), format!("expected 1 to 300 characters, found {n}")));
        }
    }
    let secret = match m.take("secret") {
        None => false,
        Some(Json::Bool(b)) => b,
        Some(other) => {
            errors.push(FieldError::new(
                format!("{at}secret"),
                format!("expected a boolean, found {}", other.type_name()),
            ));
            false
        }
    };
    let default = m.take("default");
    for unknown in m.finish() {
        errors.push(FieldError::new(format!("{at}{unknown}"), "unknown member"));
    }
    let (path, kind, description) = (path?, kind?, description?);
    if secret && !matches!(kind, ValueKind::Text { .. }) {
        errors.push(FieldError::new(format!("{at}secret"), "a secret key has the kind text"));
    }
    if let Some(d) = &default {
        if secret {
            errors.push(FieldError::new(format!("{at}default"), "a secret key has no default"));
        } else if let Err(message) = kind.check(d, false) {
            errors.push(FieldError::new(format!("{at}default"), message));
        }
    }
    Some(SchemaKey { path, kind, description, secret, default })
}

impl UnitSchema {
    /// Read and check a schema file (2.11).
    pub fn parse(bytes: &[u8]) -> Result<UnitSchema, Vec<FieldError>> {
        let value = json::parse(bytes, ReadLimits::bytes(MAX_SCHEMA_FILE_BYTES))
            .map_err(|e| vec![FieldError::new("", e.to_string())])?;
        let mut errors = Vec::new();
        let mut m = Members::of(&value).map_err(|e| vec![FieldError::new("", e)])?;
        match m.take("format") {
            Some(Json::Int(1)) => {}
            _ => errors.push(FieldError::new("format", "expected the integer 1")),
        }
        let variable = text_member(&mut m, "variable", &mut errors, "").and_then(|t| {
            AnsibleVariable::parse(&t).map_err(|e| errors.push(FieldError::new("variable", e.to_string()))).ok()
        });
        let group = text_member(&mut m, "group", &mut errors, "").and_then(|t| {
            AnsibleGroup::parse(&t).map_err(|e| errors.push(FieldError::new("group", e.to_string()))).ok()
        });
        let mut keys = Vec::new();
        match m.take("keys") {
            Some(Json::Arr(items)) => {
                if items.is_empty() || items.len() > MAX_KEYS {
                    errors
                        .push(FieldError::new("keys", format!("expected 1 to {MAX_KEYS} keys, found {}", items.len())));
                }
                for (i, item) in items.iter().enumerate() {
                    if let Some(key) = parse_key(i, item, &mut errors) {
                        keys.push(key);
                    }
                }
            }
            _ => errors.push(FieldError::new("keys", "expected an array of keys")),
        }
        for unknown in m.finish() {
            errors.push(FieldError::new(unknown, "unknown member"));
        }
        for pair in keys.windows(2) {
            if pair[0].path >= pair[1].path {
                errors.push(FieldError::new(
                    pair[1].path.as_str(),
                    "the keys are not in the byte order of their paths, or a path occurs two times",
                ));
            }
            if pair[0].path.is_prefix_of(&pair[1].path) {
                errors.push(FieldError::new(
                    pair[1].path.as_str(),
                    format!("the path {} starts this path", pair[0].path),
                ));
            }
        }
        match (variable, group, errors.is_empty()) {
            (Some(variable), Some(group), true) => Ok(UnitSchema { variable, group, keys }),
            _ => Err(errors),
        }
    }

    /// The key of a path.
    pub fn key(&self, path: &KeyPath) -> Option<&SchemaKey> {
        self.keys.binary_search_by(|k| k.path.cmp(path)).ok().map(|i| &self.keys[i])
    }

    /// The schema as JSON, in the form of 2.11. The catalog route uses it (5.15).
    pub fn to_json(&self) -> Json {
        let keys = self.keys.iter().map(key_json).collect();
        Json::object([
            ("format", Json::Int(1)),
            ("group", Json::str(self.group.as_str())),
            ("keys", Json::Arr(keys)),
            ("variable", Json::str(self.variable.as_str())),
        ])
    }
}

fn key_json(key: &SchemaKey) -> Json {
    let mut map = BTreeMap::new();
    map.insert("path".to_string(), Json::str(key.path.as_str()));
    map.insert("kind".to_string(), Json::str(key.kind.name()));
    map.insert("description".to_string(), Json::str(key.description.clone()));
    if key.secret {
        map.insert("secret".to_string(), Json::Bool(true));
    }
    if let Some(d) = &key.default {
        map.insert("default".to_string(), d.clone());
    }
    let int = |n: usize| Json::Int(n as u64);
    match &key.kind {
        ValueKind::Text { min, max, rule } => {
            map.insert("min".into(), int(*min));
            map.insert("max".into(), int(*max));
            map.insert("rule".into(), Json::str(rule.name()));
        }
        ValueKind::TextList { min_items, max_items, item_min, item_max, rule } => {
            map.insert("min_items".into(), int(*min_items));
            map.insert("max_items".into(), int(*max_items));
            map.insert("item_min".into(), int(*item_min));
            map.insert("item_max".into(), int(*item_max));
            map.insert("rule".into(), Json::str(rule.name()));
        }
        ValueKind::Enum(values) => {
            map.insert("values".into(), Json::Arr(values.iter().map(|v| Json::str(v.clone())).collect()));
        }
        ValueKind::Integer { min, max } | ValueKind::Size { min, max } => {
            map.insert("min".into(), Json::Int(*min));
            map.insert("max".into(), Json::Int(*max));
        }
        ValueKind::Duration { min_ms, max_ms } => {
            map.insert("min".into(), Json::str(format_duration(*min_ms)));
            map.insert("max".into(), Json::str(format_duration(*max_ms)));
        }
        ValueKind::Bool | ValueKind::Path | ValueKind::SocketAddr => {}
    }
    Json::Obj(map)
}

/// The checks across all schemas of the configuration (2.11): one kind and one secret flag
/// for each path, one `variable` and one `group` for each schema. Each problem is one text.
pub fn check_schema_set<'a, I>(schemas: I) -> Vec<String>
where
    I: IntoIterator<Item = (&'a SchemaId, &'a UnitSchema)>,
{
    let mut problems = Vec::new();
    let mut seen_keys: BTreeMap<&KeyPath, (&SchemaId, &'static str, bool)> = BTreeMap::new();
    let mut variables: BTreeMap<&str, &SchemaId> = BTreeMap::new();
    let mut groups: BTreeMap<&str, &SchemaId> = BTreeMap::new();
    for (id, schema) in schemas {
        if let Some(other) = variables.insert(schema.variable.as_str(), id) {
            problems.push(format!("the schemas {other} and {id} have the same variable {}", schema.variable));
        }
        if let Some(other) = groups.insert(schema.group.as_str(), id) {
            problems.push(format!("the schemas {other} and {id} have the same group {}", schema.group));
        }
        for key in &schema.keys {
            match seen_keys.get(&key.path) {
                Some((other, kind, secret)) if *kind != key.kind.name() || *secret != key.secret => {
                    problems.push(format!(
                        "the key {} has another kind or another secret flag in the schemas {other} and {id}",
                        key.path
                    ));
                }
                Some(_) => {}
                None => {
                    seen_keys.insert(&key.path, (id, key.kind.name(), key.secret));
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY: &str = r#"{
  "format": 1,
  "group": "relay_hosts",
  "keys": [
    {"default": 100, "description": "The most clients.", "kind": "integer", "max": 100000, "min": 1, "path": "limits.max_clients"},
    {"default": "info", "description": "The level.", "kind": "enum", "path": "log.level", "values": ["error", "warn", "info", "debug"]},
    {"description": "The password.", "kind": "text", "max": 1024, "min": 8, "path": "storage.password", "rule": "any", "secret": true},
    {"description": "The wait.", "kind": "duration", "max": "1m", "min": "1s", "path": "timeouts.connect"}
  ],
  "variable": "relay_config"
}"#;

    #[test]
    fn reads_a_schema_file() {
        let schema = UnitSchema::parse(RELAY.as_bytes()).unwrap();
        assert_eq!(schema.keys.len(), 4);
        let key = schema.key(&KeyPath::parse("log.level").unwrap()).unwrap();
        assert_eq!(key.default, Some(Json::str("info")));
        assert!(schema.key(&KeyPath::parse("log").unwrap()).is_none());
        let again = UnitSchema::parse(&json::to_canonical(&schema.to_json())).unwrap();
        assert_eq!(again, schema);
    }

    #[test]
    fn refuses_the_wrong_schemas() {
        let unsorted = RELAY.replace("limits.max_clients", "z.max_clients");
        assert!(UnitSchema::parse(unsorted.as_bytes()).is_err());
        let unknown = RELAY.replace("\"format\": 1,", "\"format\": 1, \"extra\": true,");
        assert!(UnitSchema::parse(unknown.as_bytes()).unwrap_err().iter().any(|e| e.path == "extra"));
        let bad_default = RELAY.replace("\"default\": 100", "\"default\": 0");
        assert!(UnitSchema::parse(bad_default.as_bytes()).is_err());
        let prefix = RELAY.replace("\"path\": \"log.level\"", "\"path\": \"limits.max_clients.x\"");
        assert!(UnitSchema::parse(prefix.as_bytes()).is_err());
    }

    /// 2.11: a schema never names its variable with a name that Ansible reserves.
    #[test]
    fn a_reserved_variable_name_is_refused() {
        for reserved in
            ["hostvars", "groups", "inventory_hostname", "omit", "vars", "environment", "group_names", "play_hosts"]
        {
            let text = RELAY.replace("\"relay_config\"", &format!("\"{reserved}\""));
            let errors = UnitSchema::parse(text.as_bytes()).unwrap_err();
            assert!(
                errors.iter().any(|e| e.path == "variable" && e.message.contains(reserved)),
                "{reserved}: {errors:?}"
            );
        }
        assert!(UnitSchema::parse(RELAY.replace("relay_config", "hostvars_config").as_bytes()).is_ok());
    }

    #[test]
    fn value_checks_name_the_range_and_the_value() {
        let k = ValueKind::Integer { min: 1, max: 10 };
        assert!(k.check(&Json::Int(5), false).is_ok());
        assert_eq!(k.check(&Json::Int(11), false).unwrap_err(), "expected an integer from 1 to 10, found 11");
        let d = ValueKind::Duration { min_ms: 1000, max_ms: 600_000 };
        assert!(d.check(&Json::str("2m"), false).is_ok());
        assert_eq!(
            d.check(&Json::str("120s"), false).unwrap_err(),
            "expected the canonical duration \"2m\", found \"120s\""
        );
        let t = ValueKind::Text { min: 8, max: 64, rule: TextRule::Any };
        assert_eq!(
            t.check(&Json::str("short"), true).unwrap_err(),
            "expected a text of 8 to 64 bytes, found (value hidden)"
        );
        assert!(t.check(&Json::str("long enough\0"), false).is_err());
        let e = ValueKind::Enum(vec!["a".into(), "b".into()]);
        assert!(e.check(&Json::str("c"), false).is_err());
        let s = ValueKind::SocketAddr;
        assert!(s.check(&Json::str("127.0.0.1:80"), false).is_ok());
        assert!(s.check(&Json::str("127.0.0.1:080"), false).is_err());
    }

    /// 2.12, 7.3: a value that is not a secret never holds `{{`, `{%` or `{#`. The message
    /// names the rule and the value. A secret value can hold them.
    #[test]
    fn a_template_marker_is_refused_in_each_text_of_a_value() {
        let text = ValueKind::Text { min: 0, max: 256, rule: TextRule::Any };
        for bad in ["{{ lookup('pipe', 'id') }}", "x {% if 1 %}", "{# c #}", "a{{b"] {
            let message = text.check(&Json::str(bad), false).unwrap_err();
            assert!(message.contains(r#"no template marker "{{", "{%" or "{#""#), "{message}");
            assert!(message.ends_with(&json::string_form(bad)), "{message}");
        }
        for good in ["", "{", "}}", "{ {", "${HOME}", "%}", "#}"] {
            assert!(text.check(&Json::str(good), false).is_ok(), "{good}");
        }
        assert!(text.check(&Json::str("pa{{ss}}word"), true).is_ok(), "a secret value can hold a marker");
        let list = ValueKind::TextList { min_items: 0, max_items: 4, item_min: 0, item_max: 64, rule: TextRule::Any };
        let items = Json::Arr(vec![Json::str("a"), Json::str("{% raw %}")]);
        assert!(list.check(&items, false).unwrap_err().contains("no template marker"));
        assert!(list.check(&Json::Arr(vec![Json::str("a"), Json::str("b")]), false).is_ok());
        assert!(ValueKind::Path.check(&Json::str("/srv/{{x}}"), false).is_err());
        assert!(ValueKind::Path.check(&Json::str("/srv/{x}"), false).is_ok());
        let e = ValueKind::Enum(vec!["{{a}}".into(), "b".into()]);
        assert!(e.check(&Json::str("{{a}}"), false).is_err());
        // A default of a schema file follows the same rule (2.11).
        let bad_default =
            RELAY.replace("\"default\": \"info\"", "\"default\": \"{{x}}\"").replace("\"debug\"]", "\"{{x}}\"]");
        assert!(UnitSchema::parse(bad_default.as_bytes()).unwrap_err().iter().any(|e| e.path == "keys[1].default"));
    }

    #[test]
    fn the_set_of_schemas_agrees() {
        let a = UnitSchema::parse(RELAY.as_bytes()).unwrap();
        let b_text = RELAY.replace("relay_hosts", "feed_hosts").replace("relay_config", "feed_config");
        let b = UnitSchema::parse(b_text.as_bytes()).unwrap();
        let (ia, ib) = (SchemaId::parse("relay").unwrap(), SchemaId::parse("feed").unwrap());
        assert!(check_schema_set([(&ia, &a), (&ib, &b)]).is_empty());
        let c_text = b_text.replace(
            "\"kind\": \"integer\", \"max\": 100000, \"min\": 1",
            "\"kind\": \"size\", \"max\": 100000, \"min\": 1",
        );
        let c = UnitSchema::parse(c_text.as_bytes()).unwrap();
        assert_eq!(check_schema_set([(&ia, &a), (&ib, &c)]).len(), 1);
        assert_eq!(check_schema_set([(&ia, &a), (&ib, &a)]).len(), 2);
    }
}
