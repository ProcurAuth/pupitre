// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pump of a run: it reads `output.log` and `events.jsonl`, scrubs them (4.21), and adds
//! them to the store in chunks (4.22).
//!
//! A `tick` adds at most one chunk of output and one chunk of events. The supervisor ticks each
//! second, so the store gets a chunk at most one time each second. `drain` adds all that
//! remains at the end of a phase. The files stay on the runner node. After a restart (4.29),
//! the pump reads them again. A small file `progress.json` in the run folder keeps the place of
//! the pump. Thus an adopted run does not read the whole file again.

use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pupitre_core::ids::{HostId, RunId};
use pupitre_core::json::{self, Json, Members, ReadLimits};
use pupitre_core::run::HostStats;
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::types::StoreError;

use crate::child::Phase;
use crate::events::{CallbackEvent, read_line, stored_line};
use crate::scrub::{Scrubber, StreamScrubber};
use crate::snapshot::disk;

/// The most bytes of one chunk of output or of events (2.28, 4.22).
pub(crate) const CHUNK_BYTES: usize = 65_536;

/// The bytes that the pump reads ahead from the events file.
const READ_AHEAD: usize = 2 * CHUNK_BYTES;

/// The bounds of one pump (4.20, 4.22).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PumpLimits {
    pub max_log_bytes: u64,
    pub max_events: u64,
    pub max_event_bytes: usize,
}

/// The place of the pump after a flush, in `progress.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Progress {
    /// The phase of the child.
    pub phase: Phase,
    /// The offset in `output.log` where the stream scrubber starts again.
    pub out_offset: u64,
    /// The stored length of the output at that offset.
    pub out_stored: u64,
    /// The offset in `events.jsonl` of the next line.
    pub ev_offset: u64,
    /// The number of the last event of the lines before that offset.
    pub ev_seq: u64,
    /// The offset in `events.jsonl` where the events of this phase start.
    pub phase_events_start: u64,
}

impl Progress {
    /// A pump that starts at the start of the files.
    pub(crate) fn zero(phase: Phase) -> Progress {
        Progress { phase, out_offset: 0, out_stored: 0, ev_offset: 0, ev_seq: 0, phase_events_start: 0 }
    }

    fn to_json(self) -> Json {
        Json::object([
            ("ev_offset", Json::Int(self.ev_offset)),
            ("ev_seq", Json::Int(self.ev_seq)),
            ("format", Json::Int(1)),
            ("out_offset", Json::Int(self.out_offset)),
            ("out_stored", Json::Int(self.out_stored)),
            ("phase", Json::str(self.phase.name())),
            ("phase_events_start", Json::Int(self.phase_events_start)),
        ])
    }

    /// Read the file. A file that is absent or not valid gives `None`. The pump then starts at
    /// the start of the files and skips the bytes that the store already holds.
    pub(crate) fn read(path: &Path) -> Option<Progress> {
        let bytes = std::fs::read(path).ok()?;
        let value = json::parse(&bytes, ReadLimits::bytes(4096)).ok()?;
        let mut m = Members::of(&value).ok()?;
        let int = |m: &mut Members, n: &str| match m.take(n)? {
            Json::Int(i) => Some(i),
            _ => None,
        };
        if m.take("format") != Some(Json::Int(1)) {
            return None;
        }
        let phase = match m.take("phase")? {
            Json::Str(p) if p == "render" => Phase::Render,
            Json::Str(p) if p == "main" => Phase::Main,
            _ => return None,
        };
        let progress = Progress {
            phase,
            out_offset: int(&mut m, "out_offset")?,
            out_stored: int(&mut m, "out_stored")?,
            ev_offset: int(&mut m, "ev_offset")?,
            ev_seq: int(&mut m, "ev_seq")?,
            phase_events_start: int(&mut m, "phase_events_start")?,
        };
        m.finish().is_empty().then_some(progress)
    }
}

/// A store error that the pump gives back, with the part that failed.
pub(crate) type PumpResult<T> = Result<T, StoreError>;

/// How an adopted run starts the pump again (4.29).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Resume {
    /// The place of the last flush, or `None`.
    pub progress: Option<Progress>,
    /// The phase of the child.
    pub phase: Phase,
    /// The marks `output_truncated` and `events_truncated` of the record.
    pub marks: (bool, bool),
}

pub(crate) struct Pump<S: Store> {
    store: Arc<S>,
    id: RunId,
    limits: PumpLimits,
    scrub: Scrubber,
    output_path: PathBuf,
    events_path: PathBuf,
    progress_path: PathBuf,
    // The output.
    stream: StreamScrubber,
    out_base: u64,
    out_read: u64,
    out_stored: u64,
    out_pending: Vec<u8>,
    out_skip: u64,
    out_truncated: bool,
    // The events.
    ev_buf: Vec<u8>,
    ev_next: u64,
    ev_read: u64,
    ev_skipping: Option<usize>,
    ev_seq: u64,
    ev_stored_through: u64,
    ev_batch: Vec<(u64, Vec<u8>)>,
    ev_truncated: bool,
    // The state for the record.
    phase: Phase,
    phase_events_start: u64,
    stats: Option<BTreeMap<HostId, HostStats>>,
    progress: Progress,
}

impl<S: Store> Pump<S> {
    /// A pump for a new run: the files are empty or absent.
    pub(crate) fn fresh(
        store: Arc<S>,
        id: RunId,
        run_folder: &Path,
        scrub: Scrubber,
        limits: PumpLimits,
        phase: Phase,
    ) -> Pump<S> {
        Pump::build(store, id, run_folder, scrub, limits, Progress::zero(phase), 0, 0, false, false)
    }

    /// A pump for an adopted run (4.29). `progress` is the place of the last flush, or `None`.
    /// The store tells the length of its output and the number of its events.
    pub(crate) async fn resume(
        store: Arc<S>,
        id: RunId,
        run_folder: &Path,
        scrub: Scrubber,
        limits: PumpLimits,
        again: Resume,
    ) -> PumpResult<Pump<S>> {
        let Resume { progress, phase, marks } = again;
        let progress = progress.unwrap_or(Progress::zero(phase));
        let stored_len = store.read_output(&id, 0, 0).await?.length;
        // The events that the store holds above the place of the progress.
        let mut through = progress.ev_seq;
        loop {
            let page = store.read_events(&id, through, 1000).await?;
            match page.last() {
                Some(last) => through = last.seq,
                None => break,
            }
        }
        let mut pump =
            Pump::build(store, id, run_folder, scrub, limits, progress, stored_len, through, marks.0, marks.1);
        pump.phase = phase;
        Ok(pump)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        store: Arc<S>,
        id: RunId,
        run_folder: &Path,
        scrub: Scrubber,
        limits: PumpLimits,
        progress: Progress,
        stored_len: u64,
        stored_events: u64,
        out_truncated: bool,
        ev_truncated: bool,
    ) -> Pump<S> {
        Pump {
            store,
            id,
            limits,
            stream: StreamScrubber::new(scrub.clone()),
            scrub,
            output_path: run_folder.join("output.log"),
            events_path: run_folder.join("events.jsonl"),
            progress_path: run_folder.join("progress.json"),
            out_base: progress.out_offset,
            out_read: progress.out_offset,
            out_stored: stored_len.max(progress.out_stored),
            out_pending: Vec::new(),
            out_skip: stored_len.saturating_sub(progress.out_stored),
            out_truncated,
            ev_buf: Vec::new(),
            ev_next: progress.ev_offset,
            ev_read: progress.ev_offset,
            ev_skipping: None,
            ev_seq: progress.ev_seq,
            ev_stored_through: stored_events,
            ev_batch: Vec::new(),
            ev_truncated: ev_truncated || stored_events >= limits.max_events,
            phase: progress.phase,
            phase_events_start: progress.phase_events_start,
            stats: None,
            progress,
        }
    }

    /// True after the bound of the stored output (4.22).
    pub(crate) fn output_truncated(&self) -> bool {
        self.out_truncated
    }

    /// True after the bound of the stored events (4.22).
    pub(crate) fn events_truncated(&self) -> bool {
        self.ev_truncated
    }

    /// The counts of the last event `stats` of the main phase.
    pub(crate) fn stats(&self) -> Option<&BTreeMap<HostId, HostStats>> {
        self.stats.as_ref()
    }

    /// The offset in `events.jsonl` where the current phase starts.
    pub(crate) fn phase_events_start(&self) -> u64 {
        self.phase_events_start
    }

    /// Start a phase. Call it after `drain` of the phase before, so that each event has the
    /// phase of its child (4.20).
    pub(crate) fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_events_start = self.ev_next;
        self.stats = None;
        self.progress.phase = phase;
        self.progress.phase_events_start = self.ev_next;
        self.save_progress();
    }

    fn read_at(path: &Path, offset: u64, max: usize) -> Vec<u8> {
        let Ok(file) = File::open(path) else { return Vec::new() };
        let mut buf = vec![0u8; max];
        match file.read_at(&mut buf, offset) {
            Ok(n) => {
                buf.truncate(n);
                buf
            }
            Err(_) => Vec::new(),
        }
    }

    fn file_len(path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }

    /// One step of the output: read at most one chunk of the file, then add one chunk to the
    /// store. The answer is true when the step moved something.
    async fn output_step(&mut self, finish: bool) -> PumpResult<bool> {
        let mut moved = false;
        if !self.out_truncated && self.out_pending.len() < CHUNK_BYTES {
            if self.out_stored + self.out_pending.len() as u64 >= self.limits.max_log_bytes {
                // The bound of 4.22: the run goes on, the full output stays in the file.
                if Self::file_len(&self.output_path) > self.out_read {
                    self.out_truncated = true;
                }
            } else {
                let raw = Self::read_at(&self.output_path, self.out_read, CHUNK_BYTES);
                if !raw.is_empty() {
                    moved = true;
                    self.out_read += raw.len() as u64;
                    let mut scrubbed = self.stream.push(&raw);
                    self.take_skip(&mut scrubbed);
                    self.out_pending.extend_from_slice(&scrubbed);
                } else if finish {
                    let mut rest = self.stream.finish();
                    self.take_skip(&mut rest);
                    moved |= !rest.is_empty();
                    self.out_pending.extend_from_slice(&rest);
                }
                let room = self.limits.max_log_bytes.saturating_sub(self.out_stored);
                if self.out_pending.len() as u64 > room {
                    self.out_pending.truncate(usize::try_from(room).unwrap_or(usize::MAX));
                    self.out_truncated = true;
                }
            }
        }
        if !self.out_pending.is_empty() {
            let n = self.out_pending.len().min(CHUNK_BYTES);
            let chunk = self.out_pending[..n].to_vec();
            match self.store.append_output(&self.id, self.out_stored, &chunk).await {
                Ok(length) => self.out_stored = length,
                Err(StoreError::Conflict { .. }) => {
                    // A call that failed after the store kept the bytes: the length tells.
                    let length = self.store.read_output(&self.id, 0, 0).await?.length;
                    if length != self.out_stored + n as u64 {
                        return Err(StoreError::Conflict { current: None });
                    }
                    self.out_stored = length;
                }
                Err(e) => return Err(e),
            }
            self.out_pending.drain(..n);
            moved = true;
        }
        if self.out_pending.is_empty() && self.out_skip == 0 {
            self.progress.out_offset = self.out_base + self.stream.decided();
            self.progress.out_stored = self.out_stored;
        }
        Ok(moved)
    }

    /// Drop the scrubbed bytes that the store already holds (4.29).
    fn take_skip(&mut self, scrubbed: &mut Vec<u8>) {
        if self.out_skip > 0 {
            let n = usize::try_from(self.out_skip).unwrap_or(usize::MAX).min(scrubbed.len());
            scrubbed.drain(..n);
            self.out_skip -= n as u64;
        }
    }

    /// The number of the next event. `None` means that the event is not stored. The cause is the
    /// bound of 4.22, or an event that the store already holds (4.29).
    fn assign_seq(&mut self) -> Option<u64> {
        if self.ev_seq >= self.limits.max_events {
            self.ev_truncated = true;
            return None;
        }
        self.ev_seq += 1;
        (self.ev_seq > self.ev_stored_through).then_some(self.ev_seq)
    }

    /// Make the stored bytes of one line of the plugin (4.20, 4.21). `None` when the line is
    /// not stored.
    fn stored_event(&mut self, line: &[u8]) -> Option<(u64, Vec<u8>)> {
        let parsed =
            if line.len() > self.limits.max_event_bytes { None } else { read_line(line, self.limits.max_event_bytes) };
        if self.phase == Phase::Main
            && let Some(stats) = parsed.as_ref().and_then(CallbackEvent::host_stats)
        {
            self.stats = Some(stats);
        }
        let seq = self.assign_seq()?;
        let now = UtcMillis::now();
        let invalid = || stored_line(None, line.len(), seq, self.phase, now);
        let bytes = match &parsed {
            Some(event) => {
                let bytes = stored_line(Some(&scrub_event(&self.scrub, event)), line.len(), seq, self.phase, now);
                // The scrubber also replaces the JSON string form of a value (4.21). A line that
                // is no longer valid JSON after it is stored as `invalid`: no secret stays.
                let scrubbed = self.scrub.scrub(&bytes);
                if json::parse(&scrubbed, ReadLimits::bytes(scrubbed.len())).is_ok() { scrubbed } else { invalid() }
            }
            None => invalid(),
        };
        Some((seq, bytes))
    }

    /// The event `invalid` of a line above the bound, which the pump did not keep (4.20).
    fn stored_event_of_length(&mut self, length: usize) -> Option<(u64, Vec<u8>)> {
        let seq = self.assign_seq()?;
        Some((seq, stored_line(None, length, seq, self.phase, UtcMillis::now())))
    }

    /// Read lines from the events file into the batch, at most one chunk of bytes.
    fn events_read(&mut self, finish: bool) -> bool {
        let mut moved = false;
        if self.ev_buf.len() < READ_AHEAD {
            let more = Self::read_at(&self.events_path, self.ev_read, READ_AHEAD);
            if !more.is_empty() {
                moved = true;
                self.ev_read += more.len() as u64;
                self.ev_buf.extend_from_slice(&more);
            }
        }
        let mut batch_bytes: usize = self.ev_batch.iter().map(|(_, b)| b.len()).sum();
        loop {
            if batch_bytes >= CHUNK_BYTES {
                break;
            }
            let Some(pos) = self.ev_buf.iter().position(|b| *b == b'\n') else {
                if let Some(skipped) = self.ev_skipping {
                    self.ev_skipping = Some(skipped + self.ev_buf.len());
                    self.ev_buf.clear();
                } else if self.ev_buf.len() > self.limits.max_event_bytes {
                    // A line above the bound: count its bytes and keep none of them.
                    self.ev_skipping = Some(self.ev_buf.len());
                    self.ev_buf.clear();
                } else if finish && !self.ev_buf.is_empty() {
                    // The child ended in the middle of a line.
                    let line = std::mem::take(&mut self.ev_buf);
                    self.ev_next = self.ev_read;
                    moved = true;
                    if let Some(item) = self.stored_event(&line) {
                        self.ev_batch.push(item);
                    }
                }
                break;
            };
            let line: Vec<u8> = self.ev_buf.drain(..=pos).collect();
            self.ev_next = self.ev_read - self.ev_buf.len() as u64;
            moved = true;
            let content = &line[..line.len() - 1];
            let item = match self.ev_skipping.take() {
                Some(skipped) => {
                    // The end of a line above the bound (4.20).
                    let total = skipped + content.len();
                    self.stored_event_of_length(total)
                }
                None => self.stored_event(content),
            };
            if let Some(item) = item {
                batch_bytes += item.1.len();
                self.ev_batch.push(item);
            }
        }
        moved
    }

    async fn events_flush(&mut self) -> PumpResult<bool> {
        if self.ev_batch.is_empty() {
            return Ok(false);
        }
        let first = self.ev_batch[0].0;
        let lines: Vec<Vec<u8>> = self.ev_batch.iter().map(|(_, b)| b.clone()).collect();
        match self.store.append_events(&self.id, first, &lines).await {
            Ok(()) => self.ev_batch.clear(),
            Err(StoreError::Conflict { .. }) => {
                // The store may hold the first lines already: drop them from the batch.
                let held =
                    self.store.read_events(&self.id, first - 1, u32::try_from(lines.len()).unwrap_or(u32::MAX)).await?;
                if held.is_empty() {
                    return Err(StoreError::Conflict { current: None });
                }
                self.ev_batch.drain(..held.len().min(self.ev_batch.len()));
            }
            Err(e) => return Err(e),
        }
        if self.ev_batch.is_empty() {
            self.progress.ev_offset = self.ev_next;
            self.progress.ev_seq = self.ev_seq;
        }
        Ok(true)
    }

    fn save_progress(&self) {
        let path = self.progress_path.with_extension("json.tmp");
        let bytes = json::to_canonical(&self.progress.to_json());
        if disk::write_file(&path, &bytes, 0o600).is_ok() {
            let _ = std::fs::rename(&path, &self.progress_path);
        }
    }

    /// One tick: at most one chunk of output and one chunk of events (4.22).
    pub(crate) async fn tick(&mut self) -> PumpResult<()> {
        let before = self.progress;
        let out = self.output_step(false).await;
        if self.ev_batch.is_empty() {
            self.events_read(false);
        }
        let ev = self.events_flush().await;
        if self.progress != before {
            self.save_progress();
        }
        out?;
        ev?;
        Ok(())
    }

    /// Add all that remains: the end of a phase (4.22). The stream scrubber ends too.
    pub(crate) async fn drain(&mut self) -> PumpResult<()> {
        loop {
            let moved_out = self.output_step(true).await?;
            let mut moved_ev = false;
            if self.ev_batch.is_empty() {
                moved_ev |= self.events_read(true);
            }
            moved_ev |= self.events_flush().await?;
            if !moved_out && !moved_ev {
                break;
            }
        }
        self.save_progress();
        Ok(())
    }
}

/// Replace each secret value in the texts of an event (4.21).
fn scrub_event(scrub: &Scrubber, event: &CallbackEvent) -> CallbackEvent {
    let text = |t: &str| String::from_utf8_lossy(&scrub.scrub(t.as_bytes())).into_owned();
    match event {
        CallbackEvent::PlaybookStart { playbook, check } => {
            CallbackEvent::PlaybookStart { playbook: text(playbook), check: *check }
        }
        CallbackEvent::PlayStart { play } => CallbackEvent::PlayStart { play: text(play) },
        CallbackEvent::TaskStart { task } => CallbackEvent::TaskStart { task: text(task) },
        CallbackEvent::HostResult { host, task, status, diff, message, omitted } => CallbackEvent::HostResult {
            host: text(host),
            task: text(task),
            status: *status,
            diff: diff.as_deref().map(text),
            message: message.as_deref().map(text),
            omitted: *omitted,
        },
        CallbackEvent::Stats { hosts } => CallbackEvent::Stats { hosts: hosts.clone() },
    }
}

/// Read the last event `stats` of the file from an offset (4.29). A line that is not valid
/// is ignored.
pub(crate) fn scan_stats(path: &Path, from: u64, max_event_bytes: usize) -> Option<BTreeMap<HostId, HostStats>> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut reader = BufReader::new(file);
    let mut found = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let content = line.strip_suffix(b"\n").unwrap_or(&line);
        // A cheap test before the parse: the kind is in each line.
        if content.len() <= max_event_bytes
            && content.windows(7).any(|w| w == b"\"stats\"")
            && let Some(stats) = read_line(content, max_event_bytes).as_ref().and_then(CallbackEvent::host_stats)
        {
            found = Some(stats);
        }
    }
    found
}
