// SPDX-License-Identifier: MIT OR Apache-2.0
//! The fault hook of the backend `files`. A test uses it to make a call fail at a known place,
//! for example between the version and the document of a write (2.19, S08). A store that no
//! test touched has no rule, and each check then passes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustix::io::Errno;

/// A place in the code of the backend where a test can inject a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    /// Each write of bytes into a file.
    Write,
    /// Each `fsync` of a file or a folder.
    Fsync,
    /// Each read of the content of a file.
    Read,
    /// After the bytes of a version, before its metadata file (2.19, step 2).
    AfterHistoryBytes,
    /// After the metadata file of a version, before the current document (2.19, step 3).
    AfterHistory,
    /// The `flock` call at open (2.32).
    Flock,
}

#[derive(Debug, Default)]
struct State {
    rules: Mutex<Vec<(FaultPoint, i32)>>,
    fsyncs: AtomicU64,
    reads: AtomicU64,
}

/// The handle of the faults of one store. Clones share the state.
#[derive(Debug, Clone, Default)]
pub struct Faults(Arc<State>);

impl Faults {
    /// Make a handle with no rule.
    pub fn new() -> Faults {
        Faults::default()
    }

    /// Add a rule: the next pass of `point` fails one time with the raw error number `errno`.
    pub fn fail_once(&self, point: FaultPoint, errno: i32) {
        if let Ok(mut rules) = self.0.rules.lock() {
            rules.push((point, errno));
        }
    }

    /// The number of `fsync` calls so far.
    pub fn fsync_calls(&self) -> u64 {
        self.0.fsyncs.load(Ordering::SeqCst)
    }

    /// The number of reads of file content so far.
    pub fn read_calls(&self) -> u64 {
        self.0.reads.load(Ordering::SeqCst)
    }

    pub(crate) fn check(&self, point: FaultPoint) -> Result<(), Errno> {
        match point {
            FaultPoint::Fsync => {
                self.0.fsyncs.fetch_add(1, Ordering::SeqCst);
            }
            FaultPoint::Read => {
                self.0.reads.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        let Ok(mut rules) = self.0.rules.lock() else { return Ok(()) };
        match rules.iter().position(|(p, _)| *p == point) {
            Some(i) => Err(Errno::from_raw_os_error(rules.remove(i).1)),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_fires_one_time() {
        let faults = Faults::new();
        faults.fail_once(FaultPoint::AfterHistory, 5);
        assert!(faults.check(FaultPoint::Write).is_ok());
        assert_eq!(faults.check(FaultPoint::AfterHistory), Err(Errno::from_raw_os_error(5)));
        assert!(faults.check(FaultPoint::AfterHistory).is_ok());
        assert_eq!(faults.fsync_calls(), 0);
        let _ = faults.check(FaultPoint::Fsync);
        assert_eq!(faults.fsync_calls(), 1);
    }
}
