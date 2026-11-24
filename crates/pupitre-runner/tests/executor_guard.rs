// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the guard of an apply (4.24) and of the hash of a snapshot between a check and its apply.

mod support;

use std::time::Duration;

use pupitre_core::doc::DocKey;
use pupitre_core::ids::{RunId, UnitId};
use pupitre_core::manifest::SnapshotHash;
use pupitre_core::run::{RunKind, RunMode, RunReason, RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;
use pupitre_runner::executor::guard::check_apply_guard;
use pupitre_store::Store;
use pupitre_store::types::LeaseName;
use support::fixture;
use support::harness::{H, quick};

fn apply_of(check: &RunRecord, n: u8) -> RunRecord {
    let mut apply = fixture::run_record(
        fixture::run_id(n),
        RunKind::Deploy,
        RunMode::Apply,
        &check.node.path(),
        &["host-a", "host-b"],
        "deploy-relay",
    );
    apply.check_run = Some(check.id);
    apply
}

/// Queue an apply the way the server does: `create_run` sets `applied_by` of the check.
async fn queue_apply(h: &H, check: &RunRecord, n: u8) -> RunRecord {
    let apply = apply_of(check, n);
    h.store.create_run(&apply, 100, Some(&check.id)).await.unwrap();
    h.wake.wake();
    apply
}

async fn passed_check(h: &mut H) -> RunRecord {
    h.process.push_script(quick());
    h.process.push_script(quick());
    h.start().await;
    let check = h.check_run(1);
    h.queue(check.clone());
    h.wait_status(&check.id, RunStatus::Passed).await
}

/// 4.16, 4.24: a check and an apply of the same inputs have the same hash, and the apply runs.
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_with_the_same_snapshot_runs_the_main_phase_only() {
    let mut h = H::new();
    let check = passed_check(&mut h).await;
    h.process.push_script(quick());
    let apply = queue_apply(&h, &check, 2).await;
    let done = h.wait_status(&apply.id, RunStatus::Passed).await;
    assert_eq!(done.snapshot, check.snapshot, "the same hash");
    assert!(done.snapshot.is_some());
    let spawns = h.process.spawns();
    assert_eq!(spawns.len(), 3, "render and main for the check, main for the apply");
    let last = &spawns[2];
    assert!(last.args.last().unwrap().ends_with("deploy-relay.yml"));
    assert!(!last.args.iter().any(|a| a == "--check" || a == "--diff"), "the apply has no `--check`");
    h.stop().await;
}

/// 4.24: the apply refuses after a change of one value.
#[tokio::test(flavor = "multi_thread")]
async fn the_apply_refuses_after_a_changed_value() {
    let mut h = H::new();
    let check = passed_check(&mut h).await;
    h.store.add_doc(fixture::stored(
        DocKey::UnitValues(fixture::env_id(), UnitId::parse("relay").unwrap()),
        r#"{"format": 1, "values": {"limits.max_clients": 301}}"#,
    ));
    let apply = queue_apply(&h, &check, 2).await;
    let done = h.wait_status(&apply.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::SnapshotChanged));
    assert_eq!(h.process.spawns().len(), 2, "no child for the apply");
    assert!(!h.secrets_dir_of(&apply.id).exists());
    let out = String::from_utf8(h.store.output(&apply.id)).unwrap();
    assert!(out.contains("snapshot"), "{out}");
    h.stop().await;
}

/// 4.24: the apply refuses after a changed secret, a changed playbook byte or a changed known hosts file.
#[tokio::test(flavor = "multi_thread")]
async fn the_apply_refuses_after_a_changed_secret_playbook_or_host_key() {
    for change in ["secret", "playbook", "known_hosts"] {
        let mut h = H::new();
        let check = passed_check(&mut h).await;
        match change {
            "secret" => h.store.put_secret_value(
                fixture::secrets()[0].0.clone(),
                "storage.password",
                b"another-value-99",
                "9999999999999999",
            ),
            "playbook" => {
                let p = h.node.caps.playbooks.values().find(|p| p.id.as_str() == "deploy-relay").unwrap().path.clone();
                std::fs::write(p, "# changed\n").unwrap();
            }
            _ => {
                let mut text = std::fs::read_to_string(&h.node.settings.known_hosts_file).unwrap();
                text.push_str("other ssh-ed25519 AAAA\n");
                std::fs::write(&h.node.settings.known_hosts_file, text).unwrap();
            }
        }
        let apply = queue_apply(&h, &check, 2).await;
        let done = h.wait_status(&apply.id, RunStatus::Failed).await;
        assert_eq!(done.reason, Some(RunReason::SnapshotChanged), "{change}");
        h.stop().await;
    }
}

/// 4.24: the time in the queue counts against `runner.check_max_age`.
#[tokio::test(flavor = "multi_thread")]
async fn the_time_in_the_queue_counts_for_the_age_of_the_check() {
    let mut h = H::new();
    h.settings().check_max_age = Duration::from_millis(500);
    let check = passed_check(&mut h).await;
    // The check is fresh at the request. Another holder keeps the environment for a while.
    h.store.force_lease(&LeaseName::Environment(fixture::env_id()), "other-node/run", Duration::from_millis(900));
    let apply = queue_apply(&h, &check, 2).await;
    let done = h.wait_status(&apply.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::CheckTooOld));
    assert_eq!(h.process.spawns().len(), 2, "no child for the apply");
    h.stop().await;
}

/// 4.24 at the start: a check that is not `passed`, or whose `applied_by` is another run.
#[tokio::test(flavor = "multi_thread")]
async fn a_check_that_is_not_the_check_of_this_apply_gives_snapshot_changed() {
    let mut h = H::new();
    let check = passed_check(&mut h).await;
    // The apply names its check, but the check names another apply.
    let mut apply = apply_of(&check, 2);
    h.store.edit_run(&check.id, |r| r.applied_by = Some(RunId::from_parts(1, [9; 10])));
    apply.check_run = Some(check.id);
    h.queue(apply.clone());
    let done = h.wait_status(&apply.id, RunStatus::Failed).await;
    assert_eq!(done.reason, Some(RunReason::SnapshotChanged));
    h.stop().await;
}

// The pure guard.

fn hash(n: u8) -> SnapshotHash {
    SnapshotHash::of_bytes(&[n])
}

fn pair(now: u64, finished: Option<u64>) -> (RunRecord, RunRecord) {
    let mut check = fixture::run_record(
        fixture::run_id(1),
        RunKind::Deploy,
        RunMode::Check,
        "environment:production/unit:relay",
        &["host-a"],
        "deploy-relay",
    );
    check.status = RunStatus::Passed;
    check.snapshot = Some(hash(1));
    check.finished_at = finished.map(UtcMillis);
    check.applied_by = Some(fixture::run_id(2));
    let mut apply = apply_of(&check, 2);
    apply.created_at = UtcMillis(now);
    (apply, check)
}

#[test]
fn the_guard_table_of_4_24() {
    let max = Duration::from_secs(1800);
    let now = UtcMillis(10_000_000);
    let (apply, check) = pair(10_000_000, Some(9_000_000));
    assert_eq!(check_apply_guard(&apply, &check, &hash(1), now, max), Ok(()));
    // The age: the bound itself is valid, one millisecond more is not.
    let (apply, check) = pair(0, Some(10_000_000 - 1_800_000));
    assert_eq!(check_apply_guard(&apply, &check, &hash(1), now, max), Ok(()));
    let (apply, check) = pair(0, Some(10_000_000 - 1_800_001));
    assert_eq!(check_apply_guard(&apply, &check, &hash(1), now, max), Err(RunReason::CheckTooOld));
    // No `finished_at`: the age is not known.
    let (apply, check) = pair(0, None);
    assert_eq!(check_apply_guard(&apply, &check, &hash(1), now, max), Err(RunReason::CheckTooOld));
    // The hash.
    let (apply, check) = pair(0, Some(9_999_000));
    assert_eq!(check_apply_guard(&apply, &check, &hash(2), now, max), Err(RunReason::SnapshotChanged));
    let mut none = check.clone();
    none.snapshot = None;
    assert_eq!(check_apply_guard(&apply, &none, &hash(1), now, max), Err(RunReason::SnapshotChanged));
    // The check: status, mode, kind, `applied_by`.
    for edit in 0..4 {
        let mut c = check.clone();
        match edit {
            0 => c.status = RunStatus::Failed,
            1 => c.mode = RunMode::Apply,
            2 => c.kind = RunKind::Command,
            _ => c.applied_by = None,
        }
        assert_eq!(check_apply_guard(&apply, &c, &hash(1), now, max), Err(RunReason::SnapshotChanged), "{edit}");
    }
}
