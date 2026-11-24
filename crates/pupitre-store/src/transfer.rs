// SPDX-License-Identifier: MIT OR Apache-2.0
//! The transfer of 2.40: `export`, `import` into an empty store, and `copy-store`.
//!
//! `export_begin` gives an `Exporter`, a consistent read of a whole store. `import_begin` gives
//! an `Importer`, a write into an empty store. `copy_store` joins the two, then it compares the
//! etags. The traits `ExportSource` and `ImportSink` let a test use a store that is in memory.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;

use pupitre_core::doc::DocKey;
use pupitre_core::etag::Etag;
use pupitre_core::ids::{KeyPath, RunId, Subject};
use pupitre_core::json::Json;
use pupitre_core::node::SecretScope;
use pupitre_core::run::RunRecord;
use pupitre_core::time::UtcMillis;

use crate::files::{FilesExporter, FilesImporter};
use crate::postgres::{PgExporter, PgImporter};
use crate::secret::SecretValue;
use crate::store::{AnyStore, Store};
use crate::types::{EventLine, StoreError, StoredDoc, Version};

/// One key with all its versions and its current document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyHistory {
    /// The key.
    pub key: DocKey,
    /// The versions, from the lowest number to the highest.
    pub versions: Vec<Version>,
    /// The current document, or `None` after a delete.
    pub current: Option<StoredDoc>,
}

/// The action of an event of a secret (2.24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretAction {
    /// `set`
    Set,
    /// `delete`
    Delete,
}

/// One event of a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretEvent {
    /// The action.
    pub action: SecretAction,
    /// The subject.
    pub by: Subject,
    /// The time.
    pub at: UtcMillis,
}

/// One secret with its value and its events. A secret that was deleted has no value and keeps
/// its events (2.24, 2.40).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretExport {
    /// The scope.
    pub scope: SecretScope,
    /// The name.
    pub name: KeyPath,
    /// The value, or `None` for a secret that was deleted.
    pub value: Option<SecretValue>,
    /// The events, oldest first.
    pub events: Vec<SecretEvent>,
}

/// One run with its output and its events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunExport {
    /// The record.
    pub record: RunRecord,
    /// The output.
    pub output: Vec<u8>,
    /// The events.
    pub events: Vec<EventLine>,
}

/// The counts of an import or a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportReport {
    /// The keys with a current document or a version.
    pub keys: u64,
    /// The versions.
    pub versions: u64,
    /// The secrets.
    pub secrets: u64,
    /// The runs.
    pub runs: u64,
}

/// The reader of a consistent copy of a store.
pub trait ExportSource {
    /// The keys that have a current document or a version, in byte order.
    fn keys(&mut self) -> impl Future<Output = Result<Vec<DocKey>, StoreError>> + Send;
    /// One key with its versions.
    fn key(&mut self, key: &DocKey) -> impl Future<Output = Result<KeyHistory, StoreError>> + Send;
    /// Each secret with its value and its events, and each deleted secret with its events.
    fn secrets(&mut self) -> impl Future<Output = Result<Vec<SecretExport>, StoreError>> + Send;
    /// The run ids, oldest first.
    fn run_ids(&mut self) -> impl Future<Output = Result<Vec<RunId>, StoreError>> + Send;
    /// One run with its output and events.
    fn run(&mut self, id: &RunId) -> impl Future<Output = Result<RunExport, StoreError>> + Send;
    /// End the export.
    fn end(self) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// The writer of an import into an empty store.
pub trait ImportSink {
    /// Write one key with its versions and its current document.
    fn key(&mut self, history: &KeyHistory) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Write one secret with its events.
    fn secret(&mut self, secret: &SecretExport) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Write one run with its output and events.
    fn run(&mut self, run: &RunExport) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// End the import and give the counts.
    fn commit(self) -> impl Future<Output = Result<ImportReport, StoreError>> + Send;
}

/// The reader of a consistent copy of a store.
#[derive(Debug)]
pub enum Exporter {
    /// The backend `files`.
    Files(FilesExporter),
    /// The backend `postgres`.
    Postgres(Box<PgExporter>),
}

impl Exporter {
    /// The keys that have a current document or a version, in byte order.
    pub async fn keys(&mut self) -> Result<Vec<DocKey>, StoreError> {
        match self {
            Exporter::Files(e) => e.keys().await,
            Exporter::Postgres(e) => e.keys().await,
        }
    }
    /// One key with its versions.
    pub async fn key(&mut self, key: &DocKey) -> Result<KeyHistory, StoreError> {
        match self {
            Exporter::Files(e) => e.key(key).await,
            Exporter::Postgres(e) => e.key(key).await,
        }
    }
    /// Each secret with its value and its events.
    pub async fn secrets(&mut self) -> Result<Vec<SecretExport>, StoreError> {
        match self {
            Exporter::Files(e) => e.secrets().await,
            Exporter::Postgres(e) => e.secrets().await,
        }
    }
    /// The run ids, oldest first.
    pub async fn run_ids(&mut self) -> Result<Vec<RunId>, StoreError> {
        match self {
            Exporter::Files(e) => e.run_ids().await,
            Exporter::Postgres(e) => e.run_ids().await,
        }
    }
    /// One run with its output and events.
    pub async fn run(&mut self, id: &RunId) -> Result<RunExport, StoreError> {
        match self {
            Exporter::Files(e) => e.run(id).await,
            Exporter::Postgres(e) => e.run(id).await,
        }
    }
    /// End the export.
    pub async fn end(self) -> Result<(), StoreError> {
        match self {
            Exporter::Files(e) => e.end().await,
            Exporter::Postgres(e) => e.end().await,
        }
    }
}

impl ExportSource for Exporter {
    fn keys(&mut self) -> impl Future<Output = Result<Vec<DocKey>, StoreError>> + Send {
        Exporter::keys(self)
    }
    fn key(&mut self, key: &DocKey) -> impl Future<Output = Result<KeyHistory, StoreError>> + Send {
        Exporter::key(self, key)
    }
    fn secrets(&mut self) -> impl Future<Output = Result<Vec<SecretExport>, StoreError>> + Send {
        Exporter::secrets(self)
    }
    fn run_ids(&mut self) -> impl Future<Output = Result<Vec<RunId>, StoreError>> + Send {
        Exporter::run_ids(self)
    }
    fn run(&mut self, id: &RunId) -> impl Future<Output = Result<RunExport, StoreError>> + Send {
        Exporter::run(self, id)
    }
    fn end(self) -> impl Future<Output = Result<(), StoreError>> + Send {
        Exporter::end(self)
    }
}

/// The writer of an import into an empty store.
#[derive(Debug)]
pub enum Importer {
    /// The backend `files`.
    Files(FilesImporter),
    /// The backend `postgres`.
    Postgres(Box<PgImporter>),
}

impl Importer {
    /// Write one key with its versions and its current document.
    pub async fn key(&mut self, history: &KeyHistory) -> Result<(), StoreError> {
        match self {
            Importer::Files(i) => i.key(history).await,
            Importer::Postgres(i) => i.key(history).await,
        }
    }
    /// Write one secret with its events.
    pub async fn secret(&mut self, secret: &SecretExport) -> Result<(), StoreError> {
        match self {
            Importer::Files(i) => i.secret(secret).await,
            Importer::Postgres(i) => i.secret(secret).await,
        }
    }
    /// Write one run with its output and events.
    pub async fn run(&mut self, run: &RunExport) -> Result<(), StoreError> {
        match self {
            Importer::Files(i) => i.run(run).await,
            Importer::Postgres(i) => i.run(run).await,
        }
    }
    /// End the import and give the counts.
    pub async fn commit(self) -> Result<ImportReport, StoreError> {
        match self {
            Importer::Files(i) => i.commit().await,
            Importer::Postgres(i) => i.commit().await,
        }
    }
}

impl ImportSink for Importer {
    fn key(&mut self, history: &KeyHistory) -> impl Future<Output = Result<(), StoreError>> + Send {
        Importer::key(self, history)
    }
    fn secret(&mut self, secret: &SecretExport) -> impl Future<Output = Result<(), StoreError>> + Send {
        Importer::secret(self, secret)
    }
    fn run(&mut self, run: &RunExport) -> impl Future<Output = Result<(), StoreError>> + Send {
        Importer::run(self, run)
    }
    fn commit(self) -> impl Future<Output = Result<ImportReport, StoreError>> + Send {
        Importer::commit(self)
    }
}

/// The current documents that a transfer wrote, by key: the etag and the version.
pub type Expected = BTreeMap<DocKey, (Etag, u64)>;

/// Move everything from a source to a sink (2.40): the keys with their versions, the secrets,
/// then the runs. It ends the export, then it commits the import. It gives the counts and the
/// current documents that it wrote, for the comparison of `copy_store`.
pub async fn transfer<E: ExportSource, I: ImportSink>(
    mut source: E,
    mut sink: I,
) -> Result<(ImportReport, Expected), StoreError> {
    let mut expected = Expected::new();
    for key in source.keys().await? {
        let history = source.key(&key).await?;
        if let Some(current) = &history.current {
            expected.insert(key.clone(), (current.etag, current.version));
        }
        sink.key(&history).await?;
    }
    for secret in source.secrets().await? {
        sink.secret(&secret).await?;
    }
    for id in source.run_ids().await? {
        let run = source.run(&id).await?;
        sink.run(&run).await?;
    }
    source.end().await?;
    Ok((sink.commit().await?, expected))
}

/// Compare the current documents of a store with the documents that a transfer wrote: the same
/// keys, the same etags and the same version numbers (2.40).
pub async fn compare_current(target: &AnyStore, expected: &Expected) -> Result<(), StoreError> {
    let listing = target.load().await?;
    let mut found: Expected = BTreeMap::new();
    for doc in listing.docs {
        found.insert(doc.key, (doc.etag, doc.version));
    }
    for (key, want) in expected {
        match found.remove(key) {
            None => {
                return Err(StoreError::Corrupt(format!("the target has no document for {key} after the transfer")));
            }
            Some(got) if got != *want => {
                return Err(StoreError::Corrupt(format!(
                    "the etag or the version of {key} differs after the transfer"
                )));
            }
            Some(_) => {}
        }
    }
    if let Some((key, _)) = found.into_iter().next() {
        return Err(StoreError::Corrupt(format!("the target has the document {key} that the source has not")));
    }
    Ok(())
}

/// Copy a store into an empty store, then compare the etags (2.40). The target MUST be empty:
/// else `NotEmpty`, and nothing is written. A difference after the copy gives `Corrupt`.
pub async fn copy_store(from: &AnyStore, to: &AnyStore) -> Result<ImportReport, StoreError> {
    // The target first: a store that is not empty stops the command before the long read.
    let sink = to.import_begin().await?;
    let source = from.export_begin().await?;
    let (report, expected) = transfer(source, sink).await?;
    compare_current(to, &expected).await?;
    Ok(report)
}

fn io_error(what: &str, path: &Path, e: &std::io::Error) -> StoreError {
    StoreError::Unavailable(format!("{what} {}: {e}", path.display()))
}

/// The metadata file of a version in the layout of 2.31.
fn version_meta_bytes(version: &Version) -> Vec<u8> {
    let m = &version.meta;
    let mut members = vec![
        ("kind", Json::str(m.kind.name())),
        ("message", Json::str(m.meta.message.as_str())),
        ("saved_at", Json::str(m.meta.saved_at.to_string())),
        ("saved_by", Json::str(m.meta.saved_by.as_str())),
    ];
    if let Some(etag) = m.etag {
        members.push(("etag", Json::str(etag.to_hex())));
    }
    pupitre_core::json::to_canonical(&Json::object(members))
}

/// Write one file of the export: a new file, with the mode 0644, flushed to the disk.
async fn write_new(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .await
        .map_err(|e| io_error("cannot write", path, &e))?;
    file.write_all(bytes).await.map_err(|e| io_error("cannot write", path, &e))?;
    file.sync_all().await.map_err(|e| io_error("cannot flush", path, &e))
}

async fn make_dir(path: &Path) -> Result<(), StoreError> {
    tokio::fs::create_dir_all(path).await.map_err(|e| io_error("cannot make the folder", path, &e))
}

/// Write the documents and their versions into an empty folder, in the layout of 2.31 (2.40).
/// A folder that does not exist is made. A folder that is not empty gives `NotEmpty`. The
/// export writes no secret and no run, and no lock file: the backend `files` makes it at open.
pub async fn export_to_dir(from: &AnyStore, dir: &Path) -> Result<ImportReport, StoreError> {
    match tokio::fs::read_dir(dir).await {
        Ok(mut entries) => {
            if entries.next_entry().await.map_err(|e| io_error("cannot read", dir, &e))?.is_some() {
                return Err(StoreError::NotEmpty);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => make_dir(dir).await?,
        Err(e) => return Err(io_error("cannot read", dir, &e)),
    }
    let mut source = from.export_begin().await?;
    let mut report = ImportReport::default();
    let meta = Json::object([
        ("format", Json::Int(1)),
        ("store_schema", Json::Int(u64::from(crate::types::STORE_SCHEMA_VERSION))),
    ]);
    write_new(&dir.join(crate::files::STORE_META_FILE), &pupitre_core::json::to_canonical(&meta)).await?;
    for key in source.keys().await? {
        let history = source.key(&key).await?;
        if history.versions.is_empty() {
            continue;
        }
        let history_dir = dir.join(crate::files::history_dir(&key));
        make_dir(&history_dir).await?;
        for version in &history.versions {
            let file = crate::files::version_file(version.meta.version);
            if let Some(bytes) = &version.bytes {
                write_new(&history_dir.join(&file), bytes).await?;
            }
            let meta_file = file.replace(".json", ".meta.json");
            write_new(&history_dir.join(meta_file), &version_meta_bytes(version)).await?;
        }
        if let Some(current) = &history.current {
            let path = dir.join(key.file_path());
            if let Some(parent) = path.parent() {
                make_dir(parent).await?;
            }
            write_new(&path, &current.bytes).await?;
        }
        report.keys += 1;
        report.versions += u64::try_from(history.versions.len()).unwrap_or(u64::MAX);
    }
    source.end().await?;
    Ok(report)
}
