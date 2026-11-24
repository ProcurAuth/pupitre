// SPDX-License-Identifier: MIT OR Apache-2.0
//! The nodes of the resource tree (3.7), their path form (3.8), and the scopes of the
//! secrets (2.24).

use std::fmt;

use crate::ids::{EnvId, HostId, IdError, UnitId};

/// The type of a node (3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeType {
    /// The service itself, `access.identity.audience`.
    Service,
    /// An environment, depth 1.
    Environment,
    /// A unit of an environment, depth 2.
    Unit,
    /// A host of a unit, depth 3.
    Host,
}

impl NodeType {
    /// The type name of the protocol.
    pub fn name(self) -> &'static str {
        match self {
            NodeType::Service => "service",
            NodeType::Environment => "environment",
            NodeType::Unit => "unit",
            NodeType::Host => "host",
        }
    }
}

/// A node of the resource tree. The server builds it from the catalog, never from a request
/// (P 4.5).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Node {
    /// The service.
    Service,
    /// `environment:<e>`.
    Environment(EnvId),
    /// `environment:<e>/unit:<u>`.
    Unit(EnvId, UnitId),
    /// `environment:<e>/unit:<u>/host:<h>`.
    Host(EnvId, UnitId, HostId),
}

/// A node path that is not valid (3.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodePathError {
    /// The path does not have the form of 3.8.
    Form,
    /// An id breaks its rule.
    Id(IdError),
}

impl fmt::Display for NodePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodePathError::Form => f.write_str(
                "expected a node path: environment:<e>, environment:<e>/unit:<u> or environment:<e>/unit:<u>/host:<h>",
            ),
            NodePathError::Id(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for NodePathError {}

impl Node {
    /// The type of the node.
    pub fn node_type(&self) -> NodeType {
        match self {
            Node::Service => NodeType::Service,
            Node::Environment(_) => NodeType::Environment,
            Node::Unit(..) => NodeType::Unit,
            Node::Host(..) => NodeType::Host,
        }
    }

    /// The environment of the node, or `None` for the service.
    pub fn environment(&self) -> Option<&EnvId> {
        match self {
            Node::Service => None,
            Node::Environment(e) | Node::Unit(e, _) | Node::Host(e, _, _) => Some(e),
        }
    }

    /// The path form of 3.8, without the service. The service has the empty text.
    pub fn path(&self) -> String {
        match self {
            Node::Service => String::new(),
            Node::Environment(e) => format!("environment:{e}"),
            Node::Unit(e, u) => format!("environment:{e}/unit:{u}"),
            Node::Host(e, u, h) => format!("environment:{e}/unit:{u}/host:{h}"),
        }
    }

    /// Read a node path of 3.8. The empty text is not a path: the service has no path in a
    /// request.
    pub fn parse_path(text: &str) -> Result<Node, NodePathError> {
        let parts: Vec<&str> = text.split('/').collect();
        let id_of = |part: &str, type_name: &str| -> Result<String, NodePathError> {
            match part.split_once(':') {
                Some((t, id)) if t == type_name => Ok(id.to_string()),
                _ => Err(NodePathError::Form),
            }
        };
        let env = |p: &str| -> Result<EnvId, NodePathError> {
            EnvId::parse(&id_of(p, "environment")?).map_err(NodePathError::Id)
        };
        match parts.as_slice() {
            [e] => Ok(Node::Environment(env(e)?)),
            [e, u] => Ok(Node::Unit(env(e)?, UnitId::parse(&id_of(u, "unit")?).map_err(NodePathError::Id)?)),
            [e, u, h] => Ok(Node::Host(
                env(e)?,
                UnitId::parse(&id_of(u, "unit")?).map_err(NodePathError::Id)?,
                HostId::parse(&id_of(h, "host")?).map_err(NodePathError::Id)?,
            )),
            _ => Err(NodePathError::Form),
        }
    }

    /// The chain of the ancestors, from the service to the parent of the node (P 4.1). The
    /// service has no ancestors.
    pub fn ancestors(&self) -> Vec<Node> {
        match self {
            Node::Service => Vec::new(),
            Node::Environment(_) => vec![Node::Service],
            Node::Unit(e, _) => vec![Node::Service, Node::Environment(e.clone())],
            Node::Host(e, u, _) => vec![Node::Service, Node::Environment(e.clone()), Node::Unit(e.clone(), u.clone())],
        }
    }

    /// The id of the node, or `None` for the service (its id is the service id of the
    /// configuration).
    pub fn id(&self) -> Option<&str> {
        match self {
            Node::Service => None,
            Node::Environment(e) => Some(e.as_str()),
            Node::Unit(_, u) => Some(u.as_str()),
            Node::Host(_, _, h) => Some(h.as_str()),
        }
    }

    /// True when the node is `self` or below it (P 4.3).
    pub fn covers(&self, other: &Node) -> bool {
        self == other || other.ancestors().contains(self)
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path())
    }
}

/// The scope of a secret (2.24).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretScope {
    /// `environment:<env>`.
    Environment(EnvId),
    /// `environment:<env>/unit:<unit>`.
    Unit(EnvId, UnitId),
}

impl SecretScope {
    /// The text form of 2.24, the node path of the scope.
    pub fn text(&self) -> String {
        self.node().path()
    }

    /// The node of the scope.
    pub fn node(&self) -> Node {
        match self {
            SecretScope::Environment(e) => Node::Environment(e.clone()),
            SecretScope::Unit(e, u) => Node::Unit(e.clone(), u.clone()),
        }
    }

    /// The folder of the scope in the layouts of 2.31 and 4.13.
    pub fn folder(&self) -> String {
        match self {
            SecretScope::Environment(e) => format!("environments/{e}"),
            SecretScope::Unit(e, u) => format!("environments/{e}/units/{u}"),
        }
    }

    /// Read the text form of 2.24.
    pub fn parse(text: &str) -> Result<SecretScope, NodePathError> {
        match Node::parse_path(text)? {
            Node::Environment(e) => Ok(SecretScope::Environment(e)),
            Node::Unit(e, u) => Ok(SecretScope::Unit(e, u)),
            _ => Err(NodePathError::Form),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip() {
        for text in ["environment:production", "environment:production/unit:relay", "environment:p/unit:r/host:host-a"]
        {
            assert_eq!(Node::parse_path(text).unwrap().path(), text);
        }
        for bad in [
            "",
            "unit:relay",
            "environment:p/host:a",
            "environment:P",
            "environment:p/unit:r/host:h/x:y",
            "environment",
        ] {
            assert!(Node::parse_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn chain_and_coverage() {
        let host = Node::parse_path("environment:p/unit:r/host:a").unwrap();
        let chain: Vec<String> = host.ancestors().iter().map(|n| n.node_type().name().to_string()).collect();
        assert_eq!(chain, vec!["service", "environment", "unit"]);
        let env = Node::parse_path("environment:p").unwrap();
        assert!(env.covers(&host));
        assert!(Node::Service.covers(&host));
        assert!(!host.covers(&env));
        let other = Node::parse_path("environment:q").unwrap();
        assert!(!other.covers(&host));
    }

    #[test]
    fn scopes() {
        let s = SecretScope::parse("environment:p/unit:r").unwrap();
        assert_eq!(s.folder(), "environments/p/units/r");
        assert!(SecretScope::parse("environment:p/unit:r/host:a").is_err());
    }
}
