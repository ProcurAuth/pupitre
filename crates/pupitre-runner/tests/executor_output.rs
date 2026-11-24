// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the output and the events of a run in the store (4.20 to 4.22).

mod support;

use std::time::Duration;

use pupitre_core::json::{self, Json, ReadLimits};
use pupitre_core::run::{RunKind, RunMode, RunStatus};
use support::fake_process::Script;
use support::fixture;
use support::harness::{H, quick, stats_line};
use support::wait_for;

fn status_run(n: u8) -> pupitre_core::run::RunRecord {
    fixture::run_record(
        fixture::run_id(n),
        RunKind::Status,
        RunMode::Apply,
        "environment:production/unit:relay",
        &["host-a"],
        "status-relay",
    )
}

fn parse(bytes: &[u8]) -> std::collections::BTreeMap<String, Json> {
    json::parse(bytes, ReadLimits::bytes(1 << 20)).unwrap().as_object().unwrap().clone()
}

/// 2.28, 4.22: the output goes to the store in chunks of 65,536 bytes at most, in order.
#[tokio::test(flavor = "multi_thread")]
async fn the_output_goes_in_chunks_of_65536_bytes_at_most() {
    let mut h = H::new();
    let output: Vec<u8> = (0..150_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    h.process.push_script(Script { output: output.clone(), ..quick() });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    h.wait_status(&run.id, RunStatus::Passed).await;
    assert_eq!(h.store.output(&run.id), output);
    let sizes = h.store.append_sizes();
    assert!(sizes.len() >= 3 && sizes.iter().all(|s| *s <= 65_536 && *s > 0), "{sizes:?}");
    assert_eq!(sizes.iter().sum::<usize>(), output.len());
    h.stop().await;
}

/// 4.22: the store gets at most one chunk of output at each period of the pump while the child runs.
#[tokio::test(flavor = "multi_thread")]
async fn the_pump_adds_at_most_one_chunk_at_each_tick() {
    let mut h = H::new();
    h.timing.pump = Duration::from_millis(300);
    // A child that writes 400,000 bytes and runs for 1.5 seconds.
    h.process.push_script(Script {
        output: vec![b'x'; 400_000],
        exit: Some((Duration::from_millis(1500), pupitre_runner::process::Exit::Code(0))),
        ..Script::default()
    });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    wait_for("the child starts", 5, || h.process.spawns().len() == 1).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    // About 4 ticks have passed: at most one chunk for each tick (the first tick is at once).
    let during = h.store.append_sizes().len();
    assert!((1..=5).contains(&during), "chunks during the run: {during}");
    h.wait_status(&run.id, RunStatus::Passed).await;
    assert_eq!(h.store.output(&run.id).len(), 400_000, "the end of the run adds all that remains");
    h.stop().await;
}

/// 4.21: a value across two chunks of the pump is replaced.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_across_two_chunks_is_replaced() {
    let mut h = H::new();
    let mut output = vec![b'a'; 65_536 - 5];
    output.extend_from_slice(b"relay-pass-1234");
    output.extend_from_slice(&[b'b'; 70_000]);
    h.process.push_script(Script { output, ..quick() });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    h.wait_status(&run.id, RunStatus::Passed).await;
    let stored = h.store.output(&run.id);
    assert!(!stored.windows(15).any(|w| w == b"relay-pass-1234"));
    assert_eq!(stored.windows(8).filter(|w| *w == b"[secret]").count(), 1);
    assert_eq!(stored.len(), 65_536 - 5 + 8 + 70_000);
    h.stop().await;
}

/// 4.21: the JSON string form of a value is replaced in the event lines.
#[tokio::test(flavor = "multi_thread")]
async fn the_json_form_of_a_secret_is_replaced_in_the_events() {
    let mut h = H::new();
    h.store.put_secret_value(
        fixture::secrets()[0].0.clone(),
        "storage.password",
        b"pa\"ss\nword-1234",
        "5555555555555555",
    );
    let line = r#"{"diff":"x pa\"ss\nword-1234 y","host":"host-a","kind":"host_result","message":"token-value-5678","status":"changed","task":"t pa\"ss\nword-1234","v":1}"#;
    h.process.push_script(Script {
        events: vec![line.to_string()],
        output: b"pa\"ss\nword-1234\n".to_vec(),
        ..quick()
    });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    h.wait_status(&run.id, RunStatus::Passed).await;
    let events = h.store.events(&run.id);
    assert_eq!(events.len(), 1);
    let text = String::from_utf8(events[0].bytes.clone()).unwrap();
    for form in ["pa\\\"ss\\nword-1234", "pa\"ss", "token-value-5678"] {
        assert!(!text.contains(form), "{form} in {text}");
    }
    let event = parse(&events[0].bytes);
    assert_eq!(event["diff"].as_str(), Some("x [secret] y"));
    assert_eq!(event["task"].as_str(), Some("t [secret]"));
    assert_eq!(event["message"].as_str(), Some("[secret]"));
    assert!(!h.store.output(&run.id).windows(8).any(|w| w == b"word-123"));
    h.stop().await;
}

/// 4.20: a line that is not valid, or longer than `runner.max_event_bytes`, becomes `invalid`.
#[tokio::test(flavor = "multi_thread")]
async fn a_line_that_is_not_valid_becomes_an_invalid_event() {
    let mut h = H::new();
    h.settings().max_event_bytes = 1024;
    let long = format!(r#"{{"kind":"task_start","task":"{}","v":1}}"#, "x".repeat(3000));
    let valid = r#"{"kind":"task_start","task":"ok","v":1}"#;
    h.process.push_script(Script {
        events: vec![
            valid.into(),
            "not json at all".into(),
            long.clone(),
            r#"{"kind":"other","v":1}"#.into(),
            valid.into(),
        ],
        ..quick()
    });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    h.wait_status(&run.id, RunStatus::Passed).await;
    let events = h.store.events(&run.id);
    let kinds: Vec<String> = events.iter().map(|e| parse(&e.bytes)["kind"].as_str().unwrap().to_string()).collect();
    assert_eq!(kinds, ["task_start", "invalid", "invalid", "invalid", "task_start"]);
    assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    assert_eq!(parse(&events[1].bytes)["length"], Json::Int("not json at all".len() as u64));
    assert_eq!(parse(&events[2].bytes)["length"], Json::Int(long.len() as u64), "the length of the long line");
    assert!(events[2].bytes.len() < 200, "the long line is not stored");
    h.stop().await;
}

/// 4.22: at `runner.max_log_bytes` the runner stores no more output, marks the record, and the run goes on.
#[tokio::test(flavor = "multi_thread")]
async fn the_output_bound_marks_the_record_and_the_run_goes_on() {
    let mut h = H::new();
    h.settings().max_log_bytes = 100;
    h.process.push_script(Script { output: vec![b'z'; 300], ..quick() });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Passed).await;
    assert!(done.output_truncated);
    assert!(!done.events_truncated);
    assert_eq!(h.store.output(&run.id).len(), 100);
    // The full output stays on the runner node (4.22).
    assert_eq!(std::fs::read(h.folders(&run.id).run.join("output.log")).unwrap().len(), 300);
    h.stop().await;
}

/// 4.22: at `runner.max_events` the runner stores no more events. The `stats` still reach the record.
#[tokio::test(flavor = "multi_thread")]
async fn the_event_bound_marks_the_record_and_the_stats_still_count() {
    let mut h = H::new();
    h.settings().max_events = 2;
    let task = r#"{"kind":"task_start","task":"t","v":1}"#.to_string();
    h.process.push_script(Script {
        events: vec![task.clone(), task.clone(), task.clone(), stats_line(&[("host-a", 0, 0)])],
        ..quick()
    });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    let done = h.wait_status(&run.id, RunStatus::Passed).await;
    assert!(done.events_truncated && !done.output_truncated);
    assert_eq!(h.store.events(&run.id).len(), 2);
    assert!(done.stats.is_some(), "the record has the counts of the last `stats`");
    h.stop().await;
}

/// 4.6: an error of the store while the child runs is logged. The pump adds the bytes later, with no gap and no copy.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_error_does_not_lose_or_copy_output() {
    let mut h = H::new();
    let output: Vec<u8> = (0..200_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    let events: Vec<String> = (0..50).map(|i| format!(r#"{{"kind":"task_start","task":"t{i}","v":1}}"#)).collect();
    h.process.push_script(Script {
        output: output.clone(),
        events,
        exit: Some((Duration::from_millis(1200), pupitre_runner::process::Exit::Code(0))),
        ..Script::default()
    });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    wait_for("the child starts", 5, || h.process.spawns().len() == 1).await;
    h.store.fail("append_output", true);
    h.store.fail("append_events", true);
    tokio::time::sleep(Duration::from_millis(400)).await;
    h.store.fail("append_output", false);
    h.store.fail("append_events", false);
    h.wait_status(&run.id, RunStatus::Passed).await;
    assert_eq!(h.store.output(&run.id), output);
    let seqs: Vec<u64> = h.store.events(&run.id).iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=50).collect::<Vec<_>>());
    h.stop().await;
}

/// 4.20: a line at the end of the file with no line end is read at the end of the run.
#[tokio::test(flavor = "multi_thread")]
async fn a_last_line_with_no_line_end_is_read_at_the_end() {
    let mut h = H::new();
    h.process.push_script(Script {
        events: vec![r#"{"kind":"task_start","task":"a","v":1}"#.into()],
        events_tail: r#"{"kind":"task_start","task":"b","v":1}"#.into(),
        ..quick()
    });
    h.start().await;
    let run = status_run(1);
    h.queue(run.clone());
    h.wait_status(&run.id, RunStatus::Passed).await;
    let events = h.store.events(&run.id);
    assert_eq!(events.len(), 2);
    assert_eq!(parse(&events[1].bytes)["task"].as_str(), Some("b"));
    h.stop().await;
}
