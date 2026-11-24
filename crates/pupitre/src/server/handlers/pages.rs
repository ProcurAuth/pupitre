// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shell of the pages and the static files (5.21, 5.23). The server serves the files of
//! `web_assets::ASSETS` and the shell.

use bytes::Bytes;
use http::StatusCode;
use http::header::{CACHE_CONTROL, HeaderValue};
use pupitre_store::Store;

use crate::access::Access;
use crate::server::ctx::Ctx;
use crate::server::error::ApiError;
use crate::server::response::{Response, bytes_response};
use crate::server::state::AppState;

/// The value of `Cache-Control` of a file below `/static/<build>/` (5.23).
pub const STATIC_CACHE: &str = "private, max-age=31536000, immutable";

fn shell(status: StatusCode, body: Bytes) -> Response {
    let mut r = bytes_response(status, "text/html; charset=utf-8", body);
    r.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

/// The shell of a page (5.23). It holds no data.
pub fn page<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    Ok(shell(StatusCode::OK, ctx.state.assets.shell.clone()))
}

/// The shell with the status 404, for a path that no page names (5.21). The page then shows
/// the message and the link to `/`.
pub fn not_found_shell<A, S>(state: &AppState<A, S>) -> Response {
    shell(StatusCode::NOT_FOUND, state.assets.shell.clone())
}

/// A file below `/static/<build>/` (5.23). Another build id gives `404 not_found`.
pub fn static_file<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let build = ctx.params.get("build").unwrap_or_default();
    let file = ctx.params.get("file").unwrap_or_default();
    if build != ctx.state.assets.build {
        return Err(ApiError::not_found());
    }
    let asset = ctx.state.assets.file(file).ok_or_else(ApiError::not_found)?;
    let mut r = bytes_response(StatusCode::OK, asset.content_type, Bytes::from_static(asset.bytes));
    r.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static(STATIC_CACHE));
    Ok(r)
}
