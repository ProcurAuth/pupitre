// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `Store` trait (2.17) and the enum `AnyStore` that dispatches its calls. Part 2 of the
//! specification gives the rule of each method; the doc comment of each method gives its
//! paragraph.

use std::future::Future;
use std::time::Duration;

use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::etag::Etag;
use pupitre_core::ids::{EnvId, KeyPath, RunId};
use pupitre_core::node::SecretScope;
use pupitre_core::run::{RunRecord, RunStatus};

use crate::files::FilesStore;
use crate::postgres::PgStore;
use crate::secret::SecretValue;
use crate::transfer::{Exporter, Importer};
use crate::types::{
    Delta, EventLine, LeaseName, LeaseOutcome, LeaseRenewal, Listing, Marks, OutputChunk, Precondition, RunFilter,
    Saved, SecretMeta, StoreError, StoredDoc, Version, VersionMeta, WriteMeta,
};

/// The contract of a backend (2.17). Each method is async, and its future is `Send`.
pub trait Store: Send + Sync {
    // Documents (2.18 to 2.23).

    /// Give each current document and the marks of this state (2.22).
    fn load(&self) -> impl Future<Output = Result<Listing, StoreError>> + Send;
    /// Give the changes since the marks (2.22).
    fn poll(&self, marks: &Marks) -> impl Future<Output = Result<Delta, StoreError>> + Send;
    /// Give the current document of a key, or `NotFound`.
    fn get(&self, key: &DocKey) -> impl Future<Output = Result<StoredDoc, StoreError>> + Send;
    /// Write a document with its precondition (2.19, 2.20).
    fn put(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> impl Future<Output = Result<Saved, StoreError>> + Send;
    /// Create a document with the version kind `import` (6.24a). The rules are the rules of `put`
    /// with the precondition `Create`. A backend that has no kind `import` keeps this default.
    fn put_import(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        meta: &WriteMeta,
    ) -> impl Future<Output = Result<Saved, StoreError>> + Send {
        async move { self.put(key, doc, &Precondition::Create, meta).await }
    }
    /// Delete a document: a version `delete`, then no current document (2.21).
    fn delete(
        &self,
        key: &DocKey,
        if_match: &Etag,
        meta: &WriteMeta,
    ) -> impl Future<Output = Result<Saved, StoreError>> + Send;
    /// Give the versions of a key, newest first, below `before`, at most `limit` (5.13).
    fn history(
        &self,
        key: &DocKey,
        before: Option<u64>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<VersionMeta>, StoreError>> + Send;
    /// Give one version with its bytes.
    fn version(&self, key: &DocKey, version: u64) -> impl Future<Output = Result<Version, StoreError>> + Send;
    /// Give each current document of an environment in one consistent state (2.23).
    fn read_environment(&self, env: &EnvId) -> impl Future<Output = Result<Vec<StoredDoc>, StoreError>> + Send;

    // Secrets (2.24 to 2.26).

    /// Give the metadata of the secrets of a scope, in the byte order of the name.
    fn list_secrets(&self, scope: &SecretScope) -> impl Future<Output = Result<Vec<SecretMeta>, StoreError>> + Send;
    /// Write a secret and its event. The answer has the new fingerprint.
    fn put_secret(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        meta: &WriteMeta,
    ) -> impl Future<Output = Result<SecretMeta, StoreError>> + Send;
    /// Delete a secret and write its event, or `NotFound`.
    fn delete_secret(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        meta: &WriteMeta,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Give the values of the secrets of the scopes. Only the runner calls it (4.13).
    fn read_secrets(
        &self,
        scopes: &[SecretScope],
    ) -> impl Future<Output = Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError>> + Send;

    // Runs (2.27).

    /// Write a new run with the status `queued`. Refuse it when `max_queued` runs are queued.
    /// With `consumes`, set `applied_by` of that check run in the same step.
    fn create_run(
        &self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Give a run record, or `NotFound`.
    fn get_run(&self, id: &RunId) -> impl Future<Output = Result<RunRecord, StoreError>> + Send;
    /// Give the runs of the filter, newest first, with an id below `before`, at most `limit`.
    fn list_runs(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RunRecord>, StoreError>> + Send;
    /// Like `list_runs`, but leave out the first `skip` runs of the answer. A backend that
    /// overrides this method copies only the runs of the page. The default copies the `skip` runs
    /// too.
    fn list_runs_page(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        skip: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RunRecord>, StoreError>> + Send {
        async move {
            let mut runs = self.list_runs(filter, before, skip.saturating_add(limit)).await?;
            runs.drain(..runs.len().min(usize::try_from(skip).unwrap_or(usize::MAX)));
            Ok(runs)
        }
    }
    /// Give the oldest queued runs whose environment is not in `busy`, at most `limit`.
    fn next_queued(
        &self,
        busy: &[EnvId],
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RunRecord>, StoreError>> + Send;
    /// Replace a run record when its stored status is `expected`. The status of `record` is
    /// `expected` or a status of 4.39 after it.
    fn update_run(
        &self,
        expected: RunStatus,
        record: &RunRecord,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Remove the oldest final runs above `keep`, with their output and events. Give their ids.
    fn prune_runs(&self, keep: u32) -> impl Future<Output = Result<Vec<RunId>, StoreError>> + Send;

    // Output and events (2.28).

    /// Add output bytes at `offset`, the current length. Give the new length.
    fn append_output(
        &self,
        id: &RunId,
        offset: u64,
        bytes: &[u8],
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
    /// Give output bytes from `offset`, at most `max` bytes, and the current length.
    fn read_output(
        &self,
        id: &RunId,
        offset: u64,
        max: u32,
    ) -> impl Future<Output = Result<OutputChunk, StoreError>> + Send;
    /// Add event lines with the numbers from `first_seq`, the next number.
    fn append_events(
        &self,
        id: &RunId,
        first_seq: u64,
        lines: &[Vec<u8>],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Give the event lines with a number above `after`, at most `limit`.
    fn read_events(
        &self,
        id: &RunId,
        after: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<EventLine>, StoreError>> + Send;

    // Leases (2.29).

    /// Take a lease when it has no holder, when it ended, or when the holder is the same.
    fn acquire_lease(
        &self,
        name: &LeaseName,
        holder: &str,
        ttl: Duration,
    ) -> impl Future<Output = Result<LeaseOutcome, StoreError>> + Send;
    /// Move the end time of a lease that the holder has.
    fn renew_lease(
        &self,
        name: &LeaseName,
        holder: &str,
        ttl: Duration,
    ) -> impl Future<Output = Result<LeaseRenewal, StoreError>> + Send;
    /// Remove a lease that the holder has.
    fn release_lease(&self, name: &LeaseName, holder: &str) -> impl Future<Output = Result<(), StoreError>> + Send;

    // Transfer (2.40).

    /// Start a consistent read of the whole store.
    fn export_begin(&self) -> impl Future<Output = Result<Exporter, StoreError>> + Send;
    /// Start a write into an empty store, or `NotEmpty`.
    fn import_begin(&self) -> impl Future<Output = Result<Importer, StoreError>> + Send;
}

/// The store of the process: an enum, with no `dyn` (1.5).
#[derive(Debug)]
pub enum AnyStore {
    /// The backend `files`.
    Files(FilesStore),
    /// The backend `postgres`.
    Postgres(PgStore),
}

impl AnyStore {
    /// The name of the backend for the log: `files` or `postgres`.
    pub fn backend_name(&self) -> &'static str {
        match self {
            AnyStore::Files(_) => "files",
            AnyStore::Postgres(_) => "postgres",
        }
    }

    /// Take the events that the store holds for the log (6.27).
    pub fn drain_events(&self) -> Vec<crate::types::StoreEvent> {
        match self {
            AnyStore::Files(s) => s.drain_events(),
            AnyStore::Postgres(s) => s.drain_events(),
        }
    }
}

macro_rules! dispatch {
    ($self:ident, $s:ident => $call:expr) => {
        match $self {
            AnyStore::Files($s) => $call,
            AnyStore::Postgres($s) => $call,
        }
    };
}

impl Store for AnyStore {
    async fn load(&self) -> Result<Listing, StoreError> {
        dispatch!(self, s => s.load().await)
    }
    async fn poll(&self, marks: &Marks) -> Result<Delta, StoreError> {
        dispatch!(self, s => s.poll(marks).await)
    }
    async fn get(&self, key: &DocKey) -> Result<StoredDoc, StoreError> {
        dispatch!(self, s => s.get(key).await)
    }
    async fn put(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        dispatch!(self, s => s.put(key, doc, precondition, meta).await)
    }
    async fn put_import(&self, key: &DocKey, doc: &CanonicalDocument, meta: &WriteMeta) -> Result<Saved, StoreError> {
        dispatch!(self, s => s.put_import(key, doc, meta).await)
    }
    async fn delete(&self, key: &DocKey, if_match: &Etag, meta: &WriteMeta) -> Result<Saved, StoreError> {
        dispatch!(self, s => s.delete(key, if_match, meta).await)
    }
    async fn history(&self, key: &DocKey, before: Option<u64>, limit: u32) -> Result<Vec<VersionMeta>, StoreError> {
        dispatch!(self, s => s.history(key, before, limit).await)
    }
    async fn version(&self, key: &DocKey, version: u64) -> Result<Version, StoreError> {
        dispatch!(self, s => s.version(key, version).await)
    }
    async fn read_environment(&self, env: &EnvId) -> Result<Vec<StoredDoc>, StoreError> {
        dispatch!(self, s => s.read_environment(env).await)
    }
    async fn list_secrets(&self, scope: &SecretScope) -> Result<Vec<SecretMeta>, StoreError> {
        dispatch!(self, s => s.list_secrets(scope).await)
    }
    async fn put_secret(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        meta: &WriteMeta,
    ) -> Result<SecretMeta, StoreError> {
        dispatch!(self, s => s.put_secret(scope, name, value, meta).await)
    }
    async fn delete_secret(&self, scope: &SecretScope, name: &KeyPath, meta: &WriteMeta) -> Result<(), StoreError> {
        dispatch!(self, s => s.delete_secret(scope, name, meta).await)
    }
    async fn read_secrets(
        &self,
        scopes: &[SecretScope],
    ) -> Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError> {
        dispatch!(self, s => s.read_secrets(scopes).await)
    }
    async fn create_run(
        &self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> Result<(), StoreError> {
        dispatch!(self, s => s.create_run(record, max_queued, consumes).await)
    }
    async fn get_run(&self, id: &RunId) -> Result<RunRecord, StoreError> {
        dispatch!(self, s => s.get_run(id).await)
    }
    async fn list_runs(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        dispatch!(self, s => s.list_runs(filter, before, limit).await)
    }
    async fn list_runs_page(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        skip: u32,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        dispatch!(self, s => s.list_runs_page(filter, before, skip, limit).await)
    }
    async fn next_queued(&self, busy: &[EnvId], limit: u32) -> Result<Vec<RunRecord>, StoreError> {
        dispatch!(self, s => s.next_queued(busy, limit).await)
    }
    async fn update_run(&self, expected: RunStatus, record: &RunRecord) -> Result<(), StoreError> {
        dispatch!(self, s => s.update_run(expected, record).await)
    }
    async fn prune_runs(&self, keep: u32) -> Result<Vec<RunId>, StoreError> {
        dispatch!(self, s => s.prune_runs(keep).await)
    }
    async fn append_output(&self, id: &RunId, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        dispatch!(self, s => s.append_output(id, offset, bytes).await)
    }
    async fn read_output(&self, id: &RunId, offset: u64, max: u32) -> Result<OutputChunk, StoreError> {
        dispatch!(self, s => s.read_output(id, offset, max).await)
    }
    async fn append_events(&self, id: &RunId, first_seq: u64, lines: &[Vec<u8>]) -> Result<(), StoreError> {
        dispatch!(self, s => s.append_events(id, first_seq, lines).await)
    }
    async fn read_events(&self, id: &RunId, after: u64, limit: u32) -> Result<Vec<EventLine>, StoreError> {
        dispatch!(self, s => s.read_events(id, after, limit).await)
    }
    async fn acquire_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseOutcome, StoreError> {
        dispatch!(self, s => s.acquire_lease(name, holder, ttl).await)
    }
    async fn renew_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseRenewal, StoreError> {
        dispatch!(self, s => s.renew_lease(name, holder, ttl).await)
    }
    async fn release_lease(&self, name: &LeaseName, holder: &str) -> Result<(), StoreError> {
        dispatch!(self, s => s.release_lease(name, holder).await)
    }
    async fn export_begin(&self) -> Result<Exporter, StoreError> {
        dispatch!(self, s => s.export_begin().await)
    }
    async fn import_begin(&self) -> Result<Importer, StoreError> {
        dispatch!(self, s => s.import_begin().await)
    }
}
