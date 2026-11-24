// SPDX-License-Identifier: MIT OR Apache-2.0
//! The code agrees with the tables of the specification: the codes of 5.4, the headers of 7.5 and
//! the routes of 3.12 and section 5.

use pupitre::server::error::Code;
use pupitre::server::headers::{CSP, PERMISSIONS_POLICY};

const API: &str = include_str!("../../../docs/05-api.md");
const SECURITY: &str = include_str!("../../../docs/07-security-and-limits.md");
const ACCESS: &str = include_str!("../../../docs/03-access.md");

fn cells(line: &str) -> Vec<String> {
    line.trim().trim_matches('|').split('|').map(|c| c.trim().to_string()).collect()
}

fn unquote(cell: &str) -> String {
    cell.trim_matches('`').to_string()
}

#[test]
fn the_codes_and_statuses_of_the_table_of_5_4() {
    let section = API.split("### 5.4 Errors").nth(1).unwrap().split("### 5.5").next().unwrap();
    let rows: Vec<(u16, String)> = section
        .lines()
        .filter(|l| l.starts_with("| 4") || l.starts_with("| 5"))
        .map(cells)
        .map(|c| (c[0].parse().unwrap(), unquote(&c[1])))
        .collect();
    assert_eq!(rows.len(), Code::ALL.len(), "the table of 5.4 and Code::ALL have the same size");
    for (status, code) in &rows {
        let found =
            Code::ALL.iter().find(|c| c.as_str() == code).unwrap_or_else(|| panic!("no code {code} in the program"));
        assert_eq!(found.status().as_u16(), *status, "{code}");
    }
    for code in Code::ALL {
        assert!(rows.iter().any(|(_, c)| c == code.as_str()), "{} is not in 5.4", code.as_str());
        // The message is a fixed English text with no value.
        assert!(!code.message().is_empty());
        assert!(code.message().ends_with('.'));
    }
    // `retryable` is true only for the codes of 503. `store_over_limit` is the one code of 503 that is not
    // retryable: a try again cannot help while the store holds more than a limit (2.41). `Retry-After` for two codes.
    for code in Code::ALL {
        let expected = code.status().as_u16() == 503 && code != Code::StoreOverLimit;
        assert_eq!(code.retryable(), expected, "{}", code.as_str());
    }
    assert_eq!(Code::ServerBusy.retry_after(), Some(1));
    assert_eq!(Code::RunQueueFull.retry_after(), Some(5));
    assert_eq!(Code::PdpUnavailable.retry_after(), None);
}

#[test]
fn the_security_headers_of_7_5() {
    let section = SECURITY.split("### 7.5 Security headers").nth(1).unwrap().split("### 7.6").next().unwrap();
    let row = |name: &str| -> String {
        let line = section.lines().find(|l| l.starts_with(&format!("| `{name}`"))).unwrap_or_else(|| panic!("{name}"));
        unquote(&cells(line)[1])
    };
    assert_eq!(row("Content-Security-Policy"), CSP);
    assert_eq!(row("Permissions-Policy"), PERMISSIONS_POLICY);
    assert_eq!(row("X-Content-Type-Options"), "nosniff");
    assert_eq!(row("X-Frame-Options"), "DENY");
    assert_eq!(row("Referrer-Policy"), "same-origin");
    assert_eq!(row("Cross-Origin-Resource-Policy"), "same-origin");
    assert_eq!(row("Cross-Origin-Opener-Policy"), "same-origin");
}

/// The routes that the table of 3.12 names, as `METHOD path` with the variables of the router.
fn spec_routes() -> Vec<String> {
    let mut out = Vec::new();
    let section = ACCESS.split("### 3.12").nth(1).unwrap().split("Rules:").next().unwrap();
    for line in section.lines().filter(|l| l.starts_with("| `") || l.starts_with("| History")) {
        let first = cells(line)[0].clone();
        for part in first.split('`').filter(|p| p.contains('/') && !p.starts_with(" and")) {
            out.push(part.trim().to_string());
        }
    }
    out
}

#[test]
fn each_route_that_3_12_names_is_a_route_of_the_router() {
    let router: Vec<String> = pupitre::server::routes::all().iter().map(|r| r.label.clone()).collect();
    let mut checked = 0;
    for entry in spec_routes() {
        // An entry is `GET /api/x`, `PUT, DELETE /api/x`, `GET .../x` or a path with a query.
        let (methods, path) = match entry.split_once(' ') {
            Some((m, p)) if m.chars().all(|c| c.is_ascii_uppercase() || c == ',') => {
                (m.trim_end_matches(',').to_string(), p.to_string())
            }
            _ => continue,
        };
        if path.starts_with("...") || !path.starts_with('/') {
            continue;
        }
        let path = path.split('?').next().unwrap_or("").to_string();
        let pattern = path.clone();
        let methods: Vec<&str> = methods.split(',').map(str::trim).filter(|m| !m.is_empty()).collect();
        for method in methods {
            if path.starts_with("/.well-known") || path == "/api/session" && method == "GET" {
                continue;
            }
            let label = format!("{method} {pattern}");
            assert!(router.contains(&label), "3.12 names {label}, and the router has no such route");
            checked += 1;
        }
    }
    assert!(checked >= 20, "{checked}");
}

#[test]
fn the_routes_of_5_8_to_5_18_are_in_the_router() {
    let router: Vec<String> = pupitre::server::routes::all().iter().map(|r| r.label.clone()).collect();
    let mut checked = 0;
    for line in API.lines().filter(|l| l.starts_with("| `")) {
        let route = cells(line)[0].replace('`', "");
        let Some((methods, path)) = route.rsplit_once(' ') else { continue };
        if !path.starts_with("/api/") {
            continue;
        }
        for method in methods.split(',').map(str::trim) {
            let label = format!("{method} {path}");
            assert!(router.contains(&label), "{label} is in section 5 and not in the router");
            checked += 1;
        }
    }
    assert!(checked >= 30, "{checked}");
}
