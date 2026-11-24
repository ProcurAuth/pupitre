// SPDX-License-Identifier: MIT OR Apache-2.0
//! The transfer of the backend `postgres` (2.40): a consistent read of the whole store, and a
//! write into an empty store in one transaction.
//!
//! Each side has its own connection, outside the pool. The export reads in one transaction at
//! the level `REPEATABLE READ`. The import writes in one transaction: a failure or a drop
//! before `commit` leaves the target empty.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use deadpool_postgres::{ClientWrapper, Object};
use pupitre_core::doc::DocKey;
use pupitre_core::etag::Etag;
use pupitre_core::ids::{RunId, Subject};
use pupitre_core::time::UtcMillis;
use tokio_postgres::types::ToSql;

use super::PgStore;
use super::rows::{VERSION_COLUMNS, etag_of, from_system, to_i64, to_system, to_u64, version_meta};
use super::runs::{decode, encode, run_id_of};
use super::secrets::{name_of, new_nonce, nonce_of, scope_of};
use crate::secret::{Sealed, StoreKeys};
use crate::transfer::{ImportReport, KeyHistory, RunExport, SecretAction, SecretEvent, SecretExport};
use crate::types::{EventLine, StoreError, StoredDoc, Version, VersionKind};

/// The most bytes of one chunk of output that the import writes.
const CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Take a connection out of the pool and open a transaction that no idle timeout ends: the
/// caller can need long between two calls.
async fn begin(store: &PgStore, mode: &str) -> Result<ClientWrapper, StoreError> {
    let client = Object::take(store.conn().await?);
    client.batch_execute(&format!("SET idle_in_transaction_session_timeout = 0; BEGIN {mode}")).await?;
    Ok(client)
}

/// The reader of a consistent copy of the store.
pub struct PgExporter {
    client: ClientWrapper,
    keys: Arc<StoreKeys>,
}

impl fmt::Debug for PgExporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgExporter")
    }
}

impl PgExporter {
    /// The keys that have a current document or a version, in byte order.
    pub async fn keys(&mut self) -> Result<Vec<DocKey>, StoreError> {
        let rows = self.client.query("SELECT key FROM documents ORDER BY key COLLATE \"C\"", &[]).await?;
        let mut keys = Vec::with_capacity(rows.len());
        for row in &rows {
            let text: String = row.try_get(0)?;
            // A row that is not a key of 2.3 is not part of the store: the poll ignores it too.
            if let Ok(key) = DocKey::parse(&text) {
                keys.push(key);
            }
        }
        Ok(keys)
    }

    /// One key with its versions and its current document.
    pub async fn key(&mut self, key: &DocKey) -> Result<KeyHistory, StoreError> {
        let text = key.text();
        let rows = self
            .client
            .query(
                &format!("SELECT {VERSION_COLUMNS}, bytes FROM document_versions WHERE key = $1 ORDER BY version"),
                &[&text],
            )
            .await?;
        let mut versions = Vec::with_capacity(rows.len());
        for row in &rows {
            versions.push(Version { meta: version_meta(row)?, bytes: row.try_get(6)? });
        }
        let row = self
            .client
            .query_opt("SELECT bytes, etag, version FROM documents WHERE key = $1 AND bytes IS NOT NULL", &[&text])
            .await?;
        let current = match row {
            Some(row) => {
                let etag: Vec<u8> = row.try_get(1)?;
                let version: i64 = row.try_get(2)?;
                Some(StoredDoc {
                    key: key.clone(),
                    bytes: row.try_get(0)?,
                    etag: etag_of(&etag, "documents.etag")?,
                    version: to_u64(version, "documents.version")?,
                })
            }
            None => None,
        };
        Ok(KeyHistory { key: key.clone(), versions, current })
    }

    /// Each secret with its value and its events. A value that does not decrypt gives
    /// `SecretUnreadable`.
    pub async fn secrets(&mut self) -> Result<Vec<SecretExport>, StoreError> {
        let mut events: BTreeMap<(String, String), Vec<SecretEvent>> = BTreeMap::new();
        let rows = self
            .client
            .query("SELECT scope, name, action, saved_by, saved_at FROM secret_events ORDER BY id", &[])
            .await?;
        for row in &rows {
            let (scope, name): (String, String) = (row.try_get(0)?, row.try_get(1)?);
            let action: String = row.try_get(2)?;
            let by: String = row.try_get(3)?;
            let at: SystemTime = row.try_get(4)?;
            let action = match action.as_str() {
                "set" => SecretAction::Set,
                "delete" => SecretAction::Delete,
                _ => return Err(StoreError::Corrupt("secret_events.action".to_string())),
            };
            events.entry((scope, name)).or_default().push(SecretEvent {
                action,
                by: Subject::parse(&by).map_err(|_| StoreError::Corrupt("secret_events.saved_by".to_string()))?,
                at: from_system(at),
            });
        }
        let rows = self
            .client
            .query(
                "SELECT scope, name, nonce, ciphertext, fingerprint FROM secrets ORDER BY scope COLLATE \"C\", name COLLATE \"C\"",
                &[],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let (scope_text, name_text): (String, String) = (row.try_get(0)?, row.try_get(1)?);
            let nonce: Vec<u8> = row.try_get(2)?;
            let sealed: Vec<u8> = row.try_get(3)?;
            let fingerprint: String = row.try_get(4)?;
            let (scope, name) = (scope_of(&scope_text)?, name_of(&name_text)?);
            let value = self
                .keys
                .open(&scope, &name, &fingerprint, nonce_of(&nonce)?, &sealed)
                .map_err(|_| StoreError::SecretUnreadable)?;
            let events = events.remove(&(scope_text, name_text)).unwrap_or_default();
            out.push(SecretExport { scope, name, value: Some(value), events });
        }
        // The events that remain belong to secrets that were deleted. They go with the transfer.
        for ((scope_text, name_text), events) in events {
            let (scope, name) = (scope_of(&scope_text)?, name_of(&name_text)?);
            out.push(SecretExport { scope, name, value: None, events });
        }
        out.sort_by(|a, b| (a.scope.text(), a.name.as_str()).cmp(&(b.scope.text(), b.name.as_str())));
        Ok(out)
    }

    /// The run ids, oldest first.
    pub async fn run_ids(&mut self) -> Result<Vec<RunId>, StoreError> {
        let rows = self.client.query("SELECT id FROM runs ORDER BY id COLLATE \"C\"", &[]).await?;
        rows.iter().map(|r| run_id_of(&r.try_get::<_, String>(0)?)).collect()
    }

    /// One run with its output and events.
    pub async fn run(&mut self, id: &RunId) -> Result<RunExport, StoreError> {
        let text = id.to_string();
        let row = self.client.query_opt("SELECT record FROM runs WHERE id = $1", &[&text]).await?;
        let Some(row) = row else { return Err(StoreError::NotFound) };
        let record = decode(&row.try_get::<_, Vec<u8>>(0)?)?;
        let chunks = self.client.query("SELECT bytes FROM run_output WHERE run_id = $1 ORDER BY seq", &[&text]).await?;
        let mut output = Vec::new();
        for chunk in &chunks {
            output.extend_from_slice(&chunk.try_get::<_, Vec<u8>>(0)?);
        }
        let rows =
            self.client.query("SELECT seq, line FROM run_events WHERE run_id = $1 ORDER BY seq", &[&text]).await?;
        let mut events = Vec::with_capacity(rows.len());
        for row in &rows {
            events
                .push(EventLine { seq: to_u64(row.try_get::<_, i64>(0)?, "run_events.seq")?, bytes: row.try_get(1)? });
        }
        Ok(RunExport { record, output, events })
    }

    /// End the export. The connection closes.
    pub async fn end(self) -> Result<(), StoreError> {
        self.client.batch_execute("COMMIT").await?;
        Ok(())
    }
}

/// The writer of an import into an empty store.
pub struct PgImporter {
    client: ClientWrapper,
    keys: Arc<StoreKeys>,
    report: ImportReport,
}

impl fmt::Debug for PgImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgImporter").field("report", &self.report).finish_non_exhaustive()
    }
}

/// The source of a key must be a consistent history (2.18, 2.40): the numbers rise, a delete has
/// no bytes, and each etag is the SHA-256 of its bytes. The current document is the last version.
fn check_history(history: &KeyHistory) -> Result<(), StoreError> {
    let bad =
        |what: &str| StoreError::Corrupt(format!("the key {} has a history that is not valid: {what}", history.key));
    let mut last = 0u64;
    for v in &history.versions {
        if v.meta.version <= last {
            return Err(bad("the version numbers do not rise"));
        }
        last = v.meta.version;
        match (&v.bytes, v.meta.etag, v.meta.kind) {
            (None, None, VersionKind::Delete) => {}
            (Some(bytes), Some(etag), kind) if kind != VersionKind::Delete && Etag::of(bytes) == etag => {}
            _ => return Err(bad("a version does not match its kind or its etag")),
        }
    }
    let newest = history.versions.last();
    match (&history.current, newest) {
        (Some(current), Some(v)) => {
            if current.version != v.meta.version
                || v.bytes.as_deref() != Some(current.bytes.as_slice())
                || v.meta.etag != Some(current.etag)
            {
                return Err(bad("the current document is not the newest version"));
            }
        }
        (None, Some(v)) if v.meta.kind != VersionKind::Delete => {
            return Err(bad("a key with no current document has a newest version that is not a delete"));
        }
        (Some(_), None) => return Err(bad("a current document has no version")),
        _ => {}
    }
    Ok(())
}

impl PgImporter {
    async fn exec(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<u64, StoreError> {
        let statement = self.client.prepare_cached(sql).await?;
        Ok(self.client.execute(&statement, params).await?)
    }

    /// Write one key with its versions and its current document.
    pub async fn key(&mut self, history: &KeyHistory) -> Result<(), StoreError> {
        check_history(history)?;
        let Some(newest) = history.versions.last() else { return Ok(()) };
        let key = history.key.text();
        let mut numbers = Vec::new();
        let mut kinds = Vec::new();
        let mut bytes = Vec::new();
        let mut etags: Vec<Option<Vec<u8>>> = Vec::new();
        let mut users = Vec::new();
        let mut times = Vec::new();
        let mut messages = Vec::new();
        for v in &history.versions {
            numbers.push(to_i64(v.meta.version, "version")?);
            kinds.push(v.meta.kind.name());
            bytes.push(v.bytes.clone());
            etags.push(v.meta.etag.map(|e| e.as_bytes().to_vec()));
            users.push(v.meta.meta.saved_by.as_str());
            times.push(to_system(v.meta.meta.saved_at));
            messages.push(v.meta.meta.message.as_str());
        }
        self.exec(
            "INSERT INTO document_versions (key, version, kind, bytes, etag, saved_by, saved_at, message) \
             SELECT $1, v, k, b, e, u, t, m FROM unnest($2::bigint[], $3::text[], $4::bytea[], $5::bytea[], \
             $6::text[], $7::timestamptz[], $8::text[]) AS x(v, k, b, e, u, t, m)",
            &[&key, &numbers, &kinds, &bytes, &etags, &users, &times, &messages],
        )
        .await?;
        let current = history.current.as_ref();
        let current_etag = current.map(|d| d.etag.as_bytes().as_slice());
        let current_bytes = current.map(|d| d.bytes.as_slice());
        self.exec(
            "INSERT INTO documents (key, kind, bytes, etag, version, change_seq, updated_at) \
             VALUES ($1, $2, $3, $4, $5, nextval('pupitre_change_seq'), $6)",
            &[
                &key,
                &history.key.kind().name(),
                &current_bytes,
                &current_etag,
                &to_i64(newest.meta.version, "version")?,
                &to_system(newest.meta.meta.saved_at),
            ],
        )
        .await?;
        self.report.keys += 1;
        self.report.versions += u64::try_from(history.versions.len()).unwrap_or(u64::MAX);
        Ok(())
    }

    /// Write one secret with its events. The value is encrypted with the key of this store. A
    /// deleted secret has no value: the import writes its events only.
    pub async fn secret(&mut self, secret: &SecretExport) -> Result<(), StoreError> {
        let (scope, name) = (secret.scope.text(), secret.name.as_str());
        if let Some(value) = &secret.value {
            let nonce = new_nonce()?;
            let Sealed { ciphertext: sealed, fingerprint } = self
                .keys
                .seal(&secret.scope, &secret.name, value, nonce)
                .map_err(|_| StoreError::Unavailable("the secret value cannot be encrypted".to_string()))?;
            let (by, at) = match secret.events.last() {
                Some(e) => (e.by.clone(), e.at),
                None => (
                    Subject::parse(Subject::IMPORT).map_err(|_| StoreError::Corrupt("subject".to_string()))?,
                    UtcMillis::now(),
                ),
            };
            self.exec(
                "INSERT INTO secrets (scope, name, nonce, ciphertext, fingerprint, updated_by, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[&scope, &name, &nonce.as_slice(), &sealed, &fingerprint, &by.as_str(), &to_system(at)],
            )
            .await?;
            self.report.secrets += 1;
        }
        for event in &secret.events {
            let action = match event.action {
                SecretAction::Set => "set",
                SecretAction::Delete => "delete",
            };
            self.exec(
                "INSERT INTO secret_events (scope, name, action, saved_by, saved_at) VALUES ($1, $2, $3, $4, $5)",
                &[&scope, &name, &action, &event.by.as_str(), &to_system(event.at)],
            )
            .await?;
        }
        Ok(())
    }

    /// Write one run with its output and events.
    pub async fn run(&mut self, run: &RunExport) -> Result<(), StoreError> {
        let id = run.record.id.to_string();
        for (n, line) in run.events.iter().enumerate() {
            if line.seq != u64::try_from(n + 1).unwrap_or(u64::MAX) {
                return Err(StoreError::Corrupt(format!(
                    "the events of the run {id} are not numbered from 1 with no gap"
                )));
            }
        }
        self.exec(
            "INSERT INTO runs (id, environment, status, record) VALUES ($1, $2, $3, $4)",
            &[&id, &run.record.environment.as_str(), &run.record.status.name(), &encode(&run.record)],
        )
        .await?;
        let mut offset = 0usize;
        for (n, chunk) in run.output.chunks(CHUNK_BYTES).enumerate() {
            self.exec(
                "INSERT INTO run_output (run_id, seq, byte_offset, bytes) VALUES ($1, $2, $3, $4)",
                &[
                    &id,
                    &to_i64(u64::try_from(n + 1).unwrap_or(u64::MAX), "seq")?,
                    &to_i64(u64::try_from(offset).unwrap_or(u64::MAX), "offset")?,
                    &chunk,
                ],
            )
            .await?;
            offset += chunk.len();
        }
        if !run.events.is_empty() {
            let seqs: Vec<i64> = run.events.iter().map(|l| to_i64(l.seq, "seq")).collect::<Result<_, _>>()?;
            let lines: Vec<&[u8]> = run.events.iter().map(|l| l.bytes.as_slice()).collect();
            self.exec(
                "INSERT INTO run_events (run_id, seq, line) SELECT $1, s, l FROM unnest($2::bigint[], $3::bytea[]) AS t(s, l)",
                &[&id, &seqs, &lines],
            )
            .await?;
        }
        self.report.runs += 1;
        Ok(())
    }

    /// End the import and give the counts. Nothing is visible before this call.
    pub async fn commit(self) -> Result<ImportReport, StoreError> {
        self.client.batch_execute("COMMIT").await?;
        Ok(self.report)
    }
}

impl PgStore {
    /// Start a consistent read of the whole store (2.40).
    pub(crate) async fn begin_export(&self) -> Result<PgExporter, StoreError> {
        let client = begin(self, "ISOLATION LEVEL REPEATABLE READ READ ONLY").await?;
        Ok(PgExporter { client, keys: Arc::clone(&self.keys) })
    }

    /// Start a write into an empty store, or `NotEmpty` (2.40). The tables are locked against
    /// other writers until `commit`.
    pub(crate) async fn begin_import(&self) -> Result<PgImporter, StoreError> {
        let client = begin(self, "ISOLATION LEVEL READ COMMITTED").await?;
        // The barrier of 2.38: the import takes change numbers, so it joins it first.
        client.execute(super::docs::WRITER_LOCK, &[&super::docs::CHANGE_LOCK]).await?;
        client
            .batch_execute(
                "LOCK TABLE documents, document_versions, secrets, secret_events, runs IN SHARE ROW EXCLUSIVE MODE",
            )
            .await?;
        let row = client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM documents) OR EXISTS (SELECT 1 FROM document_versions) \
                 OR EXISTS (SELECT 1 FROM secrets) OR EXISTS (SELECT 1 FROM secret_events) \
                 OR EXISTS (SELECT 1 FROM runs)",
                &[],
            )
            .await?;
        if row.try_get::<_, bool>(0)? {
            return Err(StoreError::NotEmpty);
        }
        Ok(PgImporter { client, keys: Arc::clone(&self.keys), report: ImportReport::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, VersionMeta, WriteMeta};
    use pupitre_core::ids::EnvId;

    fn version(n: u64, kind: VersionKind, bytes: Option<&[u8]>) -> Version {
        Version {
            meta: VersionMeta {
                version: n,
                kind,
                etag: bytes.map(Etag::of),
                meta: WriteMeta {
                    saved_by: Subject::parse("alice").unwrap(),
                    saved_at: UtcMillis(1_000),
                    message: Message::default(),
                },
            },
            bytes: bytes.map(<[u8]>::to_vec),
        }
    }

    fn history(versions: Vec<Version>, current: Option<StoredDoc>) -> KeyHistory {
        KeyHistory { key: DocKey::Environment(EnvId::parse("production").unwrap()), versions, current }
    }

    fn doc(n: u64, bytes: &[u8]) -> StoredDoc {
        StoredDoc {
            key: DocKey::Environment(EnvId::parse("production").unwrap()),
            bytes: bytes.to_vec(),
            etag: Etag::of(bytes),
            version: n,
        }
    }

    #[test]
    fn a_consistent_history_passes() {
        let h = history(
            vec![version(1, VersionKind::Create, Some(b"a")), version(2, VersionKind::Save, Some(b"b"))],
            Some(doc(2, b"b")),
        );
        assert!(check_history(&h).is_ok());
        let deleted =
            history(vec![version(1, VersionKind::Create, Some(b"a")), version(2, VersionKind::Delete, None)], None);
        assert!(check_history(&deleted).is_ok());
    }

    #[test]
    fn an_inconsistent_history_stops_the_transfer() {
        let two = || vec![version(1, VersionKind::Create, Some(b"a")), version(2, VersionKind::Save, Some(b"b"))];
        // The current document is not the newest version.
        assert!(check_history(&history(two(), Some(doc(1, b"a")))).is_err());
        // The numbers do not rise.
        let flat = vec![version(2, VersionKind::Create, Some(b"a")), version(2, VersionKind::Save, Some(b"b"))];
        assert!(check_history(&history(flat, Some(doc(2, b"b")))).is_err());
        // A delete with bytes.
        assert!(check_history(&history(vec![version(1, VersionKind::Delete, Some(b"a"))], None)).is_err());
        // A wrong etag.
        let mut wrong = version(1, VersionKind::Create, Some(b"a"));
        wrong.meta.etag = Some(Etag::of(b"x"));
        assert!(check_history(&history(vec![wrong], Some(doc(1, b"a")))).is_err());
        // A live key whose newest version is a delete, and a current document with no version.
        assert!(check_history(&history(vec![version(1, VersionKind::Create, Some(b"a"))], None)).is_err());
        assert!(check_history(&history(vec![], Some(doc(1, b"a")))).is_err());
    }
}
