// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runs of the backend `files` (2.27, 2.28, 2.35):
//!
//! ```text
//! <run id>/run.json       the record, written again at each change
//! <run id>/output.log     the output, added with O_APPEND
//! <run id>/events.jsonl   the event lines, added with O_APPEND
//! ```
//!
//! The store keeps the records in memory. The files are the truth at open.

use std::collections::BTreeMap;
use std::sync::Arc;

use pupitre_core::ids::{EnvId, RunId};
use pupitre_core::json::{self, ReadLimits};
use pupitre_core::run::{RunRecord, RunStatus};

use super::events::Events;
use super::fsio::{Dir, FType};
use super::run_dir;
use crate::transfer::RunExport;
use crate::types::{EventLine, OutputChunk, RunFilter, StoreError, StoreEvent};

const RUN_FILE: &str = "run.json";
const OUTPUT_FILE: &str = "output.log";
const EVENTS_FILE: &str = "events.jsonl";
/// The largest `run.json` that the backend reads.
const MAX_RECORD_BYTES: usize = 1 << 20;
/// The largest file of events or output that the backend reads in one call.
const MAX_READ_BYTES: usize = 1 << 30;

#[derive(Debug, Clone)]
struct RunEntry {
    record: RunRecord,
    /// The number of the last event line.
    events_last: u64,
}

/// The runs folder.
pub(crate) struct RunStore {
    root: Dir,
    tmp: Dir,
    index: BTreeMap<RunId, RunEntry>,
}

impl std::fmt::Debug for RunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunStore").field("root", &self.root.label()).field("runs", &self.index.len()).finish()
    }
}

fn corrupt(place: &str, what: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("{place}: {what}"))
}

fn encode(record: &RunRecord) -> Vec<u8> {
    json::to_canonical(&record.to_json())
}

impl RunStore {
    /// Open the folder: read each `run.json` and check the files of each run (2.35).
    pub(crate) fn open(root: Dir, tmp: Dir, events: &Arc<Events>) -> Result<RunStore, StoreError> {
        let mut index = BTreeMap::new();
        for item in root.list()? {
            if item.name == super::LOCK_FILE || item.name == super::TMP_DIR {
                continue;
            }
            let id = match RunId::parse(&item.name) {
                Ok(id) if item.ftype == FType::Dir && item.name_ok => id,
                _ => {
                    events.push(StoreEvent::EntryIgnored {
                        path: item.name.clone(),
                        reason: format!("not a folder of a run: {}", item.ftype.describe()),
                    });
                    continue;
                }
            };
            let dir = root.open_dir(&item.name)?;
            let place = format!("{}/{RUN_FILE}", dir.label());
            let Some((bytes, _)) = dir.read_file_opt(RUN_FILE, MAX_RECORD_BYTES)? else {
                events.push(StoreEvent::EntryIgnored { path: item.name, reason: "no file run.json".to_string() });
                continue;
            };
            let value = json::parse(&bytes, ReadLimits::bytes(MAX_RECORD_BYTES)).map_err(|e| corrupt(&place, e))?;
            let record = RunRecord::from_json(&value).map_err(|errors| {
                corrupt(&place, errors.first().map_or_else(|| "the record is not valid".to_string(), |e| e.to_string()))
            })?;
            if record.id != id {
                return Err(corrupt(&place, "the id of the record is not the name of the folder"));
            }
            if json::canonical_difference(&bytes, &record.to_json()).is_some() {
                return Err(corrupt(&place, "the record is not in the canonical form"));
            }
            let events_last = Self::fix_events(&dir)?;
            // The output needs no repair: the caller adds at the length that it reads.
            dir.file_len(OUTPUT_FILE)?;
            index.insert(id, RunEntry { record, events_last });
        }
        Ok(RunStore { root, tmp, index })
    }

    /// Cut a partial last line, which a crash during an append can leave. Give the number of
    /// lines.
    fn fix_events(dir: &Dir) -> Result<u64, StoreError> {
        let Some(bytes) = Self::read_events_file(dir)? else { return Ok(0) };
        let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        if complete < bytes.len() {
            dir.truncate(EVENTS_FILE, complete as u64)?;
        }
        Ok(bytes[..complete].iter().filter(|b| **b == b'\n').count() as u64)
    }

    /// The bytes of the file of events, or `None`. A file above `MAX_READ_BYTES` gives an error:
    /// the store never cuts or ignores a part of it (7.11).
    fn read_events_file(dir: &Dir) -> Result<Option<Vec<u8>>, StoreError> {
        if dir.file_len(EVENTS_FILE)? > MAX_READ_BYTES as u64 {
            return Err(corrupt(
                &format!("{}/{EVENTS_FILE}", dir.label()),
                format!("the file is above the limit of {MAX_READ_BYTES} bytes"),
            ));
        }
        Ok(dir.read_file_opt(EVENTS_FILE, MAX_READ_BYTES)?.map(|(bytes, _)| bytes))
    }

    /// True when the folder has no run.
    pub(crate) fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    fn write_record(&self, record: &RunRecord) -> Result<(), StoreError> {
        let dir = self.root.open_dir(&run_dir(&record.id))?;
        dir.write_atomic(&self.tmp, RUN_FILE, &encode(record))?;
        Ok(())
    }

    fn entry(&self, id: &RunId) -> Result<&RunEntry, StoreError> {
        self.index.get(id).ok_or(StoreError::NotFound)
    }

    /// Write a new run with the status `queued` (2.27).
    pub(crate) fn create(
        &mut self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> Result<(), StoreError> {
        if record.status != RunStatus::Queued {
            return Err(StoreError::BadTransition { from: RunStatus::Queued, to: record.status });
        }
        if self.index.contains_key(&record.id) {
            return Err(StoreError::AlreadyExists);
        }
        let queued = self.index.values().filter(|e| e.record.status == RunStatus::Queued).count();
        if queued >= usize::try_from(max_queued).unwrap_or(usize::MAX) {
            return Err(StoreError::QueueFull);
        }
        let mut marked: Option<(RunRecord, RunRecord)> = None;
        if let Some(check) = consumes {
            let before = self.entry(check)?.record.clone();
            if before.applied_by.is_some() {
                return Err(StoreError::CheckUsed);
            }
            let after = RunRecord { applied_by: Some(record.id), ..before.clone() };
            // The check is marked first: a crash then leaves a check with no apply, which is
            // the safe side.
            self.write_record(&after)?;
            marked = Some((before, after));
        }
        let made = self.create_files(record);
        if let Err(e) = made {
            if let Some((before, _)) = &marked {
                let _ = self.write_record(before);
            }
            return Err(e);
        }
        if let Some((_, after)) = marked
            && let Some(e) = self.index.get_mut(&after.id)
        {
            e.record = after;
        }
        self.index.insert(record.id, RunEntry { record: record.clone(), events_last: 0 });
        Ok(())
    }

    fn create_files(&self, record: &RunRecord) -> Result<(), StoreError> {
        let dir = self.root.open_dir_create(&run_dir(&record.id))?;
        dir.write_atomic(&self.tmp, RUN_FILE, &encode(record))?;
        Ok(())
    }

    /// Give a run record, or `NotFound`.
    pub(crate) fn get(&self, id: &RunId) -> Result<RunRecord, StoreError> {
        Ok(self.entry(id)?.record.clone())
    }

    /// Give the runs of the filter, newest first, with an id below `before`.
    pub(crate) fn list(&self, filter: &RunFilter, before: Option<&RunId>, limit: u32) -> Vec<RunRecord> {
        self.list_page(filter, before, 0, limit)
    }

    /// Like `list`, with the first `skip` runs left out. Only the runs of the page are copied,
    /// so the time under the lock does not grow with `skip` (L8).
    pub(crate) fn list_page(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        skip: u32,
        limit: u32,
    ) -> Vec<RunRecord> {
        self.index
            .iter()
            .rev()
            .filter(|(id, _)| before.is_none_or(|b| *id < b))
            .map(|(_, e)| &e.record)
            .filter(|r| filter.environment.as_ref().is_none_or(|env| r.environment == *env))
            .filter(|r| filter.status.is_none_or(|s| r.status == s))
            .skip(usize::try_from(skip).unwrap_or(usize::MAX))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .cloned()
            .collect()
    }

    /// Give the oldest queued runs whose environment is not in `busy`.
    pub(crate) fn next_queued(&self, busy: &[EnvId], limit: u32) -> Vec<RunRecord> {
        self.index
            .values()
            .map(|e| &e.record)
            .filter(|r| r.status == RunStatus::Queued && !busy.contains(&r.environment))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .cloned()
            .collect()
    }

    /// Replace a record when its stored status is `expected` (2.27).
    pub(crate) fn update(&mut self, expected: RunStatus, record: &RunRecord) -> Result<(), StoreError> {
        let stored = self.entry(&record.id)?.record.status;
        if stored != expected {
            return Err(StoreError::Conflict { current: None });
        }
        if record.status != expected && !expected.can_move_to(record.status) {
            return Err(StoreError::BadTransition { from: expected, to: record.status });
        }
        self.write_record(record)?;
        if let Some(e) = self.index.get_mut(&record.id) {
            e.record = record.clone();
        }
        Ok(())
    }

    /// Remove the oldest final runs above `keep`, with their files. Give their ids.
    pub(crate) fn prune(&mut self, keep: u32) -> Result<Vec<RunId>, StoreError> {
        let finals: Vec<RunId> =
            self.index.iter().filter(|(_, e)| e.record.status.is_final()).map(|(id, _)| *id).collect();
        let keep = usize::try_from(keep).unwrap_or(usize::MAX);
        if finals.len() <= keep {
            return Ok(Vec::new());
        }
        let mut removed = Vec::new();
        for id in &finals[..finals.len() - keep] {
            let name = run_dir(id);
            let dir = self.root.open_dir(&name)?;
            // The record goes last: a crash leaves a run that the next prune removes.
            dir.unlink_opt(OUTPUT_FILE)?;
            dir.unlink_opt(EVENTS_FILE)?;
            dir.unlink_opt(RUN_FILE)?;
            drop(dir);
            self.root.rmdir(&name)?;
            self.root.fsync()?;
            self.index.remove(id);
            removed.push(*id);
        }
        Ok(removed)
    }

    /// Add output bytes at `offset`, the current length (2.28).
    pub(crate) fn append_output(&mut self, id: &RunId, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        self.entry(id)?;
        let dir = self.root.open_dir(&run_dir(id))?;
        let length = dir.file_len(OUTPUT_FILE)?;
        if length != offset {
            return Err(StoreError::Conflict { current: None });
        }
        if bytes.is_empty() {
            return Ok(length);
        }
        Ok(dir.append(OUTPUT_FILE, bytes)?)
    }

    /// Give output bytes from `offset`.
    pub(crate) fn read_output(&self, id: &RunId, offset: u64, max: u32) -> Result<OutputChunk, StoreError> {
        self.entry(id)?;
        let dir = self.root.open_dir(&run_dir(id))?;
        let (bytes, length) = dir.read_range(OUTPUT_FILE, offset, usize::try_from(max).unwrap_or(usize::MAX))?;
        Ok(OutputChunk { bytes, length })
    }

    /// Add event lines with the numbers from `first_seq`, the next number (2.28).
    pub(crate) fn append_events(&mut self, id: &RunId, first_seq: u64, lines: &[Vec<u8>]) -> Result<(), StoreError> {
        let next = self.entry(id)?.events_last + 1;
        if first_seq != next {
            return Err(StoreError::Conflict { current: None });
        }
        if lines.is_empty() {
            return Ok(());
        }
        if lines.iter().any(|l| l.contains(&b'\n')) {
            return Err(corrupt(&run_dir(id), "an event line holds a line end"));
        }
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }
        let dir = self.root.open_dir(&run_dir(id))?;
        match dir.append(EVENTS_FILE, &bytes) {
            Ok(_) => {
                if let Some(e) = self.index.get_mut(id) {
                    e.events_last += lines.len() as u64;
                }
                Ok(())
            }
            Err(err) => {
                // A part of the lines can be in the file. Read the state again.
                if let Ok(count) = Self::fix_events(&dir)
                    && let Some(e) = self.index.get_mut(id)
                {
                    e.events_last = count;
                }
                Err(err.into())
            }
        }
    }

    /// Give the event lines with a number above `after`.
    pub(crate) fn read_events(&self, id: &RunId, after: u64, limit: u32) -> Result<Vec<EventLine>, StoreError> {
        self.entry(id)?;
        let dir = self.root.open_dir(&run_dir(id))?;
        let Some(bytes) = Self::read_events_file(&dir)? else { return Ok(Vec::new()) };
        let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        if complete == 0 {
            return Ok(Vec::new());
        }
        let skip = usize::try_from(after).unwrap_or(usize::MAX);
        let take = usize::try_from(limit).unwrap_or(usize::MAX);
        Ok(bytes[..complete - 1]
            .split(|b| *b == b'\n')
            .enumerate()
            .skip(skip)
            .take(take)
            .map(|(i, line)| EventLine { seq: i as u64 + 1, bytes: line.to_vec() })
            .collect())
    }

    // Transfer.

    /// The run ids, oldest first.
    pub(crate) fn ids(&self) -> Vec<RunId> {
        self.index.keys().copied().collect()
    }

    /// One run with its output and events.
    pub(crate) fn export(&self, id: &RunId) -> Result<RunExport, StoreError> {
        let record = self.get(id)?;
        let dir = self.root.open_dir(&run_dir(id))?;
        let output = dir.read_file_opt(OUTPUT_FILE, MAX_READ_BYTES)?.map(|(b, _)| b).unwrap_or_default();
        let events = self.read_events(id, 0, u32::MAX)?;
        Ok(RunExport { record, output, events })
    }

    /// Write one run of an import with its output and events.
    pub(crate) fn import(&mut self, run: &RunExport) -> Result<(), StoreError> {
        let id = run.record.id;
        if self.index.contains_key(&id) {
            return Err(StoreError::AlreadyExists);
        }
        for (i, line) in run.events.iter().enumerate() {
            if line.seq != i as u64 + 1 || line.bytes.contains(&b'\n') {
                return Err(corrupt(&run_dir(&id), "the event lines are not numbered from 1 with no gap"));
            }
        }
        let dir = self.root.open_dir_create(&run_dir(&id))?;
        if !run.output.is_empty() {
            dir.append(OUTPUT_FILE, &run.output)?;
        }
        let mut lines = Vec::new();
        for line in &run.events {
            lines.extend_from_slice(&line.bytes);
            lines.push(b'\n');
        }
        if !lines.is_empty() {
            dir.append(EVENTS_FILE, &lines)?;
        }
        dir.write_atomic(&self.tmp, RUN_FILE, &encode(&run.record))?;
        self.index.insert(id, RunEntry { record: run.record.clone(), events_last: run.events.len() as u64 });
        Ok(())
    }
}
