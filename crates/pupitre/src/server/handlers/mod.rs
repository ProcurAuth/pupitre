// SPDX-License-Identifier: MIT OR Apache-2.0
//! The handlers of the routes of section 5. The pipeline does the steps 1 to 5 of 3.13 and the
//! question `access` of 3.20. A handler does the steps 6 to 11. They are the shape of the request,
//! the catalog lookup, the decision of the use, the ceilings, the validation and the effect.

mod documents;
mod effective;
mod pages;
mod procurauth;
mod runs;
mod secrets;
mod session;

use pupitre_core::action::Action;
use pupitre_core::node::{Node, NodeType};
use pupitre_store::Store;

use super::ctx::{Ctx, refusal_error};
use super::error::ApiError;
use super::query::Cursor;
use super::response::Response;
use super::routes::{NodeSpec, RouteId, Rule, rule_of};
use crate::access::{Access, MatchMode};

impl<A: Access, S: Store> Ctx<A, S> {
    /// The node of a route variable form, when the path names a node of the catalog. A path
    /// that names no node gives `None` (3.14).
    pub fn node(&self, spec: NodeSpec) -> Option<Node> {
        let node = match spec {
            NodeSpec::Service => Node::Service,
            NodeSpec::Environment => Node::Environment(self.env()?),
            NodeSpec::Unit => Node::Unit(self.env()?, self.unit()?),
            NodeSpec::Host => Node::Host(self.env()?, self.unit()?, self.host()?),
        };
        self.state.catalog().node_exists(&node).then_some(node)
    }

    /// The action and the node form of a route with the rule `Use`.
    pub fn use_rule(&self) -> Result<(Action, NodeSpec), ApiError> {
        match rule_of(self.route.id) {
            Some(Rule::Use(action, node)) => Ok((action, node)),
            _ => Err(self.internal("a route has no rule of use")),
        }
    }

    /// The limit of a list page: `server.max_page_items` when absent (5.6).
    pub fn page_limit(&self) -> Result<usize, ApiError> {
        let max = u64::from(self.state.settings.max_page_items);
        let n = self.query.integer("limit", 1, max)?.unwrap_or(max);
        Ok(usize::try_from(n).unwrap_or(usize::MAX))
    }
}

/// The actions that the display filter asks for a type (3.15): the published actions whose
/// `resource_types` hold the type, in the order of 3.9.
pub fn display_actions(runner_enabled: bool, ty: NodeType) -> Vec<Action> {
    Action::published(runner_enabled).into_iter().filter(|a| a.resource_types().contains(&ty)).collect()
}

/// One item of a list that the display filter reads.
pub struct Candidate {
    /// The id of the item. The list is in the byte order of the ids.
    pub id: String,
    /// The node of the item.
    pub node: Node,
}

/// The page of a list with the display filter (3.15, 5.6). It gives the indexes of the visible
/// candidates. It gives the cursor of the next page when the list continues.
pub async fn display_page<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    tag: &'static str,
    candidates: &[Candidate],
    ty: NodeType,
    limit: usize,
) -> Result<(Vec<usize>, Option<String>), ApiError> {
    let cursor = match ctx.query.get("after") {
        Some(text) => Cursor::decode(tag, text)?,
        None => Cursor::default(),
    };
    let start = match &cursor.last {
        Some(id) => candidates.partition_point(|c| c.id.as_str() <= id.as_str()),
        None => 0,
    };
    let start = start.saturating_add(cursor.skip as usize).min(candidates.len());
    let window = &candidates[start..candidates.len().min(start.saturating_add(limit))];
    let actions = display_actions(ctx.state.settings.runner_enabled(), ty);
    let items: Vec<(Node, Vec<Action>, MatchMode)> =
        window.iter().map(|c| (c.node.clone(), actions.clone(), MatchMode::Subtree)).collect();
    let budget = ctx.state.settings.question_budget();
    let span = ctx.trace.child("access.decide").attr("action", "display");
    let result = ctx.state.access.visible(&ctx.subject, &items, budget).await;
    span.end(result.is_err());
    let visibility = match result {
        Ok(v) => v,
        Err(r) => {
            ctx.state.metrics.pdp_request(if matches!(r, crate::access::Refusal::PdpUnavailable) {
                "error"
            } else {
                "deny"
            });
            return Err(refusal_error(&r));
        }
    };
    let examined = visibility.stopped_at.unwrap_or(window.len()).min(window.len());
    let visible: Vec<usize> = visibility.visible.iter().copied().filter(|i| *i < examined).map(|i| start + i).collect();
    let more = start + examined < candidates.len();
    let next = more.then(|| {
        // The cursor holds the last visible id and the hidden items after it (3.16). It never
        // holds the id of an item that the subject cannot see.
        let next_cursor = match visible.last() {
            Some(i) => Cursor {
                last: Some(candidates[*i].id.clone()),
                skip: u32::try_from(start + examined - (*i + 1)).unwrap_or(u32::MAX),
            },
            None => Cursor {
                last: cursor.last.clone(),
                skip: cursor.skip.saturating_add(u32::try_from(examined).unwrap_or(u32::MAX)),
            },
        };
        next_cursor.encode(tag)
    });
    Ok((visible, next))
}

/// Serve the route of a request.
pub async fn dispatch<A: Access + 'static, S: Store + 'static>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    match ctx.route.id {
        RouteId::Session => session::session(ctx).await,
        RouteId::Catalog => session::catalog(ctx).await,
        RouteId::EnvironmentList => documents::environment_list(ctx).await,
        RouteId::EnvironmentCreate => documents::environment_create(ctx).await,
        RouteId::HostList => documents::host_list(ctx).await,
        RouteId::UnitList => documents::unit_list(ctx).await,
        RouteId::DocGet(kind) => documents::get(ctx, kind).await,
        RouteId::DocPut(kind) => documents::put(ctx, kind).await,
        RouteId::DocDelete(kind) => documents::delete(ctx, kind).await,
        RouteId::DocHistory(kind) => documents::history(ctx, kind).await,
        RouteId::DocVersion(kind) => documents::version(ctx, kind).await,
        RouteId::Effective => effective::effective(ctx).await,
        RouteId::SecretList(scope) => secrets::list(ctx, scope).await,
        RouteId::SecretPut(scope) => secrets::put(ctx, scope).await,
        RouteId::SecretDelete(scope) => secrets::delete(ctx, scope).await,
        RouteId::RunCreate => runs::create(ctx).await,
        RouteId::RunList => runs::list(ctx).await,
        RouteId::RunGet => runs::get(ctx).await,
        RouteId::RunEvents => runs::events(ctx).await,
        RouteId::RunOutput => runs::output(ctx).await,
        RouteId::RunCancel => runs::cancel(ctx).await,
        RouteId::WellKnownActions => procurauth::actions(ctx),
        RouteId::WellKnownResources => procurauth::resources(ctx).await,
        RouteId::Page { .. } => pages::page(ctx),
        RouteId::Static => pages::static_file(ctx),
    }
}

/// The shell with the status 404 for a path that no page names (5.21).
pub fn pages_not_found<A, S>(state: &super::state::AppState<A, S>) -> Response {
    pages::not_found_shell(state)
}
