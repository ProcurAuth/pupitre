// SPDX-License-Identifier: MIT OR Apache-2.0
//! The options route (3.16): the adapters that give the catalog to `access::options::options_route`.

use pupitre_core::node::{Node, NodeType};
use pupitre_store::catalog::Catalog;

use crate::access::Access;
use crate::access::options::OptionsTree;

/// The tree of the catalog (3.7, 3.16).
pub struct CatalogTree<'a>(pub &'a Catalog);

/// The tree of the catalog of a server. Each call of `children` takes the lock of the catalog
/// and gives it back before it returns, so no lock stays held across an `await`.
pub struct StateTree<'a, A, S>(pub &'a super::state::AppState<A, S>);

impl<A: Access, S: pupitre_store::Store> OptionsTree for StateTree<'_, A, S> {
    fn children(&self, parent: Option<&Node>, child_type: NodeType) -> Vec<(String, String)> {
        CatalogTree(&self.0.catalog()).children(parent, child_type)
    }
}

impl OptionsTree for CatalogTree<'_> {
    fn children(&self, parent: Option<&Node>, child_type: NodeType) -> Vec<(String, String)> {
        let cat = self.0;
        match (parent, child_type) {
            (None, NodeType::Environment) => cat
                .environments()
                .into_iter()
                .map(|e| {
                    let label = match cat.document(&pupitre_core::doc::DocKey::Environment(e.clone())) {
                        Some(pupitre_core::doc::Document::Environment(d)) => d.label.clone(),
                        _ => e.as_str().to_string(),
                    };
                    (e.as_str().to_string(), label)
                })
                .collect(),
            (Some(Node::Environment(e)), NodeType::Unit) if cat.node_exists(&Node::Environment(e.clone())) => cat
                .units(e)
                .into_iter()
                .map(|u| {
                    let label = cat.unit(e, u).map_or_else(|| u.as_str().to_string(), |d| d.label.clone());
                    (u.as_str().to_string(), label)
                })
                .collect(),
            (Some(Node::Unit(e, u)), NodeType::Host) => match cat.unit(e, u) {
                Some(unit) => unit.hosts.iter().map(|h| (h.as_str().to_string(), h.as_str().to_string())).collect(),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}
