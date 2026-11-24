// SPDX-License-Identifier: MIT OR Apache-2.0
//! The size of the work: 1,000 documents load in less than 2 seconds in the debug profile,
//! and a poll with no change reads no file content (2.22, 2.33).

mod files_support;

use std::time::{Duration, Instant};

use files_support::*;
use pupitre_store::files::Faults;
use pupitre_store::{Precondition, Store};

async fn fill(store: &pupitre_store::files::FilesStore, hosts: usize) {
    store.put(&key("environments/big"), &env_doc("Big"), &Precondition::Create, &meta("a", "")).await.unwrap();
    for i in 0..hosts {
        let k = key(&format!("environments/big/hosts/host-{i:04}"));
        store
            .put(&k, &host_doc(&format!("host-{i:04}.example.invalid")), &Precondition::Create, &meta("a", ""))
            .await
            .unwrap();
    }
}

/// Write the files of a store that has a version 1 for each document, with no call of the
/// store: the folders of 2.31 for one environment and `hosts` hosts.
fn fill_files(fx: &Fixture, hosts: usize) {
    let write = |relative: &str, bytes: &[u8]| {
        let path = fx.docs().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    let mut docs =
        vec![("environments/big".to_string(), "environments/big/environment.json".to_string(), env_doc("Big"))];
    for i in 0..hosts {
        let id = format!("host-{i:04}");
        docs.push((
            format!("environments/big/hosts/{id}"),
            format!("environments/big/hosts/{id}.json"),
            host_doc(&format!("{id}.example.invalid")),
        ));
    }
    for (key_text, file, doc) in docs {
        write(&file, &doc.bytes);
        write(&format!(".history/{key_text}/0000000001.json"), &doc.bytes);
        let meta = format!(
            "{{\n  \"etag\": \"{}\",\n  \"kind\": \"create\",\n  \"message\": \"\",\n  \"saved_at\": \"2026-11-24T10:00:00.000Z\",\n  \"saved_by\": \"a\"\n}}\n",
            doc.etag
        );
        write(&format!(".history/{key_text}/0000000001.meta.json"), meta.as_bytes());
    }
}

#[tokio::test]
async fn one_thousand_documents_load_in_less_than_two_seconds() {
    let fx = Fixture::new();
    pupitre_store::files::FilesStore::migrate(&fx.options()).unwrap();
    fill_files(&fx, 999);
    // The load after a start: the open reads the folders, then the catalog loads.
    let started = Instant::now();
    let store = fx.reopen();
    let open_time = started.elapsed();
    let started = Instant::now();
    let listing = store.load().await.unwrap();
    let load_time = started.elapsed();
    assert_eq!(listing.docs.len(), 1000);
    // The files and the versions agree, so the open wrote no version.
    assert!(store.drain_events().is_empty());
    assert!(listing.docs.iter().all(|d| d.version == 1));
    assert!(load_time < Duration::from_secs(2), "load took {load_time:?}");
    assert!(open_time < Duration::from_secs(10), "open took {open_time:?}");
    // The poll with no change is faster than the load.
    let started = Instant::now();
    let delta = store.poll(&listing.marks).await.unwrap();
    let poll_time = started.elapsed();
    assert!(delta.changed.is_empty() && delta.removed.is_empty());
    assert!(poll_time < Duration::from_secs(2), "poll took {poll_time:?}");
    eprintln!("open {open_time:?}, load {load_time:?}, poll {poll_time:?}");
}

#[tokio::test]
async fn a_poll_with_no_change_reads_no_file_content() {
    let fx = Fixture::new();
    pupitre_store::files::FilesStore::migrate(&fx.options()).unwrap();
    let faults = Faults::new();
    let store = pupitre_store::files::FilesStore::open_with_faults(fx.options(), faults.clone()).unwrap();
    fill(&store, 30).await;
    let listing = store.load().await.unwrap();
    assert_eq!(listing.docs.len(), 31);
    // The poll with no change: no read of any file, not even of a small file.
    let before = faults.read_calls();
    for _ in 0..3 {
        let delta = store.poll(&listing.marks).await.unwrap();
        assert!(delta.changed.is_empty() && delta.removed.is_empty());
    }
    assert_eq!(faults.read_calls(), before, "a poll with no change read a file");
    // The check is real: a poll with the marks of the start reads each document one time.
    let all = store.poll(&pupitre_store::Marks::default()).await.unwrap();
    assert_eq!(all.changed.len(), 31);
    assert_eq!(faults.read_calls(), before + 31);
    // One change outside the store: one read for the change, and one for the bytes of the answer.
    let path = fx.docs().join("environments/big/hosts/host-0003.json");
    std::fs::write(&path, host_doc("changed.example.invalid").bytes).unwrap();
    let before = faults.read_calls();
    let delta = store.poll(&all.marks).await.unwrap();
    assert_eq!(delta.changed.len(), 1);
    assert!(faults.read_calls() > before && faults.read_calls() <= before + 4);
}
