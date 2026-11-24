// SPDX-License-Identifier: MIT OR Apache-2.0
//! The actions of the action document (3.9) and the ceiling `max_hosts` (3.10).

use crate::node::NodeType;

/// The name of the action that opens the service to a subject (P 8.3, 3.20). It is never in
/// the action document.
pub const ACCESS_ACTION: &str = "access";

/// The name of the one ceiling (3.10).
pub const MAX_HOSTS: &str = "max_hosts";

/// An action of 3.9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// `config.read`
    ConfigRead,
    /// `config.write`
    ConfigWrite,
    /// `secret.write`
    SecretWrite,
    /// `inventory.write`
    InventoryWrite,
    /// `run.read`
    RunRead,
    /// `run.check`
    RunCheck,
    /// `run.apply`
    RunApply,
    /// `command.run`
    CommandRun,
}

const ALL_TYPES: &[NodeType] = &[NodeType::Service, NodeType::Environment, NodeType::Unit, NodeType::Host];

impl Action {
    /// The eight actions, in the order of 3.9 and of the action document.
    pub const ALL: [Action; 8] = [
        Action::ConfigRead,
        Action::ConfigWrite,
        Action::SecretWrite,
        Action::InventoryWrite,
        Action::RunRead,
        Action::RunCheck,
        Action::RunApply,
        Action::CommandRun,
    ];

    /// The name of the action.
    pub fn name(self) -> &'static str {
        match self {
            Action::ConfigRead => "config.read",
            Action::ConfigWrite => "config.write",
            Action::SecretWrite => "secret.write",
            Action::InventoryWrite => "inventory.write",
            Action::RunRead => "run.read",
            Action::RunCheck => "run.check",
            Action::RunApply => "run.apply",
            Action::CommandRun => "command.run",
        }
    }

    /// The action of a name, or `None`.
    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.name() == name)
    }

    /// The `resource_types` of the action (3.9).
    pub fn resource_types(self) -> &'static [NodeType] {
        match self {
            Action::SecretWrite => &[NodeType::Service, NodeType::Environment, NodeType::Unit],
            Action::InventoryWrite => &[NodeType::Service, NodeType::Environment],
            _ => ALL_TYPES,
        }
    }

    /// The ceilings that the action lists (3.10).
    pub fn ceilings(self) -> &'static [&'static str] {
        match self {
            Action::RunCheck | Action::RunApply | Action::CommandRun => &[MAX_HOSTS],
            _ => &[],
        }
    }

    /// True for the actions that need `runner.enabled = true` (3.3).
    pub fn needs_runner(self) -> bool {
        matches!(self, Action::RunRead | Action::RunCheck | Action::RunApply | Action::CommandRun)
    }

    /// The description of the action document (3.11).
    pub fn description(self) -> &'static str {
        match self {
            Action::ConfigRead => "Read the documents, the effective values and the history. Never a secret value.",
            Action::ConfigWrite => "Write the values of the layer of the node.",
            Action::SecretWrite => "Set and delete the secrets of the node. No action reads a secret.",
            Action::InventoryWrite => "Create, change and delete environments, hosts and units.",
            Action::RunRead => "Read the runs, their events and their output.",
            Action::RunCheck => "Start a check run or a status run.",
            Action::RunApply => "Start an apply run after a check run that passed.",
            Action::CommandRun => "Start a command of the configuration file.",
        }
    }

    /// The published actions for a value of `runner.enabled` (3.3, P 5.2a).
    pub fn published(runner_enabled: bool) -> Vec<Action> {
        Action::ALL.into_iter().filter(|a| runner_enabled || !a.needs_runner()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for action in Action::ALL {
            assert_eq!(Action::from_name(action.name()), Some(action));
        }
        assert_eq!(Action::from_name(ACCESS_ACTION), None);
    }

    #[test]
    fn publication_follows_the_runner() {
        assert_eq!(Action::published(true).len(), 8);
        let off: Vec<&str> = Action::published(false).iter().map(|a| a.name()).collect();
        assert_eq!(off, vec!["config.read", "config.write", "secret.write", "inventory.write"]);
    }
}
