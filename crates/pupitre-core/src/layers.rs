// SPDX-License-Identifier: MIT OR Apache-2.0
//! The resolution of the layers (2.13), the provenance (2.14) and the nested variable of a
//! host (4.12).

use std::collections::BTreeMap;

use crate::doc::Values;
use crate::ids::KeyPath;
use crate::json::Json;
use crate::schema::UnitSchema;

/// A layer of a value (2.13), in the order of the resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// The `default` of the schema.
    Default,
    /// The document `common` of the environment.
    Environment,
    /// The document `unit_values` of the unit.
    Unit,
    /// The document `host_values` of the unit on the host.
    Host,
}

impl Layer {
    /// The name of the layer.
    pub fn name(self) -> &'static str {
        match self {
            Layer::Default => "default",
            Layer::Environment => "environment",
            Layer::Unit => "unit",
            Layer::Host => "host",
        }
    }
}

/// The fingerprints of the secrets of the two secret layers (2.13, 2.26), by key path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretLayers {
    /// The scope of the environment.
    pub environment: BTreeMap<KeyPath, String>,
    /// The scope of the unit.
    pub unit: BTreeMap<KeyPath, String>,
}

/// The inputs of one resolution: a unit on a host, or a unit with no host.
#[derive(Debug, Clone, Copy)]
pub struct LayerInputs<'a> {
    /// The schema of the unit.
    pub schema: &'a UnitSchema,
    /// The document `common`, when it exists.
    pub common: Option<&'a Values>,
    /// The document `unit_values`, when it exists.
    pub unit: Option<&'a Values>,
    /// The document `host_values`, when it exists. `None` for the view of the unit.
    pub host: Option<&'a Values>,
    /// The fingerprints of the secrets.
    pub secrets: &'a SecretLayers,
}

/// The value of one layer of one key (2.14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerValue {
    /// A value of a key that is not secret.
    Value(Json),
    /// The fingerprint of a secret.
    Fingerprint(String),
}

/// A stored value that fails its check (2.13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerError {
    /// The layer of the value.
    pub layer: Layer,
    /// The message of the check.
    pub message: String,
}

/// The resolution of one key (2.14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKey {
    /// The key path.
    pub path: KeyPath,
    /// The name of the kind.
    pub kind: &'static str,
    /// True for a secret key.
    pub secret: bool,
    /// The layer of the effective value.
    pub layer: Option<Layer>,
    /// The effective value. `None` for a secret key or when no layer gives one.
    pub value: Option<Json>,
    /// The value of each layer that gives one, in layer order.
    pub layers: Vec<(Layer, LayerValue)>,
    /// The error of a stored value that fails its check.
    pub error: Option<LayerError>,
}

/// The resolution of a unit (2.13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// One item for each key of the schema, in the order of the schema.
    pub keys: Vec<ResolvedKey>,
}

/// Resolve the layers of each key of the schema (2.13).
pub fn resolve(inputs: &LayerInputs<'_>) -> Resolution {
    let mut keys = Vec::with_capacity(inputs.schema.keys.len());
    for key in &inputs.schema.keys {
        let mut item = ResolvedKey {
            path: key.path.clone(),
            kind: key.kind.name(),
            secret: key.secret,
            layer: None,
            value: None,
            layers: Vec::new(),
            error: None,
        };
        if key.secret {
            for (layer, map) in [(Layer::Environment, &inputs.secrets.environment), (Layer::Unit, &inputs.secrets.unit)]
            {
                if let Some(fp) = map.get(&key.path) {
                    item.layers.push((layer, LayerValue::Fingerprint(fp.clone())));
                    item.layer = Some(layer);
                }
            }
            keys.push(item);
            continue;
        }
        let sources = [
            (Layer::Default, key.default.as_ref()),
            (Layer::Environment, inputs.common.and_then(|v| v.values.get(&key.path))),
            (Layer::Unit, inputs.unit.and_then(|v| v.values.get(&key.path))),
            (Layer::Host, inputs.host.and_then(|v| v.values.get(&key.path))),
        ];
        for (layer, value) in sources {
            let Some(value) = value else { continue };
            item.layers.push((layer, LayerValue::Value(value.clone())));
            match key.kind.check(value, false) {
                Ok(()) => {
                    item.layer = Some(layer);
                    item.value = Some(value.clone());
                }
                Err(message) => item.error = Some(LayerError { layer, message }),
            }
        }
        if item.error.is_some() {
            item.layer = None;
            item.value = None;
        }
        keys.push(item);
    }
    Resolution { keys }
}

impl Resolution {
    /// True when a stored value fails its check (2.13). A run then stops (4.9).
    pub fn has_errors(&self) -> bool {
        self.keys.iter().any(|k| k.error.is_some())
    }

    /// The list `keys` of the provenance (2.14, 5.12).
    pub fn to_json(&self) -> Json {
        let items = self
            .keys
            .iter()
            .map(|k| {
                let mut map = BTreeMap::new();
                map.insert("path".to_string(), Json::str(k.path.as_str()));
                map.insert("kind".to_string(), Json::str(k.kind));
                map.insert("secret".to_string(), Json::Bool(k.secret));
                if let Some(layer) = k.layer {
                    map.insert("layer".to_string(), Json::str(layer.name()));
                }
                if let Some(value) = &k.value {
                    map.insert("value".to_string(), value.clone());
                }
                let layers = k
                    .layers
                    .iter()
                    .map(|(layer, v)| {
                        let shown = match v {
                            LayerValue::Value(j) => j.clone(),
                            LayerValue::Fingerprint(fp) => Json::object([("fingerprint", Json::str(fp.clone()))]),
                        };
                        (layer.name().to_string(), shown)
                    })
                    .collect();
                map.insert("layers".to_string(), Json::Obj(layers));
                if let Some(e) = &k.error {
                    map.insert(
                        "error".to_string(),
                        Json::object([("layer", Json::str(e.layer.name())), ("message", Json::str(e.message.clone()))]),
                    );
                }
                Json::Obj(map)
            })
            .collect();
        Json::Arr(items)
    }

    /// The nested object of the effective values that are not secret (4.12). Each segment of
    /// a key path is one level. A key with no effective value is absent.
    pub fn nested_variable(&self) -> Json {
        Json::Obj(self.nested_members())
    }

    /// The members of the object of `nested_variable`.
    pub fn nested_members(&self) -> BTreeMap<String, Json> {
        let mut root = BTreeMap::new();
        for key in &self.keys {
            let Some(value) = &key.value else { continue };
            let segments: Vec<&str> = key.path.segments().collect();
            insert_nested(&mut root, &segments, value.clone());
        }
        root
    }
}

fn insert_nested(map: &mut BTreeMap<String, Json>, segments: &[&str], value: Json) {
    match segments {
        [] => {}
        [last] => {
            map.insert((*last).to_string(), value);
        }
        [first, rest @ ..] => {
            let entry = map.entry((*first).to_string()).or_insert_with(|| Json::Obj(BTreeMap::new()));
            // A schema never has a path that starts another path (2.11), so the entry is an
            // object. A value in its place is replaced by nothing: the schema check prevents it.
            if let Json::Obj(inner) = entry {
                insert_nested(inner, rest, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{DocKind, Document};
    use crate::json::{self, ReadLimits};

    const SCHEMA: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
        {"default": 100, "description": "C.", "kind": "integer", "max": 100000, "min": 1, "path": "limits.max_clients"},
        {"default": "info", "description": "L.", "kind": "enum", "path": "log.level", "values": ["info", "debug"]},
        {"description": "P.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true},
        {"default": "5s", "description": "T.", "kind": "duration", "max": "1m", "min": "1s", "path": "timeouts.connect"}
    ]}"#;

    fn values(text: &str) -> Values {
        let v = json::parse(text.as_bytes(), ReadLimits::bytes(1 << 16)).unwrap();
        Document::from_json(DocKind::Common, &v).unwrap().values().cloned().unwrap_or_default()
    }

    #[test]
    fn the_example_of_the_readme() {
        let schema = UnitSchema::parse(SCHEMA.as_bytes()).unwrap();
        let common = values(r#"{"format": 1, "values": {"limits.max_clients": 200, "log.level": "info"}}"#);
        let unit = values(r#"{"format": 1, "values": {"limits.max_clients": 300, "timeouts.connect": "10s"}}"#);
        let host = values(r#"{"format": 1, "values": {"limits.max_clients": 500}}"#);
        let mut secrets = SecretLayers::default();
        secrets.unit.insert(KeyPath::parse("storage.password").unwrap(), "3f9a0c1d2e4b5a69".to_string());
        let r = resolve(&LayerInputs {
            schema: &schema,
            common: Some(&common),
            unit: Some(&unit),
            host: Some(&host),
            secrets: &secrets,
        });
        let by_path = |p: &str| r.keys.iter().find(|k| k.path.as_str() == p).unwrap();
        assert_eq!(by_path("limits.max_clients").layer, Some(Layer::Host));
        assert_eq!(by_path("limits.max_clients").value, Some(Json::Int(500)));
        assert_eq!(by_path("limits.max_clients").layers.len(), 4);
        assert_eq!(by_path("log.level").layer, Some(Layer::Environment));
        assert_eq!(by_path("timeouts.connect").value, Some(Json::str("10s")));
        assert_eq!(by_path("storage.password").layer, Some(Layer::Unit));
        assert_eq!(by_path("storage.password").value, None);
        assert!(!r.has_errors());
        let nested = String::from_utf8(json::to_compact(&r.nested_variable())).unwrap();
        assert_eq!(nested, r#"{"limits":{"max_clients":500},"log":{"level":"info"},"timeouts":{"connect":"10s"}}"#);
    }

    #[test]
    fn the_view_of_a_unit_has_no_host_layer() {
        let schema = UnitSchema::parse(SCHEMA.as_bytes()).unwrap();
        let secrets = SecretLayers::default();
        let r = resolve(&LayerInputs { schema: &schema, common: None, unit: None, host: None, secrets: &secrets });
        assert_eq!(r.keys[0].layer, Some(Layer::Default));
        assert_eq!(r.keys[2].layer, None);
    }

    #[test]
    fn a_stored_value_that_fails_gives_an_error_and_no_value() {
        let schema = UnitSchema::parse(SCHEMA.as_bytes()).unwrap();
        let unit = values(r#"{"format": 1, "values": {"log.level": "trace"}}"#);
        let secrets = SecretLayers::default();
        let r =
            resolve(&LayerInputs { schema: &schema, common: None, unit: Some(&unit), host: None, secrets: &secrets });
        let key = r.keys.iter().find(|k| k.path.as_str() == "log.level").unwrap();
        assert_eq!(key.error.as_ref().map(|e| e.layer), Some(Layer::Unit));
        assert_eq!(key.value, None);
        assert!(r.has_errors());
    }
}
