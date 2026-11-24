// SPDX-License-Identifier: MIT OR Apache-2.0
//! The response type and its builders (5.3, 5.23).

use bytes::Bytes;
use http::header::{ALLOW, CACHE_CONTROL, CONNECTION, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use http::{Response as HttpResponse, StatusCode};
use http_body_util::Full;
use pupitre_core::json::{self, Json};

use super::error::ApiError;

/// The response of a handler: the whole body is in memory.
pub type Response = HttpResponse<Full<Bytes>>;

/// A response with a status, a content type and a body.
pub fn bytes_response(status: StatusCode, content_type: &'static str, body: Bytes) -> Response {
    let mut r = HttpResponse::new(Full::new(body));
    *r.status_mut() = status;
    r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    r
}

/// A JSON response of the API (5.3): compact, UTF-8, `Cache-Control: no-store`.
pub fn json_response(status: StatusCode, value: &Json) -> Response {
    json_bytes(status, Bytes::from(json::to_compact(value)))
}

/// A JSON response with the bytes of the body.
pub fn json_bytes(status: StatusCode, body: Bytes) -> Response {
    let mut r = bytes_response(status, "application/json; charset=utf-8", body);
    r.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// The answer of an error (5.4), with the headers that its code needs.
pub fn error_response(e: &ApiError) -> Response {
    let mut r = json_bytes(e.code.status(), e.body());
    let h = r.headers_mut();
    if let Some(allow) = &e.allow
        && let Ok(v) = HeaderValue::from_str(allow)
    {
        h.insert(ALLOW, v);
    }
    if let Some(secs) = e.code.retry_after() {
        h.insert(RETRY_AFTER, HeaderValue::from(secs));
    }
    if e.close {
        h.insert(CONNECTION, HeaderValue::from_static("close"));
    }
    r
}

/// Set a header with a value that is text.
pub fn set_header(r: &mut Response, name: http::HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        r.headers_mut().insert(name, v);
    }
}
