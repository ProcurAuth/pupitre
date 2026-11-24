// SPDX-License-Identifier: MIT OR Apache-2.0
//! The documents of the backend `files` (2.18 to 2.23, 2.31, 2.33): the current files, the
//! versions in `.history`, the repair after a crash (2.19) and the changes outside Pupitre
//! (2.33).
//!
//! The store keeps an index with the last version and the last etag of each key, and the
//! signature of each current file. A poll compares the signatures and reads a file only when
//! its signature is new. Each method takes `&mut self`: the caller holds the lock of the
//! process (2.32).

use std::collections::BTreeMap;
use std::sync::Arc;

use pupitre_core::doc::{CanonicalDocument, DocKey, Document};
use pupitre_core::etag::Etag;
use pupitre_core::ids::{EnvId, HostId, Subject, UnitId};
use pupitre_core::time::UtcMillis;

use super::events::Events;
use super::faults::FaultPoint;
use super::fsio::{Dir, DirItem, FType};
use super::meta::{
    MAX_SMALL_FILE, VersionRecord, decode_version, encode_version, meta_file, parse_bytes_name, parse_meta_name,
};
use super::{HISTORY_DIR, version_file};
use crate::transfer::KeyHistory;
use crate::types::{
    FileSig, Message, Precondition, Saved, StoreError, StoreEvent, StoredDoc, Version, VersionKind, VersionMeta,
    WriteMeta,
};

/// The deepest folder of the tree of the versions: a key has 7 segments at most.
const MAX_HISTORY_DEPTH: usize = 8;

/// What the store knows about the current file of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observed {
    /// The store has not looked at the file since the last version came.
    Unknown,
    /// The file does not exist.
    Absent,
    /// The file has this signature.
    File(FileSig),
}

#[derive(Debug, Clone)]
struct KeyState {
    /// The highest version that has a metadata file.
    version: u64,
    /// The etag of the document of that version, or `None` for a version `delete` or no version.
    etag: Option<Etag>,
    observed: Observed,
    /// A write of this process wrote the version and failed before the document (2.19).
    dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    Same,
    Rewritten,
}

/// The answer of a poll: the documents that changed, the keys that are gone, and the new marks.
pub(crate) struct PollResult {
    pub(crate) changed: Vec<StoredDoc>,
    pub(crate) removed: Vec<DocKey>,
    pub(crate) marks: BTreeMap<String, FileSig>,
}

/// The documents folder.
pub(crate) struct DocStore {
    root: Dir,
    tmp: Dir,
    history_keep: u32,
    max_bytes: usize,
    index: BTreeMap<DocKey, KeyState>,
    events: Arc<Events>,
}

impl std::fmt::Debug for DocStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocStore").field("root", &self.root.label()).field("keys", &self.index.len()).finish()
    }
}

fn external_meta() -> WriteMeta {
    WriteMeta {
        saved_by: Subject::parse(Subject::EXTERNAL).unwrap_or_else(|_| Subject::anonymous()),
        saved_at: UtcMillis::now(),
        message: Message::default(),
    }
}

fn split_file_path(path: &str) -> (Vec<&str>, &str) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (dir.split('/').collect(), name),
        None => (Vec::new(), path),
    }
}

impl DocStore {
    /// Open the folder and bring the index in line with it: the repair of 2.19 and the
    /// changes of 2.33.
    pub(crate) fn open(
        root: Dir,
        tmp: Dir,
        events: Arc<Events>,
        history_keep: u32,
        max_bytes: usize,
    ) -> Result<DocStore, StoreError> {
        let mut store = DocStore { root, tmp, history_keep, max_bytes, index: BTreeMap::new(), events };
        store.sync()?;
        Ok(store)
    }

    /// A store with no scan, for the writer of a folder for a review.
    pub(crate) fn blank(root: Dir, tmp: Dir, events: Arc<Events>) -> DocStore {
        DocStore { root, tmp, history_keep: u32::MAX, max_bytes: usize::MAX / 2, index: BTreeMap::new(), events }
    }

    /// True when the store has no key with a document or a version.
    pub(crate) fn is_empty(&self) -> bool {
        !self.index.values().any(|s| s.version > 0 || matches!(s.observed, Observed::File(_)))
    }

    // History folders.

    fn hist_dir(&self, key: &DocKey, create: bool) -> Result<Option<Dir>, StoreError> {
        let text = key.text();
        let mut segments: Vec<&str> = vec![HISTORY_DIR];
        segments.extend(text.split('/'));
        Ok(self.root.open_path(&segments, create)?)
    }

    fn hist_dir_made(&self, key: &DocKey) -> Result<Dir, StoreError> {
        self.hist_dir(key, true)?.ok_or_else(|| StoreError::Corrupt(format!("{HISTORY_DIR}: no folder for {key}")))
    }

    fn versions_of(hdir: &Dir) -> Result<Vec<u64>, StoreError> {
        let mut versions: Vec<u64> =
            hdir.list()?.iter().filter(|i| i.ftype == FType::File).filter_map(|i| parse_meta_name(&i.name)).collect();
        versions.sort_unstable();
        Ok(versions)
    }

    fn read_record(hdir: &Dir, version: u64) -> Result<VersionRecord, StoreError> {
        let name = meta_file(version);
        let (bytes, _) = hdir.read_file(&name, MAX_SMALL_FILE)?;
        decode_version(&bytes, &format!("{}/{name}", hdir.label()))
    }

    /// Note a version record that is not valid. The record stays on the disk.
    fn note_bad_record(&self, key: &DocKey, version: u64, reason: &str, in_scan: bool) {
        let path = format!("{HISTORY_DIR}/{}/{}", key.text(), meta_file(version));
        if in_scan {
            self.events.scan_note(&path, reason);
        } else {
            self.events.push(StoreEvent::EntryIgnored { path, reason: reason.to_string() });
        }
    }

    /// The newest record up to `highest` that reads, with its number, or `None`. A record that
    /// is not valid gives an event `store.entry_ignored` in the scan. Other errors stop the call.
    fn newest_record(
        &self,
        hdir: &Dir,
        key: &DocKey,
        highest: u64,
    ) -> Result<Option<(u64, VersionRecord)>, StoreError> {
        let mut versions = Self::versions_of(hdir)?;
        versions.retain(|v| *v <= highest);
        for v in versions.into_iter().rev() {
            match Self::read_record(hdir, v) {
                Ok(rec) => return Ok(Some((v, rec))),
                Err(StoreError::Corrupt(reason)) => self.note_bad_record(key, v, &reason, true),
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    fn write_version(
        &self,
        key: &DocKey,
        version: u64,
        record: &VersionRecord,
        bytes: Option<&[u8]>,
    ) -> Result<(), StoreError> {
        let hdir = self.hist_dir_made(key)?;
        if let Some(bytes) = bytes {
            hdir.write_atomic(&self.tmp, &version_file(version), bytes)?;
            hdir.fault(FaultPoint::AfterHistoryBytes)?;
        }
        hdir.write_atomic(&self.tmp, &meta_file(version), &encode_version(record))?;
        Ok(())
    }

    /// Remove the oldest versions above `history_keep`. The numbers of the others stay (2.18).
    /// A failure here changes no document, so the next write tries again.
    fn prune(&self, key: &DocKey) {
        let run = || -> Result<(), StoreError> {
            let Some(hdir) = self.hist_dir(key, false)? else { return Ok(()) };
            let versions = Self::versions_of(&hdir)?;
            let keep = usize::try_from(self.history_keep).unwrap_or(usize::MAX);
            if versions.len() > keep {
                for v in &versions[..versions.len() - keep] {
                    hdir.unlink_opt(&meta_file(*v))?;
                    hdir.unlink_opt(&version_file(*v))?;
                }
                hdir.fsync()?;
            }
            Ok(())
        };
        let _ = run();
    }

    // The current files.

    fn read_current(&self, key: &DocKey) -> Result<Option<(Vec<u8>, FileSig)>, StoreError> {
        let path = key.file_path();
        let (segments, name) = split_file_path(&path);
        let Some(dir) = self.root.open_path(&segments, false)? else { return Ok(None) };
        Ok(dir.read_file_opt(name, self.max_bytes)?.map(|(bytes, st)| (bytes, st.sig)))
    }

    fn write_current(&self, key: &DocKey, bytes: &[u8]) -> Result<FileSig, StoreError> {
        let path = key.file_path();
        let (segments, name) = split_file_path(&path);
        let dir = self.root.open_path(&segments, true)?.ok_or_else(|| StoreError::Corrupt(path.clone()))?;
        Ok(dir.write_atomic(&self.tmp, name, bytes)?.sig)
    }

    fn remove_current(&self, key: &DocKey) -> Result<(), StoreError> {
        let path = key.file_path();
        let (segments, name) = split_file_path(&path);
        if let Some(dir) = self.root.open_path(&segments, false)? {
            dir.unlink_opt(name)?;
            dir.fsync()?;
        }
        Ok(())
    }

    /// The state of the key after a look at its file (2.33, 2.19).
    fn settle(&mut self, key: &DocKey, seen: Option<(&[u8], FileSig)>) -> Result<Settled, StoreError> {
        let state = self.index.get(key).cloned().unwrap_or(KeyState {
            version: 0,
            etag: None,
            observed: Observed::Unknown,
            dirty: false,
        });
        let now = seen.map(|(bytes, _)| Etag::of(bytes));
        let observed = seen.map_or(Observed::Absent, |(_, sig)| Observed::File(sig));
        let mut next = KeyState { observed, dirty: false, ..state.clone() };
        if now == state.etag {
            self.index.insert(key.clone(), next);
            return Ok(Settled::Same);
        }
        // The document is not the document of the last version.
        let may_repair = state.version >= 1 && (state.observed == Observed::Unknown || state.dirty);
        if may_repair && self.prev_matches(key, state.version, now)? {
            self.repair(key, state.version)?;
            return Ok(Settled::Rewritten);
        }
        match seen {
            None => {
                // The document is gone: a version `delete` (2.33).
                self.write_external(key, &mut next, None)?;
            }
            Some((bytes, _)) => {
                let valid =
                    bytes.len() <= self.max_bytes && Document::from_stored(key.kind(), bytes, self.max_bytes).is_ok();
                if valid {
                    self.write_external(key, &mut next, Some(bytes))?;
                }
                // A document that is not valid stays out: no version (2.33). The catalog shows
                // its errors.
            }
        }
        self.index.insert(key.clone(), next);
        Ok(Settled::Same)
    }

    /// Tell if the document of `now` is the state before the version `version`.
    fn prev_matches(&self, key: &DocKey, version: u64, now: Option<Etag>) -> Result<bool, StoreError> {
        if version == 1 {
            return Ok(now.is_none());
        }
        let Some(hdir) = self.hist_dir(key, false)? else { return Ok(false) };
        match Self::read_record(&hdir, version - 1) {
            Ok(rec) => Ok(rec.etag == now),
            Err(StoreError::Unavailable(_) | StoreError::Corrupt(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Write the current document of the version again (2.19).
    fn repair(&mut self, key: &DocKey, version: u64) -> Result<(), StoreError> {
        let hdir = self.hist_dir_made(key)?;
        let rec = Self::read_record(&hdir, version)?;
        let observed = if rec.kind == VersionKind::Delete {
            self.remove_current(key)?;
            Observed::Absent
        } else {
            let (bytes, _) = hdir.read_file(&version_file(version), self.max_bytes)?;
            if Some(Etag::of(&bytes)) != rec.etag {
                return Err(StoreError::Corrupt(format!(
                    "{}/{}: the bytes do not match the etag of the version",
                    hdir.label(),
                    version_file(version)
                )));
            }
            Observed::File(self.write_current(key, &bytes)?)
        };
        self.index.insert(key.clone(), KeyState { version, etag: rec.etag, observed, dirty: false });
        self.events.push(StoreEvent::Repaired { key: key.clone(), version });
        Ok(())
    }

    /// Write a version for a change outside Pupitre, and update the state `next`.
    fn write_external(&self, key: &DocKey, next: &mut KeyState, bytes: Option<&[u8]>) -> Result<(), StoreError> {
        let version = next.version + 1;
        let etag = bytes.map(Etag::of);
        let kind = if bytes.is_some() { VersionKind::External } else { VersionKind::Delete };
        let record = VersionRecord { kind, etag, meta: external_meta() };
        self.write_version(key, version, &record, bytes)?;
        self.events.push(StoreEvent::ExternalChange { key: key.clone(), before: next.etag, after: etag });
        next.version = version;
        next.etag = etag;
        self.prune(key);
        Ok(())
    }

    /// Read the file of a key and settle the state. Give the bytes and the signature.
    fn current(&mut self, key: &DocKey) -> Result<Option<(Vec<u8>, FileSig)>, StoreError> {
        for _ in 0..2 {
            let read = self.read_current(key)?;
            let settled = self.settle(key, read.as_ref().map(|(bytes, sig)| (bytes.as_slice(), *sig)))?;
            if settled == Settled::Same {
                return Ok(read);
            }
        }
        Err(StoreError::Corrupt(format!("{key}: the repair of the document did not end")))
    }

    // Reads.

    /// Give the current document of a key.
    pub(crate) fn get(&mut self, key: &DocKey) -> Result<StoredDoc, StoreError> {
        let (bytes, _) = self.current(key)?.ok_or(StoreError::NotFound)?;
        let version = self.index.get(key).map_or(0, |s| s.version);
        Ok(StoredDoc { key: key.clone(), etag: Etag::of(&bytes), bytes, version })
    }

    /// Give the versions of a key, newest first, below `before`.
    pub(crate) fn history(
        &mut self,
        key: &DocKey,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<VersionMeta>, StoreError> {
        self.current(key)?;
        // A key that never had a version has an empty history, as on the backend `postgres`.
        let Some(hdir) = self.hist_dir(key, false)? else {
            return Ok(Vec::new());
        };
        let mut versions = Self::versions_of(&hdir)?;
        versions.reverse();
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut out = Vec::new();
        for v in versions.into_iter().filter(|v| before.is_none_or(|b| *v < b)).take(limit) {
            let rec = match Self::read_record(&hdir, v) {
                Ok(rec) => rec,
                Err(StoreError::Corrupt(reason)) => {
                    self.note_bad_record(key, v, &reason, false);
                    continue;
                }
                Err(e) => return Err(e),
            };
            out.push(VersionMeta { version: v, kind: rec.kind, etag: rec.etag, meta: rec.meta });
        }
        Ok(out)
    }

    /// Give one version with its bytes.
    pub(crate) fn version(&mut self, key: &DocKey, version: u64) -> Result<Version, StoreError> {
        self.current(key)?;
        let hdir = self.hist_dir(key, false)?.ok_or(StoreError::NotFound)?;
        self.read_version(&hdir, version)
    }

    fn read_version(&self, hdir: &Dir, version: u64) -> Result<Version, StoreError> {
        let rec = match Self::read_record(hdir, version) {
            Ok(rec) => rec,
            Err(StoreError::Unavailable(_)) => return Err(StoreError::NotFound),
            Err(e) => return Err(e),
        };
        let bytes = if rec.kind == VersionKind::Delete {
            None
        } else {
            let name = version_file(version);
            let (bytes, _) = hdir.read_file(&name, self.max_bytes)?;
            if Some(Etag::of(&bytes)) != rec.etag {
                return Err(StoreError::Corrupt(format!(
                    "{}/{name}: the bytes do not match the etag of the version",
                    hdir.label()
                )));
            }
            Some(bytes)
        };
        Ok(Version { meta: VersionMeta { version, kind: rec.kind, etag: rec.etag, meta: rec.meta }, bytes })
    }

    // Writes.

    /// Write a document with its precondition (2.19, 2.20).
    pub(crate) fn put(
        &mut self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        self.put_as(key, doc, precondition, meta, VersionKind::Create)
    }

    /// Write a document. A create has the kind `create_kind`: `create`, or `import` for the
    /// command `import` (6.24a).
    pub(crate) fn put_as(
        &mut self,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
        create_kind: VersionKind,
    ) -> Result<Saved, StoreError> {
        let current = self.current(key)?;
        let kind = match (precondition, &current) {
            (Precondition::Create, Some(_)) => return Err(StoreError::AlreadyExists),
            (Precondition::Create, None) => create_kind,
            (Precondition::Match(_), None) => return Err(StoreError::NotFound),
            (Precondition::Match(expected), Some((bytes, _))) => {
                let have = Etag::of(bytes);
                if have != *expected {
                    return Err(StoreError::Conflict { current: Some(have) });
                }
                VersionKind::Save
            }
        };
        self.commit(key, kind, Some(&doc.bytes), meta)
    }

    /// Delete a document: a version `delete`, then no current document (2.21).
    pub(crate) fn delete(&mut self, key: &DocKey, if_match: &Etag, meta: &WriteMeta) -> Result<Saved, StoreError> {
        let Some((bytes, _)) = self.current(key)? else { return Err(StoreError::NotFound) };
        let have = Etag::of(&bytes);
        if have != *if_match {
            return Err(StoreError::Conflict { current: Some(have) });
        }
        self.commit(key, VersionKind::Delete, None, meta)
    }

    /// The steps 2 and 3 of 2.19.
    fn commit(
        &mut self,
        key: &DocKey,
        kind: VersionKind,
        bytes: Option<&[u8]>,
        meta: &WriteMeta,
    ) -> Result<Saved, StoreError> {
        let version = self.index.get(key).map_or(0, |s| s.version) + 1;
        let etag = bytes.map(Etag::of);
        let record = VersionRecord { kind, etag, meta: meta.clone() };
        self.write_version(key, version, &record, bytes)?;
        // The version exists. A failure from here leaves the key to the repair of 2.19.
        self.index.insert(key.clone(), KeyState { version, etag, observed: Observed::Unknown, dirty: true });
        self.root.fault(FaultPoint::AfterHistory)?;
        let observed = match bytes {
            Some(bytes) => Observed::File(self.write_current(key, bytes)?),
            None => {
                self.remove_current(key)?;
                Observed::Absent
            }
        };
        self.index.insert(key.clone(), KeyState { version, etag, observed, dirty: false });
        self.prune(key);
        Ok(Saved { version, etag })
    }

    // The tree of the current files.

    fn note(&self, path: &str, item: &DirItem, why: &str) {
        let reason = if item.name_ok {
            format!("{why}: {}", item.ftype.describe())
        } else {
            "the name is not UTF-8".to_string()
        };
        self.events.scan_note(path, &reason);
    }

    fn file_key(
        &self,
        dir: &Dir,
        rel: &str,
        item: &DirItem,
        key: Option<DocKey>,
        out: &mut Vec<(DocKey, FileSig)>,
    ) -> Result<(), StoreError> {
        match key {
            Some(key) if item.ftype == FType::File && item.name_ok => {
                if let Some(st) = dir.stat_opt(&item.name)? {
                    out.push((key, st.sig));
                }
            }
            _ => self.note(&format!("{rel}/{}", item.name), item, "not a file of the layout of 2.31"),
        }
        Ok(())
    }

    /// Walk the tree `environments/`. An entry that is not a file of the layout is ignored,
    /// with the event `store.entry_ignored` (2.31).
    fn walk_current(&self) -> Result<Vec<(DocKey, FileSig)>, StoreError> {
        let mut out = Vec::new();
        let Some(envs) = self.root.open_path(&["environments"], false)? else { return Ok(out) };
        for e in envs.list()? {
            let env = match EnvId::parse(&e.name) {
                Ok(env) if e.ftype == FType::Dir && e.name_ok => env,
                _ => {
                    self.note(&format!("environments/{}", e.name), &e, "not a folder of an environment");
                    continue;
                }
            };
            let env_rel = format!("environments/{}", e.name);
            let Some(env_dir) = envs.open_dir_opt(&e.name)? else { continue };
            for f in env_dir.list()? {
                match (f.name.as_str(), f.ftype) {
                    ("environment.json", _) => {
                        self.file_key(&env_dir, &env_rel, &f, Some(DocKey::Environment(env.clone())), &mut out)?
                    }
                    ("common.json", _) => {
                        self.file_key(&env_dir, &env_rel, &f, Some(DocKey::Common(env.clone())), &mut out)?
                    }
                    ("hosts", FType::Dir) => self.walk_hosts(&env_dir, &env_rel, &env, &mut out)?,
                    ("units", FType::Dir) => self.walk_units(&env_dir, &env_rel, &env, &mut out)?,
                    _ => self.note(&format!("{env_rel}/{}", f.name), &f, "not a file of the layout of 2.31"),
                }
            }
        }
        Ok(out)
    }

    fn walk_hosts(
        &self,
        env_dir: &Dir,
        env_rel: &str,
        env: &EnvId,
        out: &mut Vec<(DocKey, FileSig)>,
    ) -> Result<(), StoreError> {
        let Some(dir) = env_dir.open_dir_opt("hosts")? else { return Ok(()) };
        let rel = format!("{env_rel}/hosts");
        for h in dir.list()? {
            let key = h
                .name
                .strip_suffix(".json")
                .and_then(|id| HostId::parse(id).ok())
                .map(|host| DocKey::Host(env.clone(), host));
            self.file_key(&dir, &rel, &h, key, out)?;
        }
        Ok(())
    }

    fn walk_units(
        &self,
        env_dir: &Dir,
        env_rel: &str,
        env: &EnvId,
        out: &mut Vec<(DocKey, FileSig)>,
    ) -> Result<(), StoreError> {
        let Some(units) = env_dir.open_dir_opt("units")? else { return Ok(()) };
        let units_rel = format!("{env_rel}/units");
        for u in units.list()? {
            let unit = match UnitId::parse(&u.name) {
                Ok(unit) if u.ftype == FType::Dir && u.name_ok => unit,
                _ => {
                    self.note(&format!("{units_rel}/{}", u.name), &u, "not a folder of a unit");
                    continue;
                }
            };
            let Some(unit_dir) = units.open_dir_opt(&u.name)? else { continue };
            let unit_rel = format!("{units_rel}/{}", u.name);
            for f in unit_dir.list()? {
                match (f.name.as_str(), f.ftype) {
                    ("unit.json", _) => self.file_key(
                        &unit_dir,
                        &unit_rel,
                        &f,
                        Some(DocKey::Unit(env.clone(), unit.clone())),
                        &mut *out,
                    )?,
                    ("values.json", _) => self.file_key(
                        &unit_dir,
                        &unit_rel,
                        &f,
                        Some(DocKey::UnitValues(env.clone(), unit.clone())),
                        &mut *out,
                    )?,
                    ("hosts", FType::Dir) => {
                        let Some(hosts) = unit_dir.open_dir_opt("hosts")? else { continue };
                        let hosts_rel = format!("{unit_rel}/hosts");
                        for h in hosts.list()? {
                            let Ok(host) = HostId::parse(&h.name) else {
                                self.note(&format!("{hosts_rel}/{}", h.name), &h, "not a folder of a host");
                                continue;
                            };
                            if h.ftype != FType::Dir || !h.name_ok {
                                self.note(&format!("{hosts_rel}/{}", h.name), &h, "not a folder of a host");
                                continue;
                            }
                            let Some(host_dir) = hosts.open_dir_opt(&h.name)? else { continue };
                            let host_rel = format!("{hosts_rel}/{}", h.name);
                            for v in host_dir.list()? {
                                let key = (v.name == "values.json")
                                    .then(|| DocKey::HostValues(env.clone(), unit.clone(), host.clone()));
                                self.file_key(&host_dir, &host_rel, &v, key, &mut *out)?;
                            }
                        }
                    }
                    _ => self.note(&format!("{unit_rel}/{}", f.name), &f, "not a file of the layout of 2.31"),
                }
            }
        }
        Ok(())
    }

    /// Walk the tree `.history`. Give each key with its highest version.
    fn walk_history(&self) -> Result<Vec<(DocKey, u64)>, StoreError> {
        let mut out = Vec::new();
        let Some(root) = self.root.open_path(&[HISTORY_DIR], false)? else { return Ok(out) };
        self.walk_history_dir(&root, "", 0, &mut out)?;
        Ok(out)
    }

    fn walk_history_dir(
        &self,
        dir: &Dir,
        rel: &str,
        depth: usize,
        out: &mut Vec<(DocKey, u64)>,
    ) -> Result<(), StoreError> {
        let mut highest: Option<u64> = None;
        for item in dir.list()? {
            let path = format!("{HISTORY_DIR}/{rel}{}", item.name);
            match item.ftype {
                FType::File if item.name_ok => {
                    if let Some(v) = parse_meta_name(&item.name) {
                        highest = highest.max(Some(v));
                    } else if parse_bytes_name(&item.name).is_none() {
                        self.note(&path, &item, "not a file of the layout of 2.31");
                    }
                }
                FType::Dir if item.name_ok && depth < MAX_HISTORY_DEPTH => {
                    let Some(child) = dir.open_dir_opt(&item.name)? else { continue };
                    self.walk_history_dir(&child, &format!("{rel}{}/", item.name), depth + 1, out)?;
                }
                _ => self.note(&path, &item, "not a file of the layout of 2.31"),
            }
        }
        if let (Some(v), Ok(key)) = (highest, DocKey::parse(rel.trim_end_matches('/'))) {
            out.push((key, v));
        }
        Ok(())
    }

    // Load and poll.

    /// Bring the index in line with the versions and the files. Give the current files.
    fn sync(&mut self) -> Result<BTreeMap<String, (DocKey, FileSig)>, StoreError> {
        self.events.scan_begin();
        let files = self.sync_inner()?;
        self.events.scan_end();
        Ok(files)
    }

    fn sync_inner(&mut self) -> Result<BTreeMap<String, (DocKey, FileSig)>, StoreError> {
        for (key, version) in self.walk_history()? {
            if self.index.get(&key).is_some_and(|s| s.version >= version) {
                continue;
            }
            let hdir = self.hist_dir_made(&key)?;
            // A record that is not valid does not stop the store: the entry is ignored with an event,
            // and the newest record that reads gives the state of the key (L10, 7.11).
            let Some((version, rec)) = self.newest_record(&hdir, &key, version)? else { continue };
            if self.index.get(&key).is_some_and(|s| s.version >= version) {
                continue;
            }
            self.index.insert(key, KeyState { version, etag: rec.etag, observed: Observed::Unknown, dirty: false });
        }
        let mut files: BTreeMap<String, (DocKey, FileSig)> =
            self.walk_current()?.into_iter().map(|(key, sig)| (key.text(), (key, sig))).collect();
        for (key, sig) in files.values() {
            let known = self.index.get(key).is_some_and(|s| s.observed == Observed::File(*sig));
            if known {
                continue;
            }
            // The signature is new: read the file again (2.33).
            let read = self.read_current(key)?;
            self.settle(key, read.as_ref().map(|(bytes, sig)| (bytes.as_slice(), *sig)))?;
        }
        let gone: Vec<DocKey> = self
            .index
            .iter()
            .filter(|(key, s)| s.observed != Observed::Absent && !files.contains_key(&key.text()))
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            let path = key.file_path();
            let (segments, name) = split_file_path(&path);
            let absent = match self.root.open_path(&segments, false)? {
                None => true,
                Some(dir) => dir.stat_opt(name)?.is_none(),
            };
            if absent {
                self.current(&key)?;
            }
        }
        // A repair or a write can have replaced a file after the walk: use the state of the index.
        files.retain(|_, (key, _)| self.index.get(key).is_none_or(|s| s.observed != Observed::Absent));
        for (key, sig) in files.values_mut() {
            if let Some(Observed::File(now)) = self.index.get(key).map(|s| s.observed) {
                *sig = now;
            }
        }
        Ok(files)
    }

    /// Look at the folder again: the repair of 2.19 and the changes of 2.33.
    pub(crate) fn rescan(&mut self) -> Result<(), StoreError> {
        self.sync().map(|_| ())
    }

    /// Give the documents that changed since the marks, the keys that are gone, and the new
    /// marks (2.22). A file with a known signature is not read.
    pub(crate) fn poll(&mut self, marks: &BTreeMap<String, FileSig>) -> Result<PollResult, StoreError> {
        let files = self.sync()?;
        let mut changed = Vec::new();
        let mut new_marks = BTreeMap::new();
        for (text, (key, sig)) in &files {
            if marks.get(text) == Some(sig) {
                new_marks.insert(text.clone(), *sig);
                continue;
            }
            let Some((bytes, sig)) = self.read_current(key)? else { continue };
            let version = self.index.get(key).map_or(0, |s| s.version);
            changed.push(StoredDoc { key: key.clone(), etag: Etag::of(&bytes), bytes, version });
            new_marks.insert(text.clone(), sig);
        }
        let removed =
            marks.keys().filter(|text| !new_marks.contains_key(*text)).filter_map(|t| DocKey::parse(t).ok()).collect();
        Ok(PollResult { changed, removed, marks: new_marks })
    }

    /// Give each current document of an environment (2.23). The caller holds the lock, so no
    /// write of this process is half done.
    pub(crate) fn read_environment(&mut self, env: &EnvId) -> Result<Vec<StoredDoc>, StoreError> {
        let files = self.sync()?;
        let mut out = Vec::new();
        for (key, _) in files.values().filter(|(key, _)| key.environment() == env) {
            if let Some((bytes, _)) = self.read_current(key)? {
                let version = self.index.get(key).map_or(0, |s| s.version);
                out.push(StoredDoc { key: key.clone(), etag: Etag::of(&bytes), bytes, version });
            }
        }
        Ok(out)
    }

    // Transfer.

    /// The keys that have a current document or a version, in byte order.
    pub(crate) fn export_keys(&mut self) -> Result<Vec<DocKey>, StoreError> {
        self.sync()?;
        let mut keys: Vec<DocKey> = self
            .index
            .iter()
            .filter(|(_, s)| s.version > 0 || matches!(s.observed, Observed::File(_)))
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort_by_key(DocKey::text);
        Ok(keys)
    }

    /// One key with its versions and its current document.
    pub(crate) fn export_key(&mut self, key: &DocKey) -> Result<KeyHistory, StoreError> {
        let current = match self.get(key) {
            Ok(doc) => Some(doc),
            Err(StoreError::NotFound) => None,
            Err(e) => return Err(e),
        };
        let mut versions = Vec::new();
        if let Some(hdir) = self.hist_dir(key, false)? {
            for v in Self::versions_of(&hdir)? {
                versions.push(self.read_version(&hdir, v)?);
            }
        }
        Ok(KeyHistory { key: key.clone(), versions, current })
    }

    /// Write a key with its versions and its current document into an empty key (2.40). The
    /// numbers, the metadata and the etags stay as they are.
    pub(crate) fn import_key(&mut self, history: &KeyHistory) -> Result<(), StoreError> {
        let key = &history.key;
        if self.index.get(key).is_some_and(|s| s.version > 0 || matches!(s.observed, Observed::File(_))) {
            return Err(StoreError::AlreadyExists);
        }
        let bad = |what: &str| StoreError::Corrupt(format!("{key}: {what}"));
        let mut last: Option<u64> = None;
        for v in &history.versions {
            let n = v.meta.version;
            if n == 0 || last.is_some_and(|l| n <= l) {
                return Err(bad("the versions are not in ascending order from 1"));
            }
            match (&v.bytes, v.meta.kind) {
                (None, VersionKind::Delete) if v.meta.etag.is_none() => {}
                (Some(bytes), kind) if kind != VersionKind::Delete && v.meta.etag == Some(Etag::of(bytes)) => {}
                _ => return Err(bad("a version does not match its kind and its etag")),
            }
            last = Some(n);
        }
        let last_version = history.versions.last();
        match (&history.current, last_version) {
            (Some(doc), Some(v)) => {
                if doc.version != v.meta.version || v.bytes.as_deref() != Some(doc.bytes.as_slice()) {
                    return Err(bad("the current document is not the last version"));
                }
            }
            (None, Some(v)) if v.meta.kind != VersionKind::Delete => {
                return Err(bad("a key with no document ends with a version delete"));
            }
            (Some(doc), None) if doc.version != 0 => return Err(bad("a document has a version but no versions")),
            _ => {}
        }
        if let Some(doc) = &history.current
            && doc.etag != Etag::of(&doc.bytes)
        {
            return Err(bad("the etag does not match the bytes"));
        }
        for v in &history.versions {
            let record = VersionRecord { kind: v.meta.kind, etag: v.meta.etag, meta: v.meta.meta.clone() };
            self.write_version(key, v.meta.version, &record, v.bytes.as_deref())?;
        }
        let (observed, etag) = match &history.current {
            Some(doc) => (Observed::File(self.write_current(key, &doc.bytes)?), Some(doc.etag)),
            None => (Observed::Absent, None),
        };
        let version = last_version.map_or(0, |v| v.meta.version);
        self.index.insert(key.clone(), KeyState { version, etag, observed, dirty: false });
        Ok(())
    }
}
