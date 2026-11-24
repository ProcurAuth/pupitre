// SPDX-License-Identifier: MIT OR Apache-2.0
//! The allow-list of the header `Host` (7.4): the entries of `server.allowed_hosts`, the default
//! of a loopback listener, and the match of a request.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// A host name or an IP address, in the form that the match compares: a lower-case name with no
/// final dot, an IPv4 address, or an IPv6 address in brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostName(String);

impl HostName {
    /// The text of the host, in the canonical form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One entry of `server.allowed_hosts`. An entry with no port matches each port. An entry with a
/// port matches a request that has the same explicit port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern {
    /// The host.
    pub host: HostName,
    /// The port, when the entry has one.
    pub port: Option<u16>,
}

/// The loopback names of the default (7.4).
const LOOPBACK_NAMES: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

fn is_dns_name(text: &str) -> bool {
    if text.is_empty() || text.len() > 253 {
        return false;
    }
    text.split('.').all(|label| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// Split `host`, `host:port`, `[v6]` or `[v6]:port`. A port is a whole number from 1 to 65535.
fn split(text: &str) -> Option<(&str, Option<u16>)> {
    let port_of = |p: &str| {
        let ok = !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit());
        ok.then(|| p.parse::<u16>().ok()).flatten().filter(|n| *n != 0)
    };
    if let Some(rest) = text.strip_prefix('[') {
        let (inner, tail) = rest.split_once(']')?;
        return match tail {
            "" => Some((inner, None)),
            t => Some((inner, Some(port_of(t.strip_prefix(':')?)?))),
        };
    }
    match text.rsplit_once(':') {
        Some((host, port)) => Some((host, Some(port_of(port)?))),
        None => Some((text, None)),
    }
}

/// The canonical host of a text without port. `bracketed` is true when the text came in
/// brackets, so it must be an IPv6 address.
fn canonical_host(text: &str, bracketed: bool, strip_dot: bool) -> Option<HostName> {
    if bracketed {
        let ip: Ipv6Addr = text.parse().ok()?;
        return Some(HostName(format!("[{ip}]")));
    }
    if let Ok(ip) = text.parse::<Ipv4Addr>() {
        return Some(HostName(ip.to_string()));
    }
    let name = if strip_dot { text.strip_suffix('.').unwrap_or(text) } else { text }.to_ascii_lowercase();
    // A name of digits and dots that is not an IPv4 address is not a host name.
    let numeric = name.split('.').all(|l| l.bytes().all(|b| b.is_ascii_digit()));
    (is_dns_name(&name) && !numeric).then_some(HostName(name))
}

/// Parse the value of a header `Host`, or an entry of `server.allowed_hosts`. `None` when the
/// text is not a host with an optional port. A value of a header can have a final dot in its
/// name (`localhost.`); an entry cannot.
fn parse_authority(text: &str, strip_dot: bool) -> Option<(HostName, Option<u16>)> {
    let (host, port) = split(text)?;
    let bracketed = text.starts_with('[');
    Some((canonical_host(host, bracketed, strip_dot)?, port))
}

impl HostPattern {
    /// Parse an entry of `server.allowed_hosts`. The error text tells what the entry must be.
    pub fn parse(text: &str) -> Result<HostPattern, &'static str> {
        match parse_authority(text, false) {
            Some((host, port)) => Ok(HostPattern { host, port }),
            None => Err(
                "a host name, an IPv4 address or an IPv6 address in brackets, with an optional port from 1 to 65535",
            ),
        }
    }

    /// True when a value of the header `Host` is in the entry.
    pub fn matches(&self, host: &HostName, port: Option<u16>) -> bool {
        self.host == *host && self.port.is_none_or(|p| Some(p) == port)
    }
}

/// True when a value of the header `Host` matches one of the entries. A value that is not a host
/// with an optional port matches no entry.
pub fn host_allowed(patterns: &[HostPattern], value: &str) -> bool {
    let Some((host, port)) = parse_authority(value.trim(), true) else { return false };
    patterns.iter().any(|p| p.matches(&host, port))
}

/// The default of `server.allowed_hosts` for a listener on a loopback address: `localhost`,
/// `127.0.0.1`, `[::1]` and the IP address of the listener. `None` for another address: the key
/// is then necessary (7.4).
pub fn default_hosts(listen: SocketAddr) -> Option<Vec<HostPattern>> {
    if !listen.ip().is_loopback() {
        return None;
    }
    let mut out: Vec<HostPattern> = LOOPBACK_NAMES.iter().filter_map(|n| HostPattern::parse(n).ok()).collect();
    let own = match listen.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    if let Ok(p) = HostPattern::parse(&own)
        && !out.contains(&p)
    {
        out.push(p);
    }
    Some(out)
}

/// The text of a pattern, with the port when it has one. It is the form of an entry.
impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.port {
            Some(p) => write!(f, "{}:{p}", self.host.as_str()),
            None => f.write_str(self.host.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(list: &[&str]) -> Vec<HostPattern> {
        list.iter().map(|t| HostPattern::parse(t).unwrap()).collect()
    }

    #[test]
    fn an_entry_is_a_host_with_an_optional_port() {
        for ok in [
            "localhost",
            "a.example.invalid",
            "A.Example.Invalid:8443",
            "127.0.0.1",
            "127.0.0.1:80",
            "[::1]",
            "[::1]:8080",
        ] {
            assert!(HostPattern::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "*",
            "*.example.invalid",
            "a b",
            "a..b",
            "-a.example",
            "a.example.",
            "a.example:0",
            "a.example:65536",
            "a.example:",
            "a.example:08x",
            "::1",
            "[::1",
            "[a]",
            "[::1]x",
            "1.2.3",
            "300.1.1.1",
            "http://a.example",
            "a.example/x",
            "a@b",
            "a:1:2",
        ] {
            assert!(HostPattern::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_match_ignores_the_case_and_the_form_of_an_address() {
        let p = pats(&["a.example.invalid", "b.example.invalid:8443", "[::1]", "127.0.0.1"]);
        assert!(host_allowed(&p, "A.Example.Invalid"));
        assert!(host_allowed(&p, "a.example.invalid:9999"));
        assert!(host_allowed(&p, "a.example.invalid."));
        assert!(host_allowed(&p, "b.example.invalid:8443"));
        assert!(!host_allowed(&p, "b.example.invalid"));
        assert!(!host_allowed(&p, "b.example.invalid:8444"));
        assert!(host_allowed(&p, "[0:0:0:0:0:0:0:1]:80"));
        assert!(host_allowed(&p, "127.0.0.1:8080"));
        for bad in [
            "",
            "evil.example",
            "a.example.invalid.evil.example",
            "evil.example#a.example.invalid",
            "a.example.invalid:x",
            "a.example.invalid@evil.example",
        ] {
            assert!(!host_allowed(&p, bad), "{bad}");
        }
    }

    #[test]
    fn the_default_is_for_a_loopback_listener_only() {
        let local = default_hosts("127.0.0.1:8080".parse().unwrap()).unwrap();
        assert_eq!(local.iter().map(ToString::to_string).collect::<Vec<_>>(), ["localhost", "127.0.0.1", "[::1]"]);
        let other = default_hosts("127.0.0.2:8080".parse().unwrap()).unwrap();
        assert_eq!(other.len(), 4);
        assert!(host_allowed(&other, "127.0.0.2:8080"));
        assert!(default_hosts("[::1]:8080".parse().unwrap()).is_some());
        assert!(default_hosts("0.0.0.0:8080".parse().unwrap()).is_none());
        assert!(default_hosts("192.0.2.5:8080".parse().unwrap()).is_none());
        assert!(!host_allowed(&local, "attacker.example"));
        assert!(!host_allowed(&local, "localhost.attacker.example"));
    }
}
