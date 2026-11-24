// SPDX-License-Identifier: MIT OR Apache-2.0
//! The connections (7.10). This file has the accept loop of a listener and the clock of a
//! connection for the idle time and the header time. It refuses a connection above
//! `server.max_connections`.

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::error::{ApiError, Code};
use super::headers::{self, new_request_id};
use super::limits::{MAX_REFUSAL_TASKS, PARSER_HEAD_BYTES, PARSER_MAX_HEADERS, REFUSAL_WRITE_TIMEOUT};
use super::pipeline;
use super::response::Response;
use super::state::AppState;
use crate::access::Access;
use pupitre_store::Store;

struct Times {
    last_activity: Instant,
    /// The start of the head of the request that the server reads now. `None` while no head is
    /// on its way.
    head_started: Option<Instant>,
}

/// The activity of one connection, shared by the stream wrapper and the service.
pub struct ConnShared {
    times: Mutex<Times>,
    in_flight: AtomicU32,
}

/// Why the server closed a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The peer or the protocol ended it.
    Done,
    /// The kept connection was idle for `server.idle_timeout`.
    Idle,
    /// The headers did not arrive in `server.header_read_timeout`.
    HeaderTimeout,
    /// The graceful stop closed it.
    Stopping,
}

enum Verdict {
    /// Check again at this instant.
    Wait(Instant),
    Idle,
    HeaderTimeout,
}

impl ConnShared {
    /// The clock of a new connection. The first head has the header time from now.
    pub fn new() -> Arc<ConnShared> {
        let now = Instant::now();
        Arc::new(ConnShared {
            times: Mutex::new(Times { last_activity: now, head_started: Some(now) }),
            in_flight: AtomicU32::new(0),
        })
    }

    fn times(&self) -> std::sync::MutexGuard<'_, Times> {
        self.times.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn on_read(&self) {
        let now = Instant::now();
        let mut t = self.times();
        t.last_activity = now;
        if self.in_flight.load(Ordering::SeqCst) == 0 && t.head_started.is_none() {
            t.head_started = Some(now);
        }
    }

    fn on_write(&self) {
        self.times().last_activity = Instant::now();
    }

    /// The service calls it when a request head is complete.
    pub fn request_started(&self) {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let mut t = self.times();
        t.head_started = None;
        t.last_activity = Instant::now();
    }

    /// The service calls it when the answer is ready.
    pub fn request_finished(&self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.times().last_activity = Instant::now();
    }

    fn verdict(&self, idle: Duration, header: Duration) -> Verdict {
        let now = Instant::now();
        if self.in_flight.load(Ordering::SeqCst) > 0 {
            return Verdict::Wait(now + Duration::from_millis(500));
        }
        let t = self.times();
        let (deadline, on_expiry) = match t.head_started {
            Some(start) => (start + header, Verdict::HeaderTimeout),
            None => (t.last_activity + idle, Verdict::Idle),
        };
        if now >= deadline { on_expiry } else { Verdict::Wait(deadline) }
    }
}

/// A stream that tells the clock about each read and write.
struct TimedIo {
    inner: TcpStream,
    shared: Arc<ConnShared>,
}

impl AsyncRead for TimedIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &poll
            && buf.filled().len() > before
        {
            self.shared.on_read();
        }
        poll
    }
}

impl AsyncWrite for TimedIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &poll
            && *n > 0
        {
            self.shared.on_write();
        }
        poll
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(n)) = &poll
            && *n > 0
        {
            self.shared.on_write();
        }
        poll
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// The times of a connection.
#[derive(Debug, Clone, Copy)]
pub struct ConnTimes {
    /// The idle time of a kept connection.
    pub idle: Duration,
    /// The time to read a head.
    pub header: Duration,
}

/// Run one connection with hyper until it ends. `handler` makes the answer of each request. A
/// `true` on `close_rx` starts a graceful close.
pub async fn run_connection<F, Fut>(
    stream: TcpStream,
    shared: Arc<ConnShared>,
    handler: F,
    times: ConnTimes,
    mut close_rx: watch::Receiver<bool>,
) -> CloseReason
where
    F: Fn(hyper::Request<Incoming>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Response, Infallible>> + Send + 'static,
{
    let io = TokioIo::new(TimedIo { inner: stream, shared: shared.clone() });
    let mut builder = http1::Builder::new();
    builder.keep_alive(true).header_read_timeout(None).max_buf_size(PARSER_HEAD_BYTES).max_headers(PARSER_MAX_HEADERS);
    let conn = builder.serve_connection(io, service_fn(handler));
    tokio::pin!(conn);
    let mut watching_close = true;
    loop {
        let wake = match shared.verdict(times.idle, times.header) {
            Verdict::Wait(at) => tokio::time::Instant::from_std(at),
            Verdict::Idle => return CloseReason::Idle,
            Verdict::HeaderTimeout => return CloseReason::HeaderTimeout,
        };
        tokio::select! {
            _ = conn.as_mut() => return CloseReason::Done,
            changed = close_rx.changed(), if watching_close => {
                if changed.is_err() {
                    watching_close = false;
                } else if *close_rx.borrow() {
                    conn.as_mut().graceful_shutdown();
                    let _ = conn.as_mut().await;
                    return CloseReason::Stopping;
                }
            }
            _ = tokio::time::sleep_until(wake) => {}
        }
    }
}

struct ConnGuard<A: Access, S: Store>(Arc<AppState<A, S>>);

impl<A: Access, S: Store> Drop for ConnGuard<A, S> {
    fn drop(&mut self) {
        self.0.connection_closed();
    }
}

struct RefusalGuard(Arc<AtomicUsize>);

impl Drop for RefusalGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The text of an HTTP/1.1 refusal answer for an error, with the headers of 5.3 and 7.5.
pub fn raw_error_answer(e: &ApiError, request_id: &str) -> Vec<u8> {
    let body = e.body();
    let status = e.code.status();
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        body.len()
    );
    if let Some(secs) = e.code.retry_after() {
        head.push_str(&format!("Retry-After: {secs}\r\n"));
    }
    let mut security = http::HeaderMap::new();
    headers::add_security_headers(&mut security);
    for (name, value) in &security {
        if let Ok(v) = value.to_str() {
            head.push_str(&format!("{}: {v}\r\n", name.as_str()));
        }
    }
    head.push_str(&format!("X-Request-Id: {request_id}\r\n\r\n"));
    let mut out = head.into_bytes();
    out.extend_from_slice(&body);
    out
}

/// Answer `503 server_busy` on a connection above the limit and close it (7.10).
async fn refuse(mut stream: TcpStream) {
    let answer = raw_error_answer(&ApiError::new(Code::ServerBusy), &new_request_id());
    let work = async {
        let _ = stream.write_all(&answer).await;
        let _ = stream.shutdown().await;
        // Read what the client already sent, so that the close does not reset the connection
        // before the client reads the answer.
        let mut sink = [0u8; 1024];
        let _ = tokio::time::timeout(Duration::from_millis(200), stream.read(&mut sink)).await;
    };
    let _ = tokio::time::timeout(REFUSAL_WRITE_TIMEOUT, work).await;
}

/// The accept loop of `server.listen`. It ends when `stop_accept` becomes `true`; it then drops
/// the listener. The connections that are open stay open until the graceful stop closes them.
pub async fn accept_loop<A: Access + 'static, S: Store + 'static>(
    listener: TcpListener,
    state: Arc<AppState<A, S>>,
    mut stop_accept: watch::Receiver<bool>,
    close_connections: watch::Receiver<bool>,
) {
    let refusals = Arc::new(AtomicUsize::new(0));
    let max = u64::from(state.settings.max_connections);
    let times = ConnTimes { idle: state.settings.idle_timeout, header: state.settings.header_read_timeout };
    loop {
        let accepted = tokio::select! {
            r = listener.accept() => r,
            _ = stop_accept.changed() => {
                if *stop_accept.borrow() { return; }
                continue;
            }
        };
        let Ok((stream, peer)) = accepted else {
            // A fault such as too many open files. Wait a short time and try again.
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let _ = stream.set_nodelay(true);
        let open = state.connection_opened();
        if open > max || state.inflight_requests() > max {
            state.connection_closed();
            if refusals.fetch_add(1, Ordering::SeqCst) < MAX_REFUSAL_TASKS {
                let guard = RefusalGuard(refusals.clone());
                tokio::spawn(async move {
                    let _guard = guard;
                    refuse(stream).await;
                });
            } else {
                refusals.fetch_sub(1, Ordering::SeqCst);
                drop(stream);
            }
            continue;
        }
        let guard = ConnGuard(state.clone());
        let shared = ConnShared::new();
        let st = state.clone();
        let sh = shared.clone();
        let close_rx = close_connections.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let handler =
                move |req: hyper::Request<Incoming>| pipeline::serve_request(st.clone(), sh.clone(), peer, req);
            run_connection(stream, shared, handler, times, close_rx).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_raw_answer_has_the_headers_of_5_3_and_7_5() {
        let text = String::from_utf8(raw_error_answer(&ApiError::new(Code::ServerBusy), "rid")).unwrap();
        assert!(text.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
        for want in [
            "Retry-After: 1\r\n",
            "Connection: close\r\n",
            "content-security-policy: ",
            "x-content-type-options: nosniff\r\n",
            "X-Request-Id: rid\r\n",
            "Cache-Control: no-store\r\n",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let len: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
        assert_eq!(body.len(), len);
        assert!(body.contains("\"server_busy\""));
    }

    #[test]
    fn the_clock_follows_the_rules_of_7_10() {
        let c = ConnShared::new();
        let idle = Duration::from_secs(60);
        let header = Duration::from_secs(10);
        // A new connection waits for its first head.
        assert!(matches!(c.verdict(idle, header), Verdict::Wait(_)));
        assert!(matches!(c.verdict(idle, Duration::ZERO), Verdict::HeaderTimeout));
        c.request_started();
        assert!(matches!(c.verdict(Duration::ZERO, Duration::ZERO), Verdict::Wait(_)));
        c.request_finished();
        assert!(matches!(c.verdict(Duration::ZERO, header), Verdict::Idle));
        // A byte of the next head starts the header clock.
        c.on_read();
        assert!(matches!(c.verdict(Duration::ZERO, Duration::ZERO), Verdict::HeaderTimeout));
    }
}
