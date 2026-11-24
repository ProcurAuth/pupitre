// SPDX-License-Identifier: MIT OR Apache-2.0
//! A small in-memory implementation of `Store` for the tests of the runner. It follows the
//! rules of 2.27 to 2.29 for runs, output, events and leases. It also counts calls and fails
//! a call on demand.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    Saved, SecretMeta, StoreError, StoredDoc, Version, VersionMeta, WriteMeta,
};

#[derive(Default)]
struct State {
    docs: Vec<StoredDoc>,
    secrets: BTreeMap<(SecretScope, KeyPath), (Vec<u8>, String)>,
    runs: BTreeMap<RunId, RunRecord>,
    outputs: BTreeMap<RunId, Vec<u8>>,
    events: BTreeMap<RunId, Vec<EventLine>>,
    leases: BTreeMap<String, (String, Instant)>,
    failing: BTreeMap<&'static str, StoreError>,
    append_sizes: Vec<usize>,
    event_batches: Vec<usize>,
    calls: BTreeMap<&'static str, u64>,
}

/// The store of the tests.
#[derive(Default)]
pub struct MemStore {
    state: Mutex<State>,
}

impl std::fmt::Debug for MemStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemStore")
    }
}

#[allow(dead_code)]
impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    /// Enter a call: count it and fail it when the test asks.
    fn enter(&self, name: &'static str) -> Result<(), StoreError> {
        self.with(|s| {
            *s.calls.entry(name).or_default() += 1;
            match s.failing.get(name) {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        })
    }

    pub fn fail(&self, method: &'static str, on: bool) {
        self.with(|s| {
            if on {
                s.failing.insert(method, StoreError::Unavailable(format!("{method}: injected failure")));
            } else {
                s.failing.remove(method);
            }
        });
    }

    pub fn fail_with(&self, method: &'static str, error: StoreError) {
        self.with(|s| {
            s.failing.insert(method, error);
        });
    }

    /// The sizes of the chunks that `append_output` got.
    pub fn append_sizes(&self) -> Vec<usize> {
        self.with(|s| s.append_sizes.clone())
    }

    /// The sizes of the batches that `append_events` got.
    pub fn event_batches(&self) -> Vec<usize> {
        self.with(|s| s.event_batches.clone())
    }

    pub fn calls(&self, method: &'static str) -> u64 {
        self.with(|s| s.calls.get(method).copied().unwrap_or(0))
    }

    pub fn add_doc(&self, doc: StoredDoc) {
        self.with(|s| {
            s.docs.retain(|d| d.key != doc.key);
            s.docs.push(doc);
        });
    }

    pub fn put_secret_value(&self, scope: SecretScope, name: &str, value: &[u8], fingerprint: &str) {
        self.with(|s| {
            s.secrets.insert((scope, KeyPath::parse(name).unwrap()), (value.to_vec(), fingerprint.to_string()));
        });
    }

    pub fn insert_run(&self, record: RunRecord) {
        self.with(|s| {
            s.runs.insert(record.id, record);
        });
    }

    pub fn run(&self, id: &RunId) -> Option<RunRecord> {
        self.with(|s| s.runs.get(id).cloned())
    }

    pub fn all_runs(&self) -> Vec<RunRecord> {
        self.with(|s| s.runs.values().cloned().collect())
    }

    /// Change a run record in place, as the server does for a cancel request.
    pub fn edit_run(&self, id: &RunId, f: impl FnOnce(&mut RunRecord)) {
        self.with(|s| {
            if let Some(r) = s.runs.get_mut(id) {
                f(r);
            }
        });
    }

    pub fn output(&self, id: &RunId) -> Vec<u8> {
        self.with(|s| s.outputs.get(id).cloned().unwrap_or_default())
    }

    pub fn events(&self, id: &RunId) -> Vec<EventLine> {
        self.with(|s| s.events.get(id).cloned().unwrap_or_default())
    }

    pub fn lease_holder(&self, name: &LeaseName) -> Option<String> {
        self.with(|s| {
            s.leases.get(&name.text()).filter(|(_, until)| *until > Instant::now()).map(|(holder, _)| holder.clone())
        })
    }

    /// Take a lease for another holder, as another node does.
    pub fn force_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) {
        self.with(|s| {
            s.leases.insert(name.text(), (holder.to_string(), Instant::now() + ttl));
        });
    }
}

fn nope<T>() -> Result<T, StoreError> {
    Err(StoreError::Unavailable("not used by the tests of the runner".to_string()))
}

impl Store for MemStore {
    async fn load(&self) -> Result<Listing, StoreError> {
        nope()
    }
    async fn poll(&self, _: &Marks) -> Result<Delta, StoreError> {
        nope()
    }
    async fn get(&self, _: &DocKey) -> Result<StoredDoc, StoreError> {
        nope()
    }
    async fn put(
        &self,
        _: &DocKey,
        _: &CanonicalDocument,
        _: &Precondition,
        _: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        nope()
    }
    async fn delete(&self, _: &DocKey, _: &Etag, _: &WriteMeta) -> Result<Saved, StoreError> {
        nope()
    }
    async fn history(&self, _: &DocKey, _: Option<u64>, _: u32) -> Result<Vec<VersionMeta>, StoreError> {
        nope()
    }
    async fn version(&self, _: &DocKey, _: u64) -> Result<Version, StoreError> {
        nope()
    }
    async fn read_environment(&self, env: &EnvId) -> Result<Vec<StoredDoc>, StoreError> {
        self.enter("read_environment")?;
        Ok(self.with(|s| s.docs.iter().filter(|d| d.key.environment() == env).cloned().collect()))
    }
    async fn list_secrets(&self, scope: &SecretScope) -> Result<Vec<SecretMeta>, StoreError> {
        self.enter("list_secrets")?;
        Ok(self.with(|s| {
            s.secrets
                .iter()
                .filter(|((sc, _), _)| sc == scope)
                .map(|((sc, name), (_, fp))| SecretMeta {
                    scope: sc.clone(),
                    name: name.clone(),
                    fingerprint: fp.clone(),
                    updated_at: UtcMillis(0),
                    updated_by: pupitre_core::ids::Subject::anonymous(),
                })
                .collect()
        }))
    }
    async fn put_secret(
        &self,
        _: &SecretScope,
        _: &KeyPath,
        _: &SecretValue,
        _: &WriteMeta,
    ) -> Result<SecretMeta, StoreError> {
        nope()
    }
    async fn delete_secret(&self, _: &SecretScope, _: &KeyPath, _: &WriteMeta) -> Result<(), StoreError> {
        nope()
    }
    async fn read_secrets(
        &self,
        scopes: &[SecretScope],
    ) -> Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError> {
        self.enter("read_secrets")?;
        Ok(self.with(|s| {
            s.secrets
                .iter()
                .filter(|((sc, _), _)| scopes.contains(sc))
                .map(|((sc, name), (v, _))| (sc.clone(), name.clone(), SecretValue::new(v.clone())))
                .collect()
        }))
    }
    async fn create_run(
        &self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> Result<(), StoreError> {
        self.enter("create_run")?;
        self.with(|s| {
            let queued = s.runs.values().filter(|r| r.status == RunStatus::Queued).count();
            if queued >= max_queued as usize {
                return Err(StoreError::QueueFull);
            }
            if let Some(check) = consumes {
                let c = s.runs.get_mut(check).ok_or(StoreError::NotFound)?;
                if c.applied_by.is_some() {
                    return Err(StoreError::CheckUsed);
                }
                c.applied_by = Some(record.id);
            }
            s.runs.insert(record.id, record.clone());
            Ok(())
        })
    }
    async fn get_run(&self, id: &RunId) -> Result<RunRecord, StoreError> {
        self.enter("get_run")?;
        self.with(|s| s.runs.get(id).cloned().ok_or(StoreError::NotFound))
    }
    async fn list_runs(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        self.enter("list_runs")?;
        Ok(self.with(|s| {
            s.runs
                .values()
                .rev()
                .filter(|r| filter.environment.as_ref().is_none_or(|e| *e == r.environment))
                .filter(|r| filter.status.is_none_or(|st| st == r.status))
                .filter(|r| before.is_none_or(|b| r.id < *b))
                .take(limit as usize)
                .cloned()
                .collect()
        }))
    }
    async fn next_queued(&self, busy: &[EnvId], limit: u32) -> Result<Vec<RunRecord>, StoreError> {
        self.enter("next_queued")?;
        Ok(self.with(|s| {
            s.runs
                .values()
                .filter(|r| r.status == RunStatus::Queued && !busy.contains(&r.environment))
                .take(limit as usize)
                .cloned()
                .collect()
        }))
    }
    async fn update_run(&self, expected: RunStatus, record: &RunRecord) -> Result<(), StoreError> {
        self.enter("update_run")?;
        self.with(|s| {
            let stored = s.runs.get_mut(&record.id).ok_or(StoreError::NotFound)?;
            if stored.status != expected {
                return Err(StoreError::Conflict { current: None });
            }
            if record.status != expected && !expected.can_move_to(record.status) {
                return Err(StoreError::BadTransition { from: expected, to: record.status });
            }
            *stored = record.clone();
            Ok(())
        })
    }
    async fn prune_runs(&self, keep: u32) -> Result<Vec<RunId>, StoreError> {
        self.enter("prune_runs")?;
        Ok(self.with(|s| {
            let finals: Vec<RunId> = s.runs.values().filter(|r| r.status.is_final()).map(|r| r.id).collect();
            let excess = finals.len().saturating_sub(keep as usize);
            let removed: Vec<RunId> = finals.into_iter().take(excess).collect();
            for id in &removed {
                s.runs.remove(id);
                s.outputs.remove(id);
                s.events.remove(id);
            }
            removed
        }))
    }
    async fn append_output(&self, id: &RunId, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        self.enter("append_output")?;
        self.with(|s| {
            let out = s.outputs.entry(*id).or_default();
            if out.len() as u64 != offset {
                return Err(StoreError::Conflict { current: None });
            }
            out.extend_from_slice(bytes);
            let len = out.len() as u64;
            s.append_sizes.push(bytes.len());
            Ok(len)
        })
    }
    async fn read_output(&self, id: &RunId, offset: u64, max: u32) -> Result<OutputChunk, StoreError> {
        self.enter("read_output")?;
        Ok(self.with(|s| {
            let out = s.outputs.get(id).cloned().unwrap_or_default();
            let start = (offset as usize).min(out.len());
            let end = (start + max as usize).min(out.len());
            OutputChunk { bytes: out[start..end].to_vec(), length: out.len() as u64 }
        }))
    }
    async fn append_events(&self, id: &RunId, first_seq: u64, lines: &[Vec<u8>]) -> Result<(), StoreError> {
        self.enter("append_events")?;
        self.with(|s| {
            let ev = s.events.entry(*id).or_default();
            if ev.len() as u64 + 1 != first_seq {
                return Err(StoreError::Conflict { current: None });
            }
            for (i, l) in lines.iter().enumerate() {
                ev.push(EventLine { seq: first_seq + i as u64, bytes: l.clone() });
            }
            s.event_batches.push(lines.len());
            Ok(())
        })
    }
    async fn read_events(&self, id: &RunId, after: u64, limit: u32) -> Result<Vec<EventLine>, StoreError> {
        self.enter("read_events")?;
        Ok(self.with(|s| {
            s.events
                .get(id)
                .map(|v| v.iter().filter(|e| e.seq > after).take(limit as usize).cloned().collect())
                .unwrap_or_default()
        }))
    }
    async fn acquire_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseOutcome, StoreError> {
        self.enter("acquire_lease")?;
        Ok(self.with(|s| {
            let now = Instant::now();
            let key = name.text();
            if let Some((h, until)) = s.leases.get(&key)
                && *until > now
                && h != holder
            {
                let left = until.saturating_duration_since(now);
                return LeaseOutcome::HeldBy {
                    holder: h.clone(),
                    until: UtcMillis(UtcMillis::now().0 + left.as_millis() as u64),
                };
            }
            s.leases.insert(key, (holder.to_string(), now + ttl));
            LeaseOutcome::Acquired { until: UtcMillis(UtcMillis::now().0 + ttl.as_millis() as u64) }
        }))
    }
    async fn renew_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseRenewal, StoreError> {
        self.enter("renew_lease")?;
        Ok(self.with(|s| {
            let now = Instant::now();
            match s.leases.get_mut(&name.text()) {
                Some((h, until)) if h == holder && *until > now => {
                    *until = now + ttl;
                    LeaseRenewal::Renewed { until: UtcMillis(UtcMillis::now().0 + ttl.as_millis() as u64) }
                }
                _ => LeaseRenewal::Lost,
            }
        }))
    }
    async fn release_lease(&self, name: &LeaseName, holder: &str) -> Result<(), StoreError> {
        self.enter("release_lease")?;
        self.with(|s| {
            let key = name.text();
            if s.leases.get(&key).is_some_and(|(h, _)| h == holder) {
                s.leases.remove(&key);
            }
        });
        Ok(())
    }
    async fn export_begin(&self) -> Result<Exporter, StoreError> {
        nope()
    }
    async fn import_begin(&self) -> Result<Importer, StoreError> {
        nope()
    }
}
