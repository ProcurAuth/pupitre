// SPDX-License-Identifier: MIT OR Apache-2.0
//! The link to PostgreSQL: the TLS settings, the pool and the mapping of errors (2.37, 6.10).
//!
//! The root store of the TLS link holds only the certificates of the CA file of the
//! configuration. It never reads another root certificate (7.12).

use std::sync::Arc;
use std::time::Duration;

use deadpool_postgres::{Hook, HookError, Manager, ManagerConfig, Pool, PoolError, RecyclingMethod, Runtime};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_postgres::config::{SslMode as PgSslMode, TargetSessionAttrs};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Config, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::{PgOptions, SslMode};
use crate::types::StoreError;

/// A TLS connector that checks one server name, whatever host the client dials. Thus `host`
/// can be an address while the certificate holds `server_name`.
#[derive(Clone)]
pub(crate) struct FixedNameTls {
    inner: MakeRustlsConnect,
    name: String,
}

impl<S> MakeTlsConnect<S> for FixedNameTls
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = <MakeRustlsConnect as MakeTlsConnect<S>>::Stream;
    type TlsConnect = <MakeRustlsConnect as MakeTlsConnect<S>>::TlsConnect;
    type Error = <MakeRustlsConnect as MakeTlsConnect<S>>::Error;

    fn make_tls_connect(&mut self, _domain: &str) -> Result<Self::TlsConnect, Self::Error> {
        <MakeRustlsConnect as MakeTlsConnect<S>>::make_tls_connect(&mut self.inner, &self.name)
    }
}

/// The TLS setting of a store, built one time.
#[derive(Clone)]
pub(crate) enum Tls {
    /// `sslmode = "disable"`.
    None,
    /// `sslmode = "verify-full"`.
    Rustls(FixedNameTls),
}

fn unavailable(text: String) -> StoreError {
    StoreError::Unavailable(text)
}

/// Build the rustls client configuration. It has the ring provider, TLS 1.2 and 1.3, and the
/// certificates of the CA file only.
pub(crate) fn build_tls(options: &PgOptions) -> Result<Tls, StoreError> {
    if options.sslmode == SslMode::Disable {
        return Ok(Tls::None);
    }
    let Some(tls) = &options.tls else {
        return Err(unavailable("storage.postgres.tls: the block is necessary with sslmode verify-full".to_string()));
    };
    let mut roots = rustls::RootCertStore::empty();
    let mut count = 0usize;
    for item in CertificateDer::pem_slice_iter(&tls.ca_pem) {
        let cert =
            item.map_err(|e| unavailable(format!("storage.postgres.tls.ca_file: not a PEM certificate: {e}")))?;
        roots
            .add(cert)
            .map_err(|e| unavailable(format!("storage.postgres.tls.ca_file: a certificate is not usable: {e}")))?;
        count += 1;
    }
    if count == 0 {
        return Err(unavailable("storage.postgres.tls.ca_file: the file holds no certificate".to_string()));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
        .map_err(|e| unavailable(format!("storage.postgres: the TLS versions are not usable: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Tls::Rustls(FixedNameTls { inner: MakeRustlsConnect::new(config), name: tls.server_name.clone() }))
}

/// The `tokio-postgres` configuration: `application_name`, the primary only, the connect
/// timeout and TCP keep-alive probes.
pub(crate) fn build_config(options: &PgOptions) -> Config {
    let mut c = Config::new();
    c.host(&options.host);
    c.port(options.port);
    c.dbname(&options.database);
    c.user(&options.user);
    c.password(options.password.0.as_bytes());
    c.application_name("pupitre");
    c.connect_timeout(options.connect_timeout);
    c.keepalives(true);
    c.keepalives_idle(Duration::from_secs(10));
    c.keepalives_interval(Duration::from_secs(5));
    c.keepalives_retries(3);
    c.tcp_user_timeout(Duration::from_secs(10));
    c.target_session_attrs(TargetSessionAttrs::ReadWrite);
    c.ssl_mode(match options.sslmode {
        SslMode::Disable => PgSslMode::Disable,
        SslMode::VerifyFull => PgSslMode::Require,
    });
    c
}

/// The batch that each new connection runs: the statement timeout of 2.37, a time zone, and a
/// bound on an idle transaction.
pub(crate) fn session_batch(options: &PgOptions) -> String {
    let ms = options.statement_timeout.as_millis().max(1);
    format!(
        "SET statement_timeout = {ms}; SET idle_in_transaction_session_timeout = {ms}; \
         SET search_path = public; SET timezone = 'UTC'"
    )
}

/// Build the pool. It sends nothing on the network. A call that waits longer than
/// `connect_timeout` for a connection fails (2.37).
pub(crate) fn build_pool(options: &PgOptions, tls: &Tls) -> Result<Pool, StoreError> {
    let cfg = build_config(options);
    let manager_config = ManagerConfig { recycling_method: RecyclingMethod::Fast };
    let manager = match tls {
        Tls::None => Manager::from_config(cfg, NoTls, manager_config),
        Tls::Rustls(t) => Manager::from_config(cfg, t.clone(), manager_config),
    };
    let batch = session_batch(options);
    let size = usize::try_from(options.max_connections).unwrap_or(usize::MAX).max(1);
    Pool::builder(manager)
        .max_size(size)
        .wait_timeout(Some(options.connect_timeout))
        .create_timeout(Some(options.connect_timeout))
        .runtime(Runtime::Tokio1)
        .post_create(Hook::async_fn(move |client, _| {
            let batch = batch.clone();
            Box::pin(async move { client.batch_execute(&batch).await.map_err(HookError::Backend) })
        }))
        .build()
        .map_err(|e| unavailable(format!("storage.postgres: the pool cannot be built: {e}")))
}

/// One session outside the pool, for `migrate`. The connection task stops on drop.
pub(crate) struct Direct {
    pub(crate) client: tokio_postgres::Client,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Direct {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Open a session outside the pool and run the session batch.
pub(crate) async fn connect_direct(options: &PgOptions, tls: &Tls) -> Result<Direct, StoreError> {
    let cfg = build_config(options);
    let (client, task) = match tls {
        Tls::None => {
            let (client, conn) = cfg.connect(NoTls).await?;
            (client, tokio::spawn(async move { drop(conn.await) }))
        }
        Tls::Rustls(t) => {
            let (client, conn) = cfg.connect(t.clone()).await?;
            (client, tokio::spawn(async move { drop(conn.await) }))
        }
    };
    let direct = Direct { client, task };
    direct.client.batch_execute(&session_batch(options)).await?;
    Ok(direct)
}

/// The text of a database error: the message and the SQLSTATE. It never has the detail of the
/// error, because the detail can hold the values of a row.
pub(crate) fn describe(e: &tokio_postgres::Error) -> String {
    use std::error::Error as _;
    match e.as_db_error() {
        Some(db) => format!("{} (SQLSTATE {})", db.message(), db.code().code()),
        None => match e.source() {
            Some(s) => format!("{e}: {s}"),
            None => e.to_string(),
        },
    }
}

impl From<tokio_postgres::Error> for StoreError {
    fn from(e: tokio_postgres::Error) -> Self {
        StoreError::Unavailable(describe(&e))
    }
}

impl From<PoolError> for StoreError {
    fn from(e: PoolError) -> Self {
        let text = match &e {
            PoolError::Backend(pe) | PoolError::PostCreateHook(HookError::Backend(pe)) => describe(pe),
            PoolError::Timeout(t) => format!("no connection after the timeout ({t:?})"),
            other => other.to_string(),
        };
        StoreError::Unavailable(text)
    }
}

/// True when the error has this SQLSTATE.
pub(crate) fn has_code(e: &tokio_postgres::Error, state: &tokio_postgres::error::SqlState) -> bool {
    e.code() == Some(state)
}
