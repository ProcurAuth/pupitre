// SPDX-License-Identifier: MIT OR Apache-2.0
//! The support code of the tests of the access layer: a fake PDP on the loopback address, with
//! or without TLS, and the helpers that make tokens and configurations.
//!
//! The fake PDP speaks the Access Evaluation API of AuthZEN (one question, or a batch) over
//! HTTP/1.1 with keep-alive. A policy closure gives the answer of each question. The fake PDP
//! records each HTTP request and each question, so a test can count them and compare them.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use procurauth_core::jws::{KeySet, SigningKey, sign_compact};
use pupitre::config::{AuthzenConfig, Secret};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::{Value, json};

pub const CA_A: &str = include_str!("../access_data/ca-a.crt");
pub const CA_B: &str = include_str!("../access_data/ca-b.crt");
pub const SERVER_CERT: &str = include_str!("../access_data/server.crt");
pub const SERVER_KEY: &str = include_str!("../access_data/server.pk8");
pub const P256_PUBLIC_PEM: &str = include_str!("../access_data/edge-p256.pub");

pub const AUDIENCE: &str = "pupitre";
pub const ISSUER: &str = "https://edge.example.invalid";
pub const HEADER: &str = "X-Identity-Token";
pub const CREDENTIAL: &str = "test-credential-0123";

/// One question that the fake PDP received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub subject: String,
    pub action: String,
    /// The chain and the node: `service:pupitre/environment:production`.
    pub resource: String,
    pub match_mode: String,
    pub usage: bool,
}

/// The answer of the policy to one question.
#[derive(Debug, Clone)]
pub struct Answer {
    pub decision: bool,
    pub reason: Option<String>,
    pub limits: Vec<(String, u64)>,
    /// A body that replaces the answer, for an answer that the library must refuse.
    pub raw: Option<Value>,
}

impl Answer {
    pub fn yes() -> Answer {
        Answer { decision: true, reason: None, limits: Vec::new(), raw: None }
    }
    pub fn no() -> Answer {
        Answer { decision: false, reason: None, limits: Vec::new(), raw: None }
    }
    pub fn no_because(reason: &str) -> Answer {
        Answer { decision: false, reason: Some(reason.to_owned()), limits: Vec::new(), raw: None }
    }
    pub fn with_limit(mut self, name: &str, value: u64) -> Answer {
        self.limits.push((name.to_owned(), value));
        self
    }
    pub fn raw(value: Value) -> Answer {
        Answer { decision: true, reason: None, limits: Vec::new(), raw: Some(value) }
    }
    fn to_value(&self) -> Value {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut context = serde_json::Map::new();
        if let Some(reason) = &self.reason {
            context.insert("reason".to_owned(), json!(reason));
        }
        if !self.limits.is_empty() {
            let limits: serde_json::Map<String, Value> =
                self.limits.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            context.insert("limits".to_owned(), Value::Object(limits));
        }
        if context.is_empty() {
            json!({"decision": self.decision})
        } else {
            json!({"decision": self.decision, "context": context})
        }
    }
}

/// What the fake PDP does with the next requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Answer each request with this HTTP status and an empty JSON object.
    Status(u16),
    /// Answer with 200 and a body that is not JSON.
    Garbage,
    /// Read the request and never answer.
    Hang,
}

type Policy = Box<dyn Fn(&Question) -> Answer + Send + Sync>;

/// One HTTP request that the fake PDP received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub path: String,
    pub authorization: Option<String>,
    pub body: Value,
}

struct Shared {
    policy: Mutex<Policy>,
    mode: Mutex<Mode>,
    delay_ms: AtomicU64,
    http: Mutex<Vec<Recorded>>,
    questions: Mutex<Vec<Question>>,
    connections: AtomicUsize,
    stop: AtomicBool,
}

pub struct FakePdp {
    pub addr: SocketAddr,
    pub tls: bool,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}

fn server_config() -> Arc<rustls::ServerConfig> {
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(SERVER_CERT.as_bytes()).map(|c| c.unwrap()).collect();
    let key = PrivateKeyDer::from_pem_slice(SERVER_KEY.as_bytes()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    Arc::new(config)
}

impl FakePdp {
    /// A fake PDP on a free loopback port. `policy` answers each question.
    pub fn start(policy: impl Fn(&Question) -> Answer + Send + Sync + 'static) -> FakePdp {
        FakePdp::spawn(false, Box::new(policy))
    }

    /// The same, with TLS: the certificate is for `localhost` and `127.0.0.1`, signed by `CA_A`.
    pub fn start_tls(policy: impl Fn(&Question) -> Answer + Send + Sync + 'static) -> FakePdp {
        FakePdp::spawn(true, Box::new(policy))
    }

    fn spawn(tls: bool, policy: Policy) -> FakePdp {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            policy: Mutex::new(policy),
            mode: Mutex::new(Mode::Normal),
            delay_ms: AtomicU64::new(0),
            http: Mutex::new(Vec::new()),
            questions: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let config = tls.then(server_config);
        let worker = shared.clone();
        let handle = std::thread::spawn(move || {
            while !worker.stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        worker.connections.fetch_add(1, Ordering::SeqCst);
                        let (shared, config) = (worker.clone(), config.clone());
                        std::thread::spawn(move || serve(socket, config, &shared));
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                    Err(_) => break,
                }
            }
        });
        FakePdp { addr, tls, shared, handle: Some(handle) }
    }

    pub fn url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        if self.tls {
            format!("{scheme}://localhost:{}", self.addr.port())
        } else {
            format!("{scheme}://{}", self.addr)
        }
    }

    pub fn set_mode(&self, mode: Mode) {
        *self.shared.mode.lock().unwrap() = mode;
    }

    pub fn set_delay(&self, delay: Duration) {
        self.shared.delay_ms.store(u64::try_from(delay.as_millis()).unwrap(), Ordering::SeqCst);
    }

    pub fn set_policy(&self, policy: impl Fn(&Question) -> Answer + Send + Sync + 'static) {
        *self.shared.policy.lock().unwrap() = Box::new(policy);
    }

    /// The HTTP requests so far, the metadata reads included.
    pub fn http_requests(&self) -> Vec<Recorded> {
        self.shared.http.lock().unwrap().clone()
    }

    /// The requests to the evaluation endpoints only.
    pub fn evaluation_requests(&self) -> Vec<Recorded> {
        self.http_requests().into_iter().filter(|r| r.path.starts_with("/access/v1/")).collect()
    }

    /// The questions so far, in order, one for each item of a batch.
    pub fn questions(&self) -> Vec<Question> {
        self.shared.questions.lock().unwrap().clone()
    }

    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    pub fn clear(&self) {
        self.shared.http.lock().unwrap().clear();
        self.shared.questions.lock().unwrap().clear();
    }
}

impl Drop for FakePdp {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn node_text(resource: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(list) = resource.pointer("/properties/ancestors").and_then(Value::as_array) {
        for a in list {
            parts.push(format!("{}:{}", a["type"].as_str().unwrap_or("?"), a["id"].as_str().unwrap_or("?")));
        }
    }
    parts.push(format!("{}:{}", resource["type"].as_str().unwrap_or("?"), resource["id"].as_str().unwrap_or("?")));
    parts.join("/")
}

fn question_of(envelope: &Value, item: &Value) -> Question {
    let pick = |pointer: &str| item.pointer(pointer).or_else(|| envelope.pointer(pointer));
    let resource = pick("/resource").cloned().unwrap_or(Value::Null);
    Question {
        subject: pick("/subject/id").and_then(Value::as_str).unwrap_or("").to_owned(),
        action: pick("/action/name").and_then(Value::as_str).unwrap_or("").to_owned(),
        resource: node_text(&resource),
        match_mode: pick("/context/match").and_then(Value::as_str).unwrap_or("node").to_owned(),
        usage: pick("/context/usage").is_some(),
    }
}

fn reply(status: u16, body: &[u8]) -> Vec<u8> {
    let mut out =
        format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len())
            .into_bytes();
    out.extend_from_slice(body);
    out
}

fn handle(request_line: &str, authorization: Option<String>, body: &[u8], shared: &Shared) -> Option<Vec<u8>> {
    let path = request_line.split(' ').nth(1).unwrap_or("").to_owned();
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    shared.http.lock().unwrap().push(Recorded { path: path.clone(), authorization, body: parsed.clone() });
    let delay = shared.delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay));
    }
    if path.starts_with("/.well-known/authzen-configuration") {
        return Some(reply(404, b"{}"));
    }
    match *shared.mode.lock().unwrap() {
        Mode::Hang => return None,
        Mode::Status(code) => return Some(reply(code, b"{}")),
        Mode::Garbage => return Some(reply(200, b"this is not json")),
        Mode::Normal => {}
    }
    let policy = shared.policy.lock().unwrap();
    let answers = if let Some(items) = parsed.get("evaluations").and_then(Value::as_array) {
        let mut out = Vec::new();
        for item in items {
            let q = question_of(&parsed, item);
            let answer = policy(&q).to_value();
            shared.questions.lock().unwrap().push(q);
            out.push(answer);
        }
        json!({"evaluations": out})
    } else {
        let q = question_of(&parsed, &parsed);
        let answer = policy(&q).to_value();
        shared.questions.lock().unwrap().push(q);
        answer
    };
    Some(reply(200, answers.to_string().as_bytes()))
}

fn serve(socket: TcpStream, config: Option<Arc<rustls::ServerConfig>>, shared: &Shared) {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = socket.set_nodelay(true);
    let mut stream: Box<dyn Stream> = match config {
        Some(config) => match rustls::ServerConnection::new(config) {
            Ok(conn) => Box::new(rustls::StreamOwned::new(conn, socket)),
            Err(_) => return,
        },
        None => Box::new(socket),
    };
    let mut buf: Vec<u8> = Vec::new();
    loop {
        // The head.
        let head_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if shared.stop.load(Ordering::SeqCst) {
                        return;
                    }
                }
                Err(_) => return,
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or("").to_owned();
        let mut length = 0usize;
        let mut authorization = None;
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
                } else if name.eq_ignore_ascii_case("authorization") {
                    authorization = Some(value.trim().to_owned());
                }
            }
        }
        let total = head_end + 4 + length;
        while buf.len() < total {
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if shared.stop.load(Ordering::SeqCst) {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
        let body = buf[head_end + 4..total].to_vec();
        buf.drain(..total);
        match handle(&request_line, authorization, &body, shared) {
            Some(bytes) => {
                if stream.write_all(&bytes).is_err() || stream.flush().is_err() {
                    return;
                }
            }
            None => {
                while !shared.stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                return;
            }
        }
    }
}

// ---- Keys, tokens and configurations ---------------------------------------------------------

pub fn ed_key() -> SigningKey {
    SigningKey::from_ed25519_seed(&[7; 32])
}

pub fn es_key() -> SigningKey {
    SigningKey::from_es256_scalar(&[9; 32]).unwrap()
}

/// The JWK Set of the edge: the Ed25519 key `ed-1` and the ES256 key `es-1`.
pub fn jwks_two() -> String {
    let set = KeySet::from_keys(vec![ed_key().public_key(Some("ed-1")), es_key().public_key(Some("es-1"))]);
    let keys: Vec<Value> = set.keys().iter().map(|k| k.to_jwk()).collect();
    json!({"keys": keys}).to_string()
}

/// The time of the tests: a fixed value for the claims, near the real clock.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// A token for `sub` with the claims of the configuration, signed by `key` with this `kid`.
pub fn token(key: &SigningKey, kid: Option<&str>, sub: &str) -> String {
    token_with(key, kid, |_| {}, sub)
}

pub fn token_with(key: &SigningKey, kid: Option<&str>, edit: impl FnOnce(&mut Value), sub: &str) -> String {
    let mut header = json!({"alg": key.algorithm().as_str(), "typ": "JWT"});
    if let Some(kid) = kid {
        header["kid"] = json!(kid);
    }
    let now = now();
    let mut claims = json!({"iss": ISSUER, "sub": sub, "aud": AUDIENCE, "iat": now, "exp": now + 120});
    edit(&mut claims);
    sign_compact(&header, &claims, key)
}

pub fn config(url: &str) -> AuthzenConfig {
    AuthzenConfig {
        header: HEADER.to_owned(),
        issuer: ISSUER.to_owned(),
        audience: AUDIENCE.to_owned(),
        public_keys: jwks_two(),
        max_age: Duration::from_secs(300),
        clock_skew: Duration::from_secs(30),
        pdp_url: url.to_owned(),
        pdp_token: Secret::new(CREDENTIAL.to_owned()),
        plain_http: url.starts_with("http://"),
        pdp_ca_pem: url.starts_with("https://").then(|| CA_A.as_bytes().to_vec()),
        pdp_timeout: Duration::from_secs(2),
        cache_ttl: Duration::ZERO,
        pdp_max_connections: 4,
        cache_entries: 100,
        options_max_questions: 200,
        options_page_items: 50,
    }
}
