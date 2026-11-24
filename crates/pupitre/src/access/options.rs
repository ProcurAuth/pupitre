// SPDX-License-Identifier: MIT OR Apache-2.0
//! The options route (3.16, P 5.6 to P 5.8) as a function.
//!
//! `options_route` reads the query string, walks the children that the catalog gives, asks the
//! display questions of 3.15 and builds the answer. The server implements `OptionsTree`
//! over the catalog and maps `OptionsError` to `400 invalid_options_request` or
//! `503 pdp_unavailable`.
//!
//! The cursor is the base64url text, with no padding, of `<type>` U+000A `<parent>` U+000A
//! `<last shown id>` U+000A `<count of children after it that the walk examined and did not
//! show>`. It never holds the id of a child that the subject cannot see.

use procurauth_core::names::decode_id;
use pupitre_core::action::Action;
use pupitre_core::ids::{EnvId, HostId, Subject, UnitId};
use pupitre_core::json::Json;
use pupitre_core::node::{Node, NodeType};

use super::keys::{base64url, base64url_decode};
use super::{Access, MatchMode};

/// The longest search text, in bytes (3.16).
pub const MAX_Q_BYTES: usize = 256;
/// The longest cursor, in bytes (3.16).
pub const MAX_CURSOR_BYTES: usize = 2048;
/// The most digits in the count of a cursor.
const MAX_COUNT_DIGITS: usize = 9;

/// The children of a node, as the catalog holds them (3.16).
pub trait OptionsTree {
    /// The children of `parent` that have the type `child_type`, each with its id and its label.
    /// `parent` is `None` for the service. A parent that does not exist has no child. The order
    /// is free: the route sorts the children by the byte order of the id.
    fn children(&self, parent: Option<&Node>, child_type: NodeType) -> Vec<(String, String)>;
}

/// Why the route gives no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionsError {
    /// The query is not valid: `400 invalid_options_request`. The text says which rule it breaks.
    InvalidRequest(String),
    /// The PDP gave no valid answer: `503 pdp_unavailable`. The route never gives an empty list
    /// for a failure (3.16).
    PdpUnavailable,
}

impl std::fmt::Display for OptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptionsError::InvalidRequest(text) => f.write_str(text),
            OptionsError::PdpUnavailable => f.write_str("pdp_unavailable"),
        }
    }
}

impl std::error::Error for OptionsError {}

fn bad(text: impl Into<String>) -> OptionsError {
    OptionsError::InvalidRequest(text.into())
}

/// The query of a request, after the checks of P 5.7 and 3.16.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionsQuery {
    /// The node whose children the route lists, or `None` for the service.
    pub parent: Option<Node>,
    /// The type of the children.
    pub child_type: NodeType,
    /// The search text.
    pub q: Option<String>,
    /// The most items of the page.
    pub limit: u32,
    /// The id after which the page starts, and the count of the examined children to skip.
    pub after: Option<(String, u64)>,
}

/// The type of the children of a node: the type below it in the tree (3.16).
fn child_type_of(parent: Option<&Node>) -> Option<NodeType> {
    match parent {
        None => Some(NodeType::Environment),
        Some(Node::Environment(_)) => Some(NodeType::Unit),
        Some(Node::Unit(..)) => Some(NodeType::Host),
        Some(Node::Host(..) | Node::Service) => None,
    }
}

fn type_of_name(name: &str) -> Option<NodeType> {
    match name {
        "environment" => Some(NodeType::Environment),
        "unit" => Some(NodeType::Unit),
        "host" => Some(NodeType::Host),
        _ => None,
    }
}

fn valid_id(child_type: NodeType, id: &str) -> bool {
    match child_type {
        NodeType::Environment => EnvId::parse(id).is_ok(),
        NodeType::Unit => UnitId::parse(id).is_ok(),
        NodeType::Host => HostId::parse(id).is_ok(),
        NodeType::Service => false,
    }
}

/// The child node of a parent, or `None` when the id breaks the rule of its type.
fn child_node(parent: Option<&Node>, child_type: NodeType, id: &str) -> Option<Node> {
    match (parent, child_type) {
        (None, NodeType::Environment) => EnvId::parse(id).ok().map(Node::Environment),
        (Some(Node::Environment(e)), NodeType::Unit) => UnitId::parse(id).ok().map(|u| Node::Unit(e.clone(), u)),
        (Some(Node::Unit(e, u)), NodeType::Host) => HostId::parse(id).ok().map(|h| Node::Host(e.clone(), u.clone(), h)),
        _ => None,
    }
}

/// The cursor of 3.16.
pub fn make_cursor(child_type: NodeType, parent_text: &str, last_visible: Option<&str>, hidden_after: u64) -> String {
    base64url(
        format!("{}\n{parent_text}\n{}\n{hidden_after}", child_type.name(), last_visible.unwrap_or("")).as_bytes(),
    )
}

/// Read the cursor. It must be a cursor that this route made for the same type and parent.
fn read_cursor(text: &str, child_type: NodeType, parent_text: &str) -> Result<(String, u64), OptionsError> {
    let own = || -> Option<(String, u64)> {
        let decoded = String::from_utf8(base64url_decode(text)?).ok()?;
        let mut parts = decoded.splitn(4, '\n');
        let (t, p, id, count) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let canonical = !count.is_empty()
            && count.len() <= MAX_COUNT_DIGITS
            && count.bytes().all(|b| b.is_ascii_digit())
            && (count == "0" || !count.starts_with('0'));
        if t != child_type.name() || p != parent_text || !canonical {
            return None;
        }
        if !id.is_empty() && !valid_id(child_type, id) {
            return None;
        }
        Some((id.to_owned(), count.parse().ok()?))
    };
    own().ok_or_else(|| bad("the parameter `after` is not a cursor of this route for this type and this parent"))
}

/// Decode the value of a parameter one time (P 5.7). A `+` stays a `+`.
fn decode(name: &str, value: &str) -> Result<String, OptionsError> {
    decode_id(value).map_err(|e| bad(format!("the parameter `{name}` has a percent-encoding that is not valid: {e}")))
}

/// The parent node of a `parent` value: the path form of 3.8, with each id decoded one more
/// time (P 5.7).
fn read_parent(text: &str) -> Result<Node, OptionsError> {
    let mut plain = Vec::new();
    for segment in text.split('/') {
        let (kind, id) = segment.split_once(':').ok_or_else(|| bad("the parameter `parent` is not a node path"))?;
        plain.push(format!("{kind}:{}", decode("parent", id)?));
    }
    Node::parse_path(&plain.join("/")).map_err(|e| bad(format!("the parameter `parent` is not a node path: {e}")))
}

/// Read and check the raw query string of the route, without the `?` (P 5.7, 3.16).
pub fn parse_query(raw: &str, page_items: u32) -> Result<OptionsQuery, OptionsError> {
    const NAMES: [&str; 5] = ["parent", "type", "q", "limit", "after"];
    let mut values: [Option<&str>; 5] = [None; 5];
    for pair in raw.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        // An unknown parameter has no effect (P 5.7).
        if let Some(index) = NAMES.iter().position(|n| *n == name) {
            if values[index].is_some() {
                return Err(bad(format!("the parameter `{name}` occurs more than one time")));
            }
            values[index] = Some(value);
        }
    }
    let one = |index: usize| values[index].map(|v| decode(NAMES[index], v)).transpose();
    let parent_text = one(0)?;
    let type_text = one(1)?.ok_or_else(|| bad("the parameter `type` is absent"))?;
    let q = one(2)?;
    let after_text = one(4)?;
    let parent = parent_text.as_deref().map(read_parent).transpose()?;
    let child_type = type_of_name(&type_text).ok_or_else(|| bad("the parameter `type` is not a type of the tree"))?;
    if child_type_of(parent.as_ref()) != Some(child_type) {
        return Err(bad("the parameter `type` is not the type of the children of `parent`"));
    }
    if q.as_ref().is_some_and(|q| q.len() > MAX_Q_BYTES) {
        return Err(bad(format!("the parameter `q` has more than {MAX_Q_BYTES} bytes")));
    }
    let limit = match values[3] {
        None => page_items,
        Some(text) => {
            let digits = !text.is_empty() && text.len() <= MAX_COUNT_DIGITS && text.bytes().all(|b| b.is_ascii_digit());
            match text.parse::<u32>() {
                Ok(n) if digits && (1..=page_items).contains(&n) => n,
                _ => {
                    return Err(bad(format!("the parameter `limit` is not a whole number from 1 to {page_items}")));
                }
            }
        }
    };
    let parent_path = parent.as_ref().map(Node::path).unwrap_or_default();
    let after = match after_text {
        None => None,
        Some(text) => {
            if text.len() > MAX_CURSOR_BYTES {
                return Err(bad(format!("the parameter `after` has more than {MAX_CURSOR_BYTES} bytes")));
            }
            Some(read_cursor(&text, child_type, &parent_path)?)
        }
    };
    Ok(OptionsQuery { parent, child_type, q, limit, after })
}

fn fold(text: &str) -> String {
    text.to_ascii_lowercase()
}

/// Build the answer of the route (3.16): `{"items": [...], "next": null or text}`.
///
/// `runner_enabled` tells whether the run actions are published. `max_questions` is `access.options_max_questions` and `page_items` is
/// `access.options_page_items`. The route never changes a value of the query to make it fit.
pub async fn options_route<A: Access>(
    access: &A,
    subject: &Subject,
    raw_query: &str,
    tree: &impl OptionsTree,
    runner_enabled: bool,
    max_questions: u32,
    page_items: u32,
) -> Result<Json, OptionsError> {
    let query = parse_query(raw_query, page_items)?;
    let parent_path = query.parent.as_ref().map(Node::path).unwrap_or_default();
    let (after_id, after_skip) = match &query.after {
        Some((id, count)) => ((!id.is_empty()).then_some(id.as_str()), *count),
        None => (None, 0),
    };
    // The children in the byte order of the id, with the cursor and the skip. The search does
    // not filter here: it runs on the visible children, so the questions to the PDP never
    // depend on the names that the subject cannot see (3.16).
    let mut children = tree.children(query.parent.as_ref(), query.child_type);
    children.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    children.dedup_by(|a, b| a.0 == b.0);
    let needle = query.q.as_deref().map(fold);
    let found =
        |id: &str, label: &str| needle.as_deref().is_none_or(|n| fold(id).contains(n) || fold(label).contains(n));
    let candidates: Vec<(Node, String, String)> = children
        .into_iter()
        .filter(|(id, _)| after_id.is_none_or(|a| id.as_bytes() > a.as_bytes()))
        .filter_map(|(id, label)| child_node(query.parent.as_ref(), query.child_type, &id).map(|n| (n, id, label)))
        .skip(usize::try_from(after_skip).unwrap_or(usize::MAX))
        .collect();
    // Only the actions that this server publishes (5.2a): the run actions need the runner node.
    let actions: Vec<Action> = Action::published(runner_enabled)
        .into_iter()
        .filter(|a| a.resource_types().contains(&query.child_type))
        .collect();
    let limit = query.limit as usize;
    let mut remaining = max_questions;
    let mut shown: Vec<usize> = Vec::new();
    let mut index = 0usize;
    while shown.len() < limit && index < candidates.len() && remaining > 0 {
        // A window never holds more items than the page still needs.
        let take = (limit - shown.len()).min(candidates.len() - index);
        let window: Vec<(Node, Vec<Action>, MatchMode)> = candidates[index..index + take]
            .iter()
            .map(|(n, _, _)| (n.clone(), actions.clone(), MatchMode::Subtree))
            .collect();
        let (visibility, asked) =
            access.visible_counted(subject, &window, remaining).await.map_err(|_| OptionsError::PdpUnavailable)?;
        remaining = remaining.saturating_sub(asked);
        let processed = visibility.stopped_at.unwrap_or(take);
        shown.extend(
            visibility
                .visible
                .iter()
                .filter(|i| **i < processed)
                .map(|i| index + i)
                .filter(|&i| found(&candidates[i].1, &candidates[i].2)),
        );
        index += processed;
        if visibility.stopped_at.is_some() {
            break;
        }
    }
    // A walk that could not end one item has no progress: a cursor would repeat the request.
    // `AuthzenAccess::new` refuses a budget that is too small, so this is a fault of the caller.
    if index == 0 && !candidates.is_empty() {
        return Err(OptionsError::PdpUnavailable);
    }
    let has_children = !matches!(query.child_type, NodeType::Host);
    let items: Vec<Json> = shown
        .iter()
        .map(|&i| {
            let (_, id, label) = &candidates[i];
            Json::object([
                ("has_children", Json::Bool(has_children)),
                ("id", Json::str(id.as_str())),
                ("label", Json::str(label.as_str())),
            ])
        })
        .collect();
    // The cursor: the last shown id and the count of the children that the walk examined and did
    // not show after it (hidden, or visible and not in the search). With no visible item, the position of the request moves by what the walk
    // examined. At the end of the children there is no cursor.
    let next = if index >= candidates.len() {
        Json::Null
    } else if let Some(&last) = shown.last() {
        let skipped = (index - (last + 1)) as u64;
        Json::str(make_cursor(query.child_type, &parent_path, Some(candidates[last].1.as_str()), skipped))
    } else {
        Json::str(make_cursor(query.child_type, &parent_path, after_id, after_skip + index as u64))
    };
    Ok(Json::object([("items", Json::Arr(items)), ("next", next)]))
}
