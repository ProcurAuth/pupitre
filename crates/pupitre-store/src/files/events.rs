// SPDX-License-Identifier: MIT OR Apache-2.0
//! The events that the store holds for the log (6.27). The queue has a bound: at the bound,
//! the oldest event goes first. A scan notes each ignored entry one time, and notes it again
//! only after the entry has left and come back.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use crate::types::StoreEvent;

/// The most events that the store holds before the server takes them.
pub(crate) const MAX_EVENTS: usize = 10_000;

#[derive(Debug, Default)]
struct State {
    queue: VecDeque<StoreEvent>,
    previous: HashSet<String>,
    current: HashSet<String>,
}

/// The queue of events and the table of ignored entries.
#[derive(Debug, Default)]
pub(crate) struct Events {
    state: Mutex<State>,
}

impl Events {
    /// Add an event.
    pub(crate) fn push(&self, event: StoreEvent) {
        if let Ok(mut s) = self.state.lock() {
            if s.queue.len() >= MAX_EVENTS {
                s.queue.pop_front();
            }
            s.queue.push_back(event);
        }
    }

    /// Take each event.
    pub(crate) fn drain(&self) -> Vec<StoreEvent> {
        self.state.lock().map(|mut s| s.queue.drain(..).collect()).unwrap_or_default()
    }

    /// Start a scan.
    pub(crate) fn scan_begin(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.current.clear();
        }
    }

    /// Note an entry that the scan ignores. The event goes out when the last scan did not
    /// see the entry.
    pub(crate) fn scan_note(&self, path: &str, reason: &str) {
        let Ok(mut s) = self.state.lock() else { return };
        if s.current.len() < MAX_EVENTS {
            s.current.insert(path.to_string());
        }
        if !s.previous.contains(path) {
            if s.queue.len() >= MAX_EVENTS {
                s.queue.pop_front();
            }
            s.queue.push_back(StoreEvent::EntryIgnored { path: path.to_string(), reason: reason.to_string() });
        }
    }

    /// End a scan that went to the end.
    pub(crate) fn scan_end(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.previous = std::mem::take(&mut s.current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignored(e: &[StoreEvent]) -> Vec<&str> {
        e.iter()
            .filter_map(|e| match e {
                StoreEvent::EntryIgnored { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_ignored_entry_is_noted_one_time() {
        let ev = Events::default();
        for _ in 0..3 {
            ev.scan_begin();
            ev.scan_note("a", "r");
            ev.scan_end();
        }
        assert_eq!(ignored(&ev.drain()), ["a"]);
        ev.scan_begin();
        ev.scan_end();
        ev.scan_begin();
        ev.scan_note("a", "r");
        ev.scan_end();
        assert_eq!(ignored(&ev.drain()), ["a"]);
    }

    #[test]
    fn the_queue_has_a_bound() {
        let ev = Events::default();
        for i in 0..MAX_EVENTS + 5 {
            ev.push(StoreEvent::EntryIgnored { path: i.to_string(), reason: String::new() });
        }
        let all = ev.drain();
        assert_eq!(all.len(), MAX_EVENTS);
        assert_eq!(ignored(&all)[0], "5");
    }
}
