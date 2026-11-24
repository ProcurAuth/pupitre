// SPDX-License-Identifier: MIT OR Apache-2.0
//! The checks of the values against the schemas (2.10, 2.15) and of a secret value (2.25).

use crate::doc::Values;
use crate::error::FieldError;
use crate::ids::KeyPath;
use crate::json::Json;
use crate::schema::UnitSchema;

/// The shortest secret value in bytes (2.25).
pub const SECRET_MIN_BYTES: usize = 8;
/// The longest secret value in bytes (2.25).
pub const SECRET_MAX_BYTES: usize = 65_536;

/// Check the values of a document `unit_values` or `host_values` against the schema of the
/// unit (2.10). Each problem is one error with the key path.
pub fn check_unit_values(schema: &UnitSchema, values: &Values) -> Vec<FieldError> {
    let mut errors = Vec::new();
    for (path, value) in &values.values {
        match schema.key(path) {
            None => errors.push(FieldError::new(path.as_str(), "the schema of the unit has no such key")),
            Some(key) if key.secret => {
                errors.push(FieldError::new(path.as_str(), "a secret key is not a value; write it as a secret"));
            }
            Some(key) => {
                if let Err(message) = key.kind.check(value, false) {
                    errors.push(FieldError::new(path.as_str(), message));
                }
            }
        }
    }
    errors
}

/// Check the values of a document `common` against all schemas of the configuration (2.10,
/// 2.15). A key path needs one schema that declares it. The value must pass the check of
/// each schema that declares it. No schema may declare it as a secret key.
pub fn check_common_values<'a, I>(schemas: I, values: &Values) -> Vec<FieldError>
where
    I: IntoIterator<Item = (&'a str, &'a UnitSchema)> + Clone,
{
    let mut errors = Vec::new();
    for (path, value) in &values.values {
        let mut declared = false;
        for (id, schema) in schemas.clone() {
            let Some(key) = schema.key(path) else { continue };
            declared = true;
            if key.secret {
                errors.push(FieldError::new(
                    path.as_str(),
                    format!("the schema {id} declares a secret key; write it as a secret"),
                ));
            } else if let Err(message) = key.kind.check(value, false) {
                errors.push(FieldError::new(path.as_str(), format!("schema {id}: {message}")));
            }
        }
        if !declared {
            errors.push(FieldError::new(path.as_str(), "no schema of the configuration has this key"));
        }
    }
    errors
}

/// Check a secret value for one key path against each schema that declares the key as a
/// secret key (2.24, 2.25). The message never holds the value.
pub fn check_secret_value<'a, I>(schemas: I, name: &KeyPath, value: &str) -> Vec<FieldError>
where
    I: IntoIterator<Item = (&'a str, &'a UnitSchema)>,
{
    let mut errors = Vec::new();
    if value.len() < SECRET_MIN_BYTES || value.len() > SECRET_MAX_BYTES || value.contains('\0') {
        errors.push(FieldError::new(
            name.as_str(),
            format!(
                "expected a text of {SECRET_MIN_BYTES} to {SECRET_MAX_BYTES} bytes with no U+0000, found (value hidden)"
            ),
        ));
        return errors;
    }
    let mut declared = false;
    for (id, schema) in schemas {
        let Some(key) = schema.key(name) else { continue };
        if !key.secret {
            continue;
        }
        declared = true;
        if let Err(message) = key.kind.check(&Json::str(value), true) {
            errors.push(FieldError::new(name.as_str(), format!("schema {id}: {message}")));
        }
    }
    if !declared {
        errors.push(FieldError::new(name.as_str(), "no schema declares this secret key"));
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{self, ReadLimits};

    fn schema(tag: &str, levels: &str) -> UnitSchema {
        let text = format!(
            r#"{{"format": 1, "group": "g_{tag}", "variable": "v_{tag}", "keys": [
                {{"description": "L.", "kind": "enum", "path": "log.level", "values": [{levels}]}},
                {{"description": "P.", "kind": "text", "max": 64, "min": 8, "path": "pw", "rule": "any", "secret": true}}
            ]}}"#
        );
        UnitSchema::parse(text.as_bytes()).unwrap()
    }

    fn values(text: &str) -> Values {
        let v = json::parse(text.as_bytes(), ReadLimits::bytes(1 << 16)).unwrap();
        crate::doc::Document::from_json(crate::doc::DocKind::Common, &v).unwrap().values().cloned().unwrap_or_default()
    }

    #[test]
    fn unit_values() {
        let s = schema("s", "\"info\", \"debug\"");
        assert!(check_unit_values(&s, &values(r#"{"format": 1, "values": {"log.level": "info"}}"#)).is_empty());
        assert_eq!(check_unit_values(&s, &values(r#"{"format": 1, "values": {"log.level": "trace"}}"#)).len(), 1);
        assert_eq!(check_unit_values(&s, &values(r#"{"format": 1, "values": {"x": 1}}"#)).len(), 1);
        assert_eq!(check_unit_values(&s, &values(r#"{"format": 1, "values": {"pw": "abcdefghij"}}"#)).len(), 1);
    }

    #[test]
    fn common_values_pass_each_schema() {
        let a = schema("a", "\"info\", \"debug\"");
        let b = schema("b", "\"info\"");
        let set = [("a", &a), ("b", &b)];
        assert!(check_common_values(set, &values(r#"{"format": 1, "values": {"log.level": "info"}}"#)).is_empty());
        let e = check_common_values(set, &values(r#"{"format": 1, "values": {"log.level": "debug"}}"#));
        assert_eq!(e.len(), 1);
        assert!(e[0].message.starts_with("schema b:"));
    }

    /// 2.10, 2.12, 7.3: a document of values with a template marker in a text is refused. The
    /// error names the key path.
    #[test]
    fn a_template_marker_in_a_value_is_refused() {
        let text = r#"{"format": 1, "group": "g", "variable": "v", "keys": [
            {"description": "B.", "kind": "text", "max": 200, "min": 0, "path": "banner", "rule": "printable_ascii"},
            {"description": "L.", "kind": "text_list", "item_max": 64, "item_min": 1, "max_items": 4, "min_items": 0, "path": "names", "rule": "any"},
            {"description": "P.", "kind": "text", "max": 64, "min": 8, "path": "pw", "rule": "any", "secret": true}
        ]}"#;
        let s = UnitSchema::parse(text.as_bytes()).unwrap();
        let bad =
            values(r#"{"format": 1, "values": {"banner": "{{ lookup('pipe','id') }}", "names": ["a", "{% x %}"]}}"#);
        let errors = check_unit_values(&s, &bad);
        assert_eq!(errors.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["banner", "names"]);
        assert!(errors.iter().all(|e| e.message.contains("no template marker")), "{errors:?}");
        let errors = check_common_values([("s", &s)], &bad);
        assert_eq!(errors.len(), 2);
        assert!(check_unit_values(&s, &values(r#"{"format": 1, "values": {"banner": "{x} }}"}}"#)).is_empty());
        // A secret value can hold a marker: it is a file for Ansible, never a variable (4.13).
        assert!(check_secret_value([("s", &s)], &KeyPath::parse("pw").unwrap(), "pa{{ss}}word").is_empty());
    }

    #[test]
    fn secret_values_never_show() {
        let a = schema("a", "\"info\"");
        let name = KeyPath::parse("pw").unwrap();
        assert!(check_secret_value([("a", &a)], &name, "correct horse").is_empty());
        let e = check_secret_value([("a", &a)], &name, "short");
        assert!(!e[0].message.contains("short"));
        assert_eq!(check_secret_value([("a", &a)], &KeyPath::parse("log.level").unwrap(), "long enough").len(), 1);
    }
}
