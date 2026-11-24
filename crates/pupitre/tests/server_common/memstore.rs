// SPDX-License-Identifier: MIT OR Apache-2.0
//! An in-memory `Store` for the tests of the server. It follows 2.17 to 2.29 for the methods that
//! the server calls: documents with versions and preconditions, secrets, runs, output and events.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::etag::Etag;
use pupitre_core::ids::{EnvId, KeyPath, RunId};
use pupitre_core::node::SecretScope;
use pupitre_core::run::{RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::secret::SecretValue;
use pupitre_store::transfer::{Exporter, Importer};
use pupitre_store::types::{
    Delta, EventLine, LeaseName, LeaseOutcome, LeaseRenewal, Listing, Marks, OutputChunk, Precondition, RunFilter,
    Saved, SecretMeta, StoreError, StoredDoc, Version, VersionKind, VersionMeta, WriteMeta,
};
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct VersionRec {
    version: u64,
    kind: VersionKind,
    etag: Option<Etag>,
    bytes: Option<Vec<u8>>,
    meta: WriteMeta,
}

#[derive(Default)]
struct Inner {
    docs: BTreeMap<DocKey, Vec<VersionRec>>,
    secrets: BTreeMap<(String, String), (SecretValue, SecretMeta)>,
    runs: BTreeMap<RunId, RunRecord>,
    outputs: BTreeMap<RunId, Vec<u8>>,
    events: BTreeMap<RunId, Vec<EventLine>>,
    changed: Vec<StoredDoc>,
    removed: Vec<DocKey>,
}

/// The store of the tests.
#[derive(Default)]
pub struct MemStore {
    inner: Mutex<Inner>,
    /// When `true`, each call gives `Unavailable`.
    pub down: AtomicBool,
    /// The count of the calls of `poll`.
    pub polls: AtomicU64,
    /// The count of the calls of each method that the tests watch.
    pub calls: Mutex<BTreeMap<&'static str, u64>>,
    /// A wait after a document write is committed and before the call returns. A backend that
    /// writes in a blocking task commits even when its caller is gone: so does this wait.
    pub write_delay: Mutex<Duration>,
}

fn current(versions: &[VersionRec]) -> Option<&VersionRec> {
    versions.last().filter(|v| v.bytes.is_some())
}

impl MemStore {
    /// A new empty store.
    pub fn new() -> MemStore {
        MemStore::default()
    }

    fn check(&self, method: &'static str) -> Result<(), StoreError> {
        *self.calls.lock().unwrap().entry(method).or_default() += 1;
        if self.down.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("the test store is down".to_string()));
        }
        Ok(())
    }

    fn commit_put(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        let mut g = self.inner.lock().unwrap();
        let versions = g.docs.entry(key.clone()).or_default();
        let cur = current(versions).cloned();
        let kind = match (precondition, &cur) {
            (Precondition::Create, Some(_)) => return Err(StoreError::AlreadyExists),
            (Precondition::Create, None) => VersionKind::Create,
            (Precondition::Match(_), None) => return Err(StoreError::NotFound),
            (Precondition::Match(e), Some(c)) if c.etag != Some(*e) => {
                return Err(StoreError::Conflict { current: c.etag });
            }
            (Precondition::Match(_), Some(_)) => VersionKind::Save,
        };
        let version = versions.last().map_or(1, |v| v.version + 1);
        versions.push(VersionRec {
            version,
            kind,
            etag: Some(doc.etag),
            bytes: Some(doc.bytes.clone()),
            meta: meta.clone(),
        });
        Ok(Saved { version, etag: Some(doc.etag) })
    }

    fn commit_delete(&self, key: &DocKey, if_match: &Etag, meta: &WriteMeta) -> Result<Saved, StoreError> {
        let mut g = self.inner.lock().unwrap();
        let versions = g.docs.get_mut(key).ok_or(StoreError::NotFound)?;
        let cur = current(versions).cloned().ok_or(StoreError::NotFound)?;
        if cur.etag != Some(*if_match) {
            return Err(StoreError::Conflict { current: cur.etag });
        }
        let version = versions.last().map_or(1, |v| v.version + 1);
        versions.push(VersionRec { version, kind: VersionKind::Delete, etag: None, bytes: None, meta: meta.clone() });
        Ok(Saved { version, etag: None })
    }

    async fn after_commit(&self) {
        let delay = *self.write_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }

    /// The count of calls of a method.
    pub fn count(&self, method: &str) -> u64 {
        self.calls.lock().unwrap().get(method).copied().unwrap_or(0)
    }

    /// Write a document as another process would: the next poll reports it.
    pub fn external_put(&self, key: &DocKey, bytes: Vec<u8>) {
        let mut g = self.inner.lock().unwrap();
        let etag = Etag::of(&bytes);
        let versions = g.docs.entry(key.clone()).or_default();
        let version = versions.last().map_or(1, |v| v.version + 1);
        versions.push(VersionRec {
            version,
            kind: VersionKind::External,
            etag: Some(etag),
            bytes: Some(bytes.clone()),
            meta: meta("external"),
        });
        g.changed.push(StoredDoc { key: key.clone(), bytes, etag, version });
    }

    /// Remove a document as another process would.
    pub fn external_remove(&self, key: &DocKey) {
        let mut g = self.inner.lock().unwrap();
        if let Some(versions) = g.docs.get_mut(key) {
            let version = versions.last().map_or(1, |v| v.version + 1);
            versions.push(VersionRec {
                version,
                kind: VersionKind::Delete,
                etag: None,
                bytes: None,
                meta: meta("external"),
            });
        }
        g.removed.push(key.clone());
    }

    /// Write a document before the server starts: no change waits for the poll.
    pub fn seed_doc(&self, key: &str, doc: &pupitre_core::doc::Document) {
        let key = DocKey::parse(key).unwrap();
        let canonical = doc.canonical();
        let mut g = self.inner.lock().unwrap();
        let versions = g.docs.entry(key).or_default();
        let version = versions.last().map_or(1, |v| v.version + 1);
        versions.push(VersionRec {
            version,
            kind: VersionKind::Create,
            etag: Some(canonical.etag),
            bytes: Some(canonical.bytes),
            meta: meta("seed"),
        });
    }

    /// Put a run record as the runner node would.
    pub fn seed_run(&self, record: RunRecord) {
        self.inner.lock().unwrap().runs.insert(record.id, record);
    }

    /// The record of a run.
    pub fn run(&self, id: &RunId) -> Option<RunRecord> {
        self.inner.lock().unwrap().runs.get(id).cloned()
    }

    /// Every record, newest first.
    pub fn all_runs(&self) -> Vec<RunRecord> {
        self.inner.lock().unwrap().runs.values().rev().cloned().collect()
    }

    /// Add output bytes as the runner node would.
    pub fn seed_output(&self, id: &RunId, bytes: &[u8]) {
        self.inner.lock().unwrap().outputs.entry(*id).or_default().extend_from_slice(bytes);
    }

    /// Add one event line as the runner node would.
    pub fn seed_event(&self, id: &RunId, line: &str) {
        let mut g = self.inner.lock().unwrap();
        let lines = g.events.entry(*id).or_default();
        let seq = lines.len() as u64 + 1;
        lines.push(EventLine { seq, bytes: line.as_bytes().to_vec() });
    }

    /// The value of a secret.
    pub fn secret_value(&self, scope: &SecretScope, name: &str) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().secrets.get(&(scope.text(), name.to_string())).map(|(v, _)| v.expose().to_vec())
    }

    /// The bytes of the current document of a key.
    pub fn document(&self, key: &DocKey) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().docs.get(key).and_then(|v| current(v)).and_then(|v| v.bytes.clone())
    }
}

fn meta(by: &str) -> WriteMeta {
    WriteMeta {
        saved_by: pupitre_core::ids::Subject::parse(by).unwrap(),
        saved_at: UtcMillis::now(),
        message: pupitre_store::types::Message::default(),
    }
}

fn fingerprint(scope: &SecretScope, name: &KeyPath, value: &SecretValue) -> String {
    let mut h = Sha256::new();
    h.update(scope.text().as_bytes());
    h.update(b"\n");
    h.update(name.as_str().as_bytes());
    h.update(b"\n");
    h.update(value.expose());
    pupitre_core::etag::to_hex(&h.finalize())[..16].to_string()
}

impl Store for MemStore {
    async fn load(&self) -> Result<Listing, StoreError> {
        self.check("load")?;
        let g = self.inner.lock().unwrap();
        let docs = g
            .docs
            .iter()
            .filter_map(|(k, v)| {
                current(v).map(|c| StoredDoc {
                    key: k.clone(),
                    bytes: c.bytes.clone().unwrap_or_default(),
                    etag: c.etag.unwrap_or_else(|| Etag::of(b"")),
                    version: c.version,
                })
            })
            .collect();
        Ok(Listing { docs, marks: Marks::default() })
    }

    async fn poll(&self, _marks: &Marks) -> Result<Delta, StoreError> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        self.check("poll")?;
        let mut g = self.inner.lock().unwrap();
        Ok(Delta {
            changed: std::mem::take(&mut g.changed),
            removed: std::mem::take(&mut g.removed),
            marks: Marks::default(),
        })
    }

    async fn get(&self, key: &DocKey) -> Result<StoredDoc, StoreError> {
        self.check("get")?;
        let g = self.inner.lock().unwrap();
        let c = g.docs.get(key).and_then(|v| current(v)).ok_or(StoreError::NotFound)?;
        Ok(StoredDoc {
            key: key.clone(),
            bytes: c.bytes.clone().unwrap_or_default(),
            etag: c.etag.unwrap_or_else(|| Etag::of(b"")),
            version: c.version,
        })
    }

    async fn put(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        self.check("put")?;
        let saved = self.commit_put(key, doc, precondition, meta)?;
        self.after_commit().await;
        Ok(saved)
    }

    async fn delete(&self, key: &DocKey, if_match: &Etag, meta: &WriteMeta) -> Result<Saved, StoreError> {
        self.check("delete")?;
        let saved = self.commit_delete(key, if_match, meta)?;
        self.after_commit().await;
        Ok(saved)
    }

    async fn history(&self, key: &DocKey, before: Option<u64>, limit: u32) -> Result<Vec<VersionMeta>, StoreError> {
        self.check("history")?;
        let g = self.inner.lock().unwrap();
        let Some(versions) = g.docs.get(key) else { return Ok(Vec::new()) };
        Ok(versions
            .iter()
            .rev()
            .filter(|v| before.is_none_or(|b| v.version < b))
            .take(limit as usize)
            .map(|v| VersionMeta { version: v.version, kind: v.kind, etag: v.etag, meta: v.meta.clone() })
            .collect())
    }

    async fn version(&self, key: &DocKey, version: u64) -> Result<Version, StoreError> {
        self.check("version")?;
        let g = self.inner.lock().unwrap();
        let v = g.docs.get(key).and_then(|vs| vs.iter().find(|v| v.version == version)).ok_or(StoreError::NotFound)?;
        Ok(Version {
            meta: VersionMeta { version: v.version, kind: v.kind, etag: v.etag, meta: v.meta.clone() },
            bytes: v.bytes.clone(),
        })
    }

    async fn read_environment(&self, env: &EnvId) -> Result<Vec<StoredDoc>, StoreError> {
        self.check("read_environment")?;
        let listing = self.load().await?;
        Ok(listing.docs.into_iter().filter(|d| d.key.environment() == env).collect())
    }

    async fn list_secrets(&self, scope: &SecretScope) -> Result<Vec<SecretMeta>, StoreError> {
        self.check("list_secrets")?;
        let g = self.inner.lock().unwrap();
        let text = scope.text();
        Ok(g.secrets.iter().filter(|((s, _), _)| *s == text).map(|(_, (_, m))| m.clone()).collect())
    }

    async fn put_secret(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        meta: &WriteMeta,
    ) -> Result<SecretMeta, StoreError> {
        self.check("put_secret")?;
        let m = SecretMeta {
            scope: scope.clone(),
            name: name.clone(),
            fingerprint: fingerprint(scope, name, value),
            updated_at: meta.saved_at,
            updated_by: meta.saved_by.clone(),
        };
        self.inner
            .lock()
            .unwrap()
            .secrets
            .insert((scope.text(), name.as_str().to_string()), (SecretValue::new(value.expose().to_vec()), m.clone()));
        Ok(m)
    }

    async fn delete_secret(&self, scope: &SecretScope, name: &KeyPath, _meta: &WriteMeta) -> Result<(), StoreError> {
        self.check("delete_secret")?;
        self.inner
            .lock()
            .unwrap()
            .secrets
            .remove(&(scope.text(), name.as_str().to_string()))
            .map(|_| ())
            .ok_or(StoreError::NotFound)
    }

    async fn read_secrets(
        &self,
        _scopes: &[SecretScope],
    ) -> Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError> {
        Err(StoreError::Unavailable("read_secrets".to_string()))
    }

    async fn create_run(
        &self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> Result<(), StoreError> {
        self.check("create_run")?;
        let mut g = self.inner.lock().unwrap();
        let queued = g.runs.values().filter(|r| r.status == RunStatus::Queued).count();
        if queued >= max_queued as usize {
            return Err(StoreError::QueueFull);
        }
        if let Some(check) = consumes {
            let c = g.runs.get_mut(check).ok_or(StoreError::NotFound)?;
            if c.applied_by.is_some() {
                return Err(StoreError::CheckUsed);
            }
            c.applied_by = Some(record.id);
        }
        g.runs.insert(record.id, record.clone());
        Ok(())
    }

    async fn get_run(&self, id: &RunId) -> Result<RunRecord, StoreError> {
        self.check("get_run")?;
        self.inner.lock().unwrap().runs.get(id).cloned().ok_or(StoreError::NotFound)
    }

    async fn list_runs(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        self.check("list_runs")?;
        let g = self.inner.lock().unwrap();
        Ok(g.runs
            .values()
            .rev()
            .filter(|r| filter.environment.as_ref().is_none_or(|e| *e == r.environment))
            .filter(|r| filter.status.is_none_or(|s| s == r.status))
            .filter(|r| before.is_none_or(|b| r.id < *b))
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn next_queued(&self, _busy: &[EnvId], _limit: u32) -> Result<Vec<RunRecord>, StoreError> {
        Err(StoreError::Unavailable("next_queued".to_string()))
    }

    async fn update_run(&self, expected: RunStatus, record: &RunRecord) -> Result<(), StoreError> {
        self.check("update_run")?;
        let mut g = self.inner.lock().unwrap();
        let stored = g.runs.get_mut(&record.id).ok_or(StoreError::NotFound)?;
        if stored.status != expected {
            return Err(StoreError::Conflict { current: None });
        }
        if record.status != expected && !expected.can_move_to(record.status) {
            return Err(StoreError::BadTransition { from: expected, to: record.status });
        }
        *stored = record.clone();
        Ok(())
    }

    async fn prune_runs(&self, _keep: u32) -> Result<Vec<RunId>, StoreError> {
        Ok(Vec::new())
    }

    async fn append_output(&self, id: &RunId, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        let mut g = self.inner.lock().unwrap();
        let out = g.outputs.entry(*id).or_default();
        if out.len() as u64 != offset {
            return Err(StoreError::Conflict { current: None });
        }
        out.extend_from_slice(bytes);
        Ok(out.len() as u64)
    }

    async fn read_output(&self, id: &RunId, offset: u64, max: u32) -> Result<OutputChunk, StoreError> {
        self.check("read_output")?;
        let g = self.inner.lock().unwrap();
        let out = g.outputs.get(id).cloned().unwrap_or_default();
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(out.len());
        let end = start.saturating_add(max as usize).min(out.len());
        Ok(OutputChunk { bytes: out[start..end].to_vec(), length: out.len() as u64 })
    }

    async fn append_events(&self, id: &RunId, first_seq: u64, lines: &[Vec<u8>]) -> Result<(), StoreError> {
        let mut g = self.inner.lock().unwrap();
        let events = g.events.entry(*id).or_default();
        if first_seq != events.len() as u64 + 1 {
            return Err(StoreError::Conflict { current: None });
        }
        for (i, l) in lines.iter().enumerate() {
            events.push(EventLine { seq: first_seq + i as u64, bytes: l.clone() });
        }
        Ok(())
    }

    async fn read_events(&self, id: &RunId, after: u64, limit: u32) -> Result<Vec<EventLine>, StoreError> {
        self.check("read_events")?;
        let g = self.inner.lock().unwrap();
        Ok(g.events
            .get(id)
            .map(|e| e.iter().filter(|l| l.seq > after).take(limit as usize).cloned().collect())
            .unwrap_or_default())
    }

    async fn acquire_lease(&self, _name: &LeaseName, _holder: &str, ttl: Duration) -> Result<LeaseOutcome, StoreError> {
        Ok(LeaseOutcome::Acquired { until: UtcMillis(UtcMillis::now().0 + ttl.as_millis() as u64) })
    }

    async fn renew_lease(&self, _name: &LeaseName, _holder: &str, ttl: Duration) -> Result<LeaseRenewal, StoreError> {
        Ok(LeaseRenewal::Renewed { until: UtcMillis(UtcMillis::now().0 + ttl.as_millis() as u64) })
    }

    async fn release_lease(&self, _name: &LeaseName, _holder: &str) -> Result<(), StoreError> {
        Ok(())
    }

    async fn export_begin(&self) -> Result<Exporter, StoreError> {
        Err(StoreError::Unavailable("export_begin".to_string()))
    }

    async fn import_begin(&self) -> Result<Importer, StoreError> {
        Err(StoreError::Unavailable("import_begin".to_string()))
    }
}
