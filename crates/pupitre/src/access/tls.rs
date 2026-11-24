// SPDX-License-Identifier: MIT OR Apache-2.0
//! The link to the PDP (3.5): HTTP/1.1 over TCP, with rustls and the `ring` provider for `https`.
//!
//! The PEP library is synchronous and has no TLS code at the pinned commit (8.9). This type
//! implements its `PdpTransport`. The trust roots are the certificates of `access.pdp.ca_file`
//! only. The transport reads no environment variable, follows no redirect and uses no proxy.
//! It keeps idle connections in a small pool, and it closes an idle connection after the idle
//! time that the PEP gives (P 10.7).
//!
//! The code comes from the transport of Hune (same owner, same licence). The changes are these.
//! It has no metrics and no trace headers. It counts the requests of the current thread. Thus the
//! caller can tell a kept answer from an answer of the PDP.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use procurauth_pep::transport::{HttpResponse, PdpTransport, TransportError};
use procurauth_pep::url::{Scheme, Url};
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};

thread_local! {
    static REQUESTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn count_request() {
    REQUESTS.with(|c| c.set(c.get().saturating_add(1)));
}

/// Run `f` and give its result with the number of requests that this thread sent to the PDP
/// during `f`. A call that sent no request used only kept answers.
pub fn count_requests<T>(f: impl FnOnce() -> T) -> (T, u32) {
    let before = REQUESTS.with(std::cell::Cell::get);
    let result = f();
    let after = REQUESTS.with(std::cell::Cell::get);
    (result, after.saturating_sub(before))
}

/// The largest head of an answer: the status line and the headers.
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// The most headers in one answer.
const MAX_HEADERS: usize = 100;

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Tls(s) => &s.sock,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

struct Conn {
    stream: Stream,
    authority: String,
    last_used: Instant,
}

#[derive(Default)]
struct Pool {
    idle: Vec<Conn>,
    /// The connections that exist: the idle ones and the ones in use.
    open: usize,
}

/// The transport of the PEP: one pool of connections to the PDP.
pub struct PdpLink {
    plain_http: bool,
    tls: Option<Arc<rustls::ClientConfig>>,
    connect_timeout: Duration,
    max_connections: usize,
    max_response: u64,
    idle_close_ms: AtomicU64,
    pool: Mutex<Pool>,
    freed: Condvar,
}

impl std::fmt::Debug for PdpLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdpLink").field("plain_http", &self.plain_http).finish_non_exhaustive()
    }
}

/// A connection that the pool lent. It goes back to the pool, or it is closed.
struct Lent<'a> {
    link: &'a PdpLink,
    conn: Option<Conn>,
}

impl Lent<'_> {
    fn stream(&mut self) -> Result<&mut Stream, TransportError> {
        self.conn
            .as_mut()
            .map(|c| &mut c.stream)
            .ok_or_else(|| TransportError::Lost("the connection is gone".to_owned()))
    }

    fn give_back(mut self) {
        if let Some(mut conn) = self.conn.take() {
            conn.last_used = Instant::now();
            let mut pool = self.link.lock();
            pool.idle.push(conn);
            drop(pool);
            self.link.freed.notify_one();
        }
    }
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        if self.conn.take().is_some() {
            let mut pool = self.link.lock();
            pool.open = pool.open.saturating_sub(1);
            drop(pool);
            self.link.freed.notify_one();
        }
    }
}

/// Build the rustls configuration: `ring`, TLS 1.2 and 1.3, the CA of the file as the only roots.
pub fn tls_config(ca_pem: &[u8]) -> Result<rustls::ClientConfig, String> {
    let mut roots = rustls::RootCertStore::empty();
    let mut count = 0usize;
    for item in CertificateDer::pem_slice_iter(ca_pem) {
        let cert = item.map_err(|e| format!("the CA text is not valid PEM: {e}"))?;
        roots.add(cert).map_err(|e| format!("a CA certificate is not valid: {e}"))?;
        count += 1;
    }
    if count == 0 {
        return Err("the CA text holds no CERTIFICATE block".to_owned());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| format!("the protocol versions are not supported: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

impl PdpLink {
    /// Build the transport. With `plain_http = false`, `ca_pem` is necessary. It sends nothing.
    pub fn new(
        plain_http: bool,
        ca_pem: Option<&[u8]>,
        connect_timeout: Duration,
        max_connections: usize,
        max_response: u64,
        idle_close: Duration,
    ) -> Result<Self, String> {
        if max_connections == 0 {
            return Err("access.pdp.max_connections: expected a whole number of 1 or more, found 0".to_owned());
        }
        let tls = match (plain_http, ca_pem) {
            (false, Some(pem)) => Some(Arc::new(tls_config(pem)?)),
            (false, None) => return Err("access.pdp.ca_file: a TLS link needs the CA certificates".to_owned()),
            (true, _) => None,
        };
        Ok(PdpLink {
            plain_http,
            tls,
            connect_timeout,
            max_connections,
            max_response,
            idle_close_ms: AtomicU64::new(u64::try_from(idle_close.as_millis()).unwrap_or(u64::MAX)),
            pool: Mutex::new(Pool::default()),
            freed: Condvar::new(),
        })
    }

    /// The number of connections that exist now, idle or in use.
    #[cfg(test)]
    pub fn open_connections(&self) -> usize {
        self.lock().open
    }

    fn lock(&self) -> MutexGuard<'_, Pool> {
        self.pool.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn idle_close(&self) -> Duration {
        Duration::from_millis(self.idle_close_ms.load(Ordering::SeqCst))
    }

    fn sweep(&self, pool: &mut Pool) {
        let limit = self.idle_close();
        let before = pool.idle.len();
        pool.idle.retain(|c| c.last_used.elapsed() < limit);
        pool.open = pool.open.saturating_sub(before - pool.idle.len());
    }

    fn checkout(&self, url: &Url, deadline: Instant) -> Result<Lent<'_>, TransportError> {
        let authority = url.authority();
        let mut pool = self.lock();
        loop {
            self.sweep(&mut pool);
            if let Some(index) = pool.idle.iter().rposition(|c| c.authority == authority) {
                let conn = pool.idle.remove(index);
                if is_alive(conn.stream.tcp()) {
                    return Ok(Lent { link: self, conn: Some(conn) });
                }
                pool.open = pool.open.saturating_sub(1);
                continue;
            }
            if pool.open < self.max_connections {
                pool.open += 1;
                drop(pool);
                return match self.connect(url, &authority, deadline) {
                    Ok(stream) => {
                        Ok(Lent { link: self, conn: Some(Conn { stream, authority, last_used: Instant::now() }) })
                    }
                    Err(error) => {
                        let mut pool = self.lock();
                        pool.open = pool.open.saturating_sub(1);
                        drop(pool);
                        self.freed.notify_one();
                        Err(error)
                    }
                };
            }
            if !pool.idle.is_empty() {
                pool.idle.remove(0);
                pool.open = pool.open.saturating_sub(1);
                continue;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(TransportError::NotSent("no free connection before the time ran out".to_owned()));
            }
            pool = self.freed.wait_timeout(pool, remaining).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// TCP connection and TLS handshake, within one deadline (3.5).
    fn connect(&self, url: &Url, authority: &str, deadline: Instant) -> Result<Stream, TransportError> {
        let not_sent =
            |what: &str, e: &dyn std::fmt::Display| TransportError::NotSent(format!("{what} {authority}: {e}"));
        let mut last: Option<std::io::Error> = None;
        let mut tcp = None;
        let addrs = authority.to_socket_addrs().map_err(|e| not_sent("no address for", &e))?;
        for address in addrs {
            let remaining = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
            match TcpStream::connect_timeout(&address, remaining) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        let Some(mut sock) = tcp else {
            let e = last.unwrap_or_else(|| std::io::Error::new(ErrorKind::NotFound, "the name has no address"));
            return Err(not_sent("no connection to", &e));
        };
        let _ = sock.set_nodelay(true);
        if url.scheme == Scheme::Http {
            return Ok(Stream::Plain(sock));
        }
        let Some(config) = &self.tls else {
            return Err(TransportError::Unsupported("this link has no TLS configuration".to_owned()));
        };
        let name = ServerName::try_from(url.host.clone()).map_err(|_| {
            TransportError::Unsupported(format!("the host {:?} is not a DNS name or an address", url.host))
        })?;
        let mut conn =
            rustls::ClientConnection::new(config.clone(), name).map_err(|e| not_sent("TLS setup for", &e))?;
        while conn.is_handshaking() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(TransportError::NotSent(format!("the TLS handshake with {authority} did not end in time")));
            }
            let _ = sock.set_read_timeout(Some(remaining));
            let _ = sock.set_write_timeout(Some(remaining));
            if let Err(e) = conn.complete_io(&mut sock) {
                return Err(not_sent("TLS handshake with", &e));
            }
        }
        Ok(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, sock))))
    }

    fn exchange(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        count_request();
        self.exchange_inner(method, url, headers, body, timeout)
    }

    fn exchange_inner(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        let url = Url::parse_request(url).map_err(|e| TransportError::Unsupported(e.to_string()))?;
        if url.scheme == Scheme::Http && !self.plain_http {
            return Err(TransportError::Unsupported("an http URL needs access.pdp.plain_http = true".to_owned()));
        }
        let head = build_head(method, &url, headers, body.map_or(0, <[u8]>::len), body.is_some())?;
        let mut lent =
            self.checkout(&url, Instant::now() + self.connect_timeout.min(timeout.max(self.connect_timeout)))?;
        // The deadline of the exchange starts at the first byte written (3.5).
        let deadline = Instant::now() + timeout;
        let mut request = head;
        if let Some(body) = body {
            request.extend_from_slice(body);
        }
        write_all(lent.stream()?, &request, deadline)?;
        let (response, reusable) = read_response(lent.stream()?, deadline, self.max_response)?;
        if reusable {
            lent.give_back();
        }
        Ok(response)
    }
}

impl PdpTransport for PdpLink {
    fn supports_scheme(&self, scheme: &str) -> bool {
        match scheme {
            "https" => self.tls.is_some(),
            "http" => self.plain_http,
            _ => false,
        }
    }

    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        self.exchange("POST", url, headers, Some(body), timeout)
    }

    fn get_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        self.exchange("GET", url, headers, None, timeout)
    }

    fn set_idle_close(&self, idle_close_after: Duration) {
        self.idle_close_ms.store(u64::try_from(idle_close_after.as_millis()).unwrap_or(u64::MAX), Ordering::SeqCst);
    }
}

/// True when the PDP has not closed the connection, and no stray byte waits on it.
fn is_alive(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut byte = [0_u8; 1];
    let alive = matches!(stream.peek(&mut byte), Err(e) if e.kind() == ErrorKind::WouldBlock);
    alive && stream.set_nonblocking(false).is_ok()
}

fn bad_header(name: &str, value: &str) -> bool {
    name.is_empty()
        || name.bytes().any(|b| !(0x21..0x7f).contains(&b) || b == b':')
        || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

fn build_head(
    method: &str,
    url: &Url,
    headers: &[(String, String)],
    body_len: usize,
    has_body: bool,
) -> Result<Vec<u8>, TransportError> {
    let path = if url.path.is_empty() { "/" } else { url.path.as_str() };
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\n", url.host_header());
    if has_body {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {body_len}\r\n"));
    }
    for (name, value) in headers.iter().map(|(n, v)| (n.as_str(), v.as_str())) {
        if bad_header(name, value) {
            return Err(TransportError::NotSent(format!(
                "the header `{name}` has a character that a header must not have"
            )));
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    Ok(head.into_bytes())
}

fn failure_after_write(written: usize, error: TransportError) -> TransportError {
    if written == 0 { TransportError::NotSent(format!("the first write failed: {error}")) } else { error }
}

/// Writes the whole request. A failure before the first byte is `NotSent`.
fn write_all(stream: &mut Stream, data: &[u8], deadline: Instant) -> Result<(), TransportError> {
    let mut written = 0;
    while written < data.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(failure_after_write(written, TransportError::Timeout));
        }
        let _ = stream.tcp().set_write_timeout(Some(remaining));
        match stream.write(&data[written..]) {
            Ok(0) => {
                return Err(failure_after_write(
                    written,
                    TransportError::Lost("the connection accepts no byte".to_owned()),
                ));
            }
            Ok(n) => written += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(failure_after_write(written, TransportError::Timeout));
            }
            Err(e) => return Err(failure_after_write(written, TransportError::Lost(e.to_string()))),
        }
    }
    match stream.flush() {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Err(TransportError::Timeout),
        Err(e) => Err(TransportError::Lost(e.to_string())),
    }
}

/// A reader of the answer, with a deadline.
struct Reader<'a> {
    stream: &'a mut Stream,
    buf: Vec<u8>,
    pos: usize,
    deadline: Instant,
}

impl Reader<'_> {
    /// Reads more bytes into the buffer. It gives the number of bytes, 0 at the end.
    fn fill(&mut self) -> Result<usize, TransportError> {
        let mut chunk = [0_u8; 8192];
        loop {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(TransportError::Timeout);
            }
            let _ = self.stream.tcp().set_read_timeout(Some(remaining));
            match self.stream.read(&mut chunk) {
                Ok(n) => {
                    if self.pos > 0 && self.pos == self.buf.len() {
                        self.buf.clear();
                        self.pos = 0;
                    }
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(n);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                // A TLS peer that closes with no `close_notify` ends the answer.
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(0),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Err(TransportError::Timeout);
                }
                Err(e) => return Err(TransportError::Lost(e.to_string())),
            }
        }
    }

    fn available(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    fn line(&mut self, max: usize) -> Result<Vec<u8>, TransportError> {
        loop {
            if let Some(end) = find(self.available(), b"\r\n") {
                let line = self.available()[..end].to_vec();
                self.pos += end + 2;
                return Ok(line);
            }
            if self.available().len() > max {
                return Err(malformed("a line is too long"));
            }
            if self.fill()? == 0 {
                return Err(TransportError::Lost("the PDP closed the connection in an answer".to_owned()));
            }
        }
    }

    fn take(&mut self, count: usize, out: &mut Vec<u8>) -> Result<(), TransportError> {
        let mut left = count;
        while left > 0 {
            if self.available().is_empty() && self.fill()? == 0 {
                return Err(TransportError::Lost("the PDP closed the connection in the body".to_owned()));
            }
            let part = left.min(self.available().len());
            out.extend_from_slice(&self.available()[..part]);
            self.pos += part;
            left -= part;
        }
        Ok(())
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn malformed(text: &str) -> TransportError {
    TransportError::Malformed(text.to_owned())
}

fn header_values<'a>(headers: &'a [(String, String)], name: &'a str) -> impl Iterator<Item = &'a str> {
    headers.iter().filter(move |(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// Reads one answer. The second value is true when the connection can serve another request.
fn read_response(
    stream: &mut Stream,
    deadline: Instant,
    max_body: u64,
) -> Result<(HttpResponse, bool), TransportError> {
    let mut reader = Reader { stream, buf: Vec::new(), pos: 0, deadline };
    let status_line = match reader.line(MAX_HEAD_BYTES) {
        Err(TransportError::Lost(_)) if reader.buf.is_empty() => {
            return Err(TransportError::Lost("the PDP closed the connection before it answered".to_owned()));
        }
        other => other?,
    };
    let status_text = String::from_utf8_lossy(&status_line).into_owned();
    let mut parts = status_text.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    let code = parts.next().unwrap_or("");
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(malformed("the status line has no HTTP/1.x version"));
    }
    let status: u16 = match code.parse() {
        Ok(s) if code.len() == 3 && (100..600).contains(&s) => s,
        _ => return Err(malformed("the status code is not a number from 100 to 599")),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut head_bytes = status_line.len();
    loop {
        let line = reader.line(MAX_HEAD_BYTES)?;
        if line.is_empty() {
            break;
        }
        head_bytes += line.len();
        if head_bytes > MAX_HEAD_BYTES || headers.len() >= MAX_HEADERS {
            return Err(malformed("the head of the answer is too large"));
        }
        if line[0] == b' ' || line[0] == b'\t' {
            return Err(malformed("a header line is folded"));
        }
        let text = String::from_utf8_lossy(&line).into_owned();
        let (name, value) = text.split_once(':').ok_or_else(|| malformed("a header line has no colon"))?;
        if name.is_empty() || name.bytes().any(|b| !(0x21..0x7f).contains(&b)) {
            return Err(malformed("a header name is not valid"));
        }
        headers.push((name.to_owned(), value.trim().to_owned()));
    }
    // A status 3xx is an answer with that status: no redirect (3.5).
    let close =
        header_values(&headers, "connection").any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")));
    let keep_alive_10 = header_values(&headers, "connection")
        .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("keep-alive")));
    let mut reusable = if version == "HTTP/1.1" { !close } else { keep_alive_10 && !close };
    let chunked: Vec<&str> = header_values(&headers, "transfer-encoding").collect();
    let lengths: Vec<&str> = header_values(&headers, "content-length").collect();
    let mut body = Vec::new();
    if !chunked.is_empty() {
        if !lengths.is_empty() || chunked.len() != 1 || !chunked[0].eq_ignore_ascii_case("chunked") {
            return Err(malformed("the answer has an unsupported transfer coding"));
        }
        read_chunked(&mut reader, &mut body, max_body)?;
    } else if !lengths.is_empty() {
        let first = lengths[0];
        if lengths.iter().any(|l| *l != first) || first.is_empty() || !first.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed("the header `Content-Length` is not valid"));
        }
        let length: u64 = first.parse().map_err(|_| malformed("the header `Content-Length` is too large"))?;
        if length > max_body {
            return Err(TransportError::TooLarge);
        }
        reader.take(usize::try_from(length).map_err(|_| TransportError::TooLarge)?, &mut body)?;
    } else if status == 204 || status == 304 {
        // No body.
    } else {
        reusable = false;
        loop {
            let available = reader.available().len();
            if u64::try_from(body.len() + available).unwrap_or(u64::MAX) > max_body {
                return Err(TransportError::TooLarge);
            }
            body.extend_from_slice(reader.available());
            reader.pos = reader.buf.len();
            if reader.fill()? == 0 {
                break;
            }
        }
    }
    if !reader.available().is_empty() {
        reusable = false;
    }
    Ok((HttpResponse { status, headers, body }, reusable))
}

fn read_chunked(reader: &mut Reader<'_>, body: &mut Vec<u8>, max_body: u64) -> Result<(), TransportError> {
    loop {
        let line = reader.line(1024)?;
        let text = String::from_utf8_lossy(&line).into_owned();
        let size_text = text.split(';').next().unwrap_or("").trim();
        if size_text.is_empty() || !size_text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(malformed("a chunk size is not valid"));
        }
        let size = u64::from_str_radix(size_text, 16).map_err(|_| malformed("a chunk size is too large"))?;
        if size == 0 {
            loop {
                if reader.line(MAX_HEAD_BYTES)?.is_empty() {
                    return Ok(());
                }
            }
        }
        if u64::try_from(body.len()).unwrap_or(u64::MAX).saturating_add(size) > max_body {
            return Err(TransportError::TooLarge);
        }
        reader.take(usize::try_from(size).map_err(|_| TransportError::TooLarge)?, body)?;
        if !reader.line(2)?.is_empty() {
            return Err(malformed("a chunk has no end marker"));
        }
    }
}
