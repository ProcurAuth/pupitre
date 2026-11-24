// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check the URLs of the configuration by hand (6.13). The rules are narrow: a scheme, a
//! host, an optional port and, for one key, a path.

use std::net::{Ipv4Addr, Ipv6Addr};

use hclconf::kinds::is_dns_name;

/// An `http` or `https` URL that passed the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpUrl {
    /// True for the scheme `https`.
    pub https: bool,
    /// The host, in lower case. An IPv6 address has no brackets.
    pub host: String,
    /// The port: the one in the text, or 80 or 443.
    pub port: u16,
    /// The URL as the file gives it.
    pub text: String,
}

/// The rule text, for the error `expected <rule>, found <value>`.
pub fn rule_text(allow_path: bool) -> &'static str {
    if allow_path {
        "an absolute URL in canonical form with the scheme http or https, a host, an optional port and an optional path, and with no user info, query or fragment"
    } else {
        "an absolute URL in canonical form with the scheme http or https, a host and an optional port, and with no path, query, fragment or user info"
    }
}

/// Parse an absolute `http` or `https` URL in canonical form: lower case scheme and host, a
/// port from 1 to 65535 with no leading zero, no user info, no query, no fragment. A path
/// is valid only with `allow_path`. Give the URL and its path (empty when there is none).
pub fn parse_http_url(text: &str, allow_path: bool) -> Option<(HttpUrl, String)> {
    if !text.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return None;
    }
    let (https, rest) = if let Some(rest) = text.strip_prefix("https://") {
        (true, rest)
    } else {
        (false, text.strip_prefix("http://")?)
    };
    if rest.contains(['?', '#', '@', '\\']) {
        return None;
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    if !path.is_empty() && !allow_path {
        return None;
    }
    if path.contains("//") || (!path.is_empty() && !path.bytes().all(path_byte)) {
        return None;
    }
    let bracketed = authority.starts_with('[');
    let (host, port_text): (String, Option<&str>) = if let Some(inner) = authority.strip_prefix('[') {
        let close = inner.find(']')?;
        let host = &inner[..close];
        let after = &inner[close + 1..];
        let addr: Ipv6Addr = host.parse().ok()?;
        if addr.to_string() != host {
            return None;
        }
        let port_text = if after.is_empty() { None } else { Some(after.strip_prefix(':')?) };
        (host.to_string(), port_text)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.to_string(), None),
        }
    };
    if host.is_empty() {
        return None;
    }
    if !bracketed {
        let is_ipv4 = host.parse::<Ipv4Addr>().is_ok();
        let all_digits = host.bytes().all(|b| b.is_ascii_digit() || b == b'.');
        let lower_dns = is_dns_name(&host) && host == host.to_ascii_lowercase();
        if !(is_ipv4 || (lower_dns && !all_digits)) {
            return None;
        }
    }
    let port = match port_text {
        None => {
            if https {
                443
            } else {
                80
            }
        }
        Some(p) => {
            if p.is_empty() || p.starts_with('0') || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse::<u16>().ok()?
        }
    };
    Some((HttpUrl { https, host, port, text: text.to_string() }, path.to_string()))
}

fn path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/%".contains(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(t: &str, p: bool) -> HttpUrl {
        parse_http_url(t, p).unwrap_or_else(|| panic!("{t}")).0
    }

    #[test]
    fn accepts_the_forms_of_the_key_table() {
        let u = ok("https://clickhouse:8443", false);
        assert_eq!((u.https, u.host.as_str(), u.port), (true, "clickhouse", 8443));
        let u = ok("http://otel-collector:4318", false);
        assert_eq!((u.https, u.port), (false, 4318));
        assert_eq!(ok("https://a.example", false).port, 443);
        assert_eq!(ok("http://a.example", false).port, 80);
        assert_eq!(ok("http://127.0.0.1:8123", false).host, "127.0.0.1");
        let u = ok("http://[::1]:8123", false);
        assert_eq!((u.host.as_str(), u.port), ("::1", 8123));
        assert_eq!(ok("http://[::1]", false).port, 80);
    }

    #[test]
    fn path_only_where_permitted() {
        let (u, path) = parse_http_url("https://pdp.example:4182/v1/base", true).unwrap();
        assert_eq!((u.port, path.as_str()), (4182, "/v1/base"));
        assert_eq!(parse_http_url("https://pdp.example", true).unwrap().1, "");
        assert!(parse_http_url("https://pdp.example/v1", false).is_none());
        assert!(parse_http_url("https://pdp.example/", false).is_none());
        assert!(parse_http_url("https://pdp.example/a//b", true).is_none());
    }

    #[test]
    fn refuses_everything_else() {
        for bad in [
            "ftp://a",
            "a.example",
            "//a.example",
            "HTTP://a.example",
            "https://A.example",
            "https://user@a.example",
            "https://user:pw@a.example",
            "https://a.example?q=1",
            "https://a.example#f",
            "https://a.example:",
            "https://a.example:0",
            "https://a.example:08080",
            "https://a.example:65536",
            "https://a.example:80a",
            "https://:80",
            "https://",
            "https://a b",
            "https://a_b.example",
            "https://1.2.3",
            "https://a:1:2",
            "http://[::1",
            "http://[0:0:0:0:0:0:0:1]",
            "http://[::1]x",
            "http://[::1]:",
            "https://a.example\\x",
        ] {
            assert!(parse_http_url(bad, true).is_none(), "{bad}");
        }
    }
}
