// SPDX-License-Identifier: MIT OR Apache-2.0
//! Limits of the layout of the backend `files` (2.31, 7.11): the length of a secret name, the
//! size of a value file and the size of a file of events. A limit gives an error and never a cut value.

mod files_support;

use files_support::*;
use pupitre_core::ids::KeyPath;
use pupitre_core::run::RunStatus;
use pupitre_store::{Store, StoreError};

/// The longest name that a file name holds is 249 bytes: the file of the events adds `.jsonl`.
fn long_name(len: usize) -> KeyPath {
    // Segments of 62 bytes joined by dots, cut to `len` bytes.
    let mut text = String::new();
    while text.len() < len {
        if !text.is_empty() {
            text.push('.');
        }
        let room = (len - text.len()).min(62);
        text.push('a');
        text.push_str(&"b".repeat(room.saturating_sub(1)));
    }
    text.truncate(len);
    KeyPath::parse(&text).unwrap()
}

/// L10: a secret name that fits the rule of a key path but not a file name gives an error that says why.
/// A name of 249 bytes works.
#[tokio::test]
async fn a_secret_name_that_does_not_fit_a_file_name_is_refused_with_a_clear_error() {
    let fx = Fixture::new();
    let store = fx.open();
    let scope = unit_scope("p", "relay");
    let longest = long_name(249);
    assert_eq!(longest.as_str().len(), 249);
    store.put_secret(&scope, &longest, &value("a-secret-value"), &meta("alice", "")).await.unwrap();
    let read = store.read_secrets(std::slice::from_ref(&scope)).await.unwrap();
    assert_eq!(read[0].2.expose(), b"a-secret-value");
    for len in [250, 255] {
        let name = long_name(len);
        assert_eq!(name.as_str().len(), len);
        match store.put_secret(&scope, &name, &value("a-secret-value"), &meta("alice", "")).await {
            Err(StoreError::Unavailable(text)) => {
                assert!(text.contains("249") && text.contains(&len.to_string()), "{text}");
            }
            other => panic!("expected an error for {len} bytes, got {other:?}"),
        }
    }
    // No file with an error in its name is left behind, and the store still works.
    assert_eq!(store.list_secrets(&scope).await.unwrap().len(), 1);
}

/// L10: a value file above the limit of the layout gives an error. The old code gave a value that the read cut.
#[tokio::test]
async fn a_secret_value_file_above_the_limit_gives_an_error_and_never_a_cut_value() {
    let fx = Fixture::new();
    let store = fx.open();
    let scope = unit_scope("p", "relay");
    store.put_secret(&scope, &name("storage.password"), &value("small"), &meta("alice", "")).await.unwrap();
    let file = fx.secrets().join("values/environments/p/units/relay/storage.password");
    std::fs::write(&file, vec![b'x'; (1 << 20) + 10]).unwrap();
    match store.read_secrets(std::slice::from_ref(&scope)).await {
        Err(StoreError::Corrupt(text)) => assert!(text.contains("above the limit"), "{text}"),
        other => panic!("expected Corrupt, got {other:?}"),
    }
    assert!(matches!(store.list_secrets(&scope).await, Err(StoreError::Corrupt(_))));
    // A value file at the limit reads in full.
    std::fs::write(&file, vec![b'y'; 1 << 20]).unwrap();
    let read = store.read_secrets(std::slice::from_ref(&scope)).await.unwrap();
    assert_eq!(read[0].2.expose().len(), 1 << 20);
}

/// L10: a file of events above the limit is never cut at the open. The read and the open give an error.
#[tokio::test]
async fn a_file_of_events_above_the_limit_is_not_cut() {
    let fx = Fixture::new();
    let id = run_id(1, 1);
    let events = {
        let store = fx.open();
        store.create_run(&run_record(id, "p", RunStatus::Queued), 10, None).await.unwrap();
        store.append_events(&id, 1, &[b"{\"event\":\"start\"}".to_vec()]).await.unwrap();
        let run_folder = std::fs::read_dir(fx.runs())
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().is_dir() && e.file_name() != ".tmp")
            .expect("the folder of the run");
        run_folder.path().join("events.jsonl")
    };
    // A sparse file: the test does not write 1 GiB. The file ends with a partial line.
    let limit: u64 = 1 << 30;
    let file = std::fs::OpenOptions::new().write(true).open(&events).unwrap();
    file.set_len(limit + 7).unwrap();
    drop(file);
    let err = pupitre_store::files::FilesStore::open(fx.options()).unwrap_err();
    assert!(matches!(&err, StoreError::Corrupt(text) if text.contains("above the limit")), "{err:?}");
    assert_eq!(std::fs::metadata(&events).unwrap().len(), limit + 7, "the file is not cut");
}
