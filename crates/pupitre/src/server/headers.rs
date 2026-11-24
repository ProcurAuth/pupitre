// SPDX-License-Identifier: MIT OR Apache-2.0
//! The headers: the request id (5.3), the security headers (7.5), the entity tags and the
//! preconditions (5.5).

use http::header::{HeaderMap, HeaderName, HeaderValue};
use pupitre_core::etag::Etag;

use super::error::{ApiError, Code};

/// The policy of 7.5. It has no `'unsafe-inline'`: the HTML has no inline script and no `style`
/// attribute.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                       font-src 'self'; connect-src 'self'; base-uri 'self'; form-action 'self'; \
                       frame-ancestors 'none'";

/// The value of `Permissions-Policy` (7.5).
pub const PERMISSIONS_POLICY: &str = "camera=(), microphone=(), geolocation=(), payment=(), usb=()";

/// The header of the request id (5.3, 7.5).
pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
/// The header of the fetch metadata (7.4).
pub const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");
/// The header of the output length (5.17).
pub const X_OUTPUT_LENGTH: HeaderName = HeaderName::from_static("x-pupitre-output-length");
/// The header of the completion of a run (5.17).
pub const X_RUN_COMPLETE: HeaderName = HeaderName::from_static("x-pupitre-run-complete");

/// Make a request id: 32 lower-case hexadecimal characters from the random source of the
/// system. Pupitre gives the id (5.3). It never reads one from the request, so a client cannot
/// write its own text into the log.
pub fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // The random source failed. A time-based value is still unique enough for a log id.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        bytes = nanos.to_le_bytes();
    }
    pupitre_core::etag::to_hex(&bytes)
}

/// Add the headers of 7.5 that are the same for each answer.
pub fn add_security_headers(h: &mut HeaderMap) {
    h.insert(HeaderName::from_static("content-security-policy"), HeaderValue::from_static(CSP));
    h.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    h.insert(HeaderName::from_static("x-frame-options"), HeaderValue::from_static("DENY"));
    h.insert(HeaderName::from_static("referrer-policy"), HeaderValue::from_static("same-origin"));
    h.insert(HeaderName::from_static("cross-origin-resource-policy"), HeaderValue::from_static("same-origin"));
    h.insert(HeaderName::from_static("cross-origin-opener-policy"), HeaderValue::from_static("same-origin"));
    h.insert(HeaderName::from_static("permissions-policy"), HeaderValue::from_static(PERMISSIONS_POLICY));
}

/// The value of the header `ETag` (2.6): the hexadecimal text in double quotes.
pub fn etag_value(etag: &Etag) -> HeaderValue {
    HeaderValue::from_str(&etag.to_http()).unwrap_or_else(|_| HeaderValue::from_static("\"\""))
}

/// Read one header that occurs at most one time. A repeated header, or a value that is not
/// text, gives `400 bad_request`.
fn single(headers: &HeaderMap, name: &HeaderName) -> Result<Option<String>, ApiError> {
    let mut all = headers.get_all(name).iter();
    let Some(first) = all.next() else { return Ok(None) };
    if all.next().is_some() {
        return Err(ApiError::bad_request());
    }
    first.to_str().map(|s| Some(s.to_string())).map_err(|_| ApiError::bad_request())
}

/// The precondition of a `PUT` (5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutPrecondition {
    /// `If-None-Match: *`
    Create,
    /// `If-Match: "<etag>"`
    Match(Etag),
}

/// Read the one precondition header of a `PUT` (5.5). No header gives `428`. Two headers, a
/// repeated header or a value of another form give `400`.
pub fn put_precondition(headers: &HeaderMap) -> Result<PutPrecondition, ApiError> {
    let if_match = single(headers, &HeaderName::from_static("if-match"))?;
    let if_none = single(headers, &HeaderName::from_static("if-none-match"))?;
    match (if_match, if_none) {
        (None, None) => Err(ApiError::new(Code::PreconditionRequired)),
        (Some(_), Some(_)) => Err(ApiError::bad_request()),
        (Some(v), None) => Etag::parse_http(&v).map(PutPrecondition::Match).map_err(|_| ApiError::bad_request()),
        (None, Some(v)) if v == "*" => Ok(PutPrecondition::Create),
        (None, Some(_)) => Err(ApiError::bad_request()),
    }
}

/// Read the `If-Match` header of a `DELETE` (5.5). No header gives `428`. `If-None-Match` gives
/// `400`.
pub fn delete_precondition(headers: &HeaderMap) -> Result<Etag, ApiError> {
    if headers.contains_key(HeaderName::from_static("if-none-match")) {
        return Err(ApiError::bad_request());
    }
    match single(headers, &HeaderName::from_static("if-match"))? {
        None => Err(ApiError::new(Code::PreconditionRequired)),
        Some(v) => Etag::parse_http(&v).map_err(|_| ApiError::bad_request()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(HeaderName::from_static(k), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn etag() -> String {
        Etag::of(b"x").to_http()
    }

    #[test]
    fn a_put_needs_exactly_one_precondition_5_5() {
        assert_eq!(put_precondition(&map(&[])).unwrap_err().code, Code::PreconditionRequired);
        assert_eq!(put_precondition(&map(&[("if-none-match", "*")])), Ok(PutPrecondition::Create));
        assert_eq!(put_precondition(&map(&[("if-match", &etag())])), Ok(PutPrecondition::Match(Etag::of(b"x"))));
        for bad in [
            map(&[("if-match", &etag()), ("if-none-match", "*")]),
            map(&[("if-match", "*")]),
            map(&[("if-match", &format!("W/{}", etag()))]),
            map(&[("if-match", &etag()), ("if-match", &etag())]),
            map(&[("if-none-match", &etag())]),
            map(&[("if-match", "\"ABC\"")]),
        ] {
            assert_eq!(put_precondition(&bad).unwrap_err().code, Code::BadRequest, "{bad:?}");
        }
    }

    #[test]
    fn a_delete_needs_if_match_5_5() {
        assert_eq!(delete_precondition(&map(&[])).unwrap_err().code, Code::PreconditionRequired);
        assert_eq!(delete_precondition(&map(&[("if-match", &etag())])), Ok(Etag::of(b"x")));
        assert_eq!(delete_precondition(&map(&[("if-none-match", "*")])).unwrap_err().code, Code::BadRequest);
    }

    #[test]
    fn the_request_id_is_32_hex_characters() {
        let id = new_request_id();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(id, new_request_id());
    }

    #[test]
    fn the_security_headers_of_7_5() {
        let mut h = HeaderMap::new();
        add_security_headers(&mut h);
        assert_eq!(h.len(), 7);
        let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("style-src 'self';"));
        assert!(!csp.contains("unsafe-inline"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
    }
}
