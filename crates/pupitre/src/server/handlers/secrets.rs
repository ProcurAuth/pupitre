// SPDX-License-Identifier: MIT OR Apache-2.0
//! The routes of the secrets (5.14). No answer holds a value, and no log line holds one (7.9).

use http::StatusCode;
use pupitre_core::error::FieldError;
use pupitre_core::ids::KeyPath;
use pupitre_core::json::{Json, Members};
use pupitre_core::node::SecretScope;
use pupitre_core::validate::check_secret_value;
use pupitre_store::Store;
use pupitre_store::secret::SecretValue;
use pupitre_store::types::Message;

use crate::access::{Access, CachePolicy};
use crate::config::LogLevel;
use crate::server::ctx::Ctx;
use crate::server::error::{ApiError, Code};
use crate::server::response::{Response, json_response};
use crate::server::routes::SecretRoute;

fn scope_of<A: Access, S: Store>(ctx: &Ctx<A, S>, route: SecretRoute) -> Option<SecretScope> {
    match route {
        SecretRoute::Environment => Some(SecretScope::Environment(ctx.env()?)),
        SecretRoute::Unit => Some(SecretScope::Unit(ctx.env()?, ctx.unit()?)),
    }
}

fn invalid_secret(errors: &[FieldError]) -> ApiError {
    ApiError::with_errors(Code::InvalidSecret, errors)
}

/// `GET .../secrets`: the metadata, in the byte order of the name.
pub async fn list<A: Access, S: Store>(ctx: &Ctx<A, S>, route: SecretRoute) -> Result<Response, ApiError> {
    let (action, spec) = ctx.use_rule()?;
    ctx.authorize(action, ctx.node(spec), CachePolicy::MayUseKept).await?;
    let scope = scope_of(ctx, route).ok_or_else(ApiError::not_found)?;
    let span = ctx.trace.child("store.list_secrets");
    let listed = ctx.state.store.list_secrets(&scope).await;
    span.end(listed.is_err());
    let listed = listed.map_err(|e| ctx.store_error("list_secrets", &e))?;
    let items: Vec<Json> = listed
        .iter()
        .map(|m| {
            Json::object([
                ("fingerprint", Json::str(m.fingerprint.clone())),
                ("name", Json::str(m.name.as_str())),
                ("updated_at", Json::str(m.updated_at.to_string())),
                ("updated_by", Json::str(m.updated_by.as_str())),
            ])
        })
        .collect();
    Ok(json_response(StatusCode::OK, &Json::object([("items", Json::Arr(items))])))
}

fn log_write<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    scope: &SecretScope,
    name: &KeyPath,
    action: &str,
    fingerprint: Option<&str>,
) {
    let mut line = ctx
        .state
        .logger
        .event(LogLevel::Info, "secret.write")
        .request_id(&ctx.request_id)
        .text("scope", &scope.text())
        .text("name", name.as_str())
        .text("action", action)
        .text("subject", ctx.subject.as_str());
    if let Some(fp) = fingerprint {
        line = line.text("fingerprint", fp);
    }
    line.emit();
}

/// `PUT .../secrets/{name}`: a new value replaces the old one. There is no precondition.
pub async fn put<A: Access, S: Store>(ctx: &Ctx<A, S>, route: SecretRoute) -> Result<Response, ApiError> {
    let body = ctx.body_json()?;
    let mut members = Members::of(&body).map_err(|_| ApiError::bad_request())?;
    let value = match members.take("value") {
        Some(Json::Str(text)) => text,
        _ => return Err(ApiError::bad_request()),
    };
    if !members.finish().is_empty() {
        return Err(ApiError::bad_request());
    }
    let (action, spec) = ctx.use_rule()?;
    ctx.authorize(action, ctx.node(spec), CachePolicy::Fresh).await?;
    let scope = scope_of(ctx, route).ok_or_else(ApiError::not_found)?;
    let raw_name = ctx.params.get("name").unwrap_or_default();
    let name = KeyPath::parse(raw_name).map_err(|e| invalid_secret(&[FieldError::new("name", e.to_string())]))?;
    // The file of a secret has the name plus a suffix: the backend `files` allows 249 bytes.
    if name.as_str().len() > 249 {
        return Err(invalid_secret(&[FieldError::new("name", "the name has 249 bytes at most")]));
    }
    // The name `units` is the folder of the unit scopes in the layout of 2.31.
    if matches!(scope, SecretScope::Environment(_)) && name.as_str() == "units" {
        return Err(invalid_secret(&[FieldError::new(
            "name",
            "the name `units` is reserved in the scope of an environment",
        )]));
    }
    // The checks of 2.24 and 2.25. The messages never hold the value.
    let errors = match &scope {
        SecretScope::Environment(_) => check_secret_value(ctx.state.capabilities.schema_set(), &name, &value),
        SecretScope::Unit(env, unit_id) => {
            let schema_id = ctx.state.catalog().unit(env, unit_id).map(|u| u.schema.clone());
            match schema_id.and_then(|id| ctx.state.capabilities.schemas.get(&id)) {
                Some(spec) => check_secret_value([(spec.id.as_str(), &spec.schema)], &name, &value),
                None => vec![FieldError::new("", "the unit has no valid document with a schema of the configuration")],
            }
        }
    };
    if !errors.is_empty() {
        return Err(invalid_secret(&errors));
    }
    let meta = ctx.write_meta(Message::default());
    let span = ctx.trace.child("store.put_secret");
    let stored = ctx.state.store.put_secret(&scope, &name, &SecretValue::new(value.into_bytes()), &meta).await;
    span.end(stored.is_err());
    let stored = stored.map_err(|e| ctx.store_error("put_secret", &e))?;
    log_write(ctx, &scope, &name, "set", Some(&stored.fingerprint));
    Ok(json_response(StatusCode::OK, &Json::object([("fingerprint", Json::str(stored.fingerprint))])))
}

/// `DELETE .../secrets/{name}`: `404` when the secret does not exist.
pub async fn delete<A: Access, S: Store>(ctx: &Ctx<A, S>, route: SecretRoute) -> Result<Response, ApiError> {
    let (action, spec) = ctx.use_rule()?;
    ctx.authorize(action, ctx.node(spec), CachePolicy::Fresh).await?;
    let scope = scope_of(ctx, route).ok_or_else(ApiError::not_found)?;
    let name = KeyPath::parse(ctx.params.get("name").unwrap_or_default()).map_err(|_| ApiError::not_found())?;
    let meta = ctx.write_meta(Message::default());
    let span = ctx.trace.child("store.delete_secret");
    let result = ctx.state.store.delete_secret(&scope, &name, &meta).await;
    span.end(result.is_err());
    result.map_err(|e| ctx.store_error("delete_secret", &e))?;
    log_write(ctx, &scope, &name, "delete", None);
    Ok(json_response(StatusCode::OK, &Json::object::<[(&str, Json); 0], &str>([])))
}
