// SPDX-License-Identifier: MIT OR Apache-2.0
//! The harness of the tests of the server: a fixture with a real listener, an in-memory store and
//! a fake or the open access layer, a small HTTP client, and the catalog of capabilities.

#![allow(dead_code)]

pub mod fake_access;
pub mod memstore;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http::header::HeaderName;
use pupitre::access::Access;
use pupitre::server::log::{Logger, MemoryLog};
use pupitre::server::state::{ModeSettings, RunnerSettings};
use pupitre::server::telemetry::Telemetry;
use pupitre::server::{AppState, Assets, Parts, Running, Settings, startup};
use pupitre::web_assets::Asset;
use pupitre_core::capability::{Capabilities, PlaybookKind, PlaybookSpec, SchemaSpec};
use pupitre_core::command::{CommandSpec, ParamSpec, ParamType, Target};
use pupitre_core::doc::{Document, Environment, Host, Unit, UnitPlaybooks};
use pupitre_core::ids::{CommandId, HostId, HostLabel, ParamName, PlaybookId, SchemaId};
use pupitre_core::json::{self, Json};
use pupitre_core::schema::UnitSchema;
use pupitre_runner::Wake;
use pupitre_store::catalog::CatalogLimits;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[allow(unused_imports)]
pub use fake_access::{Asked, FakeAccess, TOKEN_ALICE, TOKEN_BOB};
#[allow(unused_imports)]
pub use memstore::MemStore;

/// The header of the identity token of the fake access layer.
pub const TOKEN_HEADER: &str = "x-identity";

const RELAY: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
    {"default": 100, "description": "C.", "kind": "integer", "max": 1000, "min": 1, "path": "limits.max_clients"},
    {"default": "info", "description": "L.", "kind": "enum", "path": "log.level", "values": ["error", "warn", "info", "debug"]},
    {"description": "P.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true}
]}"#;

const FEED: &str = r#"{"format": 1, "group": "feed_hosts", "variable": "feed_config", "keys": [
    {"description": "U.", "kind": "text", "max": 64, "min": 1, "path": "feed.url", "rule": "any"},
    {"description": "L.", "kind": "enum", "path": "log.level", "values": ["warn", "info"]}
]}"#;

/// The catalog of capabilities of the tests: two schemas, six playbooks and two commands.
pub fn capabilities() -> Capabilities {
    let mut c = Capabilities::default();
    for (id, text) in [("relay", RELAY), ("feed", FEED)] {
        let id = SchemaId::parse(id).unwrap();
        c.schemas.insert(
            id.clone(),
            SchemaSpec { id, path: "/schemas/x.json".into(), schema: UnitSchema::parse(text.as_bytes()).unwrap() },
        );
    }
    for (name, kind) in [
        ("deploy-relay", PlaybookKind::Deploy),
        ("render-relay", PlaybookKind::Render),
        ("status-relay", PlaybookKind::Status),
        ("deploy-feed", PlaybookKind::Deploy),
        ("restart-service", PlaybookKind::Command),
        ("reload-service", PlaybookKind::Command),
    ] {
        let id = PlaybookId::parse(name).unwrap();
        c.playbooks.insert(id.clone(), PlaybookSpec { id, kind, path: format!("/playbooks/{name}.yml").into() });
    }
    let restart = CommandSpec {
        id: CommandId::parse("restart-service").unwrap(),
        label: "Restart the service".into(),
        playbook: PlaybookId::parse("restart-service").unwrap(),
        target: Target::Unit,
        check: false,
        params: vec![ParamSpec {
            name: ParamName::parse("grace").unwrap(),
            label: "Grace period".into(),
            required: true,
            ty: ParamType::Duration { min_ms: 1000, max_ms: 300_000 },
        }],
    };
    let reload = CommandSpec {
        id: CommandId::parse("reload-service").unwrap(),
        label: "Reload the service".into(),
        playbook: PlaybookId::parse("reload-service").unwrap(),
        target: Target::Host,
        check: true,
        params: vec![],
    };
    c.commands.insert(restart.id.clone(), restart);
    c.commands.insert(reload.id.clone(), reload);
    c
}

/// The documents of the tests: the environment `production`, the hosts `host-a` to `host-c`, and
/// the unit `relay` on `host-a` and `host-b`.
pub fn seed(store: &MemStore) {
    store.seed_doc("environments/production", &Document::Environment(Environment { label: "Production".into() }));
    for (id, addr, labels) in [
        ("host-a", "203.0.113.10", vec!["canary"]),
        ("host-b", "203.0.113.11", vec![]),
        ("host-c", "203.0.113.12", vec![]),
    ] {
        store.seed_doc(
            &format!("environments/production/hosts/{id}"),
            &Document::Host(Host {
                address: addr.into(),
                port: Some(22),
                labels: labels.into_iter().map(|l| HostLabel::parse(l).unwrap()).collect(),
            }),
        );
    }
    store.seed_doc(
        "environments/production/units/relay",
        &unit("Relay", "relay", "deploy-relay", &["host-a", "host-b"]),
    );
}

/// A document `unit` with the playbooks `render-relay` and `status-relay` when its schema is `relay`.
pub fn unit(label: &str, schema: &str, deploy: &str, hosts: &[&str]) -> Document {
    let relay = schema == "relay";
    Document::Unit(Unit {
        label: label.into(),
        schema: SchemaId::parse(schema).unwrap(),
        playbooks: UnitPlaybooks {
            deploy: PlaybookId::parse(deploy).unwrap(),
            render: relay.then(|| PlaybookId::parse("render-relay").unwrap()),
            status: relay.then(|| PlaybookId::parse("status-relay").unwrap()),
        },
        hosts: hosts.iter().map(|h| HostId::parse(h).unwrap()).collect(),
    })
}

/// A run record of the tests. The id comes from `n`, so a higher `n` is a newer run.
pub fn run_record(
    n: u64,
    kind: pupitre_core::run::RunKind,
    mode: pupitre_core::run::RunMode,
    status: pupitre_core::run::RunStatus,
    node: &str,
    hosts: &[&str],
) -> pupitre_core::run::RunRecord {
    use pupitre_core::run::RunKind;
    let node = pupitre_core::node::Node::parse_path(node).unwrap();
    let command = (kind == RunKind::Command).then(|| CommandId::parse("reload-service").unwrap());
    pupitre_core::run::RunRecord {
        id: pupitre_core::ids::RunId::from_parts(1_700_000_000_000 + n, [n as u8; 10]),
        kind,
        mode,
        environment: node.environment().unwrap().clone(),
        node,
        hosts: hosts.iter().map(|h| HostId::parse(h).unwrap()).collect(),
        playbook: PlaybookId::parse(if kind == RunKind::Command { "reload-service" } else { "deploy-relay" }).unwrap(),
        command,
        params: (kind == RunKind::Command).then(|| pupitre_core::command::CanonicalParams(Default::default())),
        check_run: None,
        applied_by: None,
        message: None,
        subject: pupitre_core::ids::Subject::parse("alice").unwrap(),
        created_at: pupitre_core::time::UtcMillis::now(),
        started_at: None,
        finished_at: status.is_final().then(pupitre_core::time::UtcMillis::now),
        status,
        reason: None,
        snapshot: None,
        runner: None,
        exit_code: None,
        stats: None,
        cancel: None,
        output_truncated: false,
        events_truncated: false,
    }
}

/// The static files of the tests.
pub static ASSETS: &[Asset] = &[
    Asset { path: "css/base.css", content_type: "text/css; charset=utf-8", bytes: b"body { margin: 0; }\n" },
    Asset { path: "js/app.js", content_type: "text/javascript; charset=utf-8", bytes: b"'use strict';\n" },
];

/// The shell of the tests, with the two markers of 5.23.
pub const SHELL: &str =
    "<!doctype html><base href=\"{{base_path}}\"><script src=\"static/{{build}}/js/app.js\"></script>";

/// The runner keys of the tests: 100 hosts, a check that stays 10 minutes, 3 queued runs.
pub fn runner() -> RunnerSettings {
    RunnerSettings { max_hosts: 3, check_max_age: Duration::from_secs(600), max_queued_runs: 3, execute: true }
}

/// The settings of the tests. The mode `open` has the runner on and the apply on.
pub fn settings(authzen: bool) -> Settings {
    Settings {
        base_path: "/".to_string(),
        allowed_hosts: pupitre::config::hosts::default_hosts("127.0.0.1:0".parse().unwrap()).unwrap(),
        max_connections: 64,
        idle_timeout: Duration::from_secs(5),
        shutdown_grace: Duration::from_millis(200),
        header_read_timeout: Duration::from_secs(2),
        body_read_timeout: Duration::from_secs(2),
        max_request_bytes: 65_536,
        max_inflight_bodies: 8,
        max_page_items: 5,
        mode: if authzen {
            ModeSettings::Authzen {
                header: HeaderName::from_static(TOKEN_HEADER),
                options_max_questions: 50,
                options_page_items: 4,
            }
        } else {
            ModeSettings::Open { allow_apply: true }
        },
        runner: Some(runner()),
        limits: CatalogLimits {
            max_environments: 4,
            max_units_per_environment: 4,
            max_hosts_per_environment: 6,
            max_document_bytes: 4096,
            max_store_bytes: 1 << 20,
        },
        version: "0.1.0",
    }
}

/// A server of the tests.
pub struct Fixture<A: Access + 'static> {
    /// The running server.
    pub running: Option<Running<A, MemStore>>,
    /// The state of the server.
    pub state: Arc<AppState<A, MemStore>>,
    /// The store.
    pub store: Arc<MemStore>,
    /// The address of the main listener.
    pub addr: SocketAddr,
    /// The address of the ops listener.
    pub ops: SocketAddr,
    /// The log lines.
    pub log: MemoryLog,
    /// The signal of the runner loop.
    pub wake: Wake,
}

/// Start a server on `127.0.0.1` with free ports. The store holds its documents before the start.
pub async fn start<A: Access + 'static>(access: A, settings: Settings, store: Arc<MemStore>) -> Fixture<A> {
    start_with(access, settings, store, capabilities(), Duration::from_millis(25)).await
}

/// Start a server with a catalog of capabilities and a period of the poll.
pub async fn start_with<A: Access + 'static>(
    access: A,
    settings: Settings,
    store: Arc<MemStore>,
    caps: Capabilities,
    poll: Duration,
) -> Fixture<A> {
    start_full(access, settings, store, caps, poll, Telemetry::disabled()).await
}

/// Start a server with a queue of spans.
pub async fn start_full<A: Access + 'static>(
    access: A,
    settings: Settings,
    store: Arc<MemStore>,
    caps: Capabilities,
    poll: Duration,
    telemetry: Telemetry,
) -> Fixture<A> {
    let catalog = startup::load_catalog(&*store, &settings).await.unwrap();
    let (logger, log) = Logger::memory(pupitre::config::LogLevel::Debug);
    let wake = Wake::default();
    let assets = Assets::new(ASSETS, SHELL, &settings.base_path);
    let state = Arc::new(AppState::new(Parts {
        settings,
        access,
        store: store.clone(),
        catalog,
        capabilities: Arc::new(caps),
        wake: wake.clone(),
        logger,
        telemetry,
        assets,
    }));
    let main = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ops = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let running = pupitre::server::start(state.clone(), main, ops, poll, None).await.unwrap();
    Fixture { addr: running.listen_addr, ops: running.ops_addr, running: Some(running), state, store, log, wake }
}

impl<A: Access + 'static> Fixture<A> {
    /// Send a request to the main listener.
    pub async fn send(&self, req: Req) -> Resp {
        send(self.addr, req).await
    }

    /// `GET` a path.
    pub async fn get(&self, path: &str) -> Resp {
        self.send(Req::get(path)).await
    }

    /// Send a request to the ops listener.
    pub async fn ops_get(&self, path: &str) -> Resp {
        send(self.ops, Req::get(path)).await
    }

    /// Stop the server.
    pub async fn stop(mut self) {
        if let Some(r) = self.running.take() {
            r.stop("test").await;
        }
    }
}

/// A request of the tests.
#[derive(Clone)]
pub struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    content_type: bool,
}

impl Req {
    fn new(method: &str, path: &str) -> Req {
        Req { method: method.into(), path: path.into(), headers: Vec::new(), body: None, content_type: true }
    }
    pub fn get(path: &str) -> Req {
        Req::new("GET", path)
    }
    pub fn head(path: &str) -> Req {
        Req::new("HEAD", path)
    }
    pub fn post(path: &str) -> Req {
        Req::new("POST", path)
    }
    pub fn put(path: &str) -> Req {
        Req::new("PUT", path)
    }
    pub fn delete(path: &str) -> Req {
        Req::new("DELETE", path)
    }
    pub fn method(path: &str, method: &str) -> Req {
        Req::new(method, path)
    }
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Req {
        self.headers.push((name.into(), value.into()));
        self
    }
    /// The identity token of the fake access layer.
    #[must_use]
    pub fn token(self, token: &str) -> Req {
        self.header(TOKEN_HEADER, token)
    }
    /// A JSON body, with `Content-Type: application/json`.
    #[must_use]
    pub fn json(mut self, text: &str) -> Req {
        self.body = Some(text.as_bytes().to_vec());
        self
    }
    /// A body with no `Content-Type`.
    #[must_use]
    pub fn raw_body(mut self, bytes: &[u8]) -> Req {
        self.body = Some(bytes.to_vec());
        self.content_type = false;
        self
    }
    #[must_use]
    pub fn if_match(self, etag: &str) -> Req {
        self.header("If-Match", &format!("\"{etag}\""))
    }
    #[must_use]
    pub fn create(self) -> Req {
        self.header("If-None-Match", "*")
    }
}

/// A response of the tests.
#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn headers_named(&self, name: &str) -> Vec<&str> {
        self.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    /// The body as JSON.
    pub fn json(&self) -> Json {
        json::parse(&self.body, json::ReadLimits::bytes(1 << 24)).unwrap_or_else(|e| panic!("{e}: {}", self.text()))
    }
    /// The member of a JSON object body.
    pub fn member(&self, name: &str) -> Json {
        self.json()
            .as_object()
            .and_then(|m| m.get(name))
            .cloned()
            .unwrap_or_else(|| panic!("no {name}: {}", self.text()))
    }
    /// The code of an error body.
    pub fn error(&self) -> String {
        self.member("error").as_str().unwrap_or_default().to_string()
    }
    /// The ETag header without its quotes.
    pub fn etag(&self) -> String {
        self.header("etag").unwrap_or_default().trim_matches('"').to_string()
    }
}

fn parse_response(raw: &[u8]) -> Resp {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no head in {:?}", String::from_utf8_lossy(raw)));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let body = raw[split + 4..].to_vec();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers =
        lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_string(), v.trim().to_string())).collect();
    Resp { status, headers, body }
}

/// Send raw bytes and read the answer to the end of the connection.
pub async fn raw(addr: SocketAddr, bytes: &[u8]) -> Resp {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(bytes).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut out)).await;
    parse_response(&out)
}

/// The bytes of a request of the tests, with the header `Host` of `addr`.
pub fn request_bytes(addr: SocketAddr, req: &Req) -> Vec<u8> {
    request_bytes_with_host(&addr.to_string(), req)
}

/// The bytes of a request of the tests, with a given value of the header `Host`.
pub fn request_bytes_with_host(host: &str, req: &Req) -> Vec<u8> {
    let mut text = format!("{} {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n", req.method, req.path);
    let has = |n: &str| req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
    for (k, v) in &req.headers {
        text.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(body) = &req.body {
        if req.content_type && !has("content-type") {
            text.push_str("Content-Type: application/json\r\n");
        }
        text.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    text.push_str("\r\n");
    let mut bytes = text.into_bytes();
    if let Some(body) = &req.body {
        bytes.extend_from_slice(body);
    }
    bytes
}

/// Send a request and read the answer.
pub async fn send(addr: SocketAddr, req: Req) -> Resp {
    let mut resp = raw(addr, &request_bytes(addr, &req)).await;
    if req.method == "HEAD" {
        resp.body.clear();
    }
    resp
}

/// Send a request with a given value of the header `Host` and read the answer.
pub async fn send_with_host(addr: SocketAddr, host: &str, req: Req) -> Resp {
    let mut resp = raw(addr, &request_bytes_with_host(host, &req)).await;
    if req.method == "HEAD" {
        resp.body.clear();
    }
    resp
}

/// Send a request, close the write side of the connection at once, and read nothing. The client
/// is gone before the server answers.
pub async fn send_and_half_close(addr: SocketAddr, req: Req) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&request_bytes(addr, &req)).await.unwrap();
    stream.shutdown().await.unwrap();
    drop(stream);
}

/// A compact JSON text from a value.
pub fn compact(value: &Json) -> String {
    String::from_utf8(json::to_compact(value)).unwrap()
}

/// The body of a write: `{"document": <doc>, "message": <msg>}`.
pub fn write_body(doc: &Document, message: &str) -> String {
    compact(&Json::object([("document", doc.to_json()), ("message", Json::str(message))]))
}

/// A server of the mode `open` with the documents of `seed`.
pub async fn open_fixture() -> Fixture<pupitre::access::AnyAccess> {
    let store = Arc::new(MemStore::new());
    seed(&store);
    start(pupitre::access::AnyAccess::Open(pupitre::access::OpenAccess), settings(false), store).await
}

/// A server of the mode `authzen` with the fake access layer.
pub async fn fake_fixture() -> (Fixture<FakeAccess>, FakeAccess) {
    let store = Arc::new(MemStore::new());
    seed(&store);
    let fake = FakeAccess::allow_all();
    (start(fake.clone(), settings(true), store).await, fake)
}
