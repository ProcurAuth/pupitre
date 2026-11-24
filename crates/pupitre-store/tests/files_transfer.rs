// SPDX-License-Identifier: MIT OR Apache-2.0
//! The export and the import of the backend `files` (2.40), and the case S19 between two
//! stores of this backend. The transfer between `files` and `postgres` is in the tests of the
//! transfer module.

mod files_support;

use files_support::*;
use pupitre_core::doc::DocKind;
use pupitre_core::run::RunStatus;
use pupitre_store::files::{DocumentsWriter, FilesExporter, FilesImporter, FilesStore};
use pupitre_store::transfer::{Exporter, Importer, KeyHistory};
use pupitre_store::{Precondition, Store, StoreError};

fn exporter(e: Exporter) -> FilesExporter {
    match e {
        Exporter::Files(f) => f,
        Exporter::Postgres(_) => panic!("a files store gives a files exporter"),
    }
}

fn importer(i: Importer) -> FilesImporter {
    match i {
        Importer::Files(f) => f,
        Importer::Postgres(_) => panic!("a files store gives a files importer"),
    }
}

/// A store with versions, a tombstone, secrets and runs.
async fn source(fx: &Fixture) -> FilesStore {
    let store = fx.open();
    let m = meta("alice", "m");
    let env_a = env_doc("A");
    let env_b = env_doc("B");
    store.put(&key("environments/p"), &env_a, &Precondition::Create, &m).await.unwrap();
    store.put(&key("environments/p"), &env_b, &Precondition::Match(env_a.etag), &meta("bob", "second")).await.unwrap();
    let host = host_doc("host.example.invalid");
    store.put(&key("environments/p/hosts/host-a"), &host, &Precondition::Create, &m).await.unwrap();
    let gone = key("environments/p/hosts/host-b");
    let hd = host_doc("other.example.invalid");
    store.put(&gone, &hd, &Precondition::Create, &m).await.unwrap();
    store.delete(&gone, &hd.etag, &meta("bob", "removed")).await.unwrap();
    store.put(&key("environments/p/units/relay"), &unit_doc(), &Precondition::Create, &m).await.unwrap();
    store
        .put(
            &key("environments/p/units/relay/values"),
            &values_doc(DocKind::UnitValues, r#"{"limits.max_clients": 5}"#),
            &Precondition::Create,
            &m,
        )
        .await
        .unwrap();
    let scope = unit_scope("p", "relay");
    store.put_secret(&scope, &name("storage.password"), &value("first value 123"), &meta("carol", "")).await.unwrap();
    store.put_secret(&scope, &name("storage.password"), &value("second value 123"), &meta("dave", "")).await.unwrap();
    store
        .put_secret(
            &pupitre_core::node::SecretScope::Environment(env("p")),
            &name("api.token"),
            &value("token 12345678"),
            &meta("a", ""),
        )
        .await
        .unwrap();
    for i in 0..3u8 {
        let id = run_id(i as u64, i);
        store.create_run(&run_record(id, "p", RunStatus::Queued), 10, None).await.unwrap();
        store.append_output(&id, 0, format!("output {i}\n").as_bytes()).await.unwrap();
        store.append_events(&id, 1, &[b"{\"n\":1}".to_vec(), b"{\"n\":2}".to_vec()]).await.unwrap();
    }
    let mut done = run_record(run_id(0, 0), "p", RunStatus::Running);
    store.update_run(RunStatus::Queued, &done).await.unwrap();
    done.status = RunStatus::Passed;
    store.update_run(RunStatus::Running, &done).await.unwrap();
    store
}

async fn copy(from: &FilesStore, to: &FilesStore) -> pupitre_store::transfer::ImportReport {
    let mut ex = exporter(from.export_begin().await.unwrap());
    let mut im = importer(to.import_begin().await.unwrap());
    for k in ex.keys().await.unwrap() {
        let history = ex.key(&k).await.unwrap();
        im.key(&history).await.unwrap();
    }
    for s in ex.secrets().await.unwrap() {
        im.secret(&s).await.unwrap();
    }
    for id in ex.run_ids().await.unwrap() {
        let run = ex.run(&id).await.unwrap();
        im.run(&run).await.unwrap();
    }
    ex.end().await.unwrap();
    im.commit().await.unwrap()
}

#[tokio::test]
async fn s19_an_export_and_an_import_give_the_same_etags_and_versions() {
    let fa = Fixture::new();
    let fb = Fixture { key: KEY_B, ..Fixture::new() };
    let from = source(&fa).await;
    let to = fb.open();
    let report = copy(&from, &to).await;
    assert_eq!((report.keys, report.secrets, report.runs), (5, 2, 3));
    assert_eq!(report.versions, 2 + 1 + 2 + 1 + 1);
    let mut ex = exporter(from.export_begin().await.unwrap());
    for k in ex.keys().await.unwrap() {
        let a = ex.key(&k).await.unwrap();
        let mut ex_b = exporter(to.export_begin().await.unwrap());
        let b = ex_b.key(&k).await.unwrap();
        assert_eq!(a, b, "{k}");
        // The current etags are the same.
        match (from.get(&k).await, to.get(&k).await) {
            (Ok(x), Ok(y)) => assert_eq!((x.etag, x.version, x.bytes), (y.etag, y.version, y.bytes), "{k}"),
            (Err(StoreError::NotFound), Err(StoreError::NotFound)) => {}
            other => panic!("{k}: {other:?}"),
        }
        assert_eq!(from.history(&k, None, 100).await.unwrap(), to.history(&k, None, 100).await.unwrap());
    }
    // The secrets have the same values and events. The fingerprints use the key of each store.
    let scope = unit_scope("p", "relay");
    let read_a = from.read_secrets(std::slice::from_ref(&scope)).await.unwrap();
    let read_b = to.read_secrets(std::slice::from_ref(&scope)).await.unwrap();
    assert_eq!(read_a[0].2.expose(), read_b[0].2.expose());
    let fp_a = from.list_secrets(&scope).await.unwrap()[0].clone();
    let fp_b = to.list_secrets(&scope).await.unwrap()[0].clone();
    assert_ne!(fp_a.fingerprint, fp_b.fingerprint);
    assert_eq!((fp_a.updated_at, &fp_a.updated_by), (fp_b.updated_at, &fp_b.updated_by));
    let mut ex_b = exporter(to.export_begin().await.unwrap());
    let sa = exporter(from.export_begin().await.unwrap()).secrets().await.unwrap();
    assert_eq!(sa, ex_b.secrets().await.unwrap());
    assert_eq!(sa.iter().find(|s| s.name.as_str() == "storage.password").unwrap().events.len(), 2);
    // The runs have the same records, output and events.
    for id in ex.run_ids().await.unwrap() {
        assert_eq!(ex.run(&id).await.unwrap(), ex_b.run(&id).await.unwrap());
        assert_eq!(from.get_run(&id).await.unwrap(), to.get_run(&id).await.unwrap());
    }
    // The target works after a restart, and the numbers of the versions go on. An exporter
    // holds the locks of its store, so the exporters end first.
    ex.end().await.unwrap();
    ex_b.end().await.unwrap();
    drop(to);
    let to = fb.reopen();
    let k = key("environments/p");
    let current = to.get(&k).await.unwrap();
    assert_eq!(current.version, 2);
    let saved = to.put(&k, &env_doc("C"), &Precondition::Match(current.etag), &meta("a", "")).await.unwrap();
    assert_eq!(saved.version, 3);
    let tomb = key("environments/p/hosts/host-b");
    assert_eq!(
        to.put(&tomb, &host_doc("x.example.invalid"), &Precondition::Create, &meta("a", "")).await.unwrap().version,
        3
    );
    assert_eq!(to.append_output(&run_id(1, 1), 9, b"more").await.unwrap(), 13);
    assert_eq!(to.append_events(&run_id(1, 1), 3, &[b"{}".to_vec()]).await.unwrap(), ());
}

#[tokio::test]
async fn the_numbers_of_pruned_versions_stay() {
    let fa = Fixture::with_history_keep(3);
    let from = fa.open();
    let k = key("environments/p");
    let mut last = env_doc("L0");
    from.put(&k, &last, &Precondition::Create, &meta("a", "")).await.unwrap();
    for i in 1..8 {
        let d = env_doc(&format!("L{i}"));
        from.put(&k, &d, &Precondition::Match(last.etag), &meta("a", "")).await.unwrap();
        last = d;
    }
    let fb = Fixture::with_history_keep(3);
    let to = fb.open();
    copy(&from, &to).await;
    let versions: Vec<u64> = to.history(&k, None, 10).await.unwrap().iter().map(|v| v.version).collect();
    assert_eq!(versions, [8, 7, 6]);
    assert_eq!(to.get(&k).await.unwrap().etag, last.etag);
}

#[tokio::test]
async fn an_import_needs_an_empty_target() {
    let fa = Fixture::new();
    let from = source(&fa).await;
    for what in ["document", "secret", "run"] {
        let fb = Fixture::new();
        let to = fb.open();
        match what {
            "document" => {
                to.put(&key("environments/q"), &env_doc("Q"), &Precondition::Create, &meta("a", "")).await.unwrap();
            }
            "secret" => {
                to.put_secret(&unit_scope("q", "r"), &name("a.b"), &value("12345678"), &meta("a", "")).await.unwrap();
            }
            _ => {
                to.create_run(&run_record(run_id(0, 1), "q", RunStatus::Queued), 10, None).await.unwrap();
            }
        }
        assert!(matches!(to.import_begin().await, Err(StoreError::NotEmpty)), "{what}");
        // The target is the same after the refusal.
        assert!(from.export_begin().await.is_ok());
    }
    // A document that a person put in the folder also counts.
    let fb = Fixture::new();
    let to = fb.open();
    std::fs::create_dir_all(fb.docs().join("environments/q")).unwrap();
    std::fs::write(fb.docs().join("environments/q/environment.json"), b"not a document").unwrap();
    // The open found no such file: the import_begin looks again.
    assert!(matches!(to.import_begin().await, Err(StoreError::NotEmpty)));
}

#[tokio::test]
async fn an_import_refuses_a_history_that_does_not_hold_together() {
    let fa = Fixture::new();
    let from = source(&fa).await;
    let mut ex = exporter(from.export_begin().await.unwrap());
    let good: KeyHistory = ex.key(&key("environments/p")).await.unwrap();
    let cases: Vec<(&str, KeyHistory)> = vec![
        ("versions in a wrong order", {
            let mut h = good.clone();
            h.versions.reverse();
            h
        }),
        ("an etag that is not the hash of the bytes", {
            let mut h = good.clone();
            h.versions[0].meta.etag = Some(pupitre_core::etag::Etag::of(b"other"));
            h
        }),
        ("a current document that is not the last version", {
            let mut h = good.clone();
            h.current.as_mut().unwrap().version = 1;
            h
        }),
        ("a key with no document and no delete", {
            let mut h = good.clone();
            h.current = None;
            h
        }),
    ];
    for (why, history) in cases {
        let fb = Fixture::new();
        let to = fb.open();
        let mut im = importer(to.import_begin().await.unwrap());
        let err = im.key(&history).await.unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)), "{why}: {err:?}");
        assert!(!fb.docs().join("environments/p/environment.json").exists(), "{why}");
    }
    // The same key two times.
    let fb = Fixture::new();
    let to = fb.open();
    let mut im = importer(to.import_begin().await.unwrap());
    im.key(&good).await.unwrap();
    assert_eq!(im.key(&good).await.unwrap_err(), StoreError::AlreadyExists);
}

#[tokio::test]
async fn the_writer_of_a_folder_of_documents_makes_the_layout_with_no_secret_and_no_run() {
    let fa = Fixture::new();
    let from = source(&fa).await;
    let fx = Fixture::new();
    let target = fx.dir.path().join("review");
    let mut writer = DocumentsWriter::create(&target).unwrap();
    let mut ex = exporter(from.export_begin().await.unwrap());
    for k in ex.keys().await.unwrap() {
        writer.write_key(&ex.key(&k).await.unwrap()).unwrap();
    }
    let report = writer.finish().unwrap();
    assert_eq!((report.keys, report.secrets, report.runs), (5, 0, 0));
    let files = files_below(&target);
    assert!(files.contains(&"pupitre-store.json".to_string()));
    assert!(
        files.iter().all(|f| !f.starts_with("values/") && !f.contains(".lock") && !f.contains(".tmp")),
        "{files:?}"
    );
    assert!(target.join("environments/p/environment.json").exists());
    assert!(!target.join(".tmp").exists());
    // The files are the files of the source, byte for byte.
    for f in files_below(&fa.docs()) {
        if f.starts_with("environments/") || f.starts_with(".history/") {
            assert_eq!(std::fs::read(target.join(&f)).unwrap(), std::fs::read(fa.docs().join(&f)).unwrap(), "{f}");
        }
    }
    // A non-empty folder is refused, and an empty one is accepted.
    assert!(matches!(DocumentsWriter::create(&target), Err(StoreError::NotEmpty)));
    let empty = fx.dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(DocumentsWriter::create(&empty).is_ok());
    // The folder is a store: `migrate` adds the secrets and the runs, and the open works.
    let mut options = fx.options();
    options.path = target.clone();
    FilesStore::migrate(&options).unwrap();
    let store = FilesStore::open(options).unwrap();
    assert_eq!(store.get(&key("environments/p")).await.unwrap().version, 2);
    assert_eq!(store.history(&key("environments/p"), None, 10).await.unwrap().len(), 2);
    assert!(store.drain_events().iter().all(|e| !matches!(e, pupitre_store::StoreEvent::ExternalChange { .. })));
}

#[tokio::test]
async fn put_import_makes_a_version_of_the_kind_import() {
    let fx = Fixture::new();
    let store = fx.open();
    let k = key("environments/p");
    let d = env_doc("P");
    let saved = store.put_import(&k, &d, &meta("import", "inventory")).await.unwrap();
    assert_eq!((saved.version, saved.etag), (1, Some(d.etag)));
    let v = &store.history(&k, None, 10).await.unwrap()[0];
    assert_eq!((v.kind, v.meta.saved_by.as_str()), (pupitre_store::VersionKind::Import, "import"));
    // The rules of a create: a key with a document is refused, and a later save is a `save`.
    assert_eq!(store.put_import(&k, &d, &meta("import", "")).await.unwrap_err(), StoreError::AlreadyExists);
    let d2 = env_doc("P2");
    store.put(&k, &d2, &Precondition::Match(d.etag), &meta("a", "")).await.unwrap();
    assert_eq!(store.history(&k, None, 1).await.unwrap()[0].kind, pupitre_store::VersionKind::Save);
}
