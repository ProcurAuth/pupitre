// SPDX-License-Identifier: MIT OR Apache-2.0
//! The routes of the main listener (section 5) and the rule of each route (3.12).
//!
//! The router table and the table of rules are two tables. A test fails when a route of the
//! router has no rule (3.12). A route with no rule gives `403 forbidden`, asks nothing and
//! writes the log line `access.no_rule`.

use std::sync::OnceLock;

use http::Method;
use pupitre_core::action::Action;
use pupitre_core::doc::DocKind;

/// The scope of a secret route (5.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretRoute {
    /// `/api/environments/{e}/secrets`
    Environment,
    /// `/api/environments/{e}/units/{u}/secrets`
    Unit,
}

/// The identity of a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteId {
    /// `GET /api/session` (5.7)
    Session,
    /// `GET /api/catalog` (5.15)
    Catalog,
    /// `GET /api/environments` (5.8)
    EnvironmentList,
    /// `POST /api/environments` (5.8)
    EnvironmentCreate,
    /// `GET` of a document (5.8 to 5.11)
    DocGet(DocKind),
    /// `PUT` of a document (5.5)
    DocPut(DocKind),
    /// `DELETE` of a document (5.5)
    DocDelete(DocKind),
    /// `GET <document path>/history` (5.13)
    DocHistory(DocKind),
    /// `GET <document path>/history/{n}` (5.13)
    DocVersion(DocKind),
    /// `GET /api/environments/{e}/hosts` (5.9)
    HostList,
    /// `GET /api/environments/{e}/units` (5.10)
    UnitList,
    /// `GET .../units/{u}/effective` (5.12)
    Effective,
    /// `GET .../secrets` (5.14)
    SecretList(SecretRoute),
    /// `PUT .../secrets/{name}` (5.14)
    SecretPut(SecretRoute),
    /// `DELETE .../secrets/{name}` (5.14)
    SecretDelete(SecretRoute),
    /// `POST /api/runs` (5.16)
    RunCreate,
    /// `GET /api/runs` (5.16)
    RunList,
    /// `GET /api/runs/{r}` (5.16)
    RunGet,
    /// `GET /api/runs/{r}/events` (5.17)
    RunEvents,
    /// `GET /api/runs/{r}/output` (5.17)
    RunOutput,
    /// `POST /api/runs/{r}/cancel` (5.18)
    RunCancel,
    /// `GET /.well-known/procurauth/actions` (5.19)
    WellKnownActions,
    /// `GET /.well-known/procurauth/resources` (5.19)
    WellKnownResources,
    /// A page of 5.21: the shell.
    Page {
        /// True for the pages of the runs: they are off with `runner.enabled = false` (3.3).
        runs: bool,
    },
    /// A file below `/static/<build>/` (5.23)
    Static,
}

/// The node of a question, as a form of the parameters of the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSpec {
    /// S
    Service,
    /// E(e)
    Environment,
    /// U(e,u)
    Unit,
    /// H(e,u,h)
    Host,
}

/// The rule of a route (3.12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// No token and no question (3.11).
    NoToken,
    /// A token, then the question `access` on S (3.20).
    Access,
    /// A token and the questions of the options route (3.16).
    Options,
    /// A token, then the questions of the display filter (3.15).
    Display,
    /// A token, then one use on a node (3.12).
    Use(Action, NodeSpec),
    /// A token, then the use action of the kind and the mode of the request (4.3).
    RunRequest,
    /// A token, then `run.read` on the node of the run.
    RunRead,
    /// A token, then the use action of the kind and the mode of the run (4.27).
    RunCancel,
}

/// The place of a path, for an unknown path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// A path of the API or of the ProcurAuth routes: an unknown path gives `404 not_found`.
    Api,
    /// A path of a page: an unknown path gives the shell with the status 404 (5.21).
    Page,
}

/// The kind of body that a route reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// No body. A body gives `400 bad_request`.
    None,
    /// A JSON body, within `server.max_request_bytes`.
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Lit(&'static str),
    Var(&'static str),
    Rest(&'static str),
}

/// One route of the router.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    /// The identity.
    pub id: RouteId,
    /// The method. A `HEAD` request uses the route `GET`.
    pub method: Method,
    /// The pattern, for example `/api/environments/{e}`. It is the label of the metrics.
    pub label: String,
    /// The query parameters that the route names (5.3).
    pub query: &'static [&'static str],
    /// The body that the route reads.
    pub body: BodyKind,
    segs: Vec<Seg>,
}

/// The values of the variables of a pattern.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params(Vec<(&'static str, String)>);

impl Params {
    /// The value of a variable.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }
}

/// The result of a match.
#[derive(Debug)]
pub enum Matched {
    /// A route and its parameters.
    Route(&'static RouteSpec, Params),
    /// The path exists with other methods. The text is the value of `Allow`.
    MethodNotAllowed(String),
    /// No route has this path.
    NotFound(Region),
}

const LIST_QUERY: &[&str] = &["limit", "after"];
const HISTORY_QUERY: &[&str] = &["limit", "before"];

fn parse_pattern(pattern: &'static str) -> Vec<Seg> {
    pattern
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| match s.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
            Some(name) => match name.strip_prefix('*') {
                Some(rest) => Seg::Rest(rest),
                None => Seg::Var(name),
            },
            None => Seg::Lit(s),
        })
        .collect()
}

fn build() -> Vec<RouteSpec> {
    let mut routes: Vec<RouteSpec> = Vec::new();
    let mut add =
        |id: RouteId, method: Method, pattern: &'static str, query: &'static [&'static str], body: BodyKind| {
            routes.push(RouteSpec {
                id,
                label: format!("{method} {pattern}"),
                method,
                query,
                body,
                segs: parse_pattern(pattern),
            });
        };
    add(RouteId::Session, Method::GET, "/api/session", &[], BodyKind::None);
    add(RouteId::Catalog, Method::GET, "/api/catalog", &[], BodyKind::None);
    add(RouteId::EnvironmentList, Method::GET, "/api/environments", LIST_QUERY, BodyKind::None);
    add(RouteId::EnvironmentCreate, Method::POST, "/api/environments", &[], BodyKind::Json);
    // The documents (5.8 to 5.11): the path, and the kind of its document.
    let docs: [(DocKind, &'static str, &'static str, &'static str); 6] = [
        (
            DocKind::Environment,
            "/api/environments/{e}",
            "/api/environments/{e}/history",
            "/api/environments/{e}/history/{n}",
        ),
        (
            DocKind::Host,
            "/api/environments/{e}/hosts/{h}",
            "/api/environments/{e}/hosts/{h}/history",
            "/api/environments/{e}/hosts/{h}/history/{n}",
        ),
        (
            DocKind::Unit,
            "/api/environments/{e}/units/{u}",
            "/api/environments/{e}/units/{u}/history",
            "/api/environments/{e}/units/{u}/history/{n}",
        ),
        (
            DocKind::Common,
            "/api/environments/{e}/common",
            "/api/environments/{e}/common/history",
            "/api/environments/{e}/common/history/{n}",
        ),
        (
            DocKind::UnitValues,
            "/api/environments/{e}/units/{u}/values",
            "/api/environments/{e}/units/{u}/values/history",
            "/api/environments/{e}/units/{u}/values/history/{n}",
        ),
        (
            DocKind::HostValues,
            "/api/environments/{e}/units/{u}/hosts/{h}/values",
            "/api/environments/{e}/units/{u}/hosts/{h}/values/history",
            "/api/environments/{e}/units/{u}/hosts/{h}/values/history/{n}",
        ),
    ];
    for (kind, path, history, version) in docs {
        add(RouteId::DocGet(kind), Method::GET, path, &[], BodyKind::None);
        add(RouteId::DocPut(kind), Method::PUT, path, &[], BodyKind::Json);
        add(RouteId::DocDelete(kind), Method::DELETE, path, &[], BodyKind::None);
        add(RouteId::DocHistory(kind), Method::GET, history, HISTORY_QUERY, BodyKind::None);
        add(RouteId::DocVersion(kind), Method::GET, version, &[], BodyKind::None);
    }
    add(RouteId::HostList, Method::GET, "/api/environments/{e}/hosts", LIST_QUERY, BodyKind::None);
    add(RouteId::UnitList, Method::GET, "/api/environments/{e}/units", LIST_QUERY, BodyKind::None);
    add(RouteId::Effective, Method::GET, "/api/environments/{e}/units/{u}/effective", &["host"], BodyKind::None);
    for (scope, list, one) in [
        (SecretRoute::Environment, "/api/environments/{e}/secrets", "/api/environments/{e}/secrets/{name}"),
        (
            SecretRoute::Unit,
            "/api/environments/{e}/units/{u}/secrets",
            "/api/environments/{e}/units/{u}/secrets/{name}",
        ),
    ] {
        add(RouteId::SecretList(scope), Method::GET, list, &[], BodyKind::None);
        add(RouteId::SecretPut(scope), Method::PUT, one, &[], BodyKind::Json);
        add(RouteId::SecretDelete(scope), Method::DELETE, one, &[], BodyKind::None);
    }
    add(RouteId::RunCreate, Method::POST, "/api/runs", &[], BodyKind::Json);
    add(RouteId::RunList, Method::GET, "/api/runs", &["environment", "status", "limit", "after"], BodyKind::None);
    add(RouteId::RunGet, Method::GET, "/api/runs/{r}", &[], BodyKind::None);
    add(RouteId::RunEvents, Method::GET, "/api/runs/{r}/events", &["after", "limit"], BodyKind::None);
    add(RouteId::RunOutput, Method::GET, "/api/runs/{r}/output", &["offset", "limit"], BodyKind::None);
    add(RouteId::RunCancel, Method::POST, "/api/runs/{r}/cancel", &[], BodyKind::None);
    add(RouteId::WellKnownActions, Method::GET, "/.well-known/procurauth/actions", &[], BodyKind::None);
    // The options route reads its own query string (3.16): the table names no parameter.
    add(RouteId::WellKnownResources, Method::GET, "/.well-known/procurauth/resources", &[], BodyKind::None);
    for (pattern, runs) in [
        ("/", false),
        ("/e/{e}", false),
        ("/e/{e}/units/{u}", false),
        ("/e/{e}/units/{u}/hosts/{h}", false),
        ("/e/{e}/hosts/{h}", false),
        ("/e/{e}/units/{u}/history", false),
        ("/e/{e}/units/{u}/run", true),
        ("/e/{e}/commands", true),
        ("/e/{e}/commands/{c}", true),
        ("/runs", true),
        ("/runs/{r}", true),
    ] {
        add(RouteId::Page { runs }, Method::GET, pattern, &[], BodyKind::None);
    }
    add(RouteId::Static, Method::GET, "/static/{build}/{*file}", &[], BodyKind::None);
    routes
}

/// The routes of the router.
pub fn all() -> &'static [RouteSpec] {
    static ROUTES: OnceLock<Vec<RouteSpec>> = OnceLock::new();
    ROUTES.get_or_init(build)
}

fn match_segments(segs: &[Seg], parts: &[&str]) -> Option<Params> {
    let mut params = Vec::new();
    let mut i = 0;
    for seg in segs {
        match seg {
            Seg::Lit(text) => {
                if parts.get(i) != Some(text) {
                    return None;
                }
                i += 1;
            }
            Seg::Var(name) => {
                let part = parts.get(i)?;
                params.push((*name, (*part).to_string()));
                i += 1;
            }
            Seg::Rest(name) => {
                if i >= parts.len() {
                    return None;
                }
                params.push((*name, parts[i..].join("/")));
                return Some(Params(params));
            }
        }
    }
    (i == parts.len()).then_some(Params(params))
}

/// Remove the base path from a request path (5.2). The result starts with no `/`. A path
/// outside the base path gives `None`.
pub fn strip_base<'a>(base_path: &str, path: &'a str) -> Option<&'a str> {
    let rest = path.strip_prefix(base_path.trim_end_matches('/'))?;
    if base_path == "/" {
        return rest.strip_prefix('/');
    }
    rest.strip_prefix('/')
}

fn region_of(parts: &[&str]) -> Region {
    match parts.first() {
        Some(&"api") | Some(&".well-known") => Region::Api,
        _ => Region::Page,
    }
}

/// Match a method and a path below the base path (5.2).
pub fn route(method: &Method, rel: &str) -> Matched {
    let parts: Vec<&str> = if rel.is_empty() { Vec::new() } else { rel.split('/').collect() };
    if parts.iter().any(|p| p.is_empty()) {
        return Matched::NotFound(region_of(&parts));
    }
    let wanted = if *method == Method::HEAD { &Method::GET } else { method };
    let mut allowed: Vec<&Method> = Vec::new();
    for spec in all() {
        let Some(params) = match_segments(&spec.segs, &parts) else { continue };
        if spec.method == *wanted {
            return Matched::Route(spec, params);
        }
        if !allowed.contains(&&spec.method) {
            allowed.push(&spec.method);
        }
    }
    if allowed.is_empty() {
        return Matched::NotFound(region_of(&parts));
    }
    let mut names: Vec<String> = allowed.iter().map(|m| m.as_str().to_string()).collect();
    if names.iter().any(|n| n == "GET") {
        names.push("HEAD".to_string());
    }
    names.sort();
    Matched::MethodNotAllowed(names.join(", "))
}

/// The rule of a route (3.12). A route with no rule gives `None`.
pub fn rule_of(id: RouteId) -> Option<Rule> {
    use Action::{ConfigRead, ConfigWrite, InventoryWrite, SecretWrite};
    use NodeSpec::{Environment as E, Host as H, Service as S, Unit as U};
    Some(match id {
        RouteId::Session | RouteId::Catalog | RouteId::Static | RouteId::Page { .. } => Rule::Access,
        // The two lists of 3.15 use the display filter. `GET .../hosts` asks `config.read` on E(e).
        RouteId::EnvironmentList | RouteId::UnitList => Rule::Display,
        RouteId::HostList => Rule::Use(ConfigRead, E),
        RouteId::EnvironmentCreate => Rule::Use(InventoryWrite, S),
        RouteId::DocGet(kind) | RouteId::DocHistory(kind) | RouteId::DocVersion(kind) => Rule::Use(
            ConfigRead,
            match kind {
                DocKind::Environment | DocKind::Host | DocKind::Common => E,
                DocKind::Unit | DocKind::UnitValues => U,
                DocKind::HostValues => H,
            },
        ),
        RouteId::DocPut(kind) | RouteId::DocDelete(kind) => match kind {
            DocKind::Environment | DocKind::Host | DocKind::Unit => Rule::Use(InventoryWrite, E),
            DocKind::Common => Rule::Use(ConfigWrite, E),
            DocKind::UnitValues => Rule::Use(ConfigWrite, U),
            DocKind::HostValues => Rule::Use(ConfigWrite, H),
        },
        // The node of this route depends on the parameter `host` (3.12): the handler builds it.
        RouteId::Effective => Rule::Use(ConfigRead, U),
        RouteId::SecretList(SecretRoute::Environment) => Rule::Use(ConfigRead, E),
        RouteId::SecretList(SecretRoute::Unit) => Rule::Use(ConfigRead, U),
        RouteId::SecretPut(SecretRoute::Environment) | RouteId::SecretDelete(SecretRoute::Environment) => {
            Rule::Use(SecretWrite, E)
        }
        RouteId::SecretPut(SecretRoute::Unit) | RouteId::SecretDelete(SecretRoute::Unit) => Rule::Use(SecretWrite, U),
        RouteId::RunCreate => Rule::RunRequest,
        RouteId::RunList => Rule::Display,
        RouteId::RunGet | RouteId::RunEvents | RouteId::RunOutput => Rule::RunRead,
        RouteId::RunCancel => Rule::RunCancel,
        RouteId::WellKnownActions => Rule::NoToken,
        RouteId::WellKnownResources => Rule::Options,
    })
}

/// True for the routes that `runner.enabled = false` turns off (3.3).
pub fn needs_runner(id: RouteId) -> bool {
    matches!(
        id,
        RouteId::RunCreate
            | RouteId::RunList
            | RouteId::RunGet
            | RouteId::RunEvents
            | RouteId::RunOutput
            | RouteId::RunCancel
            | RouteId::Page { runs: true }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(method: Method, path: &str) -> (RouteId, Params) {
        match route(&method, path) {
            Matched::Route(spec, params) => (spec.id, params),
            other => panic!("{path}: {other:?}"),
        }
    }

    #[test]
    fn the_base_path_is_removed_5_2() {
        assert_eq!(strip_base("/", "/api/session"), Some("api/session"));
        assert_eq!(strip_base("/", "/"), Some(""));
        assert_eq!(strip_base("/console/", "/console/api/session"), Some("api/session"));
        assert_eq!(strip_base("/console/", "/console/"), Some(""));
        assert_eq!(strip_base("/console/", "/api/session"), None);
        assert_eq!(strip_base("/console/", "/consolex/api"), None);
        assert_eq!(strip_base("/console/", "/console"), None);
    }

    #[test]
    fn a_route_has_its_parameters() {
        let (id, p) = found(Method::PUT, "api/environments/production/units/relay/hosts/host-a/values");
        assert_eq!(id, RouteId::DocPut(DocKind::HostValues));
        assert_eq!((p.get("e"), p.get("u"), p.get("h")), (Some("production"), Some("relay"), Some("host-a")));
        let (id, p) = found(Method::GET, "api/environments/p/common/history/12");
        assert_eq!(id, RouteId::DocVersion(DocKind::Common));
        assert_eq!(p.get("n"), Some("12"));
        let (id, p) = found(Method::GET, "static/abcdef123456/css/base.css");
        assert_eq!(id, RouteId::Static);
        assert_eq!((p.get("build"), p.get("file")), (Some("abcdef123456"), Some("css/base.css")));
        assert_eq!(found(Method::GET, "").0, RouteId::Page { runs: false });
        assert_eq!(found(Method::GET, "runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42").0, RouteId::Page { runs: true });
    }

    #[test]
    fn head_uses_the_get_route_5_3() {
        assert_eq!(found(Method::HEAD, "api/session").0, RouteId::Session);
        assert!(matches!(route(&Method::HEAD, "api/runs/x/cancel"), Matched::MethodNotAllowed(_)));
    }

    #[test]
    fn another_method_gives_the_allow_list_5_3() {
        let Matched::MethodNotAllowed(allow) = route(&Method::PATCH, "api/environments/p") else { panic!() };
        assert_eq!(allow, "DELETE, GET, HEAD, PUT");
        let Matched::MethodNotAllowed(allow) = route(&Method::OPTIONS, "api/session") else { panic!() };
        assert_eq!(allow, "GET, HEAD");
        let Matched::MethodNotAllowed(allow) = route(&Method::DELETE, "api/environments") else { panic!() };
        assert_eq!(allow, "GET, HEAD, POST");
    }

    #[test]
    fn an_unknown_path_has_a_region() {
        assert!(matches!(route(&Method::GET, "api/nothing"), Matched::NotFound(Region::Api)));
        assert!(matches!(route(&Method::GET, ".well-known/other"), Matched::NotFound(Region::Api)));
        assert!(matches!(route(&Method::GET, "some/page"), Matched::NotFound(Region::Page)));
        assert!(matches!(route(&Method::GET, "api//session"), Matched::NotFound(Region::Api)));
        assert!(matches!(route(&Method::GET, "api/session/"), Matched::NotFound(Region::Api)));
    }

    #[test]
    fn each_route_of_the_router_has_a_rule_3_12() {
        for spec in all() {
            assert!(rule_of(spec.id).is_some(), "{} has no rule", spec.label);
        }
    }

    #[test]
    fn no_two_routes_have_one_method_and_one_label() {
        let mut seen = std::collections::BTreeSet::new();
        for spec in all() {
            assert!(seen.insert(spec.label.clone()), "{}", spec.label);
        }
    }

    #[test]
    fn the_run_routes_are_the_routes_that_runner_enabled_false_turns_off_3_3() {
        for spec in all() {
            let runs = spec.label.contains("/runs") || spec.label.contains("commands") || spec.label.ends_with("/run");
            let is_api_or_page_of_runs = needs_runner(spec.id);
            assert_eq!(runs, is_api_or_page_of_runs, "{}", spec.label);
        }
    }
}
