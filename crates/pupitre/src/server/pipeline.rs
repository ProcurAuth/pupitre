// SPDX-License-Identifier: MIT OR Apache-2.0
//! The steps of a request of the main listener, in the order of 3.13.
//!
//! 1. The limits of the request line, the headers and the declared body size (7.10).
//! 2. The write guard on each `POST`, `PUT` and `DELETE` (7.4).
//! 3. The token check, in the mode `authzen`, on each route except the action document (3.4).
//! 4. A feature that is off (3.3).
//! 5. The body read, within its limit (7.10).
//!
//! The handler does the steps 6 to 11 (see `handlers`).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use http::header::{CACHE_CONTROL, CONTENT_LENGTH, HeaderMap, HeaderValue, TRANSFER_ENCODING};
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::{Body as _, Incoming};
use pupitre_core::ids::Subject;
use pupitre_store::Store;
use tokio::sync::{OwnedSemaphorePermit, TryAcquireError};

use super::conn::ConnShared;
use super::ctx::{Ctx, refusal_error};
use super::error::{ApiError, Code};
use super::handlers::{self, pages_not_found};
use super::headers::{self, X_REQUEST_ID, new_request_id};
use super::limits::{MAX_HEADER_BYTES, MAX_HEADERS, MAX_TARGET_BYTES};
use super::query::Query;
use super::response::{Response, error_response};
use super::routes::{self, BodyKind, Matched, Region, RouteId, RouteSpec, Rule, rule_of};
use super::state::{AppState, ModeSettings};
use super::telemetry::Trace;
use crate::access::Access;
use crate::config::LogLevel;

/// What the steps give back: the label of the route, the answer and the subject for the log.
struct Handled {
    label: &'static str,
    result: Result<Response, ApiError>,
    subject: String,
}

impl Handled {
    fn error(label: &'static str, e: ApiError) -> Self {
        Handled { label, result: Err(e), subject: "-".to_string() }
    }
}

/// Step 1: the size of the request target and of the headers (7.10).
pub fn check_head_limits(uri: &http::Uri, headers: &HeaderMap) -> Result<(), ApiError> {
    let target = uri.path_and_query().map_or(uri.path().len(), |p| p.as_str().len());
    if target > MAX_TARGET_BYTES {
        return Err(ApiError::new(Code::UriTooLong));
    }
    let bytes: usize = headers.iter().map(|(n, v)| n.as_str().len() + v.as_bytes().len() + 4).sum();
    if headers.len() > MAX_HEADERS || bytes > MAX_HEADER_BYTES {
        return Err(ApiError::new(Code::HeadersTooLarge));
    }
    Ok(())
}

fn has_body(headers: &HeaderMap) -> bool {
    headers.contains_key(TRANSFER_ENCODING)
        || headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > 0)
}

fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers.get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok())
}

/// Read a body within the limit and the time of 7.10.
async fn read_body(mut body: Incoming, limit: u64, timeout: Duration) -> Result<Bytes, ApiError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = BytesMut::new();
    loop {
        let frame = match tokio::time::timeout_at(deadline, body.frame()).await {
            Err(_) => return Err(ApiError::new(Code::RequestTimeout).closing()),
            Ok(None) => break,
            Ok(Some(Err(_))) => return Err(ApiError::bad_request().closing()),
            Ok(Some(Ok(frame))) => frame,
        };
        if let Ok(data) = frame.into_data() {
            if buf.len() as u64 + data.len() as u64 > limit {
                return Err(ApiError::new(Code::PayloadTooLarge).closing());
            }
            buf.extend_from_slice(&data);
        }
    }
    Ok(buf.freeze())
}

/// The place of a body (7.10): a permit of `server.max_inflight_bodies`.
fn take_place<A, S>(state: &AppState<A, S>) -> Result<OwnedSemaphorePermit, ApiError> {
    match state.bodies.clone().try_acquire_owned() {
        Ok(p) => Ok(p),
        Err(TryAcquireError::NoPermits | TryAcquireError::Closed) => Err(ApiError::new(Code::ServerBusy)),
    }
}

fn fallback_route() -> &'static RouteSpec {
    routes::all()
        .iter()
        .find(|r| r.id == RouteId::Page { runs: false } && r.method == Method::GET)
        .unwrap_or(&routes::all()[0])
}

/// The token check of step 3. The values of the header come from the configuration.
fn authenticate<A: Access, S: Store>(state: &AppState<A, S>, headers: &HeaderMap) -> Result<Subject, ApiError> {
    let values: Vec<&[u8]> = match &state.settings.mode {
        ModeSettings::Open { .. } => Vec::new(),
        ModeSettings::Authzen { header, .. } => headers.get_all(header).iter().map(HeaderValue::as_bytes).collect(),
    };
    state.access.authenticate(&values).map_err(|r| refusal_error(&r))
}

/// The method for the log and the spans. A token that is not a standard method is `other`: a
/// client controls the length of the token, and it must not make a long line of the log.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        Method::PATCH => "PATCH",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => "other",
    }
}

/// The steps 1 to 5 of 3.13 for one request, then the handler.
async fn process<A: Access + 'static, S: Store + 'static>(
    state: &Arc<AppState<A, S>>,
    request_id: &str,
    trace: &Arc<Trace>,
    req: hyper::Request<Incoming>,
) -> Handled {
    let (parts, body) = req.into_parts();
    // Step 1: the limits of the head, the base path and the route.
    if let Err(e) = check_head_limits(&parts.uri, &parts.headers) {
        return Handled::error("unmatched", e);
    }
    // The header `Host` is in the list of the configuration (7.4). It comes before each route,
    // also a `GET`, because a page that a rebound name serves reads the answers.
    if let Err(e) = super::guard::check_host(&state.settings.allowed_hosts, &parts.uri, &parts.headers) {
        return Handled::error("unmatched", if has_body(&parts.headers) { e.closing() } else { e });
    }
    let Some(rel) = routes::strip_base(&state.settings.base_path, parts.uri.path()) else {
        return Handled::error("unmatched", ApiError::not_found());
    };
    let (spec, params) = match routes::route(&parts.method, rel) {
        Matched::MethodNotAllowed(allow) => return Handled::error("unmatched", ApiError::method_not_allowed(allow)),
        Matched::NotFound(region) => {
            let is_get = parts.method == Method::GET || parts.method == Method::HEAD;
            if region == Region::Page && is_get {
                return unknown_page(state, request_id, trace, &parts.headers).await;
            }
            return Handled::error("unmatched", ApiError::not_found());
        }
        Matched::Route(spec, params) => (spec, params),
    };
    let label: &'static str = spec.label.as_str();
    let fail = |e: ApiError, subject: &str| Handled { label, result: Err(e), subject: subject.to_string() };
    // The options route reads its own query string (3.16). A page gets the shell for any query
    // string: the page keeps its state in the address and a reload must work (5.21).
    let query = if spec.id == RouteId::WellKnownResources || matches!(spec.id, RouteId::Page { .. }) {
        Query::default()
    } else {
        match Query::parse(parts.uri.query(), spec.query) {
            Ok(q) => q,
            Err(e) => return fail(e, "-"),
        }
    };
    let body_present = has_body(&parts.headers);
    let max_body = state.settings.max_request_bytes;
    if spec.body == BodyKind::Json
        && let Some(n) = declared_length(&parts.headers)
        && n > max_body
    {
        return fail(ApiError::new(Code::PayloadTooLarge).closing(), "-");
    }
    // Step 2: the write guard (7.4).
    if matches!(parts.method, Method::POST | Method::PUT | Method::DELETE) {
        if let Err(e) = super::guard::cross_site_guard(&parts.headers) {
            return fail(e, "-");
        }
        if body_present && let Err(e) = super::guard::check_content_type(&parts.headers) {
            return fail(e, "-");
        }
    }
    // Step 3: the token (3.4).
    let rule = rule_of(spec.id);
    let mut subject = Subject::anonymous();
    if rule != Some(Rule::NoToken) {
        match authenticate(state, &parts.headers) {
            Ok(s) => subject = s,
            Err(e) => {
                let e = if body_present { e.closing() } else { e };
                return fail(e, "-");
            }
        }
    }
    let subject_text = subject.as_str().to_string();
    let Some(rule) = rule else {
        state
            .logger
            .event(LogLevel::Error, "access.no_rule")
            .request_id(request_id)
            .text("method", method_label(&parts.method))
            .text("route", label)
            .emit();
        return fail(ApiError::forbidden("none"), &subject_text);
    };
    // Step 4: a feature that is off gives `404` with no question (3.3).
    if !state.settings.runner_enabled() && routes::needs_runner(spec.id) {
        return fail(ApiError::not_found(), &subject_text);
    }
    // A write while the catalog is not ready (2.41): its state is empty or old, so the limits
    // and the rules across documents cannot hold. A write that cannot add a document stays
    // possible: a cancel, and a delete of a document or a secret of a catalog that is loaded.
    // So the operator can reduce the count, and the next poll makes the catalog ready again.
    if matches!(parts.method, Method::POST | Method::PUT | Method::DELETE) && spec.id != RouteId::RunCancel {
        let blocked = {
            let catalog = state.catalog();
            if catalog.is_ready() {
                None
            } else {
                let removes = matches!(spec.id, RouteId::DocDelete(_) | RouteId::SecretDelete(_));
                if removes && catalog.is_loaded() { None } else { Some(catalog.limit_error()) }
            }
        };
        if let Some(limit) = blocked {
            // The only reason for a catalog that is not ready is a limit (2.41).
            let e = match limit {
                Some(limit) => ApiError::store_over_limit(limit),
                None => ApiError::new(Code::StoreUnavailable),
            };
            return fail(if body_present { e.closing() } else { e }, &subject_text);
        }
    }
    // Step 5: the body (7.10).
    let mut bytes = Bytes::new();
    let mut _place = None;
    match spec.body {
        BodyKind::None => {
            if body_present {
                return fail(ApiError::bad_request().closing(), &subject_text);
            }
        }
        BodyKind::Json => {
            match take_place(state) {
                Ok(p) => _place = Some(p),
                Err(e) => return fail(e, &subject_text),
            }
            match read_body(body, max_body, state.settings.body_read_timeout).await {
                Ok(b) => bytes = b,
                Err(e) => return fail(e, &subject_text),
            }
        }
    }
    let ctx = Ctx {
        state: state.clone(),
        request_id: request_id.to_string(),
        subject,
        route: spec,
        params,
        query,
        raw_query: parts.uri.query().map(str::to_string),
        headers: parts.headers,
        body: bytes,
        trace: trace.clone(),
    };
    // A route with no action asks the question `access` first (3.20).
    if rule == Rule::Access
        && let Err(e) = ctx.authorize_access().await
    {
        return fail(e, &subject_text);
    }
    let result = handlers::dispatch(&ctx).await;
    Handled { label, result, subject: subject_text }
}

/// A path that no route names. A `GET` of a page gets the shell with the status 404. The token
/// and the question `access` come first (5.21, 3.20).
async fn unknown_page<A: Access + 'static, S: Store + 'static>(
    state: &Arc<AppState<A, S>>,
    request_id: &str,
    trace: &Arc<Trace>,
    headers: &HeaderMap,
) -> Handled {
    let subject = match authenticate(state, headers) {
        Ok(s) => s,
        Err(e) => return Handled::error("unmatched", e),
    };
    let subject_text = subject.as_str().to_string();
    let ctx = Ctx {
        state: state.clone(),
        request_id: request_id.to_string(),
        subject,
        route: fallback_route(),
        params: routes::Params::default(),
        query: Query::default(),
        raw_query: None,
        headers: HeaderMap::new(),
        body: Bytes::new(),
        trace: trace.clone(),
    };
    if let Err(e) = ctx.authorize_access().await {
        return Handled { label: "unmatched", result: Err(e), subject: subject_text };
    }
    Handled { label: "unmatched", result: Ok(pages_not_found(state)), subject: subject_text }
}

/// Add the headers that each answer of the main listener has (5.3, 7.5, 5.23).
fn finish(mut response: Response, request_id: &str, is_head: bool) -> (Response, u64) {
    let h = response.headers_mut();
    headers::add_security_headers(h);
    if let Ok(v) = HeaderValue::from_str(request_id) {
        h.insert(X_REQUEST_ID, v);
    }
    if !h.contains_key(CACHE_CONTROL) {
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    let len = response.body().size_hint().exact().unwrap_or(0);
    if is_head {
        // The same headers as the `GET`, with the length of its body, and no body (5.3).
        response.headers_mut().insert(CONTENT_LENGTH, HeaderValue::from(len));
        *response.body_mut() = Full::new(Bytes::new());
        return (response, 0);
    }
    (response, len)
}

/// The end of a request, whatever the way: the clock of the connection and the count of the
/// requests in flight (7.10).
struct RequestGuard<A: Access, S: Store> {
    state: Arc<AppState<A, S>>,
    conn: Arc<ConnShared>,
}

impl<A: Access, S: Store> Drop for RequestGuard<A, S> {
    fn drop(&mut self) {
        self.conn.request_finished();
        self.state.request_ended();
    }
}

/// Run one request of the main listener through the steps of 3.13 and give its answer.
async fn run_request<A: Access + 'static, S: Store + 'static>(
    state: Arc<AppState<A, S>>,
    guard: RequestGuard<A, S>,
    request_id: String,
    req: hyper::Request<Incoming>,
) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let trace = Arc::new(state.telemetry.start_trace("http.request"));
    let handled = process(&state, &request_id, &trace, req).await;
    let response = match handled.result {
        Ok(r) => r,
        Err(e) => error_response(&e),
    };
    let (response, _len) = finish(response, &request_id, method == Method::HEAD);
    let status = response.status().as_u16();
    let elapsed = started.elapsed();
    state.metrics.http_request(handled.label, status, elapsed);
    trace.attr("route", handled.label);
    trace.attr("method", method_label(&method));
    trace.attr("status", status.to_string());
    trace.end(status >= StatusCode::INTERNAL_SERVER_ERROR.as_u16());
    state
        .logger
        .event(LogLevel::Info, "http.request")
        .request_id(&request_id)
        .text("method", method_label(&method))
        .text("route", handled.label)
        .int("status", u64::from(status))
        .int("duration_ms", u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .text("subject", &handled.subject)
        .emit();
    drop(guard);
    response
}

/// Run one request of the main listener through the steps of 3.13. It never fails: each fault
/// is an answer.
///
/// The request runs in its own task. A client that closes the connection drops this future, but
/// not the task: the write, the catalog, the log and the metrics of the request stay complete
/// (7.10). The count of the requests in flight keeps the task inside `server.max_connections`.
pub async fn serve_request<A: Access + 'static, S: Store + 'static>(
    state: Arc<AppState<A, S>>,
    conn: Arc<ConnShared>,
    _peer: SocketAddr,
    req: hyper::Request<Incoming>,
) -> Result<Response, Infallible> {
    conn.request_started();
    state.request_begun();
    let guard = RequestGuard { state: state.clone(), conn };
    let request_id = new_request_id();
    let task = tokio::spawn(run_request(state.clone(), guard, request_id.clone(), req));
    match task.await {
        Ok(response) => Ok(response),
        Err(_) => {
            // The task of the request ended with no answer. The release profile has `panic = "abort"`
            // (5.4), so a panic never comes here: it stops the process. This branch runs when a build
            // unwinds (tests, the dev profile), and when the runtime cancels the task. The answer is `500`.
            state.logger.event(LogLevel::Error, "http.handler_failed").request_id(&request_id).emit();
            let (response, _len) = finish(error_response(&ApiError::new(Code::InternalError)), &request_id, false);
            Ok(response)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_of_the_head_7_10() {
        let uri: http::Uri = "/api/session".parse().unwrap();
        let mut h = HeaderMap::new();
        assert!(check_head_limits(&uri, &h).is_ok());
        let long = format!("/{}", "a".repeat(MAX_TARGET_BYTES));
        assert_eq!(check_head_limits(&long.parse().unwrap(), &HeaderMap::new()).unwrap_err().code, Code::UriTooLong);
        let ok = format!("/{}", "a".repeat(MAX_TARGET_BYTES - 1));
        assert!(check_head_limits(&ok.parse().unwrap(), &HeaderMap::new()).is_ok());
        for i in 0..MAX_HEADERS {
            h.insert(
                http::HeaderName::from_bytes(format!("x-h{i}").as_bytes()).unwrap(),
                HeaderValue::from_static("v"),
            );
        }
        assert!(check_head_limits(&uri, &h).is_ok());
        h.insert(http::HeaderName::from_static("x-extra"), HeaderValue::from_static("v"));
        assert_eq!(check_head_limits(&uri, &h).unwrap_err().code, Code::HeadersTooLarge);
        let mut big = HeaderMap::new();
        big.insert(
            http::HeaderName::from_static("x-big"),
            HeaderValue::from_str(&"v".repeat(MAX_HEADER_BYTES)).unwrap(),
        );
        assert_eq!(check_head_limits(&uri, &big).unwrap_err().code, Code::HeadersTooLarge);
    }
}
