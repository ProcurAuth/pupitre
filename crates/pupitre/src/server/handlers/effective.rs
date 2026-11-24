// SPDX-License-Identifier: MIT OR Apache-2.0
//! `GET /api/environments/{e}/units/{u}/effective` (5.12): the layers 0 to 2 of a unit, or the
//! layers 0 to 3 of a unit on a host (2.13, 2.14).

use http::StatusCode;
use pupitre_core::doc::{DocKey, Document, Values};
use pupitre_core::error::FieldError;
use pupitre_core::ids::HostId;
use pupitre_core::json::Json;
use pupitre_core::layers::{LayerInputs, SecretLayers, resolve};
use pupitre_core::node::{Node, SecretScope};
use pupitre_store::Store;
use pupitre_store::catalog::{Catalog, EntryState};

use crate::access::{Access, CachePolicy};
use crate::server::ctx::Ctx;
use crate::server::error::{ApiError, Code};
use crate::server::response::{Response, json_response};

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::with_errors(Code::InvalidDocument, &[FieldError::new("", message)])
}

/// The values of one document of the catalog. An absent document gives `None`. A document that
/// fails its checks stops the view with its errors.
fn values_of(cat: &Catalog, key: &DocKey) -> Result<Option<Values>, ApiError> {
    match cat.get(key).map(|e| &e.state) {
        None => Ok(None),
        Some(EntryState::Invalid(errors)) => Err(ApiError::with_errors(Code::InvalidDocument, errors)),
        Some(EntryState::Valid(doc)) => Ok(doc.values().cloned()),
    }
}

/// The route of 5.12.
pub async fn effective<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let (action, _) = ctx.use_rule()?;
    // The node is the unit, or the host of the unit with the parameter `host` (3.12). A host
    // that is not in the unit is no node: the answer is `404` after the decision (3.14).
    let host_param = ctx.query.get("host");
    let ids = ctx.env().zip(ctx.unit());
    let node = ids.as_ref().and_then(|(e, u)| match host_param {
        None => Some(Node::Unit(e.clone(), u.clone())),
        Some(text) => HostId::parse(text).ok().map(|h| Node::Host(e.clone(), u.clone(), h)),
    });
    let node = node.filter(|n| ctx.state.catalog().node_exists(n));
    ctx.authorize(action, node.clone(), CachePolicy::MayUseKept).await?;
    let (env, unit_id) = ids.ok_or_else(ApiError::not_found)?;
    let host = match &node {
        Some(Node::Host(_, _, h)) => Some(h.clone()),
        _ => None,
    };
    let (unit, common, unit_values, host_values) = {
        let cat = ctx.state.catalog();
        let unit = match cat.get(&DocKey::Unit(env.clone(), unit_id.clone())).map(|e| &e.state) {
            Some(EntryState::Valid(Document::Unit(u))) => u.clone(),
            Some(EntryState::Invalid(errors)) => return Err(ApiError::with_errors(Code::InvalidDocument, errors)),
            _ => return Err(ApiError::not_found()),
        };
        let common = values_of(&cat, &DocKey::Common(env.clone()))?;
        let unit_values = values_of(&cat, &DocKey::UnitValues(env.clone(), unit_id.clone()))?;
        let host_values = match &host {
            Some(h) => values_of(&cat, &DocKey::HostValues(env.clone(), unit_id.clone(), h.clone()))?,
            None => None,
        };
        (unit, common, unit_values, host_values)
    };
    let Some(spec) = ctx.state.capabilities.schemas.get(&unit.schema) else {
        return Err(invalid(format!("the configuration has no schema {} for the unit", unit.schema)));
    };
    let mut secrets = SecretLayers::default();
    for (scope, map) in [
        (SecretScope::Environment(env.clone()), &mut secrets.environment),
        (SecretScope::Unit(env.clone(), unit_id.clone()), &mut secrets.unit),
    ] {
        let span = ctx.trace.child("store.list_secrets");
        let listed = ctx.state.store.list_secrets(&scope).await;
        span.end(listed.is_err());
        for meta in listed.map_err(|e| ctx.store_error("list_secrets", &e))? {
            map.insert(meta.name, meta.fingerprint);
        }
    }
    let resolution = resolve(&LayerInputs {
        schema: &spec.schema,
        common: common.as_ref(),
        unit: unit_values.as_ref(),
        host: host_values.as_ref(),
        secrets: &secrets,
    });
    let mut pairs = vec![
        ("environment", Json::str(env.as_str())),
        ("keys", resolution.to_json()),
        ("schema", Json::str(unit.schema.as_str())),
        ("unit", Json::str(unit_id.as_str())),
    ];
    if let Some(h) = &host {
        pairs.push(("host", Json::str(h.as_str())));
    }
    Ok(json_response(StatusCode::OK, &Json::object(pairs)))
}
