// SPDX-License-Identifier: MIT OR Apache-2.0
//! The catalog of capabilities: the schemas, the playbooks and the commands of the
//! configuration file (4.2). The store never holds a part of it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use hclconf::value::format_duration;

use crate::command::{CommandSpec, ParamType};
use crate::ids::{CommandId, KeyPath, PlaybookId, SchemaId};
use crate::json::Json;
use crate::schema::UnitSchema;

/// The kind of a playbook (4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlaybookKind {
    /// `deploy`
    Deploy,
    /// `render`
    Render,
    /// `status`
    Status,
    /// `command`
    Command,
}

impl PlaybookKind {
    /// The four kinds.
    pub const ALL: [PlaybookKind; 4] =
        [PlaybookKind::Deploy, PlaybookKind::Render, PlaybookKind::Status, PlaybookKind::Command];

    /// The name of the kind.
    pub fn name(self) -> &'static str {
        match self {
            PlaybookKind::Deploy => "deploy",
            PlaybookKind::Render => "render",
            PlaybookKind::Status => "status",
            PlaybookKind::Command => "command",
        }
    }

    /// The kind of a name.
    pub fn from_name(name: &str) -> Option<PlaybookKind> {
        PlaybookKind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A block `playbook "<id>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybookSpec {
    /// The id.
    pub id: PlaybookId,
    /// The kind.
    pub kind: PlaybookKind,
    /// The absolute path of the playbook file.
    pub path: PathBuf,
}

/// A block `schema "<id>"` with the schema of its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSpec {
    /// The id.
    pub id: SchemaId,
    /// The absolute path of the schema file.
    pub path: PathBuf,
    /// The schema that the file holds.
    pub schema: UnitSchema,
}

/// The catalog of capabilities (4.2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// The schemas, by id.
    pub schemas: BTreeMap<SchemaId, SchemaSpec>,
    /// The playbooks, by id. Empty with `runner.enabled = false`.
    pub playbooks: BTreeMap<PlaybookId, PlaybookSpec>,
    /// The commands, by id. Empty with `runner.enabled = false`.
    pub commands: BTreeMap<CommandId, CommandSpec>,
}

impl Capabilities {
    /// The schemas with their ids as text, for the checks of 2.15.
    pub fn schema_set(&self) -> Vec<(&str, &UnitSchema)> {
        self.schemas.values().map(|s| (s.id.as_str(), &s.schema)).collect()
    }

    /// True when a schema declares the key path as a secret key (2.24).
    pub fn is_secret_key(&self, path: &KeyPath) -> bool {
        self.schemas.values().any(|s| s.schema.key(path).is_some_and(|k| k.secret))
    }

    /// The playbook of an id, when it has the kind.
    pub fn playbook_of_kind(&self, id: &PlaybookId, kind: PlaybookKind) -> Option<&PlaybookSpec> {
        self.playbooks.get(id).filter(|p| p.kind == kind)
    }

    /// True when an inventory group of a schema has this name (2.8).
    pub fn is_schema_group(&self, name: &str) -> bool {
        self.schemas.values().any(|s| s.schema.group.as_str() == name)
    }

    /// The body of `GET /api/catalog` (5.15). It holds no path.
    pub fn catalog_json(&self, runner_enabled: bool) -> Json {
        let schemas = self
            .schemas
            .values()
            .map(|s| {
                let mut obj = s.schema.to_json();
                if let Json::Obj(map) = &mut obj {
                    map.remove("format");
                    map.insert("id".to_string(), Json::str(s.id.as_str()));
                }
                obj
            })
            .collect();
        let mut pairs = vec![("schemas", Json::Arr(schemas))];
        if runner_enabled {
            let playbooks = self
                .playbooks
                .values()
                .map(|p| Json::object([("id", Json::str(p.id.as_str())), ("kind", Json::str(p.kind.name()))]))
                .collect();
            let commands = self.commands.values().map(command_json).collect();
            pairs.push(("playbooks", Json::Arr(playbooks)));
            pairs.push(("commands", Json::Arr(commands)));
        }
        Json::object(pairs)
    }
}

fn command_json(c: &CommandSpec) -> Json {
    let params = c
        .params
        .iter()
        .map(|p| {
            let mut map = BTreeMap::new();
            map.insert("name".to_string(), Json::str(p.name.as_str()));
            map.insert("label".to_string(), Json::str(p.label.clone()));
            map.insert("required".to_string(), Json::Bool(p.required));
            map.insert("type".to_string(), Json::str(p.ty.name()));
            match &p.ty {
                ParamType::Text { min, max } | ParamType::Integer { min, max } => {
                    map.insert("min".to_string(), Json::Int(*min));
                    map.insert("max".to_string(), Json::Int(*max));
                }
                ParamType::Duration { min_ms, max_ms } => {
                    map.insert("min_duration".to_string(), Json::str(format_duration(*min_ms)));
                    map.insert("max_duration".to_string(), Json::str(format_duration(*max_ms)));
                }
                ParamType::Choice(choices) => {
                    map.insert(
                        "choices".to_string(),
                        Json::Arr(choices.iter().map(|c| Json::str(c.clone())).collect()),
                    );
                }
                ParamType::Boolean => {}
            }
            Json::Obj(map)
        })
        .collect();
    Json::object([
        ("check", Json::Bool(c.check)),
        ("id", Json::str(c.id.as_str())),
        ("label", Json::str(c.label.clone())),
        ("params", Json::Arr(params)),
        ("playbook", Json::str(c.playbook.as_str())),
        ("target", Json::str(c.target.name())),
    ])
}
