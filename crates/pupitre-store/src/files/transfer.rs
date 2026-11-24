// SPDX-License-Identifier: MIT OR Apache-2.0
//! The export and the import of the backend `files` (2.40), and the writer of a folder of
//! documents for a review in Git.
//!
//! `FilesExporter` and `FilesImporter` have the methods of `Exporter` and `Importer` of the
//! module `transfer`, with the same names and signatures. The enums call them.

use std::path::Path;
use std::sync::Arc;

use pupitre_core::doc::DocKey;
use pupitre_core::ids::RunId;
use pupitre_core::time::UtcMillis;

use super::events::Events;
use super::faults::Faults;
use super::fsio::{Dir, Modes};
use super::meta::{CURRENT_SCHEMA, encode_store_meta};
use super::{Inner, STORE_META_FILE, TMP_DIR, blocking};
use crate::transfer::{ImportReport, KeyHistory, RunExport, SecretExport};
use crate::types::StoreError;

/// The reader of an export of the backend (2.40). The store of a command has one process and
/// the lock of 2.32, so no write of another process changes the state during the export.
#[derive(Debug)]
pub struct FilesExporter {
    pub(super) inner: Arc<Inner>,
}

impl FilesExporter {
    /// The keys that have a current document or a version, in byte order.
    pub async fn keys(&mut self) -> Result<Vec<DocKey>, StoreError> {
        blocking(&self.inner, |i| i.lock_docs()?.export_keys()).await
    }

    /// One key with its versions.
    pub async fn key(&mut self, key: &DocKey) -> Result<KeyHistory, StoreError> {
        let key = key.clone();
        blocking(&self.inner, move |i| i.lock_docs()?.export_key(&key)).await
    }

    /// Each secret with its value and its events.
    pub async fn secrets(&mut self) -> Result<Vec<SecretExport>, StoreError> {
        blocking(&self.inner, |i| i.lock_secrets()?.export()).await
    }

    /// The run ids, oldest first.
    pub async fn run_ids(&mut self) -> Result<Vec<RunId>, StoreError> {
        blocking(&self.inner, |i| Ok(i.lock_runs()?.ids())).await
    }

    /// One run with its output and events.
    pub async fn run(&mut self, id: &RunId) -> Result<RunExport, StoreError> {
        let id = *id;
        blocking(&self.inner, move |i| i.lock_runs()?.export(&id)).await
    }

    /// End the export.
    pub async fn end(self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// The writer of an import into an empty backend (2.40).
#[derive(Debug)]
pub struct FilesImporter {
    pub(super) inner: Arc<Inner>,
    pub(super) report: ImportReport,
}

impl FilesImporter {
    /// Write one key with its versions and its current document.
    pub async fn key(&mut self, history: &KeyHistory) -> Result<(), StoreError> {
        let history = history.clone();
        let versions = history.versions.len() as u64;
        blocking(&self.inner, move |i| i.lock_docs()?.import_key(&history)).await?;
        self.report.keys += 1;
        self.report.versions += versions;
        Ok(())
    }

    /// Write one secret with its events.
    pub async fn secret(&mut self, secret: &SecretExport) -> Result<(), StoreError> {
        let secret = secret.clone();
        let live = secret.value.is_some();
        blocking(&self.inner, move |i| i.lock_secrets()?.import(&secret, UtcMillis::now())).await?;
        if live {
            self.report.secrets += 1;
        }
        Ok(())
    }

    /// Write one run with its output and events.
    pub async fn run(&mut self, run: &RunExport) -> Result<(), StoreError> {
        let run = run.clone();
        blocking(&self.inner, move |i| i.lock_runs()?.import(&run)).await?;
        self.report.runs += 1;
        Ok(())
    }

    /// End the import and give the counts. Each write of the import is on the disk already:
    /// the file layer calls `fsync` for each file.
    pub async fn commit(self) -> Result<ImportReport, StoreError> {
        Ok(self.report)
    }
}

/// The writer of a folder of documents for a review in Git (2.40, `export`). It writes the
/// layout of 2.31 for `storage.files.path`: `pupitre-store.json`, the documents and the
/// versions. It writes no secret, no run and no lock file.
#[derive(Debug)]
pub struct DocumentsWriter {
    root: Dir,
    docs: super::docs::DocStore,
    report: ImportReport,
}

impl DocumentsWriter {
    /// Start a folder. The folder is new, or it is empty. Else the answer is `NotEmpty`.
    pub fn create(dir: &Path) -> Result<DocumentsWriter, StoreError> {
        let faults = Faults::new();
        let root = Dir::open_or_create_root(dir, faults, Modes::DOCUMENTS)?;
        if !root.list()?.is_empty() {
            return Err(StoreError::NotEmpty);
        }
        root.write_atomic(&root.open_dir_create(TMP_DIR)?, STORE_META_FILE, &encode_store_meta(CURRENT_SCHEMA))?;
        let tmp = root.open_dir(TMP_DIR)?;
        let docs = super::docs::DocStore::blank(root.clone(), tmp, Arc::new(Events::default()));
        Ok(DocumentsWriter { root, docs, report: ImportReport::default() })
    }

    /// Write one key with its versions and its current document.
    pub fn write_key(&mut self, history: &KeyHistory) -> Result<(), StoreError> {
        self.docs.import_key(history)?;
        self.report.keys += 1;
        self.report.versions += history.versions.len() as u64;
        Ok(())
    }

    /// End the folder: remove the folder `.tmp`. Give the counts.
    pub fn finish(self) -> Result<ImportReport, StoreError> {
        let DocumentsWriter { root, docs, report } = self;
        drop(docs);
        root.rmdir(TMP_DIR)?;
        root.fsync()?;
        Ok(report)
    }
}
