// SPDX-License-Identifier: MIT OR Apache-2.0
//! The two ProcurAuth routes (5.19): the action document (3.11) and the options route (3.16).
//! In the mode `open`, both give `404 not_found`.

use http::StatusCode;
use pupitre_store::Store;

use crate::access::Access;
use crate::access::options::{OptionsError, options_route};
use crate::server::ctx::Ctx;
use crate::server::error::{ApiError, Code};
use crate::server::options::StateTree;
use crate::server::response::{Response, bytes_response, json_response};
use crate::server::state::ModeSettings;

/// `GET /.well-known/procurauth/actions`: the same bytes at each read, with no question.
pub fn actions<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let bytes = ctx.state.access.action_document().ok_or_else(ApiError::not_found)?;
    let mut r = bytes_response(StatusCode::OK, "application/json", bytes::Bytes::copy_from_slice(bytes));
    r.headers_mut().insert(http::header::CACHE_CONTROL, http::HeaderValue::from_static("no-store"));
    Ok(r)
}

/// `GET /.well-known/procurauth/resources`: the children of a node (3.16). It needs a valid
/// token and has no action of its own.
pub async fn resources<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let ModeSettings::Authzen { options_max_questions, options_page_items, .. } = &ctx.state.settings.mode else {
        return Err(ApiError::not_found());
    };
    let raw = ctx.raw_query.as_deref().unwrap_or("");
    let span = ctx.trace.child("access.decide").attr("action", "options");
    let result = options_route(
        &ctx.state.access,
        &ctx.subject,
        raw,
        &StateTree(&ctx.state),
        ctx.state.settings.runner_enabled(),
        *options_max_questions,
        *options_page_items,
    )
    .await;
    span.end(matches!(result, Err(OptionsError::PdpUnavailable)));
    match result {
        Ok(body) => Ok(json_response(StatusCode::OK, &body)),
        Err(OptionsError::InvalidRequest(_)) => Err(ApiError::new(Code::InvalidOptionsRequest)),
        Err(OptionsError::PdpUnavailable) => Err(ApiError::new(Code::PdpUnavailable)),
    }
}
