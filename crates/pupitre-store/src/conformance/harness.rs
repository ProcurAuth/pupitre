// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `Harness` trait: what a backend gives to the cases of the suite.

use std::future::Future;

use pupitre_core::doc::{CanonicalDocument, DocKey};

use super::support::test_key;
use crate::secret::SecretKey;
use crate::store::AnyStore;
use crate::types::{Precondition, StoreError, WriteMeta};

/// The options of a fresh store.
#[derive(Debug, Clone)]
pub struct FreshOptions {
    /// `history_keep` of the backend (2.18).
    pub history_keep: u32,
    /// `storage.secret_key`.
    pub secret_key: SecretKey,
}

impl Default for FreshOptions {
    fn default() -> Self {
        FreshOptions { history_keep: 100, secret_key: test_key() }
    }
}

/// What a crash between the version and the document leaves (S08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashOutcome {
    /// The write left nothing: the version and the document are both absent. A backend with
    /// atomic writes gives this.
    RolledBack,
    /// The write left a version that the open found and wrote as the current document (2.19).
    Repaired,
}

/// A backend, as the cases of the suite see it. Each method is async, and its future is `Send`.
pub trait Harness: Send + Sync {
    /// The name of the backend, for the messages of a failed case.
    fn name(&self) -> &'static str;

    /// Make a store that is empty and ready: its storage is new, and its store schema and its
    /// key check are current.
    fn fresh(&self, options: FreshOptions) -> impl Future<Output = Result<AnyStore, StoreError>> + Send;

    /// Close a store and open its storage again, with the same options and the same key. The
    /// answer is the result of the open.
    fn reopen(&self, store: AnyStore) -> impl Future<Output = Result<AnyStore, StoreError>> + Send;

    /// Close a store and open its storage again with another key. S13 expects `KeyMismatch`.
    fn reopen_with_key(
        &self,
        store: AnyStore,
        key: SecretKey,
    ) -> impl Future<Output = Result<AnyStore, StoreError>> + Send;

    /// A second handle on the same storage, or `None` when the backend allows one handle only
    /// (the backend `files` holds a lock). S09 then writes with the first handle.
    fn other_handle(&self, store: &AnyStore) -> impl Future<Output = Result<Option<AnyStore>, StoreError>> + Send;

    /// Write a document, with a fault that stops the write after the version and before the
    /// document (2.19), then open the storage again. The answer is the new store and the outcome.
    fn crash_after_version(
        &self,
        store: AnyStore,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> impl Future<Output = Result<(AnyStore, CrashOutcome), StoreError>> + Send;

    /// Make a storage with a store schema that is older than the schema of the binary (S20).
    /// The three methods that follow work on this storage.
    fn prepare_old_schema(&self) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Open the prepared storage. S20 expects `MigrateNeeded`.
    fn open_prepared(&self) -> impl Future<Output = Result<AnyStore, StoreError>> + Send;

    /// Run `migrate` on the prepared storage.
    fn migrate_prepared(&self) -> impl Future<Output = Result<(), StoreError>> + Send;
}
