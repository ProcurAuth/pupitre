// SPDX-License-Identifier: MIT OR Apache-2.0
//! The backend `files` (2.31 to 2.35): one folder of documents, one folder of secrets and one
//! folder of runs, with one writer process.
//!
//! | Module | Content | Paragraphs |
//! | --- | --- | --- |
//! | `fsio` | The file layer: `O_NOFOLLOW`, atomic writes, modes, `flock` | 2.32 |
//! | `docs` | The documents, the versions, the repair, the outside changes | 2.18 to 2.23, 2.33 |
//! | `secrets` | The secret values, their metadata and events | 2.24, 2.34 |
//! | `runs` | The run records, the output and the events | 2.27, 2.28, 2.35 |
//! | `transfer` | The exporter, the importer and the writer of a folder of documents | 2.40 |
//!
//! Each call of the trait runs on a blocking thread of the runtime. Each folder has one mutex
//! in the process and one `flock` between processes. A write checks its precondition and writes
//! under the mutex, so two writers with one base etag cannot both succeed (2.20).

mod docs;
mod events;
mod faults;
mod fsio;
mod meta;
mod runs;
mod secrets;
mod transfer;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::etag::Etag;
use pupitre_core::ids::{EnvId, KeyPath, RunId};
use pupitre_core::node::SecretScope;
use pupitre_core::run::{RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;
use rustix::fd::OwnedFd;
use rustix::io::Errno;

use self::docs::DocStore;
use self::events::Events;
pub use self::faults::{FaultPoint, Faults};
use self::fsio::{Dir, FsErr, Modes};
use self::meta::{
    CURRENT_SCHEMA, MAX_SMALL_FILE, decode_secrets_meta, decode_store_meta, encode_secrets_meta, encode_store_meta,
};
use self::runs::RunStore;
use self::secrets::SecretStore;
pub use self::transfer::{DocumentsWriter, FilesExporter, FilesImporter};
use crate::secret::{SecretKey, SecretValue, StoreKeys};
use crate::store::Store;
use crate::transfer::{Exporter, Importer};
use crate::types::{
    Delta, EventLine, LeaseName, LeaseOutcome, LeaseRenewal, Listing, Marks, MarksInner, OutputChunk, Precondition,
    RunFilter, Saved, SecretMeta, StoreError, StoreEvent, StoredDoc, Version, VersionKind, VersionMeta, WriteMeta,
};

/// The name of the meta file of the documents folder (2.31).
pub const STORE_META_FILE: &str = "pupitre-store.json";
/// The name of the meta file of the secrets folder (2.31).
pub const SECRETS_META_FILE: &str = "pupitre-secrets.json";
/// The name of the lock file of each folder (2.32).
pub const LOCK_FILE: &str = ".lock";
/// The folder of the versions (2.31).
pub const HISTORY_DIR: &str = ".history";
/// The folder of the temporary files (2.32).
pub const TMP_DIR: &str = ".tmp";

/// The options of the backend, from the block `storage.files` (6.10).
#[derive(Debug, Clone)]
pub struct FilesOptions {
    /// `storage.files.path`
    pub path: PathBuf,
    /// `storage.files.secrets_path`
    pub secrets_path: PathBuf,
    /// `storage.files.runs_path`
    pub runs_path: PathBuf,
    /// `storage.files.rescan_interval`
    pub rescan_interval: Duration,
    /// `storage.files.history_keep`
    pub history_keep: u32,
    /// `storage.secret_key`
    pub secret_key: SecretKey,
    /// `limits.max_document_bytes`: a larger file is read as an error, not in full.
    pub max_document_bytes: usize,
}

/// What `FilesStore::migrate` did (2.30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateOutcome {
    /// The store had no store schema. The command made it.
    Created,
    /// The store had an older store schema. The command changed it.
    Updated {
        /// The older version.
        from: u32,
    },
    /// The store had the store schema of this binary. The command changed nothing.
    Current,
}

#[derive(Debug)]
struct Lease {
    holder: String,
    until: Instant,
    until_ms: UtcMillis,
}

/// The shared state of one store.
struct Inner {
    docs: Mutex<DocStore>,
    secrets: Mutex<SecretStore>,
    runs: Mutex<RunStore>,
    leases: Mutex<HashMap<LeaseName, Lease>>,
    events: Arc<Events>,
    /// The three `flock` locks. They end with the process or with this value (2.32).
    _locks: Vec<OwnedFd>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner").finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, StoreError> {
    mutex.lock().map_err(|_| StoreError::Unavailable("a call of the store ended with a failure".to_string()))
}

impl Inner {
    fn lock_docs(&self) -> Result<MutexGuard<'_, DocStore>, StoreError> {
        lock(&self.docs)
    }
    fn lock_secrets(&self) -> Result<MutexGuard<'_, SecretStore>, StoreError> {
        lock(&self.secrets)
    }
    fn lock_runs(&self) -> Result<MutexGuard<'_, RunStore>, StoreError> {
        lock(&self.runs)
    }
}

/// Run a blocking call of the file layer on a thread of the runtime.
async fn blocking<T, F>(inner: &Arc<Inner>, call: F) -> Result<T, StoreError>
where
    T: Send + 'static,
    F: FnOnce(&Inner) -> Result<T, StoreError> + Send + 'static,
{
    let inner = Arc::clone(inner);
    tokio::task::spawn_blocking(move || call(&inner))
        .await
        .map_err(|_| StoreError::Unavailable("a call of the store did not end".to_string()))?
}

/// The three folders of the backend, open.
struct Roots {
    docs: Dir,
    secrets: Dir,
    runs: Dir,
}

impl Roots {
    fn open(options: &FilesOptions, faults: &Faults, create: bool) -> Result<Roots, StoreError> {
        let open = |path: &std::path::Path, modes: Modes| {
            if create {
                Dir::open_or_create_root(path, faults.clone(), modes)
            } else {
                Dir::open_root(path, faults.clone(), modes)
            }
        };
        Ok(Roots {
            docs: open(&options.path, Modes::DOCUMENTS)?,
            secrets: open(&options.secrets_path, Modes::SECRETS)?,
            runs: open(&options.runs_path, Modes::RUNS)?,
        })
    }

    fn lock_all(&self) -> Result<Vec<OwnedFd>, StoreError> {
        let mut locks = Vec::new();
        for dir in [&self.docs, &self.secrets, &self.runs] {
            match dir.lock(LOCK_FILE) {
                Ok(Some(fd)) => locks.push(fd),
                Ok(None) => return Err(StoreError::Locked { path: dir.label().to_string() }),
                Err(FsErr { errno: Errno::NOLCK | Errno::OPNOTSUPP | Errno::NOSYS, .. }) => {
                    return Err(StoreError::Unavailable(format!(
                        "the file system of the folder {} does not support flock",
                        dir.label()
                    )));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(locks)
    }

    /// The secrets folder MUST NOT give a right to a group or to the others (2.31).
    fn check_secrets_mode(&self) -> Result<(), StoreError> {
        let mode = self.secrets.stat_self()?.mode;
        if mode & 0o077 != 0 {
            return Err(StoreError::Unavailable(format!(
                "the folder {} has the mode {mode:04o}; the secrets folder needs the mode 0700",
                self.secrets.label()
            )));
        }
        Ok(())
    }

    /// Open the folder `.tmp` of each folder and remove the files that a crash left in it.
    fn temp_dirs(&self) -> Result<[Dir; 3], StoreError> {
        let mut out = Vec::new();
        for dir in [&self.docs, &self.secrets, &self.runs] {
            let tmp = dir.open_dir_create(TMP_DIR)?;
            for item in tmp.list()? {
                tmp.unlink_opt(&item.name)?;
            }
            out.push(tmp);
        }
        let [a, b, c]: [Dir; 3] =
            out.try_into().map_err(|_| StoreError::Corrupt("the folders .tmp are not three".to_string()))?;
        Ok([a, b, c])
    }
}

fn derive_keys(key: &SecretKey) -> Result<StoreKeys, StoreError> {
    StoreKeys::derive(key).map_err(|_| StoreError::Unavailable("the derivation of the store keys failed".to_string()))
}

/// The backend `files`.
#[derive(Debug)]
pub struct FilesStore {
    options: FilesOptions,
    inner: Arc<Inner>,
}

impl FilesStore {
    /// Open the three folders: take the locks, check the store schema and the key check
    /// (2.30, 2.26, 2.32), and repair a write that a crash left (2.19).
    pub fn open(options: FilesOptions) -> Result<FilesStore, StoreError> {
        FilesStore::open_with_faults(options, Faults::new())
    }

    /// Open the store with a fault hook. A test uses it to make a call fail at a known place.
    pub fn open_with_faults(options: FilesOptions, faults: Faults) -> Result<FilesStore, StoreError> {
        let roots = Roots::open(&options, &faults, false)?;
        let locks = roots.lock_all()?;
        let keys = derive_keys(&options.secret_key)?;
        let found = match roots.docs.read_file_opt(STORE_META_FILE, MAX_SMALL_FILE)? {
            None => None,
            Some((bytes, _)) => Some(decode_store_meta(&bytes, &format!("{}/{STORE_META_FILE}", roots.docs.label()))?),
        };
        match found {
            None => return Err(StoreError::MigrateNeeded { found: None }),
            Some(n) if n < CURRENT_SCHEMA => return Err(StoreError::MigrateNeeded { found: Some(n) }),
            Some(n) if n > CURRENT_SCHEMA => return Err(StoreError::StoreNewer { found: n }),
            Some(_) => {}
        }
        let Some((bytes, _)) = roots.secrets.read_file_opt(SECRETS_META_FILE, MAX_SMALL_FILE)? else {
            return Err(StoreError::MigrateNeeded { found: None });
        };
        let stored = decode_secrets_meta(&bytes, &format!("{}/{SECRETS_META_FILE}", roots.secrets.label()))?;
        if stored != keys.key_check() {
            return Err(StoreError::KeyMismatch);
        }
        roots.check_secrets_mode()?;
        let [docs_tmp, secrets_tmp, runs_tmp] = roots.temp_dirs()?;
        let events = Arc::new(Events::default());
        let docs = DocStore::open(
            roots.docs.clone(),
            docs_tmp,
            Arc::clone(&events),
            options.history_keep,
            options.max_document_bytes,
        )?;
        let secrets = SecretStore::new(roots.secrets.clone(), secrets_tmp, keys, options.history_keep);
        let runs = RunStore::open(roots.runs.clone(), runs_tmp, &events)?;
        Ok(FilesStore {
            options,
            inner: Arc::new(Inner {
                docs: Mutex::new(docs),
                secrets: Mutex::new(secrets),
                runs: Mutex::new(runs),
                leases: Mutex::new(HashMap::new()),
                events,
                _locks: locks,
            }),
        })
    }

    /// Make the store schema, or change it to the version of this binary, and write the key
    /// check (2.30). The command `migrate` calls it. It makes a folder that does not exist
    /// when its parent exists. It takes the locks, so it stops when a server runs.
    pub fn migrate(options: &FilesOptions) -> Result<MigrateOutcome, StoreError> {
        let faults = Faults::new();
        let roots = Roots::open(options, &faults, true)?;
        let _locks = roots.lock_all()?;
        let keys = derive_keys(&options.secret_key)?;
        roots.check_secrets_mode()?;
        let [docs_tmp, secrets_tmp, _runs_tmp] = roots.temp_dirs()?;
        let found = match roots.docs.read_file_opt(STORE_META_FILE, MAX_SMALL_FILE)? {
            None => None,
            Some((bytes, _)) => Some(decode_store_meta(&bytes, &format!("{}/{STORE_META_FILE}", roots.docs.label()))?),
        };
        if let Some(n) = found.filter(|n| *n > CURRENT_SCHEMA) {
            return Err(StoreError::StoreNewer { found: n });
        }
        if let Some((bytes, _)) = roots.secrets.read_file_opt(SECRETS_META_FILE, MAX_SMALL_FILE)? {
            let stored = decode_secrets_meta(&bytes, &format!("{}/{SECRETS_META_FILE}", roots.secrets.label()))?;
            if stored != keys.key_check() {
                return Err(StoreError::KeyMismatch);
            }
        } else {
            roots.secrets.write_atomic(&secrets_tmp, SECRETS_META_FILE, &encode_secrets_meta(&keys.key_check()))?;
        }
        Ok(match found {
            Some(n) if n == CURRENT_SCHEMA => MigrateOutcome::Current,
            other => {
                roots.docs.write_atomic(&docs_tmp, STORE_META_FILE, &encode_store_meta(CURRENT_SCHEMA))?;
                match other {
                    None => MigrateOutcome::Created,
                    Some(from) => MigrateOutcome::Updated { from },
                }
            }
        })
    }

    /// Create a document with the version kind `import` (2.18, 6.24a). The rules are the rules of
    /// `put` with the precondition `Create`: the key has no current document.
    pub async fn put_import(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        let (key, doc, meta) = (key.clone(), doc.clone(), meta.clone());
        blocking(&self.inner, move |i| {
            i.lock_docs()?.put_as(&key, &doc, &Precondition::Create, &meta, VersionKind::Import)
        })
        .await
    }

    /// The options of the store.
    pub fn options(&self) -> &FilesOptions {
        &self.options
    }

    /// Take the events for the log.
    pub fn drain_events(&self) -> Vec<StoreEvent> {
        self.inner.events.drain()
    }
}

/// The path of the folder of the versions of a key, relative to the documents folder (2.31).
pub fn history_dir(key: &DocKey) -> String {
    format!("{HISTORY_DIR}/{}", key.text())
}

/// The file name of a version: 10 digits with zeros at the start (2.31).
pub fn version_file(version: u64) -> String {
    format!("{version:010}.json")
}

/// The path of the file of a secret value, relative to the secrets folder (2.31).
pub fn secret_value_file(scope: &SecretScope, name: &KeyPath) -> String {
    format!("values/{}/{}", scope.folder(), name.as_str())
}

/// The folder of a run, relative to the runs folder (2.31).
pub fn run_dir(id: &RunId) -> String {
    id.to_string()
}

fn ms_after(ttl: Duration) -> UtcMillis {
    let ttl_ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
    UtcMillis(UtcMillis::now().0.saturating_add(ttl_ms))
}

impl Inner {
    fn acquire(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseOutcome, StoreError> {
        let now = Instant::now();
        let mut leases = lock(&self.leases)?;
        if let Some(l) = leases.get(name)
            && l.until > now
            && l.holder != holder
        {
            return Ok(LeaseOutcome::HeldBy { holder: l.holder.clone(), until: l.until_ms });
        }
        let until_ms = ms_after(ttl);
        leases.insert(name.clone(), Lease { holder: holder.to_string(), until: now + ttl, until_ms });
        Ok(LeaseOutcome::Acquired { until: until_ms })
    }

    fn renew(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseRenewal, StoreError> {
        let now = Instant::now();
        let mut leases = lock(&self.leases)?;
        match leases.get_mut(name) {
            Some(l) if l.holder == holder && l.until > now => {
                l.until = now + ttl;
                l.until_ms = ms_after(ttl);
                Ok(LeaseRenewal::Renewed { until: l.until_ms })
            }
            _ => Ok(LeaseRenewal::Lost),
        }
    }

    fn release(&self, name: &LeaseName, holder: &str) -> Result<(), StoreError> {
        let mut leases = lock(&self.leases)?;
        if leases.get(name).is_some_and(|l| l.holder == holder) {
            leases.remove(name);
        }
        Ok(())
    }

    fn poll(&self, marks: &Marks) -> Result<Delta, StoreError> {
        let map = match &marks.inner {
            MarksInner::Empty => BTreeMap::new(),
            MarksInner::Files(map) => map.clone(),
            MarksInner::Postgres { .. } => {
                return Err(StoreError::Corrupt("the marks come from another backend".to_string()));
            }
        };
        let polled = self.lock_docs()?.poll(&map)?;
        Ok(Delta {
            changed: polled.changed,
            removed: polled.removed,
            marks: Marks { inner: MarksInner::Files(polled.marks) },
        })
    }
}

impl Store for FilesStore {
    async fn load(&self) -> Result<Listing, StoreError> {
        let delta = blocking(&self.inner, |i| i.poll(&Marks::default())).await?;
        Ok(Listing { docs: delta.changed, marks: delta.marks })
    }

    async fn poll(&self, marks: &Marks) -> Result<Delta, StoreError> {
        let marks = marks.clone();
        blocking(&self.inner, move |i| i.poll(&marks)).await
    }

    async fn get(&self, key: &DocKey) -> Result<StoredDoc, StoreError> {
        let key = key.clone();
        blocking(&self.inner, move |i| i.lock_docs()?.get(&key)).await
    }

    async fn put(
        &self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        let (key, doc, precondition, meta) = (key.clone(), doc.clone(), precondition.clone(), meta.clone());
        blocking(&self.inner, move |i| i.lock_docs()?.put(&key, &doc, &precondition, &meta)).await
    }

    async fn put_import(&self, key: &DocKey, doc: &CanonicalDocument, meta: &WriteMeta) -> Result<Saved, StoreError> {
        FilesStore::put_import(self, key, doc, meta).await
    }
    async fn delete(&self, key: &DocKey, if_match: &Etag, meta: &WriteMeta) -> Result<Saved, StoreError> {
        let (key, if_match, meta) = (key.clone(), *if_match, meta.clone());
        blocking(&self.inner, move |i| i.lock_docs()?.delete(&key, &if_match, &meta)).await
    }

    async fn history(&self, key: &DocKey, before: Option<u64>, limit: u32) -> Result<Vec<VersionMeta>, StoreError> {
        let key = key.clone();
        blocking(&self.inner, move |i| i.lock_docs()?.history(&key, before, limit)).await
    }

    async fn version(&self, key: &DocKey, version: u64) -> Result<Version, StoreError> {
        let key = key.clone();
        blocking(&self.inner, move |i| i.lock_docs()?.version(&key, version)).await
    }

    async fn read_environment(&self, env: &EnvId) -> Result<Vec<StoredDoc>, StoreError> {
        let env = env.clone();
        blocking(&self.inner, move |i| i.lock_docs()?.read_environment(&env)).await
    }

    async fn list_secrets(&self, scope: &SecretScope) -> Result<Vec<SecretMeta>, StoreError> {
        let scope = scope.clone();
        blocking(&self.inner, move |i| i.lock_secrets()?.list(&scope)).await
    }

    async fn put_secret(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        meta: &WriteMeta,
    ) -> Result<SecretMeta, StoreError> {
        let (scope, name, value, meta) = (scope.clone(), name.clone(), value.clone(), meta.clone());
        blocking(&self.inner, move |i| i.lock_secrets()?.put(&scope, &name, &value, &meta)).await
    }

    async fn delete_secret(&self, scope: &SecretScope, name: &KeyPath, meta: &WriteMeta) -> Result<(), StoreError> {
        let (scope, name, meta) = (scope.clone(), name.clone(), meta.clone());
        blocking(&self.inner, move |i| i.lock_secrets()?.delete(&scope, &name, &meta)).await
    }

    async fn read_secrets(
        &self,
        scopes: &[SecretScope],
    ) -> Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError> {
        let scopes = scopes.to_vec();
        blocking(&self.inner, move |i| i.lock_secrets()?.read(&scopes)).await
    }

    async fn create_run(
        &self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> Result<(), StoreError> {
        let (record, consumes) = (record.clone(), consumes.copied());
        blocking(&self.inner, move |i| i.lock_runs()?.create(&record, max_queued, consumes.as_ref())).await
    }

    async fn get_run(&self, id: &RunId) -> Result<RunRecord, StoreError> {
        let id = *id;
        blocking(&self.inner, move |i| i.lock_runs()?.get(&id)).await
    }

    async fn list_runs(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        let (filter, before) = (filter.clone(), before.copied());
        blocking(&self.inner, move |i| Ok(i.lock_runs()?.list(&filter, before.as_ref(), limit))).await
    }

    async fn list_runs_page(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        skip: u32,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        let (filter, before) = (filter.clone(), before.copied());
        blocking(&self.inner, move |i| Ok(i.lock_runs()?.list_page(&filter, before.as_ref(), skip, limit))).await
    }

    async fn next_queued(&self, busy: &[EnvId], limit: u32) -> Result<Vec<RunRecord>, StoreError> {
        let busy = busy.to_vec();
        blocking(&self.inner, move |i| Ok(i.lock_runs()?.next_queued(&busy, limit))).await
    }

    async fn update_run(&self, expected: RunStatus, record: &RunRecord) -> Result<(), StoreError> {
        let record = record.clone();
        blocking(&self.inner, move |i| i.lock_runs()?.update(expected, &record)).await
    }

    async fn prune_runs(&self, keep: u32) -> Result<Vec<RunId>, StoreError> {
        blocking(&self.inner, move |i| i.lock_runs()?.prune(keep)).await
    }

    async fn append_output(&self, id: &RunId, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        let (id, bytes) = (*id, bytes.to_vec());
        blocking(&self.inner, move |i| i.lock_runs()?.append_output(&id, offset, &bytes)).await
    }

    async fn read_output(&self, id: &RunId, offset: u64, max: u32) -> Result<OutputChunk, StoreError> {
        let id = *id;
        blocking(&self.inner, move |i| i.lock_runs()?.read_output(&id, offset, max)).await
    }

    async fn append_events(&self, id: &RunId, first_seq: u64, lines: &[Vec<u8>]) -> Result<(), StoreError> {
        let (id, lines) = (*id, lines.to_vec());
        blocking(&self.inner, move |i| i.lock_runs()?.append_events(&id, first_seq, &lines)).await
    }

    async fn read_events(&self, id: &RunId, after: u64, limit: u32) -> Result<Vec<EventLine>, StoreError> {
        let id = *id;
        blocking(&self.inner, move |i| i.lock_runs()?.read_events(&id, after, limit)).await
    }

    async fn acquire_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseOutcome, StoreError> {
        let (name, holder) = (name.clone(), holder.to_string());
        blocking(&self.inner, move |i| i.acquire(&name, &holder, ttl)).await
    }

    async fn renew_lease(&self, name: &LeaseName, holder: &str, ttl: Duration) -> Result<LeaseRenewal, StoreError> {
        let (name, holder) = (name.clone(), holder.to_string());
        blocking(&self.inner, move |i| i.renew(&name, &holder, ttl)).await
    }

    async fn release_lease(&self, name: &LeaseName, holder: &str) -> Result<(), StoreError> {
        let (name, holder) = (name.clone(), holder.to_string());
        blocking(&self.inner, move |i| i.release(&name, &holder)).await
    }

    async fn export_begin(&self) -> Result<Exporter, StoreError> {
        blocking(&self.inner, |i| i.lock_docs()?.rescan()).await?;
        Ok(Exporter::Files(FilesExporter { inner: Arc::clone(&self.inner) }))
    }

    async fn import_begin(&self) -> Result<Importer, StoreError> {
        let empty = blocking(&self.inner, |i| {
            let mut docs = i.lock_docs()?;
            docs.rescan()?;
            Ok(docs.is_empty() && i.lock_secrets()?.is_empty()? && i.lock_runs()?.is_empty())
        })
        .await?;
        if !empty {
            return Err(StoreError::NotEmpty);
        }
        Ok(Importer::Files(FilesImporter { inner: Arc::clone(&self.inner), report: Default::default() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pupitre_core::ids::{EnvId, UnitId};

    #[test]
    fn the_layout_of_2_31() {
        let key = DocKey::parse("environments/p/units/r/hosts/a/values").unwrap();
        assert_eq!(history_dir(&key), ".history/environments/p/units/r/hosts/a/values");
        assert_eq!(version_file(12), "0000000012.json");
        let scope = SecretScope::Unit(EnvId::parse("p").unwrap(), UnitId::parse("r").unwrap());
        assert_eq!(
            secret_value_file(&scope, &KeyPath::parse("storage.password").unwrap()),
            "values/environments/p/units/r/storage.password"
        );
    }
}
