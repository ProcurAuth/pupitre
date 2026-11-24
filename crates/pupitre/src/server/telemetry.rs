// SPDX-License-Identifier: MIT OR Apache-2.0
//! The spans (6.29): `http.request`, `access.decide`, `store.<method>`, `run.snapshot` and
//! `run.child`. A queue of `telemetry.max_queued_spans` spans waits for the export. Above it,
//! the new span is dropped and counted (6.28). A span attribute never holds a secret or a
//! value of a document (7.9).
//!
//! The export is OTLP over HTTP with the JSON encoding to `<endpoint>/v1/traces`, at the period
//! `telemetry.export_interval`. The resource has `service.name`. An `https` endpoint uses TLS with
//! the CA file of the configuration and no other root certificate (C8 of 6.14).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use pupitre_core::json::{self, Json};
use tokio::net::TcpStream;

use crate::config::TelemetryConfig;

/// The most spans in one export request.
pub const BATCH_SPANS: usize = 512;
/// The time that one export request can take.
pub const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct SpanRecord {
    trace_id: [u8; 16],
    span_id: [u8; 8],
    parent: Option<[u8; 8]>,
    name: &'static str,
    start_ns: u64,
    end_ns: u64,
    attrs: Vec<(&'static str, String)>,
    error: bool,
}

#[derive(Debug)]
struct Inner {
    service_name: String,
    max_queued: usize,
    queue: Mutex<VecDeque<SpanRecord>>,
    dropped: AtomicU64,
}

/// The span queue. A disabled handle costs nothing.
#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    inner: Option<Arc<Inner>>,
}

fn now_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    if getrandom::fill(&mut out).is_err() {
        // A span id needs no strong source. The clock is enough when the system source fails.
        let n = now_ns().to_le_bytes();
        for (i, b) in out.iter_mut().enumerate() {
            *b = n[i % 8];
        }
    }
    out
}

impl Telemetry {
    /// No span. The block `telemetry` has `enabled = false`.
    pub fn disabled() -> Telemetry {
        Telemetry { inner: None }
    }

    /// A queue for the export. The caller starts `export_task` for it.
    pub fn enabled(service_name: &str, max_queued: u32) -> Telemetry {
        Telemetry {
            inner: Some(Arc::new(Inner {
                service_name: service_name.to_string(),
                max_queued: usize::try_from(max_queued).unwrap_or(usize::MAX),
                queue: Mutex::new(VecDeque::new()),
                dropped: AtomicU64::new(0),
            })),
        }
    }

    /// True when spans are on.
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// The spans that the queue dropped (6.28).
    pub fn dropped(&self) -> u64 {
        self.inner.as_ref().map_or(0, |i| i.dropped.load(Ordering::Relaxed))
    }

    /// The spans that wait for the export.
    pub fn queued(&self) -> usize {
        self.inner.as_ref().map_or(0, |i| i.queue.lock().unwrap_or_else(PoisonError::into_inner).len())
    }

    /// Start the trace of a request. The root span is `http.request`.
    pub fn start_trace(&self, name: &'static str) -> Trace {
        Trace {
            telemetry: self.clone(),
            trace_id: random(),
            root: Mutex::new(Some(SpanBuilder::new(name, None, random()))),
        }
    }

    fn push(&self, record: SpanRecord) {
        let Some(inner) = &self.inner else { return };
        let mut queue = inner.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.len() >= inner.max_queued {
            inner.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        queue.push_back(record);
    }

    fn take(&self, max: usize) -> Vec<SpanRecord> {
        let Some(inner) = &self.inner else { return Vec::new() };
        let mut queue = inner.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let n = queue.len().min(max);
        queue.drain(..n).collect()
    }

    fn body(&self, spans: &[SpanRecord]) -> Vec<u8> {
        let Some(inner) = &self.inner else { return Vec::new() };
        let hex = |b: &[u8]| Json::str(pupitre_core::etag::to_hex(b));
        let text = |k: &str, v: &str| {
            Json::object([("key", Json::str(k)), ("value", Json::object([("stringValue", Json::str(v))]))])
        };
        let items: Vec<Json> = spans
            .iter()
            .map(|s| {
                let mut pairs = vec![
                    ("traceId", hex(&s.trace_id)),
                    ("spanId", hex(&s.span_id)),
                    ("name", Json::str(s.name)),
                    ("kind", Json::Int(if s.parent.is_none() { 2 } else { 1 })),
                    ("startTimeUnixNano", Json::str(s.start_ns.to_string())),
                    ("endTimeUnixNano", Json::str(s.end_ns.to_string())),
                    ("attributes", Json::Arr(s.attrs.iter().map(|(k, v)| text(k, v)).collect())),
                    ("status", Json::object([("code", Json::Int(if s.error { 2 } else { 1 }))])),
                ];
                if let Some(p) = &s.parent {
                    pairs.push(("parentSpanId", hex(p)));
                }
                Json::object(pairs)
            })
            .collect();
        let doc = Json::object([(
            "resourceSpans",
            Json::Arr(vec![Json::object([
                (
                    "resource",
                    Json::object([("attributes", Json::Arr(vec![text("service.name", &inner.service_name)]))]),
                ),
                (
                    "scopeSpans",
                    Json::Arr(vec![Json::object([
                        ("scope", Json::object([("name", Json::str("pupitre"))])),
                        ("spans", Json::Arr(items)),
                    ])]),
                ),
            ])]),
        )]);
        json::to_compact(&doc)
    }
}

#[derive(Debug)]
struct SpanBuilder {
    name: &'static str,
    parent: Option<[u8; 8]>,
    span_id: [u8; 8],
    start_ns: u64,
    attrs: Vec<(&'static str, String)>,
}

impl SpanBuilder {
    fn new(name: &'static str, parent: Option<[u8; 8]>, span_id: [u8; 8]) -> SpanBuilder {
        SpanBuilder { name, parent, span_id, start_ns: now_ns(), attrs: Vec::new() }
    }
}

/// The spans of one request.
#[derive(Debug)]
pub struct Trace {
    telemetry: Telemetry,
    trace_id: [u8; 16],
    root: Mutex<Option<SpanBuilder>>,
}

impl Trace {
    /// Add an attribute to the root span. A value is never a secret (7.9).
    pub fn attr(&self, key: &'static str, value: impl Into<String>) {
        if !self.telemetry.is_enabled() {
            return;
        }
        if let Some(root) = self.root.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            root.attrs.push((key, value.into()));
        }
    }

    /// Start a child span of the root span.
    pub fn child(&self, name: &'static str) -> Span {
        if !self.telemetry.is_enabled() {
            return Span { inner: None };
        }
        let parent = self.root.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|r| r.span_id);
        Span { inner: Some((self.telemetry.clone(), self.trace_id, SpanBuilder::new(name, parent, random()))) }
    }

    /// End the root span.
    pub fn end(&self, error: bool) {
        let Some(root) = self.root.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
        self.telemetry.push(SpanRecord {
            trace_id: self.trace_id,
            span_id: root.span_id,
            parent: None,
            name: root.name,
            start_ns: root.start_ns,
            end_ns: now_ns(),
            attrs: root.attrs,
            error,
        });
    }
}

/// A child span. `end` closes it. A span that nobody ends is dropped without a record.
#[derive(Debug)]
pub struct Span {
    inner: Option<(Telemetry, [u8; 16], SpanBuilder)>,
}

impl Span {
    /// Add an attribute. A value is never a secret (7.9).
    #[must_use]
    pub fn attr(mut self, key: &'static str, value: impl Into<String>) -> Span {
        if let Some((_, _, b)) = &mut self.inner {
            b.attrs.push((key, value.into()));
        }
        self
    }

    /// End the span.
    pub fn end(mut self, error: bool) {
        if let Some((telemetry, trace_id, b)) = self.inner.take() {
            telemetry.push(SpanRecord {
                trace_id,
                span_id: b.span_id,
                parent: b.parent,
                name: b.name,
                start_ns: b.start_ns,
                end_ns: now_ns(),
                attrs: b.attrs,
                error,
            });
        }
    }
}

/// The endpoint of the export, from `telemetry.otlp_endpoint` (6.13): the scheme, the host with
/// its port, and the path of the traces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// True for the scheme `https`.
    pub tls: bool,
    /// The host and the port, in the form `host:port`.
    pub authority: String,
    /// The host without the port.
    pub host: String,
}

impl Endpoint {
    /// Read the URL. Stage 2 checked its form (6.13).
    pub fn parse(url: &str) -> Option<Endpoint> {
        let (tls, rest) = match (url.strip_prefix("http://"), url.strip_prefix("https://")) {
            (Some(r), _) => (false, r),
            (_, Some(r)) => (true, r),
            _ => return None,
        };
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
            return None;
        }
        let default_port = if tls { "443" } else { "80" };
        let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
            let (addr, tail) = inner.split_once(']')?;
            let port = match tail {
                "" => default_port,
                t => t.strip_prefix(':').filter(|p| p.bytes().all(|b| b.is_ascii_digit()))?,
            };
            (format!("[{addr}]"), port.to_string())
        } else {
            match rest.rsplit_once(':') {
                Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
                    (h.to_string(), p.to_string())
                }
                _ => (rest.to_string(), default_port.to_string()),
            }
        };
        Some(Endpoint { tls, authority: format!("{host}:{port}"), host })
    }
}

/// The request of one batch: HTTP/1.1, `Connection: close`.
fn request_bytes(endpoint: &Endpoint, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "POST /v1/traces HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.authority,
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

/// The status of the answer, from its first line.
fn status_of(answer: &[u8]) -> Result<(), String> {
    let line = answer.split(|b| *b == b'\n').next().unwrap_or_default();
    let text = String::from_utf8_lossy(line);
    match text.split(' ').nth(1).and_then(|s| s.parse::<u16>().ok()) {
        Some(code) if (200..300).contains(&code) => Ok(()),
        Some(code) => Err(format!("the collector answered {code}")),
        None => Err("the collector gave no status".to_string()),
    }
}

/// Send one batch over TLS with the CA file of `telemetry.ca_file` and no other root certificate.
/// The call blocks, so the caller runs it in `spawn_blocking`.
fn post_tls(endpoint: &Endpoint, ca_pem: &[u8], body: &[u8]) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;

    use rustls::pki_types::pem::PemObject;
    let mut roots = rustls::RootCertStore::empty();
    for item in rustls::pki_types::CertificateDer::pem_slice_iter(ca_pem) {
        let der = item.map_err(|e| format!("the CA file: {e}"))?;
        roots.add(der).map_err(|e| format!("the CA file: {e}"))?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(endpoint.host.trim_matches(['[', ']']).to_string())
        .map_err(|e| format!("the host of the endpoint: {e}"))?;
    let connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| format!("tls: {e}"))?;
    let address = endpoint
        .authority
        .to_socket_addrs()
        .map_err(|e| format!("resolve: {e}"))?
        .next()
        .ok_or_else(|| "resolve: no address".to_string())?;
    let tcp = std::net::TcpStream::connect_timeout(&address, EXPORT_TIMEOUT).map_err(|e| format!("connect: {e}"))?;
    tcp.set_read_timeout(Some(EXPORT_TIMEOUT)).map_err(|e| format!("socket: {e}"))?;
    tcp.set_write_timeout(Some(EXPORT_TIMEOUT)).map_err(|e| format!("socket: {e}"))?;
    let mut tls = rustls::StreamOwned::new(connection, tcp);
    tls.write_all(&request_bytes(endpoint, body)).map_err(|e| format!("send: {e}"))?;
    let mut answer = Vec::new();
    // A collector can close with no close_notify: the bytes that came are enough.
    let _ = tls.read_to_end(&mut answer);
    status_of(&answer)
}

/// Send one batch to the collector: plain HTTP/1.1 for the scheme `http`, TLS for `https`.
async fn post(endpoint: &Endpoint, ca_pem: Option<&[u8]>, body: Vec<u8>) -> Result<(), String> {
    if endpoint.tls {
        let Some(ca) = ca_pem.map(<[u8]>::to_vec) else {
            return Err("an https endpoint needs a CA file".to_string());
        };
        let endpoint = endpoint.clone();
        return tokio::task::spawn_blocking(move || post_tls(&endpoint, &ca, &body))
            .await
            .map_err(|e| format!("export task: {e}"))?;
    }
    let stream = TcpStream::connect(&endpoint.authority).await.map_err(|e| format!("connect: {e}"))?;
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| format!("handshake: {e}"))?;
    let driver = tokio::spawn(async move {
        let _ = conn.await;
    });
    let request = hyper::Request::builder()
        .method("POST")
        .uri("/v1/traces")
        .header("host", endpoint.authority.as_str())
        .header("content-type", "application/json")
        .header("connection", "close")
        .body(Full::new(bytes::Bytes::from(body)))
        .map_err(|e| format!("request: {e}"))?;
    let response = sender.send_request(request).await.map_err(|e| format!("send: {e}"))?;
    let status = response.status();
    let _ = response.into_body().collect().await;
    driver.abort();
    if status.is_success() { Ok(()) } else { Err(format!("the collector answered {status}")) }
}

/// The task that exports the queue at the period of `telemetry.export_interval` (6.29). A failed
/// export drops the batch and writes the line `telemetry.export_failed` one time for each change
/// of state.
pub async fn export_task(
    telemetry: Telemetry,
    config: TelemetryConfig,
    logger: super::log::Logger,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let TelemetryConfig::Enabled { otlp_endpoint, export_interval, ca_pem, .. } = config else { return };
    let Some(endpoint) = Endpoint::parse(&otlp_endpoint) else { return };
    let mut failing = false;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(export_interval) => {}
            _ = stop.changed() => {
                if *stop.borrow() { return; }
            }
        }
        loop {
            let batch = telemetry.take(BATCH_SPANS);
            if batch.is_empty() {
                break;
            }
            let body = telemetry.body(&batch);
            let result = match tokio::time::timeout(EXPORT_TIMEOUT, post(&endpoint, ca_pem.as_deref(), body)).await {
                Ok(r) => r,
                Err(_) => Err("the export took too long".to_string()),
            };
            match (result, failing) {
                (Err(e), false) => {
                    failing = true;
                    logger.event(crate::config::LogLevel::Warn, "telemetry.export_failed").text("error", &e).emit();
                }
                (Ok(()), true) => {
                    failing = false;
                    logger.event(crate::config::LogLevel::Info, "telemetry.export_recovered").emit();
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_handle_makes_no_span() {
        let t = Telemetry::disabled();
        let trace = t.start_trace("http.request");
        trace.attr("route", "x");
        trace.child("store.get").end(false);
        trace.end(false);
        assert_eq!(t.queued(), 0);
        assert_eq!(t.dropped(), 0);
    }

    #[test]
    fn the_queue_drops_the_new_span_above_its_bound_and_counts_6_29() {
        let t = Telemetry::enabled("pupitre", 2);
        for _ in 0..5 {
            t.start_trace("http.request").end(false);
        }
        assert_eq!(t.queued(), 2);
        assert_eq!(t.dropped(), 3);
    }

    #[test]
    fn a_trace_gives_the_spans_of_6_29_with_one_trace_id() {
        let t = Telemetry::enabled("pupitre-test", 16);
        let trace = t.start_trace("http.request");
        trace.attr("route", "GET /api/session");
        trace.child("access.decide").attr("action", "config.read").end(false);
        trace.child("store.get").end(true);
        trace.end(false);
        let spans = t.take(10);
        assert_eq!(spans.len(), 3);
        assert!(spans.iter().all(|s| s.trace_id == spans[0].trace_id));
        let root = spans.iter().find(|s| s.parent.is_none()).unwrap();
        assert_eq!(root.name, "http.request");
        assert!(spans.iter().filter(|s| s.parent.is_some()).all(|s| s.parent == Some(root.span_id)));
        assert!(spans.iter().any(|s| s.name == "store.get" && s.error));
        let body = String::from_utf8(t.body(&spans)).unwrap();
        assert!(body.contains("\"service.name\""));
        assert!(body.contains("pupitre-test"));
        assert!(body.contains("\"name\":\"access.decide\""));
    }

    #[tokio::test]
    async fn an_https_endpoint_needs_a_ca_file_and_a_good_one() {
        let e = Endpoint::parse("https://127.0.0.1:1").unwrap();
        assert!(post(&e, None, Vec::new()).await.unwrap_err().contains("CA file"));
        let bad = post(&e, Some(b"not a pem"), Vec::new()).await;
        // Text that is not a PEM block holds no certificate: the handshake then cannot trust a server.
        assert!(bad.is_err());
    }

    #[test]
    fn the_status_of_an_answer() {
        assert!(status_of(b"HTTP/1.1 200 OK\r\n\r\n").is_ok());
        assert!(status_of(b"HTTP/1.1 204 No Content\r\n").is_ok());
        assert_eq!(status_of(b"HTTP/1.1 503 x\r\n").unwrap_err(), "the collector answered 503");
        assert!(status_of(b"").is_err());
    }

    #[tokio::test]
    async fn a_batch_goes_to_a_collector_over_http_6_29() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|v| v.trim().parse().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= len {
                        break;
                    }
                }
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        let t = Telemetry::enabled("pupitre-test", 8);
        t.start_trace("http.request").end(false);
        let batch = t.take(10);
        let e = Endpoint::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        post(&e, None, t.body(&batch)).await.unwrap();
        let got = server.await.unwrap();
        assert!(got.starts_with("POST /v1/traces HTTP/1.1\r\n"), "{got}");
        assert!(got.to_ascii_lowercase().contains("content-type: application/json"));
        assert!(got.contains("\"resourceSpans\""));
    }

    #[tokio::test]
    async fn the_export_task_sends_the_queue_and_logs_a_failure_one_time_6_29() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = seen.clone();
        // The first two requests get 500, the next ones get 200.
        let collector = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 65536];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") || got.len() < 400 {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    if got.ends_with(b"}") {
                        break;
                    }
                }
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let status = if n < 2 { "500 Internal Server Error" } else { "200 OK" };
                let _ = s
                    .write_all(
                        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
                    )
                    .await;
            }
        });
        let t = Telemetry::enabled("pupitre-test", 8);
        let (logger, log) = super::super::log::Logger::memory(crate::config::LogLevel::Debug);
        let config = TelemetryConfig::Enabled {
            otlp_endpoint: format!("http://127.0.0.1:{port}"),
            ca_pem: None,
            service_name: "pupitre-test".to_string(),
            export_interval: Duration::from_millis(30),
            max_queued_spans: 8,
        };
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(export_task(t.clone(), config, logger, stop_rx));
        for _ in 0..4 {
            t.start_trace("http.request").end(false);
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        let _ = stop_tx.send(true);
        let _ = task.await;
        collector.abort();
        assert!(seen.load(std::sync::atomic::Ordering::SeqCst) >= 3, "the task sent the batches");
        // Two failures in a row give one line; the recovery gives one line.
        assert_eq!(log.events("telemetry.export_failed").len(), 1);
        assert_eq!(log.events("telemetry.export_recovered").len(), 1);
        assert_eq!(t.queued(), 0);
    }

    #[test]
    fn the_endpoint_has_its_port() {
        let e = Endpoint::parse("http://collector.example.invalid:4318").unwrap();
        assert_eq!(
            (e.tls, e.authority.as_str(), e.host.as_str()),
            (false, "collector.example.invalid:4318", "collector.example.invalid")
        );
        let e = Endpoint::parse("https://collector.example.invalid").unwrap();
        assert_eq!((e.tls, e.authority.as_str()), (true, "collector.example.invalid:443"));
        assert!(Endpoint::parse("ftp://x").is_none());
    }
}
