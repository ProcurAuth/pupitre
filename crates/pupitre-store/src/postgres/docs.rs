// SPDX-License-Identifier: MIT OR Apache-2.0
//! The documents of the backend `postgres`: writes, reads, history and poll (2.18 to 2.23,
//! 2.37, 2.38).

use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::etag::Etag;
use pupitre_core::ids::EnvId;
use tokio_postgres::Row;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::ToSql;

use super::PgStore;
use super::conn::has_code;
use super::rows::{VERSION_COLUMNS, etag_of, opt_etag, to_i64, to_system, to_u64, version_meta};
use crate::types::{
    Delta, Listing, Marks, MarksInner, Precondition, Saved, StoreError, StoreEvent, StoredDoc, Version, VersionKind,
    VersionMeta, WriteMeta,
};

const LOCK_ROW: &str = "SELECT version, etag FROM documents WHERE key = $1 FOR UPDATE";
const INSERT_VERSION: &str = "INSERT INTO document_versions \
    (key, version, kind, bytes, etag, saved_by, saved_at, message) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)";
const UPSERT_DOCUMENT: &str = "INSERT INTO documents (key, kind, bytes, etag, version, change_seq, updated_at) \
    VALUES ($1, $2, $3, $4, $5, nextval('pupitre_change_seq'), clock_timestamp()) \
    ON CONFLICT (key) DO UPDATE SET bytes = EXCLUDED.bytes, etag = EXCLUDED.etag, version = EXCLUDED.version, \
    change_seq = EXCLUDED.change_seq, updated_at = EXCLUDED.updated_at";
const PRUNE_VERSIONS: &str = "DELETE FROM document_versions WHERE key = $1 AND version <= $2";
/// The most events that the store keeps for the log between two calls of `drain_events`.
const MAX_EVENTS: usize = 1000;
const DOCUMENT_COLUMNS: &str = "key, bytes, etag, version, change_seq";
/// The advisory lock of the change barrier (2.37, 2.38). A writer takes it in shared mode before
/// it takes a change number. A load or a poll takes it in exclusive mode for the time of its read.
pub(crate) const CHANGE_LOCK: i64 = 0x5075_7069_7472_6332;
/// A writer joins the barrier. The lock ends with the transaction.
pub(crate) const WRITER_LOCK: &str = "SELECT pg_advisory_xact_lock_shared($1)";
/// A reader of the change numbers waits for each writer that began before it.
const READER_LOCK: &str = "SELECT pg_advisory_xact_lock($1)";

impl PgStore {
    /// A stored key that is not a key of 2.3: the store ignores the row and keeps an event.
    fn parse_key(&self, text: &str) -> Option<DocKey> {
        match DocKey::parse(text) {
            Ok(key) => Some(key),
            Err(e) => {
                // The log takes the events at each poll. The bound keeps a store that nobody drains small.
                if let Ok(mut events) = self.events.lock()
                    && events.len() < MAX_EVENTS
                {
                    events.push(StoreEvent::EntryIgnored { path: text.to_string(), reason: e.to_string() });
                }
                None
            }
        }
    }

    /// The rows of `documents` as live documents and removed keys, with the highest change number.
    fn split_rows(&self, rows: &[Row], marks: i64) -> Result<(Vec<StoredDoc>, Vec<DocKey>, i64), StoreError> {
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        let mut highest = marks;
        for row in rows {
            let key: String = row.try_get(0)?;
            let bytes: Option<Vec<u8>> = row.try_get(1)?;
            let etag: Option<Vec<u8>> = row.try_get(2)?;
            let version: i64 = row.try_get(3)?;
            let seq: i64 = row.try_get(4)?;
            highest = highest.max(seq);
            let Some(key) = self.parse_key(&key) else { continue };
            match (bytes, etag) {
                (Some(bytes), Some(etag)) => changed.push(StoredDoc {
                    key,
                    bytes,
                    etag: etag_of(&etag, "documents.etag")?,
                    version: to_u64(version, "documents.version")?,
                }),
                _ => removed.push(key),
            }
        }
        Ok((changed, removed, highest))
    }

    /// Read each row of `documents` that has a change number above `since`, at a time when no
    /// writer has a change number that is not committed (2.38). The exclusive lock waits for
    /// each writer that began before. A writer that begins later waits for the end of this read.
    /// The read is one statement, so it sees one consistent state.
    async fn read_changes(&self, since: i64) -> Result<Vec<Row>, StoreError> {
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        tx.execute(READER_LOCK, &[&CHANGE_LOCK]).await?;
        let rows = tx
            .query(
                &format!("SELECT {DOCUMENT_COLUMNS} FROM documents WHERE change_seq > $1 ORDER BY key COLLATE \"C\""),
                &[&since],
            )
            .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Give each current document and the marks of this state (2.22). The load takes the barrier
    /// of 2.38, so the marks cover each change that a writer committed or will commit with a lower number.
    pub(crate) async fn load_docs(&self) -> Result<Listing, StoreError> {
        let rows = self.read_changes(i64::MIN).await?;
        let (docs, _removed, highest) = self.split_rows(&rows, 0)?;
        Ok(Listing { docs, marks: Marks { inner: MarksInner::Postgres { change_seq: highest } } })
    }

    /// Give the changes since the marks (2.22, 2.38). A transaction takes its change number
    /// before its commit. The barrier makes the poll wait until each such transaction ends, so
    /// no later commit has a number below the marks that the poll gives.
    pub(crate) async fn poll_docs(&self, marks: &Marks) -> Result<Delta, StoreError> {
        let since = match &marks.inner {
            MarksInner::Empty => 0,
            MarksInner::Postgres { change_seq } => *change_seq,
            MarksInner::Files(_) => {
                return Err(StoreError::Corrupt("the marks come from the backend files".to_string()));
            }
        };
        let rows = self.read_changes(since).await?;
        let (changed, removed, highest) = self.split_rows(&rows, since)?;
        Ok(Delta { changed, removed, marks: Marks { inner: MarksInner::Postgres { change_seq: highest } } })
    }

    /// Give the current document of a key, or `NotFound`.
    pub(crate) async fn get_doc(&self, key: &DocKey) -> Result<StoredDoc, StoreError> {
        let conn = self.conn().await?;
        let row = conn
            .query_opt(
                "SELECT bytes, etag, version FROM documents WHERE key = $1 AND bytes IS NOT NULL",
                &[&key.text()],
            )
            .await?;
        let Some(row) = row else { return Err(StoreError::NotFound) };
        let bytes: Vec<u8> = row.try_get(0)?;
        let etag: Vec<u8> = row.try_get(1)?;
        let version: i64 = row.try_get(2)?;
        Ok(StoredDoc {
            key: key.clone(),
            bytes,
            etag: etag_of(&etag, "documents.etag")?,
            version: to_u64(version, "documents.version")?,
        })
    }

    /// Write a document (2.19, 2.20, 2.37). The check and the write are one transaction. The
    /// row of the key is locked, so two writers with one base etag cannot both succeed.
    pub(crate) async fn put_doc(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        let kind = match precondition {
            Precondition::Create => VersionKind::Create,
            Precondition::Match(_) => VersionKind::Save,
        };
        self.write_version(key, Some(doc), precondition, kind, meta).await
    }

    /// Create a document with the version kind `import` (6.24a).
    pub(crate) async fn put_import_doc(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        self.write_version(key, Some(doc), &Precondition::Create, VersionKind::Import, meta).await
    }

    /// Delete a document: a version `delete`, then no current document (2.21).
    pub(crate) async fn delete_doc(
        &self,
        key: &DocKey,
        if_match: &Etag,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        self.write_version(key, None, &Precondition::Match(*if_match), VersionKind::Delete, meta).await
    }

    async fn write_version(
        &self,
        key: &DocKey,
        doc: Option<&CanonicalDocument>,
        precondition: &Precondition,
        kind: VersionKind,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        let key_text = key.text();
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        // The barrier of 2.38: join it before the first change number of this transaction.
        tx.execute(WRITER_LOCK, &[&CHANGE_LOCK]).await?;
        let row = tx.query_opt(LOCK_ROW, &[&key_text]).await?;
        let (previous, current): (i64, Option<Etag>) = match row {
            Some(row) => {
                let version: i64 = row.try_get(0)?;
                let etag: Option<Vec<u8>> = row.try_get(1)?;
                (version, opt_etag(etag, "documents.etag")?)
            }
            None => (0, None),
        };
        match (precondition, &current) {
            (Precondition::Create, Some(_)) => return Err(StoreError::AlreadyExists),
            (Precondition::Match(_), None) => return Err(StoreError::NotFound),
            (Precondition::Match(expected), Some(current)) if expected != current => {
                return Err(StoreError::Conflict { current: Some(*current) });
            }
            _ => {}
        }
        let version = previous + 1;
        let etag = doc.map(|d| d.etag);
        let bytes = doc.map(|d| d.bytes.as_slice());
        let etag_bytes = etag.as_ref().map(|e| e.as_bytes().as_slice());
        let params: [&(dyn ToSql + Sync); 8] = [
            &key_text,
            &version,
            &kind.name(),
            &bytes,
            &etag_bytes,
            &meta.saved_by.as_str(),
            &to_system(meta.saved_at),
            &meta.message.as_str(),
        ];
        match tx.execute(INSERT_VERSION, &params).await {
            Ok(_) => {}
            Err(e) if has_code(&e, &SqlState::UNIQUE_VIOLATION) => return Err(StoreError::AlreadyExists),
            Err(e) => return Err(e.into()),
        }
        #[cfg(any(test, feature = "conformance"))]
        self.faults.after_version()?;
        let doc_kind = key.kind().name();
        tx.execute(UPSERT_DOCUMENT, &[&key_text, &doc_kind, &bytes, &etag_bytes, &version]).await?;
        #[cfg(any(test, feature = "conformance"))]
        self.faults.before_commit().await;
        let keep = i64::from(self.options.history_keep);
        tx.execute(PRUNE_VERSIONS, &[&key_text, &(version - keep)]).await?;
        tx.commit().await?;
        Ok(Saved { version: to_u64(version, "version")?, etag })
    }

    /// Give the versions of a key, newest first, below `before`, at most `limit` (5.13).
    pub(crate) async fn history_of(
        &self,
        key: &DocKey,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<VersionMeta>, StoreError> {
        let before = before.map(|b| to_i64(b, "before")).transpose()?;
        let conn = self.conn().await?;
        let rows = conn
            .query(
                &format!(
                    "SELECT {VERSION_COLUMNS} FROM document_versions WHERE key = $1 \
                     AND ($2::bigint IS NULL OR version < $2) ORDER BY version DESC LIMIT $3"
                ),
                &[&key.text(), &before, &i64::from(limit)],
            )
            .await?;
        rows.iter().map(version_meta).collect()
    }

    /// Give one version with its bytes, or `NotFound`.
    pub(crate) async fn version_of(&self, key: &DocKey, version: u64) -> Result<Version, StoreError> {
        let conn = self.conn().await?;
        let row = conn
            .query_opt(
                &format!("SELECT {VERSION_COLUMNS}, bytes FROM document_versions WHERE key = $1 AND version = $2"),
                &[&key.text(), &to_i64(version, "version")?],
            )
            .await?;
        let Some(row) = row else { return Err(StoreError::NotFound) };
        let bytes: Option<Vec<u8>> = row.try_get(6)?;
        Ok(Version { meta: version_meta(&row)?, bytes })
    }

    /// Give each current document of an environment in one consistent state (2.23). The read
    /// has the level `REPEATABLE READ`.
    pub(crate) async fn read_env(&self, env: &EnvId) -> Result<Vec<StoredDoc>, StoreError> {
        let key = format!("environments/{env}");
        let prefix = format!("{key}/");
        let mut conn = self.conn().await?;
        let tx = conn
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .await?;
        let rows = tx
            .query(
                &format!(
                    "SELECT {DOCUMENT_COLUMNS} FROM documents WHERE bytes IS NOT NULL \
                     AND (key = $1 OR starts_with(key, $2)) ORDER BY key COLLATE \"C\""
                ),
                &[&key, &prefix],
            )
            .await?;
        tx.commit().await?;
        Ok(self.split_rows(&rows, 0)?.0)
    }
}
