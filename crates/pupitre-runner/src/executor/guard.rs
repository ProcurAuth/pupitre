// SPDX-License-Identifier: MIT OR Apache-2.0
//! The guard of an apply at the start of the run (4.24).

use std::time::Duration;

use pupitre_core::manifest::SnapshotHash;
use pupitre_core::run::{RunMode, RunReason, RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;

/// Check an apply against its check run. The answer is the reason of a failure at the start
/// (4.24): `check_too_old` or `snapshot_changed`.
///
/// The rules of the request (existence, kind, mode, status and `applied_by`) are the rules of
/// the server. They are checked here again, because the time in the queue counts and a check
/// run can change between the request and the start. A check run that does not pass them
/// gives `snapshot_changed`: the runner cannot show that the snapshot is the one that the
/// person saw.
pub fn check_apply_guard(
    apply: &RunRecord,
    check: &RunRecord,
    hash: &SnapshotHash,
    now: UtcMillis,
    max_age: Duration,
) -> Result<(), RunReason> {
    let max_age_ms = u64::try_from(max_age.as_millis()).unwrap_or(u64::MAX);
    let is_check = check.kind == apply.kind && check.mode == RunMode::Check && check.status == RunStatus::Passed;
    if !is_check || check.applied_by.as_ref() != Some(&apply.id) {
        return Err(RunReason::SnapshotChanged);
    }
    // The age counts from the end of the check, and the time in the queue counts (4.24).
    match check.finished_at {
        Some(finished) if now.since(finished) <= max_age_ms => {}
        _ => return Err(RunReason::CheckTooOld),
    }
    match &check.snapshot {
        Some(expected) if expected == hash => Ok(()),
        _ => Err(RunReason::SnapshotChanged),
    }
}
