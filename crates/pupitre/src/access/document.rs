// SPDX-License-Identifier: MIT OR Apache-2.0
//! The action document of 3.11.

use procurauth_pep::document::{ActionDocument, ActionDocumentBuilder, ServedDocument};
use pupitre_core::action::{Action, MAX_HOSTS};
use pupitre_core::json::Json;
use pupitre_core::node::NodeType;

/// The three types below the service (3.11): the name, the parent and the description.
const TYPES: [(&str, &str, &str); 3] = [
    ("environment", "service", "One environment: a set of hosts and units."),
    ("unit", "environment", "One deployed service of an environment."),
    ("host", "unit", "One host of a unit."),
];
/// The description of the ceiling `max_hosts` (3.11).
const CEILING_DESCRIPTION: &str = "The most hosts that one run or one command can reach.";
const CEILING_LABEL: &str = "Most hosts of one run";
const CEILING_UNIT: &str = "hosts";

/// The action document of 3.11, built with the builder of the library `procurauth-pep`. The
/// builder checks the rules of P 5.2, P 5.3 and P 6.1. The error is the first finding.
pub fn library_document(audience: &str, runner_enabled: bool, max_hosts: u64) -> Result<ActionDocument, String> {
    let mut builder = ActionDocumentBuilder::new(audience);
    for (name, parent, description) in TYPES {
        builder = builder.resource_type(name, parent, description);
    }
    for action in Action::published(runner_enabled) {
        let types: Vec<&str> = action.resource_types().iter().map(|t| t.name()).collect();
        builder = builder.action(action.name(), &types, action.description(), action.ceilings());
    }
    if runner_enabled {
        builder = builder.ceiling(MAX_HOSTS, CEILING_LABEL, CEILING_DESCRIPTION, CEILING_UNIT, 1, max_hosts);
    }
    builder.build().map_err(|findings| {
        findings.first().map_or_else(|| "the action document is not valid".to_owned(), ToString::to_string)
    })
}

/// The bytes that the route of 3.11 serves: the document of the library as compact JSON.
pub fn action_document_bytes(audience: &str, runner_enabled: bool, max_hosts: u64) -> Result<Vec<u8>, String> {
    let document = library_document(audience, runner_enabled, max_hosts)?;
    let served = ServedDocument::new(&document).map_err(|findings| {
        findings.first().map_or_else(|| "the action document is not valid".to_owned(), ToString::to_string)
    })?;
    Ok(served.body().to_vec())
}

/// The action document of 3.11 for a service id, a value of `runner.enabled` and a value of
/// `runner.max_hosts`.
pub fn action_document(audience: &str, runner_enabled: bool, max_hosts: u64) -> Json {
    let resource_types = TYPES
        .iter()
        .map(|(name, parent, description)| {
            Json::object([
                ("description", Json::str(*description)),
                ("name", Json::str(*name)),
                ("parent", Json::str(*parent)),
            ])
        })
        .collect();
    let actions = Action::published(runner_enabled)
        .into_iter()
        .map(|a| {
            let mut pairs = vec![
                ("description", Json::str(a.description())),
                ("name", Json::str(a.name())),
                (
                    "resource_types",
                    Json::Arr(a.resource_types().iter().map(|t: &NodeType| Json::str(t.name())).collect()),
                ),
            ];
            if !a.ceilings().is_empty() {
                pairs.push(("limits", Json::Arr(a.ceilings().iter().map(|c| Json::str(*c)).collect())));
            }
            Json::object(pairs)
        })
        .collect();
    let mut pairs = vec![
        ("actions", Json::Arr(actions)),
        ("resource_types", Json::Arr(resource_types)),
        ("server", Json::str(audience)),
    ];
    if runner_enabled {
        pairs.push((
            "limits",
            Json::Arr(vec![Json::object([
                ("description", Json::str(CEILING_DESCRIPTION)),
                ("kind", Json::str("ceiling")),
                ("label", Json::str(CEILING_LABEL)),
                ("max", Json::Int(max_hosts)),
                ("min", Json::Int(1)),
                ("name", Json::str(MAX_HOSTS)),
                ("unit", Json::str(CEILING_UNIT)),
            ])]),
        ));
    }
    Json::object(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pupitre_core::json::{self, ReadLimits};

    #[test]
    fn the_bytes_of_the_library_are_the_compact_form_of_the_document_of_3_11() {
        for (enabled, max) in [(true, 100u64), (true, 1), (true, 9_007_199_254_740_991), (false, 100)] {
            let bytes = action_document_bytes("pupitre", enabled, max).unwrap();
            let expected = json::to_compact(&action_document("pupitre", enabled, max));
            assert_eq!(String::from_utf8_lossy(&bytes), String::from_utf8_lossy(&expected));
        }
        // The builder of the library refuses a bad service id.
        assert!(action_document_bytes("", true, 100).is_err());
        assert!(action_document_bytes("a\nb", true, 100).is_err());
    }

    #[test]
    fn the_document_of_3_11() {
        let doc = action_document("pupitre", true, 100);
        let bytes = json::to_compact(&doc);
        assert!(bytes.len() < 8192);
        let again = json::parse(&bytes, ReadLimits::bytes(65_536)).unwrap();
        let actions = again.as_object().and_then(|m| m.get("actions")).and_then(Json::as_array).unwrap();
        assert_eq!(actions.len(), 8);
        let apply = actions
            .iter()
            .find(|a| a.as_object().and_then(|m| m.get("name")) == Some(&Json::str("run.apply")))
            .unwrap();
        assert_eq!(apply.as_object().and_then(|m| m.get("limits")), Some(&Json::Arr(vec![Json::str("max_hosts")])));
        let off = action_document("pupitre", false, 100);
        let m = off.as_object().unwrap();
        assert!(!m.contains_key("limits"));
        assert_eq!(m.get("actions").and_then(Json::as_array).map(<[Json]>::len), Some(4));
        assert!(!m.contains_key("resources"));
    }
}
