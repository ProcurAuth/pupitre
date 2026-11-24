// SPDX-License-Identifier: MIT OR Apache-2.0
//! The layout of 2.31 byte for byte, the modes and the lock of 2.32, and the links.

mod files_support;

use files_support::*;
use pupitre_core::doc::DocKind;
use pupitre_core::etag::Etag;
use pupitre_core::run::RunStatus;
use pupitre_store::files::{FaultPoint, Faults, FilesStore};
use pupitre_store::{Precondition, Store, StoreError};

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[tokio::test]
async fn the_layout_of_2_31_byte_for_byte() {
    let fx = Fixture::new();
    let store = fx.open();
    let m = meta("alice", "first");
    let env_d = env_doc("Production");
    let host_d = host_doc("203.0.113.10");
    let unit_d = unit_doc();
    let uv = values_doc(DocKind::UnitValues, r#"{"limits.max_clients": 200}"#);
    let hv = values_doc(DocKind::HostValues, r#"{"log.level": "debug"}"#);
    let common = values_doc(DocKind::Common, r#"{"log.level": "info"}"#);
    let keys = [
        ("environments/production", &env_d),
        ("environments/production/common", &common),
        ("environments/production/hosts/host-a", &host_d),
        ("environments/production/units/relay", &unit_d),
        ("environments/production/units/relay/values", &uv),
        ("environments/production/units/relay/hosts/host-a/values", &hv),
    ];
    for (k, d) in keys {
        store.put(&key(k), d, &Precondition::Create, &m).await.unwrap();
    }
    // A second version of one key.
    let env2 = env_doc("Production 2");
    store
        .put(&key("environments/production"), &env2, &Precondition::Match(env_d.etag), &meta("bob", "second"))
        .await
        .unwrap();
    store
        .delete(&key("environments/production/common"), &common.etag, &meta("bob", "line one\nline two"))
        .await
        .unwrap();

    let docs = fx.docs();
    let expected = sorted(
        [
            ".lock",
            "pupitre-store.json",
            "environments/production/environment.json",
            "environments/production/hosts/host-a.json",
            "environments/production/units/relay/unit.json",
            "environments/production/units/relay/values.json",
            "environments/production/units/relay/hosts/host-a/values.json",
            ".history/environments/production/0000000001.json",
            ".history/environments/production/0000000001.meta.json",
            ".history/environments/production/0000000002.json",
            ".history/environments/production/0000000002.meta.json",
            ".history/environments/production/common/0000000001.json",
            ".history/environments/production/common/0000000001.meta.json",
            ".history/environments/production/common/0000000002.meta.json",
            ".history/environments/production/hosts/host-a/0000000001.json",
            ".history/environments/production/hosts/host-a/0000000001.meta.json",
            ".history/environments/production/units/relay/0000000001.json",
            ".history/environments/production/units/relay/0000000001.meta.json",
            ".history/environments/production/units/relay/values/0000000001.json",
            ".history/environments/production/units/relay/values/0000000001.meta.json",
            ".history/environments/production/units/relay/hosts/host-a/values/0000000001.json",
            ".history/environments/production/units/relay/hosts/host-a/values/0000000001.meta.json",
        ]
        .map(String::from)
        .to_vec(),
    );
    assert_eq!(sorted(files_below(&docs)), expected);
    let read = |p: &str| std::fs::read(docs.join(p)).unwrap();
    assert_eq!(read("pupitre-store.json"), b"{\n  \"format\": 1,\n  \"store_schema\": 1\n}\n");
    assert_eq!(read("environments/production/environment.json"), env2.bytes);
    assert_eq!(read("environments/production/hosts/host-a.json"), host_d.bytes);
    assert_eq!(read("environments/production/units/relay/hosts/host-a/values.json"), hv.bytes);
    assert_eq!(read(".history/environments/production/0000000001.json"), env_d.bytes);
    assert_eq!(read(".history/environments/production/0000000002.json"), env2.bytes);
    assert_eq!(
        String::from_utf8(read(".history/environments/production/0000000001.meta.json")).unwrap(),
        format!(
            "{{\n  \"etag\": \"{}\",\n  \"kind\": \"create\",\n  \"message\": \"first\",\n  \"saved_at\": \"2026-11-24T10:00:00.000Z\",\n  \"saved_by\": \"alice\"\n}}\n",
            env_d.etag
        )
    );
    assert_eq!(
        String::from_utf8(read(".history/environments/production/0000000002.meta.json")).unwrap(),
        format!(
            "{{\n  \"etag\": \"{}\",\n  \"kind\": \"save\",\n  \"message\": \"second\",\n  \"saved_at\": \"2026-11-24T10:00:00.000Z\",\n  \"saved_by\": \"bob\"\n}}\n",
            env2.etag
        )
    );
    // A delete: a metadata file with no etag, no bytes file, and no current document.
    assert_eq!(
        String::from_utf8(read(".history/environments/production/common/0000000002.meta.json")).unwrap(),
        "{\n  \"kind\": \"delete\",\n  \"message\": \"line one\\nline two\",\n  \"saved_at\": \"2026-11-24T10:00:00.000Z\",\n  \"saved_by\": \"bob\"\n}\n"
    );
    assert!(!docs.join("environments/production/common.json").exists());
    // The folder .tmp is empty after the writes.
    assert!(std::fs::read_dir(docs.join(".tmp")).unwrap().next().is_none());

    // The secrets folder.
    let scope = unit_scope("production", "relay");
    store
        .put_secret(&scope, &name("storage.password"), &value("correct horse battery staple"), &meta("carol", ""))
        .await
        .unwrap();
    let secrets = fx.secrets();
    let expected_secrets = sorted(
        [
            ".lock",
            "pupitre-secrets.json",
            "values/environments/production/units/relay/storage.password",
            "meta/environments/production/units/relay/storage.password.json",
            "events/environments/production/units/relay/storage.password.jsonl",
        ]
        .map(String::from)
        .to_vec(),
    );
    assert_eq!(sorted(files_below(&secrets)), expected_secrets);
    let sread = |p: &str| std::fs::read(secrets.join(p)).unwrap();
    // The value has no line end added (2.34).
    assert_eq!(sread("values/environments/production/units/relay/storage.password"), b"correct horse battery staple");
    assert_eq!(
        String::from_utf8(sread("meta/environments/production/units/relay/storage.password.json")).unwrap(),
        "{\n  \"saved_at\": \"2026-11-24T10:00:00.000Z\",\n  \"saved_by\": \"carol\"\n}\n"
    );
    assert_eq!(
        String::from_utf8(sread("events/environments/production/units/relay/storage.password.jsonl")).unwrap(),
        "{\"action\":\"set\",\"at\":\"2026-11-24T10:00:00.000Z\",\"by\":\"carol\",\"name\":\"storage.password\",\"scope\":\"environment:production/unit:relay\"}\n"
    );

    // The runs folder.
    let id = run_id(0, 1);
    let record = run_record(id, "production", RunStatus::Queued);
    store.create_run(&record, 10, None).await.unwrap();
    store.append_output(&id, 0, b"PLAY [all]\n").await.unwrap();
    store.append_events(&id, 1, &[b"{\"seq\":1}".to_vec(), b"{\"seq\":2}".to_vec()]).await.unwrap();
    let runs = fx.runs();
    let expected_runs = sorted(
        [".lock".to_string(), format!("{id}/run.json"), format!("{id}/output.log"), format!("{id}/events.jsonl")]
            .to_vec(),
    );
    assert_eq!(sorted(files_below(&runs)), expected_runs);
    let canonical = pupitre_core::json::to_canonical(&record.to_json());
    assert_eq!(std::fs::read(runs.join(format!("{id}/run.json"))).unwrap(), canonical);
    assert_eq!(std::fs::read(runs.join(format!("{id}/output.log"))).unwrap(), b"PLAY [all]\n");
    assert_eq!(std::fs::read(runs.join(format!("{id}/events.jsonl"))).unwrap(), b"{\"seq\":1}\n{\"seq\":2}\n");
    // The etags of the files are the SHA-256 of their bytes (2.6).
    assert_eq!(Etag::of(&read("environments/production/environment.json")), env2.etag);
}

#[tokio::test]
async fn the_modes_of_2_32() {
    let fx = Fixture::new();
    let store = fx.open();
    store.put(&key("environments/p"), &env_doc("P"), &Precondition::Create, &meta("a", "")).await.unwrap();
    store
        .put(
            &key("environments/p/hosts/host-a"),
            &host_doc("host.example.invalid"),
            &Precondition::Create,
            &meta("a", ""),
        )
        .await
        .unwrap();
    store.put_secret(&unit_scope("p", "r"), &name("a.b"), &value("12345678"), &meta("a", "")).await.unwrap();
    let id = run_id(0, 1);
    store.create_run(&run_record(id, "p", RunStatus::Queued), 10, None).await.unwrap();
    store.append_output(&id, 0, b"x").await.unwrap();
    store.append_events(&id, 1, &[b"{}".to_vec()]).await.unwrap();
    for (root, file, dir) in [(fx.docs(), 0o644, 0o755), (fx.secrets(), 0o600, 0o700), (fx.runs(), 0o640, 0o750)] {
        for p in entries_below(&root) {
            let is_dir = std::fs::symlink_metadata(&p).unwrap().is_dir();
            let want = if is_dir { dir } else { file };
            assert_eq!(mode_of(&p), want, "{}", p.display());
        }
    }
}

#[tokio::test]
async fn a_secrets_folder_with_a_group_right_stops_the_open() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    drop(fx.open());
    std::fs::set_permissions(fx.secrets(), std::fs::Permissions::from_mode(0o750)).unwrap();
    let err = FilesStore::open(fx.options()).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("0750") && text.contains("0700"), "{text}");
    std::fs::set_permissions(fx.secrets(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(FilesStore::open(fx.options()).is_ok());
}

#[tokio::test]
async fn a_second_open_stops_with_the_lock_of_2_32() {
    let fx = Fixture::new();
    let first = fx.open();
    let err = FilesStore::open(fx.options()).unwrap_err();
    assert_eq!(err, StoreError::Locked { path: fx.docs().display().to_string() });
    assert_eq!(err.to_string(), format!("the folder {} is locked by another process", fx.docs().display()));
    assert!(matches!(FilesStore::migrate(&fx.options()).unwrap_err(), StoreError::Locked { .. }));
    drop(first);
    assert!(FilesStore::open(fx.options()).is_ok());
}

#[tokio::test]
async fn a_lock_on_one_of_the_three_folders_stops_the_open() {
    use rustix::fs::{FlockOperation, flock};
    let fx = Fixture::new();
    drop(fx.open());
    for (path, expect) in [(fx.runs(), fx.runs()), (fx.secrets(), fx.secrets())] {
        let file = std::fs::File::options().read(true).write(true).open(path.join(".lock")).unwrap();
        flock(&file, FlockOperation::NonBlockingLockExclusive).unwrap();
        let err = FilesStore::open(fx.options()).unwrap_err();
        assert_eq!(err, StoreError::Locked { path: expect.display().to_string() });
        drop(file);
    }
    assert!(FilesStore::open(fx.options()).is_ok());
}

#[tokio::test]
async fn a_file_system_with_no_flock_stops_the_open() {
    let fx = Fixture::new();
    drop(fx.open());
    let faults = Faults::new();
    faults.fail_once(FaultPoint::Flock, 37); // ENOLCK
    let err = FilesStore::open_with_faults(fx.options(), faults).unwrap_err();
    assert!(matches!(&err, StoreError::Unavailable(t) if t.contains("does not support flock")), "{err:?}");
}

#[tokio::test]
async fn a_symbolic_link_is_an_error_and_nothing_is_written_through_it() {
    let fx = Fixture::new();
    let store = fx.open();
    let elsewhere = fx.dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    // A link in place of the folder of an environment.
    std::fs::create_dir_all(fx.docs().join("environments")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, fx.docs().join("environments/p")).unwrap();
    let k = key("environments/p");
    let err = store.put(&k, &env_doc("P"), &Precondition::Create, &meta("a", "")).await.unwrap_err();
    assert!(matches!(err, StoreError::Corrupt(_)), "{err:?}");
    assert!(matches!(store.get(&k).await.unwrap_err(), StoreError::Corrupt(_)));
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    // The poll ignores the link, with an event.
    let delta = store.poll(&Default::default()).await.unwrap();
    assert!(delta.changed.is_empty());
    let events = store.drain_events();
    assert!(
        events.iter().any(|e| matches!(e, pupitre_store::StoreEvent::EntryIgnored { path, reason }
            if path == "environments/p" && reason.contains("symbolic link"))),
        "{events:?}"
    );
    // A link in place of the file of a document.
    std::fs::remove_file(fx.docs().join("environments/p")).unwrap();
    std::fs::create_dir_all(fx.docs().join("environments/q")).unwrap();
    std::fs::write(elsewhere.join("target.json"), env_doc("Q").bytes).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("target.json"), fx.docs().join("environments/q/environment.json"))
        .unwrap();
    let q = key("environments/q");
    assert!(matches!(store.get(&q).await.unwrap_err(), StoreError::Corrupt(_)));
    assert!(store.put(&q, &env_doc("Q2"), &Precondition::Create, &meta("a", "")).await.is_err());
    assert_eq!(std::fs::read(elsewhere.join("target.json")).unwrap(), env_doc("Q").bytes);
    // A link in place of `.history`.
    std::fs::remove_file(fx.docs().join("environments/q/environment.json")).unwrap();
    std::fs::remove_dir_all(fx.docs().join(".history")).ok();
    std::os::unix::fs::symlink(&elsewhere, fx.docs().join(".history")).unwrap();
    assert!(store.put(&q, &env_doc("Q3"), &Precondition::Create, &meta("a", "")).await.is_err());
    assert!(!elsewhere.join("environments").exists());
    // A link in place of a folder of the secrets: no value goes through it.
    std::os::unix::fs::symlink(&elsewhere, fx.secrets().join("values")).unwrap();
    let err =
        store.put_secret(&unit_scope("p", "r"), &name("a.b"), &value("12345678"), &meta("a", "")).await.unwrap_err();
    assert!(matches!(err, StoreError::Corrupt(_)), "{err:?}");
    assert!(!elsewhere.join("environments").exists());
}

#[tokio::test]
async fn a_link_in_place_of_a_folder_stops_the_open() {
    let fx = Fixture::new();
    drop(fx.open());
    let moved = fx.dir.path().join("moved");
    std::fs::rename(fx.runs(), &moved).unwrap();
    std::os::unix::fs::symlink(&moved, fx.runs()).unwrap();
    assert!(FilesStore::open(fx.options()).is_err());
}

#[tokio::test]
async fn a_leftover_in_the_tmp_folder_is_removed_at_open() {
    let fx = Fixture::new();
    drop(fx.open());
    std::fs::write(fx.docs().join(".tmp/0123456789abcdef.tmp"), b"half").unwrap();
    std::fs::write(fx.secrets().join(".tmp/aaaaaaaaaaaaaaaa.tmp"), b"half").unwrap();
    let store = fx.reopen();
    assert!(std::fs::read_dir(fx.docs().join(".tmp")).unwrap().next().is_none());
    assert!(std::fs::read_dir(fx.secrets().join(".tmp")).unwrap().next().is_none());
    drop(store);
}

#[tokio::test]
async fn a_failed_write_of_the_bytes_of_a_version_leaves_no_version() {
    let fx = Fixture::new();
    FilesStore::migrate(&fx.options()).unwrap();
    let faults = Faults::new();
    let k = key("environments/p");
    let (a, b) = (env_doc("A"), env_doc("B"));
    {
        let store = FilesStore::open_with_faults(fx.options(), faults.clone()).unwrap();
        store.put(&k, &a, &Precondition::Create, &meta("a", "")).await.unwrap();
        faults.fail_once(FaultPoint::AfterHistoryBytes, 5);
        assert!(store.put(&k, &b, &Precondition::Match(a.etag), &meta("a", "")).await.is_err());
        // The bytes are in `.history`, the metadata is not: the version does not exist.
        assert!(fx.docs().join(".history/environments/p/0000000002.json").exists());
        assert!(!fx.docs().join(".history/environments/p/0000000002.meta.json").exists());
        assert_eq!(store.get(&k).await.unwrap().bytes, a.bytes);
    }
    let store = fx.reopen();
    assert_eq!(store.get(&k).await.unwrap().version, 1);
    // The next write makes the version 2 and replaces the bytes that the crash left.
    let saved = store.put(&k, &b, &Precondition::Match(a.etag), &meta("a", "")).await.unwrap();
    assert_eq!(saved.version, 2);
    assert_eq!(store.version(&k, 2).await.unwrap().bytes, Some(b.bytes));
}

#[tokio::test]
async fn a_failed_fsync_gives_an_error_that_names_the_path() {
    let fx = Fixture::new();
    FilesStore::migrate(&fx.options()).unwrap();
    let faults = Faults::new();
    let store = FilesStore::open_with_faults(fx.options(), faults.clone()).unwrap();
    faults.fail_once(FaultPoint::Fsync, 5);
    let err =
        store.put(&key("environments/p"), &env_doc("A"), &Precondition::Create, &meta("a", "")).await.unwrap_err();
    assert!(matches!(&err, StoreError::Unavailable(t) if t.contains(&fx.docs().display().to_string())), "{err:?}");
    // Nothing is half written: a retry works.
    store.put(&key("environments/p"), &env_doc("A"), &Precondition::Create, &meta("a", "")).await.unwrap();
    assert!(faults.fsync_calls() >= 4);
}
