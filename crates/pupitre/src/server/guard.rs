// SPDX-License-Identifier: MIT OR Apache-2.0
//! The guards of 7.4: the header `Host`, the cross-site check and the media type of a body. The
//! check of `Host` runs on each request. The other two run on each
//! `POST`, `PUT` and `DELETE` of the main listener before the server reads the body.

use http::header::{CONTENT_TYPE, HOST, HeaderMap, ORIGIN};

use super::error::{ApiError, Code};
use super::headers::SEC_FETCH_SITE;
use crate::config::hosts::{HostPattern, host_allowed};

fn split_host_port(text: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail {
            "" => None,
            t => Some(t.strip_prefix(':')?.parse::<u16>().ok()?),
        };
        return Some((format!("[{}]", host.to_ascii_lowercase()), port));
    }
    match text.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') || host.is_empty() {
                return None;
            }
            Some((host.to_ascii_lowercase(), Some(port.parse::<u16>().ok()?)))
        }
        None if text.is_empty() => None,
        None => Some((text.to_ascii_lowercase(), None)),
    }
}

/// Compare the host and the port of `Origin` with the header `Host` (7.4). A host without a
/// port has the default port of the scheme of `Origin`.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let (default_port, rest) = if let Some(r) = origin.strip_prefix("http://") {
        (80u16, r)
    } else if let Some(r) = origin.strip_prefix("https://") {
        (443u16, r)
    } else {
        return false;
    };
    if rest.contains(['/', '?', '#', '@']) {
        return false;
    }
    let (Some((origin_host, origin_port)), Some((host_host, host_port))) =
        (split_host_port(rest), split_host_port(host.trim()))
    else {
        return false;
    };
    origin_host == host_host && origin_port.unwrap_or(default_port) == host_port.unwrap_or(default_port)
}

/// The check of the header `Host` (7.4). The request has exactly one header `Host`, its value is
/// an entry of `server.allowed_hosts`, and so is the authority of a request target in the
/// absolute form. Else the answer is `421 misdirected_request`.
pub fn check_host(allowed: &[HostPattern], uri: &http::Uri, headers: &HeaderMap) -> Result<(), ApiError> {
    let refuse = || ApiError::new(Code::MisdirectedRequest);
    let mut hosts = headers.get_all(HOST).iter();
    let host = hosts.next().and_then(|h| h.to_str().ok()).ok_or_else(refuse)?;
    if hosts.next().is_some() || !host_allowed(allowed, host) {
        return Err(refuse());
    }
    match uri.authority() {
        Some(authority) if !host_allowed(allowed, authority.as_str()) => Err(refuse()),
        _ => Ok(()),
    }
}

/// The cross-site guard of 7.4. `403 cross_site_request` when the request comes from another
/// site. A client that sends neither `Sec-Fetch-Site` nor `Origin` passes.
pub fn cross_site_guard(headers: &HeaderMap) -> Result<(), ApiError> {
    let refuse = || ApiError::new(Code::CrossSiteRequest);
    let mut fetch = headers.get_all(SEC_FETCH_SITE).iter();
    if let Some(value) = fetch.next() {
        if fetch.next().is_some() {
            return Err(refuse());
        }
        let ok = value.to_str().is_ok_and(|v| matches!(v.trim(), "same-origin" | "none"));
        return if ok { Ok(()) } else { Err(refuse()) };
    }
    let mut origins = headers.get_all(ORIGIN).iter();
    let Some(origin) = origins.next() else { return Ok(()) };
    if origins.next().is_some() {
        return Err(refuse());
    }
    let mut hosts = headers.get_all(HOST).iter();
    let host = hosts.next().and_then(|h| h.to_str().ok());
    match (origin.to_str(), host) {
        (Ok(o), Some(h)) if hosts.next().is_none() && origin_matches_host(o, h) => Ok(()),
        _ => Err(refuse()),
    }
}

/// True for `application/json` with an optional `charset=utf-8` and no other parameter (7.4).
pub fn is_json_content_type(value: &str) -> bool {
    let mut parts = value.split(';');
    let media = parts.next().unwrap_or("").trim();
    if !media.eq_ignore_ascii_case("application/json") {
        return false;
    }
    let mut charset_seen = false;
    for param in parts {
        let Some((name, val)) = param.trim().split_once('=') else { return false };
        let val = val.trim().trim_matches('"');
        if charset_seen || !name.trim().eq_ignore_ascii_case("charset") || !val.eq_ignore_ascii_case("utf-8") {
            return false;
        }
        charset_seen = true;
    }
    true
}

/// `415 unsupported_media_type` unless the request has one `Content-Type: application/json`.
pub fn check_content_type(headers: &HeaderMap) -> Result<(), ApiError> {
    let mut all = headers.get_all(CONTENT_TYPE).iter();
    let ok = match (all.next(), all.next()) {
        (Some(v), None) => v.to_str().is_ok_and(is_json_content_type),
        _ => false,
    };
    if ok { Ok(()) } else { Err(ApiError::new(Code::UnsupportedMediaType)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{HeaderName, HeaderValue};

    fn map(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(HeaderName::from_static(k), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn fetch_metadata_decides_first_7_4() {
        assert!(cross_site_guard(&map(&[("sec-fetch-site", "same-origin")])).is_ok());
        assert!(cross_site_guard(&map(&[("sec-fetch-site", "none")])).is_ok());
        for bad in ["cross-site", "same-site", "", "SAME-ORIGIN"] {
            assert_eq!(
                cross_site_guard(&map(&[("sec-fetch-site", bad)])).unwrap_err().code,
                Code::CrossSiteRequest,
                "{bad}"
            );
        }
        // Sec-Fetch-Site wins over a matching Origin.
        let h = map(&[
            ("sec-fetch-site", "cross-site"),
            ("origin", "http://a.example.invalid"),
            ("host", "a.example.invalid"),
        ]);
        assert_eq!(cross_site_guard(&h).unwrap_err().code, Code::CrossSiteRequest);
    }

    #[test]
    fn the_origin_must_equal_the_host_7_4() {
        let ok = |o: &str, h: &str| cross_site_guard(&map(&[("origin", o), ("host", h)])).is_ok();
        assert!(ok("http://a.example.invalid:8080", "a.example.invalid:8080"));
        assert!(ok("http://a.example.invalid", "a.example.invalid:80"));
        assert!(ok("https://a.example.invalid", "a.example.invalid"));
        assert!(ok("https://a.example.invalid", "A.Example.Invalid:443"));
        assert!(ok("http://[::1]:8080", "[::1]:8080"));
        assert!(!ok("http://a.example.invalid", "a.example.invalid:8080"));
        assert!(!ok("https://a.example.invalid", "a.example.invalid:80"));
        assert!(!ok("null", "a.example.invalid"));
        assert!(!ok("http://b.example.invalid", "a.example.invalid"));
        assert!(!ok("http://a.example.invalid/x", "a.example.invalid"));
        assert!(!ok("ftp://a.example.invalid", "a.example.invalid"));
        // No Host header: the Origin cannot match.
        assert!(cross_site_guard(&map(&[("origin", "http://a.example.invalid")])).is_err());
        // A client with neither header passes.
        assert!(cross_site_guard(&map(&[])).is_ok());
    }

    #[test]
    fn the_media_type_of_a_body_7_4() {
        for ok in ["application/json", "application/json; charset=utf-8", "Application/JSON;charset=UTF-8"] {
            assert!(check_content_type(&map(&[("content-type", ok)])).is_ok(), "{ok}");
        }
        for bad in ["text/plain", "application/json; charset=latin1", "application/json; x=y", "application/jsonx"] {
            assert_eq!(
                check_content_type(&map(&[("content-type", bad)])).unwrap_err().code,
                Code::UnsupportedMediaType,
                "{bad}"
            );
        }
        assert_eq!(check_content_type(&map(&[])).unwrap_err().code, Code::UnsupportedMediaType);
    }
}
