// SPDX-License-Identifier: MIT OR Apache-2.0
//! The service under test of the series U of the ProcurAuth conformance suite (3.24, 8.12).
//!
//! The test `serve` is ignored: it serves until the script `tests/conformance/run.sh` stops it.
//! It is a small HTTP service that uses only the access layer of Pupitre in the mode `authzen`.
//! It has the routes that the profile `tests/conformance/pep-profile.json` names: three routes of
//! uses, the action document, the options route and the reset route. It reads the profile of the
//! runner, so that the keys, the issuer, the credential and the address of the PDP have one source.
//!
//! The test `the_profile_matches_the_service` runs with the other tests. It checks that the
//! profile and the routes of this file agree.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use procurauth_core::jws::SigningKey;
use pupitre::access::options::{OptionsError, OptionsTree, options_route};
use pupitre::access::{Access, AnyAccess, AuthzenAccess, CachePolicy, MatchMode, Question, Refusal};
use pupitre::config::{AuthzenConfig, Secret};
use pupitre_core::action::Action;
use pupitre_core::json::to_compact;
use pupitre_core::node::{Node, NodeType};
use serde_json::{Value, json};

const MAX_HOSTS: u32 = 100;

/// The tree of the options route: two environments, one unit each, one host each.
struct Tree;

impl OptionsTree for Tree {
    fn children(&self, parent: Option<&Node>, child_type: NodeType) -> Vec<(String, String)> {
        let one = |id: &str, label: &str| vec![(id.to_owned(), label.to_owned())];
        match (parent, child_type) {
            (None, NodeType::Environment) => {
                vec![("production".to_owned(), "Production".to_owned()), ("staging".to_owned(), "Staging".to_owned())]
            }
            (Some(Node::Environment(_)), NodeType::Unit) => one("relay", "Relay"),
            (Some(Node::Unit(..)), NodeType::Host) => one("host-a", "host-a"),
            _ => Vec::new(),
        }
    }
}

struct State {
    config: AuthzenConfig,
    access: Mutex<Arc<AnyAccess>>,
}

impl State {
    fn access(&self) -> Arc<AnyAccess> {
        self.access.lock().unwrap().clone()
    }

    /// Build the access layer again: no kept answer stays (the reset route of the profile).
    fn reset(&self) {
        *self.access.lock().unwrap() = Arc::new(build(&self.config));
    }
}

fn build(config: &AuthzenConfig) -> AnyAccess {
    AnyAccess::Authzen(Box::new(AuthzenAccess::new(config, true, u64::from(MAX_HOSTS)).unwrap()))
}

/// Read a file of the profile: the path starts at the folder of the profile. The text has no last line end.
fn read_file(dir: &Path, name: &str) -> String {
    let text = std::fs::read_to_string(dir.join(name)).unwrap();
    text.strip_suffix('\n').unwrap_or(&text).to_owned()
}

/// The configuration of the access layer, from the profile of the runner.
fn config_of(profile: &Value, dir: &Path) -> AuthzenConfig {
    let pep = &profile["pep"];
    let mut keys = Vec::new();
    for key in pep["token"]["keys"].as_array().unwrap() {
        let signing = SigningKey::from_pkcs8_pem(&read_file(dir, key["file"].as_str().unwrap())).unwrap();
        keys.push(signing.public_key(key["kid"].as_str()).to_jwk());
    }
    AuthzenConfig {
        header: pep["token"]["header"].as_str().unwrap().to_owned(),
        issuer: pep["token"]["iss"].as_str().unwrap().to_owned(),
        audience: pep["service"].as_str().unwrap().to_owned(),
        public_keys: json!({"keys": keys}).to_string(),
        max_age: Duration::from_secs(pep["token"]["max_age_seconds"].as_u64().unwrap()),
        clock_skew: Duration::from_secs(pep["token"]["skew_seconds"].as_u64().unwrap()),
        pdp_url: format!("http://{}", pep["pdp"]["listen"].as_str().unwrap()),
        pdp_token: Secret::new(read_file(dir, pep["pdp"]["credential_file"].as_str().unwrap())),
        plain_http: true,
        pdp_ca_pem: None,
        pdp_timeout: Duration::from_secs(2),
        // The series U tests no kept answer of Pupitre, and a kept answer would hide a question.
        cache_ttl: Duration::ZERO,
        pdp_max_connections: 4,
        cache_entries: 100,
        options_max_questions: 200,
        options_page_items: 50,
    }
}

fn profile_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance")
}

fn json_reply(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .body(Full::new(Bytes::from(body.to_owned())))
        .unwrap()
}

/// The map of a refusal to the HTTP answer (3.21). The body never holds the reason of the PDP.
fn refusal_reply(refusal: &Refusal) -> Response<Full<Bytes>> {
    match refusal {
        Refusal::Unauthenticated => json_reply(StatusCode::UNAUTHORIZED, r#"{"error":"unauthenticated"}"#),
        Refusal::Forbidden { action } => {
            json_reply(StatusCode::FORBIDDEN, &format!(r#"{{"action":"{action}","error":"forbidden"}}"#))
        }
        Refusal::ApprovalCannotWait => json_reply(StatusCode::FORBIDDEN, r#"{"error":"approval_cannot_wait"}"#),
        Refusal::NotFound => json_reply(StatusCode::NOT_FOUND, r#"{"error":"not_found"}"#),
        Refusal::PdpUnavailable => json_reply(StatusCode::SERVICE_UNAVAILABLE, r#"{"error":"pdp_unavailable"}"#),
    }
}

async fn use_route(
    access: &AnyAccess,
    subject: &pupitre_core::ids::Subject,
    action: Action,
    node: Node,
    policy: CachePolicy,
) -> Response<Full<Bytes>> {
    let question = Question { action, node, match_mode: MatchMode::Node };
    match access.decide(subject, &question, policy).await {
        Err(refusal) => refusal_reply(&refusal),
        Ok(granted) => {
            // The run of one host is the operation of the route: the effective bound of 3.10.
            let bound = granted.max_hosts.map_or(u64::from(MAX_HOSTS), |m| m.min(u64::from(MAX_HOSTS)));
            if bound < 1 {
                return json_reply(StatusCode::FORBIDDEN, r#"{"error":"ceiling_exceeded"}"#);
            }
            json_reply(StatusCode::OK, "{}")
        }
    }
}

async fn handle(state: Arc<State>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let access = state.access();
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    // The action document needs no token and asks nothing (3.11).
    if path == "/.well-known/procurauth/actions" {
        return Ok(match (&method, access.action_document()) {
            (&Method::GET, Some(bytes)) => json_reply(StatusCode::OK, std::str::from_utf8(bytes).unwrap()),
            (&Method::HEAD, Some(_)) => json_reply(StatusCode::OK, ""),
            _ => json_reply(StatusCode::NOT_FOUND, r#"{"error":"not_found"}"#),
        });
    }
    if path == "/conformance/reset" && method == Method::POST {
        state.reset();
        return Ok(json_reply(StatusCode::OK, "{}"));
    }
    // Each other route needs a valid token from the configured header (3.4).
    let values: Vec<&[u8]> = req.headers().get_all(state.config.header.as_str()).iter().map(|v| v.as_bytes()).collect();
    let subject = match access.authenticate(&values) {
        Ok(subject) => subject,
        Err(refusal) => return Ok(refusal_reply(&refusal)),
    };
    let reply = match (method.as_str(), path.as_str()) {
        ("GET", "/.well-known/procurauth/resources") => {
            match options_route(
                &*access,
                &subject,
                &query,
                &Tree,
                true,
                state.config.options_max_questions,
                state.config.options_page_items,
            )
            .await
            {
                Ok(answer) => json_reply(StatusCode::OK, std::str::from_utf8(&to_compact(&answer)).unwrap()),
                Err(OptionsError::InvalidRequest(_)) => {
                    json_reply(StatusCode::BAD_REQUEST, r#"{"error":"invalid_options_request"}"#)
                }
                Err(OptionsError::PdpUnavailable) => refusal_reply(&Refusal::PdpUnavailable),
            }
        }
        // A read route: a kept answer can serve (3.17).
        ("GET", "/conformance/plain") => {
            let node = Node::parse_path("environment:production/unit:relay/host:host-a").unwrap();
            use_route(&access, &subject, Action::ConfigRead, node, CachePolicy::MayUseKept).await
        }
        // A run route: never a kept answer, and the ceiling `max_hosts` applies (3.10).
        ("GET", "/conformance/ceiling") => {
            let node = Node::parse_path("environment:production/unit:relay").unwrap();
            use_route(&access, &subject, Action::RunApply, node, CachePolicy::Fresh).await
        }
        ("GET", "/conformance/service") => {
            use_route(&access, &subject, Action::InventoryWrite, Node::Service, CachePolicy::Fresh).await
        }
        _ => json_reply(StatusCode::NOT_FOUND, r#"{"error":"not_found"}"#),
    };
    Ok(reply)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "serves until it is stopped: tests/conformance/run.sh starts it"]
async fn serve() {
    let profile_path = std::env::var("PUPITRE_CONFORMANCE_PROFILE")
        .expect("PUPITRE_CONFORMANCE_PROFILE names the profile of the runner");
    let listen =
        std::env::var("PUPITRE_CONFORMANCE_LISTEN").expect("PUPITRE_CONFORMANCE_LISTEN gives the address to serve");
    let profile_path = PathBuf::from(profile_path);
    let profile: Value = serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
    let dir = profile_path.parent().unwrap().to_owned();
    let config = config_of(&profile, &dir);
    let state = Arc::new(State { access: Mutex::new(Arc::new(build(&config))), config });
    let listener = tokio::net::TcpListener::bind(&listen).await.unwrap();
    println!("conformance service: listening on {listen}");
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let state = state.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req| handle(state.clone(), req));
            let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, service).await;
        });
    }
}

/// The profile of the runner and the routes of this file agree, and the access layer builds from it.
#[test]
fn the_profile_matches_the_service() {
    let dir = profile_dir();
    let profile: Value = serde_json::from_slice(&std::fs::read(dir.join("pep-profile.json")).unwrap()).unwrap();
    let config = config_of(&profile, &dir);
    assert_eq!(config.audience, "pupitre");
    let access = build(&config);
    assert!(access.action_document().is_some());
    let ops = &profile["pep"]["operations"];
    assert_eq!(ops["use_plain"]["path"], "/conformance/plain");
    assert_eq!(ops["use_ceiling"]["path"], "/conformance/ceiling");
    assert_eq!(ops["use_service"]["path"], "/conformance/service");
    assert_eq!(profile["pep"]["reset"]["path"], "/conformance/reset");
    // The operations name actions that the document publishes.
    let document: Value = serde_json::from_slice(access.action_document().unwrap()).unwrap();
    for op in ["use_plain", "use_ceiling", "use_service"] {
        let name = ops[op]["action"].as_str().unwrap();
        assert!(document["actions"].as_array().unwrap().iter().any(|a| a["name"] == name), "{op}: {name}");
    }
    // The ceiling of the profile is a limit of the document, and the value is the one the service runs with.
    assert_eq!(document["limits"][0]["name"], ops["use_ceiling"]["ceiling"]);
    assert_eq!(ops["use_ceiling"]["ceiling_value"], MAX_HOSTS);
}
