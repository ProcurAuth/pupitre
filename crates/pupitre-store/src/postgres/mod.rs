// SPDX-License-Identifier: MIT OR Apache-2.0
//! The backend `postgres` (2.36 to 2.39): the tables of `migrations/0001.sql`, the writes in
//! one transaction, the poll by change number and the cipher of the secrets.
//!
//! The store keeps the bytes of the documents and the run records. It never parses a document:
//! the catalog does it (2.17). A write is one transaction (2.37). The poll reads by change
//! number (2.38). The secrets are encrypted before they leave the process (2.39).

mod conn;
mod docs;
mod leases;
mod migrate;
mod rows;
mod runs;
mod secrets;
#[cfg(any(test, feature = "conformance"))]
pub mod testing;
mod transfer;

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deadpool_postgres::{Object, Pool};
use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::etag::Etag;
use pupitre_core::ids::{EnvId, KeyPath, RunId};
use pupitre_core::node::SecretScope;
use pupitre_core::run::{RunRecord, RunStatus};

pub use self::migrate::{MIGRATIONS, MigrateReport, Migration, migrate, migrate_with};
pub use self::transfer::{PgExporter, PgImporter};
use crate::secret::{SecretKey, SecretValue, StoreKeys};
use crate::store::Store;
use crate::transfer::{Exporter, Importer};
use crate::types::{
    Delta, EventLine, LeaseName, LeaseOutcome, LeaseRenewal, Listing, Marks, OutputChunk, Precondition, RunFilter,
    STORE_SCHEMA_VERSION, Saved, SecretMeta, StoreError, StoreEvent, StoredDoc, Version, VersionMeta, WriteMeta,
};

/// The SQL of the store schema 1 (2.36). The command `migrate` runs it in one transaction.
pub const MIGRATION_0001: &str = include_str!("../../migrations/0001.sql");

/// The TLS mode of the link to PostgreSQL (6.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    /// Check the certificate and the name.
    VerifyFull,
    /// No TLS.
    Disable,
}

/// The TLS settings of the link, with `verify-full`.
#[derive(Clone)]
pub struct PgTls {
    /// The bytes of the CA file, read and checked at start.
    pub ca_pem: Vec<u8>,
    /// The name that the certificate must hold.
    pub server_name: String,
}

impl fmt::Debug for PgTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PgTls {{ ca_pem: {} bytes, server_name: {:?} }}", self.ca_pem.len(), self.server_name)
    }
}

/// The password of the account. `Debug` never shows it.
#[derive(Clone)]
pub struct PgPassword(pub String);

impl fmt::Debug for PgPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgPassword(<hidden>)")
    }
}

/// The options of the backend, from the block `storage.postgres` (6.10).
#[derive(Debug, Clone)]
pub struct PgOptions {
    /// `storage.postgres.host`
    pub host: String,
    /// `storage.postgres.port`
    pub port: u16,
    /// `storage.postgres.database`
    pub database: String,
    /// `storage.postgres.user`
    pub user: String,
    /// `storage.postgres.password`
    pub password: PgPassword,
    /// `storage.postgres.sslmode`
    pub sslmode: SslMode,
    /// The block `storage.postgres.tls`, with `verify-full`.
    pub tls: Option<PgTls>,
    /// `storage.postgres.max_connections`
    pub max_connections: u32,
    /// `storage.postgres.connect_timeout`
    pub connect_timeout: Duration,
    /// `storage.postgres.statement_timeout`
    pub statement_timeout: Duration,
    /// `storage.postgres.poll_interval`
    pub poll_interval: Duration,
    /// `storage.postgres.history_keep`
    pub history_keep: u32,
    /// `storage.secret_key`
    pub secret_key: SecretKey,
    /// `limits.max_document_bytes`
    pub max_document_bytes: usize,
}

/// The backend `postgres`.
pub struct PgStore {
    pub(crate) options: PgOptions,
    pub(crate) pool: Pool,
    pub(crate) keys: Arc<StoreKeys>,
    events: Mutex<Vec<StoreEvent>>,
    #[cfg(any(test, feature = "conformance"))]
    pub(crate) faults: testing::Faults,
}

impl fmt::Debug for PgStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgStore").field("options", &self.options).finish_non_exhaustive()
    }
}

impl PgStore {
    /// Connect, check the encoding, the store schema and the key check (2.30, 2.26). A store
    /// with no schema or an older schema gives `MigrateNeeded`. A newer schema gives `StoreNewer`.
    /// A key that differs from the key of the store gives `KeyMismatch`.
    pub async fn open(options: PgOptions) -> Result<PgStore, StoreError> {
        PgStore::open_for(options, STORE_SCHEMA_VERSION).await
    }

    /// Like `open`, with the store schema that the binary needs. The tests of 2.30 use it to
    /// play a binary of another version.
    #[doc(hidden)]
    pub async fn open_for(options: PgOptions, binary_schema: u32) -> Result<PgStore, StoreError> {
        let tls = conn::build_tls(&options)?;
        let pool = conn::build_pool(&options, &tls)?;
        let keys = StoreKeys::derive(&options.secret_key)
            .map_err(|_| StoreError::Unavailable("storage.secret_key: the keys cannot be derived".to_string()))?;
        let store = PgStore {
            options,
            pool,
            keys: Arc::new(keys),
            events: Mutex::new(Vec::new()),
            #[cfg(any(test, feature = "conformance"))]
            faults: testing::Faults::default(),
        };
        let client = store.conn().await?;
        migrate::check_encoding(&client).await?;
        migrate::check_store(&client, binary_schema, &store.keys).await?;
        drop(client);
        Ok(store)
    }

    /// The options of the store.
    pub fn options(&self) -> &PgOptions {
        &self.options
    }

    /// Take the events for the log. The backend `postgres` has none in this version.
    pub fn drain_events(&self) -> Vec<StoreEvent> {
        self.events.lock().map(|mut e| std::mem::take(&mut *e)).unwrap_or_default()
    }

    /// Take a connection of the pool.
    pub(crate) async fn conn(&self) -> Result<Object, StoreError> {
        Ok(self.pool.get().await?)
    }
}

impl Store for PgStore {
    async fn load(&self) -> Result<Listing, StoreError> {
        self.load_docs().await
    }
    async fn poll(&self, marks: &Marks) -> Result<Delta, StoreError> {
        self.poll_docs(marks).await
    }
    async fn get(&self, key: &DocKey) -> Result<StoredDoc, StoreError> {
        self.get_doc(key).await
    }
    async fn put(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        self.put_doc(key, doc, precondition, meta).await
    }
    async fn put_import(&self, key: &DocKey, doc: &CanonicalDocument, meta: &WriteMeta) -> Result<Saved, StoreError> {
        self.put_import_doc(key, doc, meta).await
    }
    async fn delete(&self, key: &DocKey, if_match: &Etag, meta: &WriteMeta) -> Result<Saved, StoreError> {
        self.delete_doc(key, if_match, meta).await
    }
    async fn history(&self, key: &DocKey, before: Option<u64>, limit: u32) -> Result<Vec<VersionMeta>, StoreError> {
        self.history_of(key, before, limit).await
    }
    async fn version(&self, key: &DocKey, version: u64) -> Result<Version, StoreError> {
        self.version_of(key, version).await
    }
    async fn read_environment(&self, env: &EnvId) -> Result<Vec<StoredDoc>, StoreError> {
        self.read_env(env).await
    }
    async fn list_secrets(&self, scope: &SecretScope) -> Result<Vec<SecretMeta>, StoreError> {
        self.list_secrets_of(scope).await
    }
    async fn put_secret(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        meta: &WriteMeta,
    ) -> Result<SecretMeta, StoreError> {
        self.put_secret_of(scope, name, value, meta).await
    }
    async fn delete_secret(&self, scope: &SecretScope, name: &KeyPath, meta: &WriteMeta) -> Result<(), StoreError> {
        self.delete_secret_of(scope, name, meta).await
    }
    async fn read_secrets(
        &self,
        scopes: &[SecretScope],
    ) -> Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError> {
        self.read_secrets_of(scopes).await
    }
    async fn create_run(
        &self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> Result<(), StoreError> {
        self.create_run_of(record, max_queued, consumes).await
    }
    async fn get_run(&self, id: &RunId) -> Result<RunRecord, StoreError> {
        self.get_run_of(id).await
    }
    async fn list_runs(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        self.list_runs_of(filter, before, limit).await
    }
    async fn list_runs_page(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        skip: u32,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        self.list_runs_page_of(filter, before, skip, limit).await
    }
    async fn next_queued(&self, busy: &[EnvId], limit: u32) -> Result<Vec<RunRecord>, StoreError> {
        self.next_queued_of(busy, limit).await
    }
    async fn update_run(&self, expected: RunStatus, record: &RunRecord) -> Result<(), StoreError> {
        self.update_run_of(expected, record).await
    }
    async fn prune_runs(&self, keep: u32) -> Result<Vec<RunId>, StoreError> {
        self.prune_runs_of(keep).await
    }
    async fn append_output(&self, id: &RunId, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        self.append_output_of(id, offset, bytes).await
    }
    async fn read_output(&self, id: &RunId, offset: u64, max: u32) -> Result<OutputChunk, StoreError> {
        self.read_output_of(id, offset, max).await
    }
    async fn append_events(&self, id: &RunId, first_seq: u64, lines: &[Vec<u8>]) -> Result<(), StoreError> {
        self.append_events_of(id, first_seq, lines).await
    }
    async fn read_events(&self, id: &RunId, after: u64, limit: u32) -> Result<Vec<EventLine>, StoreError> {
        self.read_events_of(id, after, limit).await
    }
    async fn acquire_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseOutcome, StoreError> {
        self.acquire_lease_of(name, holder, ttl).await
    }
    async fn renew_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseRenewal, StoreError> {
        self.renew_lease_of(name, holder, ttl).await
    }
    async fn release_lease(&self, name: &LeaseName, holder: &str) -> Result<(), StoreError> {
        self.release_lease_of(name, holder).await
    }
    async fn export_begin(&self) -> Result<Exporter, StoreError> {
        Ok(Exporter::Postgres(Box::new(self.begin_export().await?)))
    }
    async fn import_begin(&self) -> Result<Importer, StoreError> {
        Ok(Importer::Postgres(Box::new(self.begin_import().await?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_migration_makes_each_table_of_2_36() {
        for table in [
            "pupitre_meta",
            "documents",
            "document_versions",
            "secrets",
            "secret_events",
            "runs",
            "run_output",
            "run_events",
            "leases",
        ] {
            assert!(MIGRATION_0001.contains(&format!("CREATE TABLE {table} (")), "{table}");
        }
        assert!(MIGRATION_0001.contains("CREATE SEQUENCE pupitre_change_seq"));
    }
}
