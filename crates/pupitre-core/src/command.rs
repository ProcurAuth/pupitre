// SPDX-License-Identifier: MIT OR Apache-2.0
//! The commands of the configuration file (4.32), their parameters (4.33) and the canonical
//! form of the parameters (4.34).

use std::collections::BTreeMap;

use hclconf::value::{format_duration, parse_duration};

use crate::error::FieldError;
use crate::ids::{CommandId, ParamName, PlaybookId};
use crate::json::{self, Json};
use crate::node::NodeType;
use crate::text::{NO_TEMPLATE_MARKER, template_marker};

/// The most parameters of one command (4.32).
pub const MAX_PARAMS: usize = 16;

/// The type of the node of a launch (4.32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `environment`
    Environment,
    /// `unit`
    Unit,
    /// `host`
    Host,
}

impl Target {
    /// The name in the configuration.
    pub fn name(self) -> &'static str {
        match self {
            Target::Environment => "environment",
            Target::Unit => "unit",
            Target::Host => "host",
        }
    }

    /// The node type of the target.
    pub fn node_type(self) -> NodeType {
        match self {
            Target::Environment => NodeType::Environment,
            Target::Unit => NodeType::Unit,
            Target::Host => NodeType::Host,
        }
    }
}

/// The type of a parameter with its range (4.33).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    /// `text`: a count of characters.
    Text {
        /// The fewest characters.
        min: u64,
        /// The most characters.
        max: u64,
    },
    /// `integer`
    Integer {
        /// The lowest value.
        min: u64,
        /// The highest value.
        max: u64,
    },
    /// `boolean`
    Boolean,
    /// `duration`, in milliseconds.
    Duration {
        /// The lowest value.
        min_ms: u64,
        /// The highest value.
        max_ms: u64,
    },
    /// `choice`: 1 to 32 texts.
    Choice(Vec<String>),
}

impl ParamType {
    /// The name of the type.
    pub fn name(&self) -> &'static str {
        match self {
            ParamType::Text { .. } => "text",
            ParamType::Integer { .. } => "integer",
            ParamType::Boolean => "boolean",
            ParamType::Duration { .. } => "duration",
            ParamType::Choice(_) => "choice",
        }
    }

    fn expected(&self) -> String {
        match self {
            ParamType::Text { min, max } => format!("a text of {min} to {max} characters with no control character"),
            ParamType::Integer { min, max } => format!("an integer from {min} to {max}"),
            ParamType::Boolean => "true or false".to_string(),
            ParamType::Duration { min_ms, max_ms } => {
                format!("a canonical duration from {} to {}", format_duration(*min_ms), format_duration(*max_ms))
            }
            ParamType::Choice(choices) => {
                let quoted: Vec<String> = choices.iter().map(|c| json::string_form(c)).collect();
                format!("one of {}", quoted.join(", "))
            }
        }
    }

    fn check(&self, value: &Json) -> bool {
        match (self, value) {
            (ParamType::Text { min, max }, Json::Str(t)) => {
                let n = t.chars().count() as u64;
                n >= *min && n <= *max && !t.chars().any(char::is_control)
            }
            (ParamType::Integer { min, max }, Json::Int(n)) => n >= min && n <= max,
            (ParamType::Boolean, Json::Bool(_)) => true,
            (ParamType::Duration { min_ms, max_ms }, Json::Str(t)) => {
                parse_duration(t).is_some_and(|ms| format_duration(ms) == *t && ms >= *min_ms && ms <= *max_ms)
            }
            (ParamType::Choice(choices), Json::Str(t)) => choices.iter().any(|c| c == t),
            _ => false,
        }
    }
}

/// A block `param "<name>"` (4.33).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSpec {
    /// The name.
    pub name: ParamName,
    /// The label of the page.
    pub label: String,
    /// True when the request must hold the parameter.
    pub required: bool,
    /// The type and its range.
    pub ty: ParamType,
}

/// A block `command "<id>"` (4.32).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// The id.
    pub id: CommandId,
    /// The label of the page.
    pub label: String,
    /// The playbook of kind `command`.
    pub playbook: PlaybookId,
    /// The type of the node of a launch.
    pub target: Target,
    /// True when an apply needs a check (4.24).
    pub check: bool,
    /// The parameters, in the order of the file.
    pub params: Vec<ParamSpec>,
}

/// The parameters of a run in the canonical form of 4.34.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CanonicalParams(pub BTreeMap<String, Json>);

impl CanonicalParams {
    /// The form of the run record (4.34): the JSON of the request.
    pub fn to_json(&self) -> Json {
        Json::Obj(self.0.clone())
    }

    /// The value of `pupitre_params` (4.14): a duration becomes an integer of milliseconds.
    pub fn to_ansible(&self, spec: &CommandSpec) -> Json {
        Json::Obj(self.ansible_members(spec))
    }

    /// The members of the object of `to_ansible`.
    pub fn ansible_members(&self, spec: &CommandSpec) -> BTreeMap<String, Json> {
        let mut out = BTreeMap::new();
        for (name, value) in &self.0 {
            let is_duration =
                spec.params.iter().any(|p| p.name.as_str() == name && matches!(p.ty, ParamType::Duration { .. }));
            let v = match (is_duration, value) {
                (true, Json::Str(t)) => parse_duration(t).map_or_else(|| value.clone(), Json::Int),
                _ => value.clone(),
            };
            out.insert(name.clone(), v);
        }
        out
    }
}

/// Check the parameters of a request against a command (4.33). The answer is the canonical
/// form, or one error for each problem.
pub fn check_params(spec: &CommandSpec, params: &Json) -> Result<CanonicalParams, Vec<FieldError>> {
    let Json::Obj(given) = params else {
        return Err(vec![FieldError::new("params", format!("expected an object, found {}", params.type_name()))]);
    };
    let mut errors = Vec::new();
    for name in given.keys() {
        if !spec.params.iter().any(|p| p.name.as_str() == name) {
            errors.push(FieldError::new(format!("params.{name}"), "the command has no such parameter"));
        }
    }
    let mut out = BTreeMap::new();
    for p in &spec.params {
        match given.get(p.name.as_str()) {
            None if p.required => errors.push(FieldError::new(format!("params.{}", p.name), "missing parameter")),
            None => {}
            Some(value) if !p.ty.check(value) => {
                let shown = String::from_utf8_lossy(&json::to_compact(value)).into_owned();
                errors.push(FieldError::new(
                    format!("params.{}", p.name),
                    format!("expected {}, found {shown}", p.ty.expected()),
                ));
            }
            // A text never holds a template marker of Ansible (4.33, 7.3).
            Some(Json::Str(text)) if matches!(p.ty, ParamType::Text { .. }) && template_marker(text).is_some() => {
                errors.push(FieldError::new(
                    format!("params.{}", p.name),
                    format!("expected a text with {NO_TEMPLATE_MARKER}, found {}", json::string_form(text)),
                ));
            }
            Some(value) => {
                out.insert(p.name.as_str().to_string(), value.clone());
            }
        }
    }
    if errors.is_empty() { Ok(CanonicalParams(out)) } else { Err(errors) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> CommandSpec {
        CommandSpec {
            id: CommandId::parse("restart-service").unwrap(),
            label: "Restart the service".into(),
            playbook: PlaybookId::parse("restart-service").unwrap(),
            target: Target::Unit,
            check: false,
            params: vec![
                ParamSpec {
                    name: ParamName::parse("grace").unwrap(),
                    label: "Grace period".into(),
                    required: true,
                    ty: ParamType::Duration { min_ms: 1000, max_ms: 300_000 },
                },
                ParamSpec {
                    name: ParamName::parse("mode").unwrap(),
                    label: "Mode".into(),
                    required: false,
                    ty: ParamType::Choice(vec!["soft".into(), "hard".into()]),
                },
            ],
        }
    }

    fn obj(text: &str) -> Json {
        json::parse(text.as_bytes(), json::ReadLimits::bytes(4096)).unwrap()
    }

    #[test]
    fn accepts_and_converts_a_duration() {
        let p = check_params(&spec(), &obj(r#"{"grace": "90s"}"#)).unwrap();
        assert_eq!(p.to_json(), obj(r#"{"grace": "90s"}"#));
        assert_eq!(p.to_ansible(&spec()), obj(r#"{"grace": 90000}"#));
    }

    #[test]
    fn refuses_with_the_name_the_range_and_the_value() {
        let e = check_params(&spec(), &obj(r#"{"grace": "10m", "mode": "x", "other": 1}"#)).unwrap_err();
        assert_eq!(e.len(), 3);
        assert!(e.iter().any(|f| f.message == "expected a canonical duration from 1s to 5m, found \"10m\""));
        let e = check_params(&spec(), &obj(r#"{"grace": "60s"}"#)).unwrap_err();
        assert_eq!(e[0].path, "params.grace");
        let e = check_params(&spec(), &obj(r#"{}"#)).unwrap_err();
        assert_eq!(e[0].message, "missing parameter");
    }

    /// 4.33, 7.3: a text parameter never holds `{{`, `{%` or `{#`. The error names the
    /// parameter, the rule and the value.
    #[test]
    fn a_text_parameter_with_a_template_marker_is_refused() {
        let mut s = spec();
        s.params.push(ParamSpec {
            name: ParamName::parse("banner").unwrap(),
            label: "Banner".into(),
            required: false,
            ty: ParamType::Text { min: 0, max: 200 },
        });
        for bad in [r#"{{ lookup('pipe','touch M') }}"#, "{% if 1 %}", "x{#y"] {
            let body = Json::object([("grace", Json::str("90s")), ("banner", Json::str(bad))]);
            let e = check_params(&s, &body).unwrap_err();
            assert_eq!(e.len(), 1);
            assert_eq!(e[0].path, "params.banner");
            assert_eq!(
                e[0].message,
                format!(
                    r#"expected a text with no template marker "{{{{", "{{%" or "{{#", found {}"#,
                    json::string_form(bad)
                )
            );
        }
        let body = Json::object([("grace", Json::str("90s")), ("banner", Json::str("{ok} }}"))]);
        assert!(check_params(&s, &body).is_ok());
    }
}
