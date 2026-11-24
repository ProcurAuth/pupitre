// SPDX-License-Identifier: MIT OR Apache-2.0
//! The cases S01 to S18 and S20 of 2.43 on the backend `files`. The case S19 is in
//! `files_transfer.rs`. The suite of the module `conformance` runs the same cases through
//! `contract_files.rs` when it exists.

mod files_support;

use std::time::Duration;

use files_support::*;
use pupitre_core::doc::DocKey;
use pupitre_core::etag::Etag;
use pupitre_core::run::RunStatus;
use pupitre_store::files::{FaultPoint, Faults, FilesOptions, FilesStore, MigrateOutcome};
use pupitre_store::secret::SecretKey;
use pupitre_store::{
    LeaseName, LeaseOutcome, LeaseRenewal, Marks, Precondition, RunFilter, Store, StoreError, StoreEvent, VersionKind,
};

fn create() -> Precondition {
    Precondition::Create
}

fn matching(etag: Etag) -> Precondition {
    Precondition::Match(etag)
}

#[tokio::test]
async fn s01_a_create_then_a_get_give_the_same_bytes_etag_and_version_1() {
    let fx = Fixture::new();
    let store = fx.open();
    let doc = env_doc("Production");
    let k = key("environments/production");
    let saved = store.put(&k, &doc, &create(), &meta("alice", "first")).await.unwrap();
    assert_eq!((saved.version, saved.etag), (1, Some(doc.etag)));
    let got = store.get(&k).await.unwrap();
    assert_eq!((got.bytes, got.etag, got.version), (doc.bytes, doc.etag, 1));
    assert!(matches!(store.get(&key("environments/other")).await, Err(StoreError::NotFound)));
}

#[tokio::test]
async fn s02_a_save_with_the_current_etag_makes_version_2_and_an_old_etag_conflicts() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let (a, b, c) = (env_doc("A"), env_doc("B"), env_doc("C"));
    store.put(&k, &a, &create(), &meta("alice", "")).await.unwrap();
    let saved = store.put(&k, &b, &matching(a.etag), &meta("bob", "second")).await.unwrap();
    assert_eq!(saved.version, 2);
    // The old etag: Conflict with the current etag, and nothing changes.
    let err = store.put(&k, &c, &matching(a.etag), &meta("bob", "")).await.unwrap_err();
    assert_eq!(err, StoreError::Conflict { current: Some(b.etag) });
    let got = store.get(&k).await.unwrap();
    assert_eq!((got.bytes, got.version), (b.bytes.clone(), 2));
    assert_eq!(store.history(&k, None, 10).await.unwrap().len(), 2);
    // A save of a key with no document is NotFound.
    let err = store.put(&key("environments/none"), &c, &matching(a.etag), &meta("bob", "")).await.unwrap_err();
    assert_eq!(err, StoreError::NotFound);
    // A delete with an old etag also conflicts.
    let err = store.delete(&k, &a.etag, &meta("bob", "")).await.unwrap_err();
    assert_eq!(err, StoreError::Conflict { current: Some(b.etag) });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s03_two_saves_at_the_same_time_with_one_base_etag_exactly_one_succeeds() {
    let fx = Fixture::new();
    let store = std::sync::Arc::new(fx.open());
    let k = key("environments/p");
    let base = env_doc("Base");
    store.put(&k, &base, &create(), &meta("alice", "")).await.unwrap();
    for round in 0..20 {
        let current = store.get(&k).await.unwrap().etag;
        let (s1, s2) = (store.clone(), store.clone());
        let (k1, k2) = (k.clone(), k.clone());
        let a = env_doc(&format!("A{round}"));
        let b = env_doc(&format!("B{round}"));
        let t1 = tokio::spawn(async move { s1.put(&k1, &a, &matching(current), &meta("alice", "")).await });
        let t2 = tokio::spawn(async move { s2.put(&k2, &b, &matching(current), &meta("bob", "")).await });
        let (r1, r2) = (t1.await.unwrap(), t2.await.unwrap());
        let wins = [r1.is_ok(), r2.is_ok()].iter().filter(|w| **w).count();
        assert_eq!(wins, 1, "round {round}: {r1:?} {r2:?}");
        let lost = if r1.is_ok() { r2 } else { r1 };
        assert!(matches!(lost, Err(StoreError::Conflict { current: Some(_) })));
    }
}

#[tokio::test]
async fn s04_create_on_a_current_key_gives_already_exists_and_after_a_delete_the_next_version() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let doc = env_doc("A");
    store.put(&k, &doc, &create(), &meta("alice", "")).await.unwrap();
    assert_eq!(store.put(&k, &doc, &create(), &meta("alice", "")).await.unwrap_err(), StoreError::AlreadyExists);
    let deleted = store.delete(&k, &doc.etag, &meta("alice", "gone")).await.unwrap();
    assert_eq!((deleted.version, deleted.etag), (2, None));
    // A key with only a version `delete` is free.
    let again = store.put(&k, &env_doc("B"), &create(), &meta("alice", "")).await.unwrap();
    assert_eq!(again.version, 3);
    let versions = store.history(&k, None, 10).await.unwrap();
    let kinds: Vec<_> = versions.iter().map(|v| (v.version, v.kind)).collect();
    assert_eq!(kinds, [(3, VersionKind::Create), (2, VersionKind::Delete), (1, VersionKind::Create)]);
}

#[tokio::test]
async fn s05_a_delete_writes_a_version_delete_get_gives_not_found_and_a_poll_reports_the_key() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let doc = env_doc("A");
    store.put(&k, &doc, &create(), &meta("alice", "")).await.unwrap();
    let listing = store.load().await.unwrap();
    assert_eq!(listing.docs.len(), 1);
    store.delete(&k, &doc.etag, &meta("bob", "remove")).await.unwrap();
    assert_eq!(store.get(&k).await.unwrap_err(), StoreError::NotFound);
    let newest = &store.history(&k, None, 1).await.unwrap()[0];
    assert_eq!((newest.version, newest.kind, newest.etag), (2, VersionKind::Delete, None));
    assert_eq!(newest.meta.saved_by.as_str(), "bob");
    let v2 = store.version(&k, 2).await.unwrap();
    assert_eq!(v2.bytes, None);
    let delta = store.poll(&listing.marks).await.unwrap();
    assert_eq!(delta.removed, vec![k.clone()]);
    assert!(delta.changed.is_empty());
    // A delete of a key with no document is NotFound.
    assert_eq!(store.delete(&k, &doc.etag, &meta("bob", "")).await.unwrap_err(), StoreError::NotFound);
}

#[tokio::test]
async fn s06_history_gives_the_versions_newest_first_with_pages_and_version_gives_the_bytes() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let mut docs = vec![env_doc("L0")];
    store.put(&k, &docs[0], &create(), &meta("alice", "m0")).await.unwrap();
    for i in 1..7 {
        let d = env_doc(&format!("L{i}"));
        store.put(&k, &d, &matching(docs[i - 1].etag), &meta("alice", &format!("m{i}"))).await.unwrap();
        docs.push(d);
    }
    let page1 = store.history(&k, None, 3).await.unwrap();
    assert_eq!(page1.iter().map(|v| v.version).collect::<Vec<_>>(), [7, 6, 5]);
    let page2 = store.history(&k, Some(5), 3).await.unwrap();
    assert_eq!(page2.iter().map(|v| v.version).collect::<Vec<_>>(), [4, 3, 2]);
    let page3 = store.history(&k, Some(2), 3).await.unwrap();
    assert_eq!(page3.iter().map(|v| v.version).collect::<Vec<_>>(), [1]);
    assert_eq!(page1[0].meta.message.as_str(), "m6");
    assert_eq!(page1[0].etag, Some(docs[6].etag));
    for (i, d) in docs.iter().enumerate() {
        let v = store.version(&k, i as u64 + 1).await.unwrap();
        assert_eq!(v.bytes.as_deref(), Some(d.bytes.as_slice()));
        assert_eq!(v.meta.etag, Some(d.etag));
    }
    assert_eq!(store.version(&k, 8).await.unwrap_err(), StoreError::NotFound);
    assert_eq!(store.history(&key("environments/none"), None, 3).await.unwrap(), vec![]);
}

#[tokio::test]
async fn s07_history_keep_removes_the_oldest_versions_and_keeps_the_numbers() {
    let fx = Fixture::with_history_keep(3);
    let store = fx.open();
    let k = key("environments/p");
    let mut last = env_doc("L0");
    store.put(&k, &last, &create(), &meta("alice", "")).await.unwrap();
    for i in 1..10 {
        let d = env_doc(&format!("L{i}"));
        store.put(&k, &d, &matching(last.etag), &meta("alice", "")).await.unwrap();
        last = d;
    }
    let versions = store.history(&k, None, 100).await.unwrap();
    assert_eq!(versions.iter().map(|v| v.version).collect::<Vec<_>>(), [10, 9, 8]);
    assert_eq!(store.version(&k, 7).await.unwrap_err(), StoreError::NotFound);
    let next = store.put(&k, &env_doc("L10"), &matching(last.etag), &meta("alice", "")).await.unwrap();
    assert_eq!(next.version, 11);
    // The bytes of the pruned versions are gone too.
    let leftovers = files_below(&fx.docs().join(".history/environments/p"));
    assert_eq!(leftovers.len(), 6, "{leftovers:?}");
}

#[tokio::test]
async fn s08_a_crash_between_the_version_and_the_document_is_repaired_at_the_next_open() {
    let fx = Fixture::new();
    let k = key("environments/p");
    let (a, b) = (env_doc("A"), env_doc("B"));
    {
        let faults = Faults::new();
        FilesStore::migrate(&fx.options()).unwrap();
        let store = FilesStore::open_with_faults(fx.options(), faults.clone()).unwrap();
        store.put(&k, &a, &create(), &meta("alice", "")).await.unwrap();
        faults.fail_once(FaultPoint::AfterHistory, 5);
        let err = store.put(&k, &b, &matching(a.etag), &meta("bob", "lost?")).await.unwrap_err();
        assert!(matches!(err, StoreError::Unavailable(_)), "{err:?}");
        // The version is on the disk, and the document is still the old one.
        let on_disk = std::fs::read(fx.docs().join("environments/p/environment.json")).unwrap();
        assert_eq!(on_disk, a.bytes);
        assert!(fx.docs().join(".history/environments/p/0000000002.meta.json").exists());
    }
    let store = fx.reopen();
    let got = store.get(&k).await.unwrap();
    assert_eq!((got.bytes.as_slice(), got.version), (b.bytes.as_slice(), 2));
    let events = store.drain_events();
    assert!(events.contains(&StoreEvent::Repaired { key: k.clone(), version: 2 }), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, StoreEvent::ExternalChange { .. })));
    assert_eq!(store.history(&k, None, 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn s08_the_same_repair_runs_in_the_process_after_a_failed_write() {
    let fx = Fixture::new();
    let faults = Faults::new();
    FilesStore::migrate(&fx.options()).unwrap();
    let store = FilesStore::open_with_faults(fx.options(), faults.clone()).unwrap();
    let k = key("environments/p");
    let (a, b) = (env_doc("A"), env_doc("B"));
    store.put(&k, &a, &create(), &meta("alice", "")).await.unwrap();
    let listing = store.load().await.unwrap();
    faults.fail_once(FaultPoint::AfterHistory, 5);
    assert!(store.put(&k, &b, &matching(a.etag), &meta("bob", "")).await.is_err());
    // The next poll finds the version that is newer than the document, and repairs it.
    let delta = store.poll(&listing.marks).await.unwrap();
    assert_eq!(delta.changed.len(), 1);
    assert_eq!(delta.changed[0].bytes, b.bytes);
    assert_eq!(delta.changed[0].version, 2);
    // A delete that fails the same way is repaired as a delete.
    faults.fail_once(FaultPoint::AfterHistory, 5);
    assert!(store.delete(&k, &b.etag, &meta("bob", "")).await.is_err());
    let delta = store.poll(&delta.marks).await.unwrap();
    assert_eq!(delta.removed, vec![k.clone()]);
    assert_eq!(store.get(&k).await.unwrap_err(), StoreError::NotFound);
}

#[tokio::test]
async fn s08_a_crash_before_the_first_document_is_repaired() {
    let fx = Fixture::new();
    let faults = Faults::new();
    FilesStore::migrate(&fx.options()).unwrap();
    {
        let store = FilesStore::open_with_faults(fx.options(), faults.clone()).unwrap();
        faults.fail_once(FaultPoint::AfterHistory, 5);
        assert!(store.put(&key("environments/p"), &env_doc("A"), &create(), &meta("a", "")).await.is_err());
        assert!(!fx.docs().join("environments/p/environment.json").exists());
    }
    let store = fx.reopen();
    assert_eq!(store.get(&key("environments/p")).await.unwrap().version, 1);
}

#[tokio::test]
async fn s09_a_poll_reports_a_change_and_reports_nothing_new_when_nothing_changed() {
    let fx = Fixture::new();
    let store = fx.open();
    let listing = store.load().await.unwrap();
    assert!(listing.docs.is_empty());
    let delta = store.poll(&listing.marks).await.unwrap();
    assert!(delta.changed.is_empty() && delta.removed.is_empty());
    // A write of the store itself.
    let k = key("environments/p");
    let a = env_doc("A");
    store.put(&k, &a, &create(), &meta("alice", "")).await.unwrap();
    let delta = store.poll(&delta.marks).await.unwrap();
    assert_eq!(delta.changed.len(), 1);
    assert_eq!(delta.changed[0].key, k);
    let delta = store.poll(&delta.marks).await.unwrap();
    assert!(delta.changed.is_empty() && delta.removed.is_empty());
    // A write of another handle: a person who writes the file in the folder.
    let h = key("environments/p/hosts/host-a");
    let path = fx.docs().join("environments/p/hosts");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("host-a.json"), host_doc("host.example.invalid").bytes).unwrap();
    let delta = store.poll(&delta.marks).await.unwrap();
    assert_eq!(delta.changed.len(), 1);
    assert_eq!(delta.changed[0].key, h);
    assert!(store.poll(&delta.marks).await.unwrap().changed.is_empty());
    // A poll with the marks of the start reports each document again.
    assert_eq!(store.poll(&Marks::default()).await.unwrap().changed.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s10_read_environment_never_shows_half_of_a_write() {
    let fx = Fixture::new();
    let store = std::sync::Arc::new(fx.open());
    let k = key("environments/p");
    let first = env_doc("L0");
    store.put(&k, &first, &create(), &meta("w", "")).await.unwrap();
    let h = key("environments/p/hosts/host-a");
    store.put(&h, &host_doc("host.example.invalid"), &create(), &meta("w", "")).await.unwrap();
    let writer = {
        let store = store.clone();
        let k = k.clone();
        tokio::spawn(async move {
            let mut etag = first.etag;
            for i in 1..60 {
                let d = env_doc(&format!("L{i}"));
                store.put(&k, &d, &matching(etag), &meta("w", "")).await.unwrap();
                etag = d.etag;
            }
        })
    };
    let mut last_version = 0;
    for _ in 0..60 {
        let docs = store.read_environment(&env("p")).await.unwrap();
        assert_eq!(docs.len(), 2);
        for d in &docs {
            // Each document is whole: the bytes are canonical and the etag matches them.
            assert_eq!(d.etag, Etag::of(&d.bytes));
            let kind = d.key.kind();
            pupitre_core::doc::Document::from_stored(kind, &d.bytes, 1 << 20).unwrap();
        }
        let e = docs.iter().find(|d| d.key == k).unwrap();
        assert!(e.version >= last_version);
        last_version = e.version;
        // The version of the document is a version whose etag is the etag of these bytes.
        let v = store.version(&k, e.version).await;
        if let Ok(v) = v {
            assert_eq!(v.meta.etag, Some(e.etag));
        }
    }
    writer.await.unwrap();
    assert!(store.read_environment(&env("none")).await.unwrap().is_empty());
}

#[tokio::test]
async fn s11_a_secret_write_a_list_a_read_for_a_run_and_a_delete() {
    let fx = Fixture::new();
    let store = fx.open();
    let scope = unit_scope("production", "relay");
    let env_scope = pupitre_core::node::SecretScope::Environment(env("production"));
    let n1 = name("storage.password");
    let n2 = name("api.token");
    let secret = "correct horse battery staple";
    let put = store.put_secret(&scope, &n1, &value(secret), &meta("alice", "")).await.unwrap();
    store.put_secret(&scope, &n2, &value("token-0123456789"), &meta("bob", "")).await.unwrap();
    store.put_secret(&env_scope, &n2, &value("env-token-0123456789"), &meta("bob", "")).await.unwrap();
    assert_eq!(put.fingerprint.len(), 16);
    let list = store.list_secrets(&scope).await.unwrap();
    assert_eq!(list.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["api.token", "storage.password"]);
    assert_eq!(list[1].fingerprint, put.fingerprint);
    assert_eq!(list[1].updated_by.as_str(), "alice");
    // No value in the list: the metadata has no member for it, and its text form holds none.
    assert!(!format!("{list:?}").contains(secret));
    // The read for a run gives the values of the scopes that the caller names.
    let read = store.read_secrets(&[scope.clone(), env_scope.clone()]).await.unwrap();
    assert_eq!(read.len(), 3);
    assert_eq!(read[0].0, scope);
    assert_eq!(read[1].2.expose(), b"correct horse battery staple");
    assert_eq!(read[2].0, env_scope);
    assert!(store.read_secrets(&[unit_scope("production", "other")]).await.unwrap().is_empty());
    // The events are in the export and hold no value.
    let mut ex = store.export_begin().await.unwrap();
    let all = match &mut ex {
        pupitre_store::transfer::Exporter::Files(f) => f.secrets().await.unwrap(),
        pupitre_store::transfer::Exporter::Postgres(_) => unreachable!(),
    };
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|s| s.events.len() == 1));
    for f in files_below(&fx.secrets()) {
        if f.starts_with("events/") || f.starts_with("meta/") {
            let text = std::fs::read_to_string(fx.secrets().join(&f)).unwrap();
            assert!(!text.contains("battery") && !text.contains("token-0123"), "{f}");
        }
    }
    // A second write replaces the value, and a delete removes it.
    store.put_secret(&scope, &n1, &value("another value 12345"), &meta("carol", "")).await.unwrap();
    assert_ne!(store.list_secrets(&scope).await.unwrap()[1].fingerprint, put.fingerprint);
    store.delete_secret(&scope, &n1, &meta("carol", "")).await.unwrap();
    assert_eq!(store.list_secrets(&scope).await.unwrap().len(), 1);
    assert_eq!(store.delete_secret(&scope, &n1, &meta("carol", "")).await.unwrap_err(), StoreError::NotFound);
    let events =
        std::fs::read_to_string(fx.secrets().join("events/environments/production/units/relay/storage.password.jsonl"))
            .unwrap();
    let actions: Vec<&str> = events.lines().map(|l| if l.contains("\"set\"") { "set" } else { "delete" }).collect();
    assert_eq!(actions, ["set", "set", "delete"]);
}

#[tokio::test]
async fn s11_the_events_of_a_secret_keep_the_newest_history_keep() {
    let fx = Fixture::with_history_keep(3);
    let store = fx.open();
    let scope = unit_scope("p", "r");
    let n = name("a.b");
    for i in 0..8 {
        store
            .put_secret(&scope, &n, &value(&format!("value-number-{i}")), &meta(&format!("user{i}"), ""))
            .await
            .unwrap();
    }
    let text = std::fs::read_to_string(fx.secrets().join("events/environments/p/units/r/a.b.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert!(text.contains("user7") && text.contains("user5") && !text.contains("user4"));
}

#[tokio::test]
async fn s11_a_secret_named_units_in_an_environment_scope_is_refused() {
    // The layout of 2.31 puts the unit scopes in a folder `units` next to the values of the
    // environment scope. A secret of that name would use the same path.
    let fx = Fixture::new();
    let store = fx.open();
    let env_scope = pupitre_core::node::SecretScope::Environment(env("p"));
    let err = store.put_secret(&env_scope, &name("units"), &value("12345678"), &meta("a", "")).await.unwrap_err();
    assert!(matches!(err, StoreError::Unavailable(_)), "{err:?}");
    store.put_secret(&unit_scope("p", "r"), &name("units"), &value("12345678"), &meta("a", "")).await.unwrap();
}

#[tokio::test]
async fn s12_the_fingerprint_of_a_known_key_scope_name_and_value_equals_the_vector() {
    let fx = Fixture::new();
    let store = fx.open();
    let m = store
        .put_secret(
            &unit_scope("production", "relay"),
            &name("storage.password"),
            &value("correct horse battery staple"),
            &meta("a", ""),
        )
        .await
        .unwrap();
    // The vectors of `secret.rs` come from an independent computation (HMAC-SHA-256 in Python).
    assert_eq!(m.fingerprint, "59b50c54cf977b23");
    let meta_file = std::fs::read_to_string(fx.secrets().join("pupitre-secrets.json")).unwrap();
    assert_eq!(
        meta_file,
        "{\n  \"format\": 1,\n  \"key_check\": \"ce03a3f53a0a890ed2ff2c22961a12a9fe01b4f47a1c7a9e993f4e4194684dc9\"\n}\n"
    );
    // The same value in another scope has another fingerprint.
    let other = store
        .put_secret(
            &unit_scope("production", "feed"),
            &name("storage.password"),
            &value("correct horse battery staple"),
            &meta("a", ""),
        )
        .await
        .unwrap();
    assert_ne!(other.fingerprint, m.fingerprint);
}

#[tokio::test]
async fn s13_a_wrong_secret_key_stops_the_open() {
    let fx = Fixture::new();
    drop(fx.open());
    let mut options = fx.options();
    options.secret_key = SecretKey::parse(KEY_B).unwrap();
    let err = FilesStore::open(options.clone()).unwrap_err();
    assert_eq!(err, StoreError::KeyMismatch);
    assert_eq!(err.to_string(), "storage.secret_key does not match the key of the store");
    // `migrate` with the wrong key does not change the key check either.
    assert_eq!(FilesStore::migrate(&options).unwrap_err(), StoreError::KeyMismatch);
    assert!(FilesStore::open(fx.options()).is_ok());
}

#[tokio::test]
async fn s14_create_run_above_max_queued_gives_queue_full_and_one_check_gives_one_apply() {
    let fx = Fixture::new();
    let store = fx.open();
    for i in 0..3u8 {
        store.create_run(&run_record(run_id(i as u64, i), "p", RunStatus::Queued), 3, None).await.unwrap();
    }
    let err = store.create_run(&run_record(run_id(10, 10), "p", RunStatus::Queued), 3, None).await.unwrap_err();
    assert_eq!(err, StoreError::QueueFull);
    assert_eq!(
        store.create_run(&run_record(run_id(0, 0), "p", RunStatus::Queued), 99, None).await.unwrap_err(),
        StoreError::AlreadyExists
    );
    // A run that is not queued is refused.
    let err = store.create_run(&run_record(run_id(11, 11), "p", RunStatus::Running), 99, None).await.unwrap_err();
    assert!(matches!(err, StoreError::BadTransition { .. }));
    assert_eq!(store.list_runs(&RunFilter::default(), None, 10).await.unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s14_two_applies_that_consume_one_check_exactly_one_succeeds() {
    let fx = Fixture::new();
    let store = std::sync::Arc::new(fx.open());
    let check = run_record(run_id(0, 1), "p", RunStatus::Queued);
    store.create_run(&check, 10, None).await.unwrap();
    let mut tasks = Vec::new();
    for n in 2..8u8 {
        let store = store.clone();
        let id = check.id;
        tasks.push(tokio::spawn(async move {
            store.create_run(&run_record(run_id(n as u64, n), "p", RunStatus::Queued), 10, Some(&id)).await
        }));
    }
    let mut ok = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(()) => ok += 1,
            Err(e) => assert_eq!(e, StoreError::CheckUsed),
        }
    }
    assert_eq!(ok, 1);
    let used = store.get_run(&check.id).await.unwrap();
    let applied = used.applied_by.unwrap();
    assert_eq!(store.get_run(&applied).await.unwrap().status, RunStatus::Queued);
    let err = store
        .create_run(&run_record(run_id(20, 20), "p", RunStatus::Queued), 10, Some(&run_id(99, 99)))
        .await
        .unwrap_err();
    assert_eq!(err, StoreError::NotFound);
    // The state of the check is on the disk.
    drop(store);
    let again = fx.reopen();
    assert_eq!(again.get_run(&check.id).await.unwrap().applied_by, Some(applied));
}

#[tokio::test]
async fn s15_update_run_with_a_wrong_expected_status_conflicts_and_a_bad_transition_is_refused() {
    let fx = Fixture::new();
    let store = fx.open();
    let id = run_id(0, 1);
    let queued = run_record(id, "p", RunStatus::Queued);
    store.create_run(&queued, 10, None).await.unwrap();
    let mut running = queued.clone();
    running.status = RunStatus::Running;
    // The stored status is queued, not running.
    assert_eq!(
        store.update_run(RunStatus::Running, &running).await.unwrap_err(),
        StoreError::Conflict { current: None }
    );
    store.update_run(RunStatus::Queued, &running).await.unwrap();
    assert_eq!(store.get_run(&id).await.unwrap().status, RunStatus::Running);
    // Running to queued is outside 4.39.
    let err = store.update_run(RunStatus::Running, &queued).await.unwrap_err();
    assert_eq!(err, StoreError::BadTransition { from: RunStatus::Running, to: RunStatus::Queued });
    // Running to running writes the fields.
    let mut more = running.clone();
    more.exit_code = Some(0);
    store.update_run(RunStatus::Running, &more).await.unwrap();
    let mut done = more.clone();
    done.status = RunStatus::Passed;
    store.update_run(RunStatus::Running, &done).await.unwrap();
    // A final status is final.
    let err = store.update_run(RunStatus::Passed, &running).await.unwrap_err();
    assert!(matches!(err, StoreError::BadTransition { .. }));
    assert_eq!(store.get_run(&id).await.unwrap().exit_code, Some(0));
    assert_eq!(
        store.update_run(RunStatus::Queued, &run_record(run_id(5, 5), "p", RunStatus::Queued)).await.unwrap_err(),
        StoreError::NotFound
    );
}

#[tokio::test]
async fn s15_the_runs_list_in_the_order_of_the_id() {
    let fx = Fixture::new();
    let store = fx.open();
    for i in 0..5u8 {
        let env = if i % 2 == 0 { "p" } else { "q" };
        store.create_run(&run_record(run_id(i as u64, i), env, RunStatus::Queued), 10, None).await.unwrap();
    }
    let all = store.list_runs(&RunFilter::default(), None, 10).await.unwrap();
    assert_eq!(
        all.iter().map(|r| r.id).collect::<Vec<_>>(),
        (0..5u8).rev().map(|i| run_id(i as u64, i)).collect::<Vec<_>>()
    );
    let page = store.list_runs(&RunFilter::default(), Some(&all[1].id), 2).await.unwrap();
    assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), vec![all[2].id, all[3].id]);
    let only_q = RunFilter { environment: Some(env("q")), status: None };
    assert_eq!(store.list_runs(&only_q, None, 10).await.unwrap().len(), 2);
    let none = RunFilter { environment: None, status: Some(RunStatus::Passed) };
    assert!(store.list_runs(&none, None, 10).await.unwrap().is_empty());
    // The oldest queued runs of the environments that are not busy.
    let next = store.next_queued(&[env("p")], 10).await.unwrap();
    assert_eq!(next.iter().map(|r| r.environment.as_str()).collect::<Vec<_>>(), ["q", "q"]);
    assert!(next[0].id < next[1].id);
    assert_eq!(store.next_queued(&[], 3).await.unwrap().len(), 3);
}

#[tokio::test]
async fn s16_append_output_and_events_with_a_wrong_offset_or_number_conflict_and_add_nothing() {
    let fx = Fixture::new();
    let store = fx.open();
    let id = run_id(0, 1);
    store.create_run(&run_record(id, "p", RunStatus::Queued), 10, None).await.unwrap();
    assert_eq!(store.append_output(&id, 0, b"hello ").await.unwrap(), 6);
    assert_eq!(store.append_output(&id, 6, b"world").await.unwrap(), 11);
    // A second call after a crash adds nothing two times.
    assert_eq!(store.append_output(&id, 6, b"world").await.unwrap_err(), StoreError::Conflict { current: None });
    assert_eq!(store.append_output(&id, 20, b"x").await.unwrap_err(), StoreError::Conflict { current: None });
    let chunk = store.read_output(&id, 0, 100).await.unwrap();
    assert_eq!((chunk.bytes.as_slice(), chunk.length), (b"hello world".as_slice(), 11));
    let chunk = store.read_output(&id, 6, 3).await.unwrap();
    assert_eq!((chunk.bytes.as_slice(), chunk.length), (b"wor".as_slice(), 11));
    let chunk = store.read_output(&id, 40, 3).await.unwrap();
    assert_eq!((chunk.bytes.len(), chunk.length), (0, 11));
    store.append_events(&id, 1, &[b"{\"a\":1}".to_vec(), b"{\"a\":2}".to_vec()]).await.unwrap();
    assert_eq!(
        store.append_events(&id, 1, &[b"{\"a\":9}".to_vec()]).await.unwrap_err(),
        StoreError::Conflict { current: None }
    );
    assert_eq!(
        store.append_events(&id, 4, &[b"{\"a\":9}".to_vec()]).await.unwrap_err(),
        StoreError::Conflict { current: None }
    );
    store.append_events(&id, 3, &[b"{\"a\":3}".to_vec()]).await.unwrap();
    let all = store.read_events(&id, 0, 100).await.unwrap();
    assert_eq!(
        all.iter().map(|e| (e.seq, e.bytes.clone())).collect::<Vec<_>>(),
        vec![(1, b"{\"a\":1}".to_vec()), (2, b"{\"a\":2}".to_vec()), (3, b"{\"a\":3}".to_vec())]
    );
    let tail = store.read_events(&id, 1, 1).await.unwrap();
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].seq, 2);
    // A line with a line end is refused.
    assert!(store.append_events(&id, 4, &[b"a\nb".to_vec()]).await.is_err());
    // An unknown run.
    assert_eq!(store.append_output(&run_id(9, 9), 0, b"x").await.unwrap_err(), StoreError::NotFound);
    // The state survives a restart.
    drop(store);
    let again = fx.reopen();
    assert_eq!(again.read_output(&id, 0, 100).await.unwrap().length, 11);
    assert_eq!(again.append_events(&id, 4, &[b"{\"a\":4}".to_vec()]).await.unwrap(), ());
}

#[tokio::test]
async fn s17_a_lease_acquire_held_renew_end_second_holder_and_lost() {
    let fx = Fixture::new();
    let store = fx.open();
    let name = LeaseName::Runner;
    let ttl = Duration::from_millis(300);
    let first = store.acquire_lease(&name, "node-a", ttl).await.unwrap();
    assert!(matches!(first, LeaseOutcome::Acquired { .. }));
    match store.acquire_lease(&name, "node-b", ttl).await.unwrap() {
        LeaseOutcome::HeldBy { holder, .. } => assert_eq!(holder, "node-a"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(store.renew_lease(&name, "node-a", ttl).await.unwrap(), LeaseRenewal::Renewed { .. }));
    assert_eq!(store.renew_lease(&name, "node-b", ttl).await.unwrap(), LeaseRenewal::Lost);
    tokio::time::sleep(Duration::from_millis(400)).await;
    // The end time has passed: the first holder cannot renew, and a second holder acquires.
    assert_eq!(store.renew_lease(&name, "node-a", ttl).await.unwrap(), LeaseRenewal::Lost);
    assert!(matches!(store.acquire_lease(&name, "node-b", ttl).await.unwrap(), LeaseOutcome::Acquired { .. }));
    assert_eq!(store.renew_lease(&name, "node-a", ttl).await.unwrap(), LeaseRenewal::Lost);
    // A release removes the lease only for its holder.
    store.release_lease(&name, "node-a").await.unwrap();
    assert!(matches!(store.acquire_lease(&name, "node-a", ttl).await.unwrap(), LeaseOutcome::HeldBy { .. }));
    store.release_lease(&name, "node-b").await.unwrap();
    assert!(matches!(store.acquire_lease(&name, "node-a", ttl).await.unwrap(), LeaseOutcome::Acquired { .. }));
    // The names are separate.
    let env_lease = LeaseName::Environment(env("p"));
    assert!(matches!(store.acquire_lease(&env_lease, "node-b", ttl).await.unwrap(), LeaseOutcome::Acquired { .. }));
}

#[tokio::test]
async fn s18_prune_runs_removes_the_oldest_final_runs_and_never_a_queued_or_running_run() {
    let fx = Fixture::new();
    let store = fx.open();
    let mut ids = Vec::new();
    for i in 0..6u8 {
        let id = run_id(i as u64, i);
        ids.push(id);
        let r = run_record(id, "p", RunStatus::Queued);
        store.create_run(&r, 100, None).await.unwrap();
        store.append_output(&id, 0, b"output").await.unwrap();
        store.append_events(&id, 1, &[b"{}".to_vec()]).await.unwrap();
        // The runs 0, 1 and 2 end. The run 3 runs. The runs 4 and 5 wait.
        if i < 4 {
            let mut running = r.clone();
            running.status = RunStatus::Running;
            store.update_run(RunStatus::Queued, &running).await.unwrap();
            if i < 3 {
                let mut done = running.clone();
                done.status = if i == 1 { RunStatus::Failed } else { RunStatus::Passed };
                store.update_run(RunStatus::Running, &done).await.unwrap();
            }
        }
    }
    assert!(store.prune_runs(5).await.unwrap().is_empty());
    let removed = store.prune_runs(1).await.unwrap();
    assert_eq!(removed, vec![ids[0], ids[1]]);
    assert_eq!(store.get_run(&ids[0]).await.unwrap_err(), StoreError::NotFound);
    assert_eq!(store.read_output(&ids[0], 0, 10).await.unwrap_err(), StoreError::NotFound);
    assert!(!fx.runs().join(ids[0].to_string()).exists());
    for id in &ids[2..] {
        assert!(store.get_run(id).await.is_ok());
    }
    // Keep 0: the last final run goes too, and the run 3 and the queued runs stay.
    assert_eq!(store.prune_runs(0).await.unwrap(), vec![ids[2]]);
    assert_eq!(store.list_runs(&RunFilter::default(), None, 10).await.unwrap().len(), 3);
    assert_eq!(store.read_output(&ids[5], 0, 10).await.unwrap().bytes, b"output");
}

fn corrupt_schema(fx: &Fixture, text: &str) {
    std::fs::write(fx.docs().join("pupitre-store.json"), text).unwrap();
}

#[tokio::test]
async fn s20_an_old_store_schema_stops_the_open_and_migrate_makes_it_current() {
    let fx = Fixture::new();
    let k = key("environments/p");
    // No store schema at all.
    let err = FilesStore::open(fx.options()).unwrap_err();
    assert!(matches!(err, StoreError::Unavailable(_)), "the folders do not exist: {err:?}");
    std::fs::create_dir_all(fx.docs()).unwrap();
    std::fs::create_dir_all(fx.secrets()).unwrap();
    std::fs::set_permissions(fx.secrets(), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    std::fs::create_dir_all(fx.runs()).unwrap();
    let err = FilesStore::open(fx.options()).unwrap_err();
    assert_eq!(err, StoreError::MigrateNeeded { found: None });
    assert_eq!(err.to_string(), "run pupitre migrate");
    assert_eq!(FilesStore::migrate(&fx.options()).unwrap(), MigrateOutcome::Created);
    {
        let store = FilesStore::open(fx.options()).unwrap();
        store.put(&k, &env_doc("A"), &create(), &meta("a", "")).await.unwrap();
    }
    // `migrate` on a current store changes nothing.
    let before = std::fs::read(fx.docs().join("pupitre-store.json")).unwrap();
    assert_eq!(FilesStore::migrate(&fx.options()).unwrap(), MigrateOutcome::Current);
    assert_eq!(std::fs::read(fx.docs().join("pupitre-store.json")).unwrap(), before);
    // An old schema.
    corrupt_schema(&fx, "{\n  \"format\": 1,\n  \"store_schema\": 0\n}\n");
    let err = FilesStore::open(fx.options()).unwrap_err();
    assert_eq!(err, StoreError::MigrateNeeded { found: Some(0) });
    assert_eq!(err.to_string(), "run pupitre migrate");
    assert_eq!(FilesStore::migrate(&fx.options()).unwrap(), MigrateOutcome::Updated { from: 0 });
    let store = FilesStore::open(fx.options()).unwrap();
    assert_eq!(store.get(&k).await.unwrap().version, 1);
    drop(store);
    // A newer schema stops both commands.
    corrupt_schema(&fx, "{\n  \"format\": 1,\n  \"store_schema\": 2\n}\n");
    let err = FilesStore::open(fx.options()).unwrap_err();
    assert_eq!(err, StoreError::StoreNewer { found: 2 });
    assert_eq!(err.to_string(), "the store is newer than this binary");
    assert_eq!(FilesStore::migrate(&fx.options()).unwrap_err(), StoreError::StoreNewer { found: 2 });
    // A file that is not valid is a corrupt store, never a repair.
    corrupt_schema(&fx, "{\"format\": 2, \"store_schema\": 1}");
    assert!(matches!(FilesStore::open(fx.options()).unwrap_err(), StoreError::Corrupt(_)));
    assert!(matches!(FilesStore::migrate(&fx.options()).unwrap_err(), StoreError::Corrupt(_)));
}

#[test]
fn the_options_type_has_the_fields_of_the_hand_off() {
    let fx = Fixture::new();
    let o: FilesOptions = fx.options();
    assert_eq!(o.history_keep, 100);
    assert_eq!(o.max_document_bytes, 262_144);
    let _: &DocKey = &key("environments/p");
}
