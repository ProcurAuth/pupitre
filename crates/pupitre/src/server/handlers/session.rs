// SPDX-License-Identifier: MIT OR Apache-2.0
//! `GET /api/session` (5.7) and `GET /api/catalog` (5.15). Both ask the question `access` on the
//! service (3.20): the pipeline asked it before the handler.

use http::StatusCode;
use pupitre_core::json::Json;
use pupitre_store::Store;

use crate::access::Access;
use crate::server::ctx::Ctx;
use crate::server::error::ApiError;
use crate::server::response::{Response, json_response};
use crate::server::state::ModeSettings;

/// The route of 5.7.
pub async fn session<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let settings = &ctx.state.settings;
    let allow_apply = match &settings.mode {
        ModeSettings::Open { allow_apply } => *allow_apply,
        ModeSettings::Authzen { .. } => true,
    };
    let runner = match &settings.runner {
        Some(_) => Json::object([("allow_apply", Json::Bool(allow_apply)), ("enabled", Json::Bool(true))]),
        None => Json::object([("allow_apply", Json::Bool(false)), ("enabled", Json::Bool(false))]),
    };
    // With `runner.enabled = false` no run exists, so the limit of the hosts is 0.
    let max_hosts = settings.runner.map_or(0, |r| u64::from(r.max_hosts));
    let body = Json::object([
        (
            "limits",
            Json::object([
                (
                    "max_document_bytes",
                    Json::Int(
                        u64::try_from(settings.limits.max_document_bytes)
                            .unwrap_or(u64::MAX)
                            .min(pupitre_core::json::MAX_SAFE_INTEGER),
                    ),
                ),
                ("max_hosts", Json::Int(max_hosts)),
                ("max_page_items", Json::Int(u64::from(settings.max_page_items))),
            ]),
        ),
        ("mode", Json::str(settings.mode_name())),
        ("runner", runner),
        ("subject", Json::str(ctx.subject.as_str())),
        ("version", Json::str(settings.version)),
    ]);
    Ok(json_response(StatusCode::OK, &body))
}

/// The route of 5.15. With `runner.enabled = false`, the answer has no commands and no playbooks.
pub async fn catalog<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let body = ctx.state.capabilities.catalog_json(ctx.state.settings.runner_enabled());
    Ok(json_response(StatusCode::OK, &body))
}
