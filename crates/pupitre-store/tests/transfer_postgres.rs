// SPDX-License-Identifier: MIT OR Apache-2.0
//! 2.40 on PostgreSQL: the transfer. The backend `files` is not part of this branch, so a small
//! store in memory stands for it: it implements the same traits `ExportSource` and `ImportSink`
//! that the backend `files` uses. The tests of the transfer between two real backends are S19.
#![cfg(feature = "conformance")]

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use common::{doc, env_key, meta, ready};
use pupitre_core::doc::DocKey;
use pupitre_core::etag::Etag;
use pupitre_core::ids::Subject;
use pupitre_core::ids::{EnvId, KeyPath, RunId, UnitId};
use pupitre_core::node::SecretScope;
use pupitre_core::time::UtcMillis;
use pupitre_store::secret::SecretValue;
use pupitre_store::transfer::{
    Expected, ExportSource, ImportReport, ImportSink, KeyHistory, RunExport, SecretAction, SecretEvent, SecretExport,
    compare_current, copy_store, export_to_dir, transfer,
};
use pupitre_store::{AnyStore, Message, Precondition, Store, StoreError, Version, VersionKind, VersionMeta, WriteMeta};

/// The state of the store in memory.
#[derive(Debug, Clone, Default, PartialEq)]
struct Memory {
    keys: Vec<KeyHistory>,
    secrets: Vec<SecretExport>,
    runs: Vec<RunExport>,
}

impl ExportSource for Memory {
    async fn keys(&mut self) -> Result<Vec<DocKey>, StoreError> {
        Ok(self.keys.iter().map(|k| k.key.clone()).collect())
    }
    async fn key(&mut self, key: &DocKey) -> Result<KeyHistory, StoreError> {
        self.keys.iter().find(|k| k.key == *key).cloned().ok_or(StoreError::NotFound)
    }
    async fn secrets(&mut self) -> Result<Vec<SecretExport>, StoreError> {
        Ok(self.secrets.clone())
    }
    async fn run_ids(&mut self) -> Result<Vec<RunId>, StoreError> {
        Ok(self.runs.iter().map(|r| r.record.id).collect())
    }
    async fn run(&mut self, id: &RunId) -> Result<RunExport, StoreError> {
        self.runs.iter().find(|r| r.record.id == *id).cloned().ok_or(StoreError::NotFound)
    }
    async fn end(self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// A sink in memory. It keeps what it gets, and gives the counts on `commit`.
#[derive(Debug, Default)]
struct MemorySink {
    got: Memory,
    fail_on_key: Option<String>,
}

impl ImportSink for MemorySink {
    async fn key(&mut self, history: &KeyHistory) -> Result<(), StoreError> {
        if self.fail_on_key.as_deref() == Some(history.key.text().as_str()) {
            return Err(StoreError::Unavailable("a fault of the test".to_string()));
        }
        self.got.keys.push(history.clone());
        Ok(())
    }
    async fn secret(&mut self, secret: &SecretExport) -> Result<(), StoreError> {
        self.got.secrets.push(secret.clone());
        Ok(())
    }
    async fn run(&mut self, run: &RunExport) -> Result<(), StoreError> {
        self.got.runs.push(run.clone());
        Ok(())
    }
    async fn commit(self) -> Result<ImportReport, StoreError> {
        Ok(ImportReport {
            keys: self.got.keys.len() as u64,
            versions: self.got.keys.iter().map(|k| k.versions.len() as u64).sum(),
            secrets: self.got.secrets.len() as u64,
            runs: self.got.runs.len() as u64,
        })
    }
}

fn wmeta(by: &str, at: u64, message: &str) -> WriteMeta {
    WriteMeta {
        saved_by: Subject::parse(by).expect("subject"),
        saved_at: UtcMillis(at),
        message: Message::parse(message).expect("message"),
    }
}

fn version(n: u64, kind: VersionKind, bytes: Option<&[u8]>, by: &str, at: u64) -> Version {
    Version {
        meta: VersionMeta { version: n, kind, etag: bytes.map(Etag::of), meta: wmeta(by, at, "") },
        bytes: bytes.map(<[u8]>::to_vec),
    }
}

/// A state like the one of a folder after `history_keep`: the oldest versions are gone, a key
/// has a version `external`, and a key has no current document.
fn files_like() -> Memory {
    let (a, b) = (doc("One"), doc("Two"));
    let current = |key: &DocKey, d: &pupitre_core::doc::CanonicalDocument, n: u64| pupitre_store::StoredDoc {
        key: key.clone(),
        bytes: d.bytes.clone(),
        etag: d.etag,
        version: n,
    };
    let (k1, k2, k3) =
        (env_key("production"), env_key("staging"), DocKey::Common(EnvId::parse("staging").expect("id")));
    let unit = SecretScope::Unit(EnvId::parse("production").expect("id"), UnitId::parse("relay").expect("id"));
    let mut memory = Memory::default();
    memory.keys.push(KeyHistory {
        key: k1.clone(),
        versions: vec![
            version(7, VersionKind::Save, Some(&a.bytes), "alice", 1_795_514_400_000),
            version(8, VersionKind::External, Some(&b.bytes), "external", 1_795_514_401_000),
        ],
        current: Some(current(&k1, &b, 8)),
    });
    memory.keys.push(KeyHistory {
        key: k2.clone(),
        versions: vec![version(1, VersionKind::Import, Some(&a.bytes), "import", 1_795_514_402_000)],
        current: Some(current(&k2, &a, 1)),
    });
    memory.keys.push(KeyHistory {
        key: k3,
        versions: vec![
            version(4, VersionKind::Create, Some(&a.bytes), "alice", 1_795_514_403_000),
            version(5, VersionKind::Delete, None, "bob", 1_795_514_404_000),
        ],
        current: None,
    });
    memory.secrets.push(SecretExport {
        scope: unit,
        name: KeyPath::parse("storage.password").expect("path"),
        value: Some(SecretValue::new(b"a-secret-value".to_vec())),
        events: vec![
            SecretEvent {
                action: SecretAction::Set,
                by: Subject::parse("alice").expect("s"),
                at: UtcMillis(1_795_514_405_000),
            },
            SecretEvent {
                action: SecretAction::Set,
                by: Subject::parse("bob").expect("s"),
                at: UtcMillis(1_795_514_406_000),
            },
        ],
    });
    memory
}

/// A state that has a run with output and events.
fn with_run(mut memory: Memory) -> Memory {
    use pupitre_core::run::RunStatus;
    let id = RunId::from_parts(1_795_514_400_000, [1; 10]);
    let mut record = run_record(id);
    record.status = RunStatus::Passed;
    memory.runs.push(RunExport {
        record,
        output: b"output of the run\n".to_vec(),
        events: vec![
            pupitre_store::EventLine { seq: 1, bytes: b"{\"a\":1}".to_vec() },
            pupitre_store::EventLine { seq: 2, bytes: b"{\"a\":2}".to_vec() },
        ],
    });
    memory
}

fn run_record(id: RunId) -> pupitre_core::run::RunRecord {
    use pupitre_core::ids::{HostId, PlaybookId};
    use pupitre_core::node::Node;
    use pupitre_core::run::{RunKind, RunMode, RunStatus};
    pupitre_core::run::RunRecord {
        id,
        kind: RunKind::Deploy,
        mode: RunMode::Check,
        environment: EnvId::parse("production").expect("id"),
        node: Node::Environment(EnvId::parse("production").expect("id")),
        hosts: vec![HostId::parse("host-a").expect("id")],
        playbook: PlaybookId::parse("deploy-relay").expect("id"),
        command: None,
        params: None,
        check_run: None,
        applied_by: None,
        message: None,
        subject: Subject::parse("alice").expect("s"),
        created_at: UtcMillis(1_795_514_400_000),
        started_at: None,
        finished_at: None,
        status: RunStatus::Queued,
        reason: None,
        snapshot: None,
        runner: None,
        exit_code: None,
        stats: None,
        cancel: None,
        output_truncated: false,
        events_truncated: false,
    }
}

/// A store in memory to PostgreSQL and back: the same versions, bytes, metadata, etags, secrets and runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_store_like_files_goes_to_postgres_and_back() {
    let (_db, store) = ready().await;
    let original = with_run(files_like());
    let store = AnyStore::Postgres(store);
    let (report, expected) =
        transfer(original.clone(), store.import_begin().await.expect("import")).await.expect("transfer");
    assert_eq!(report, ImportReport { keys: 3, versions: 5, secrets: 1, runs: 1 });
    compare_current(&store, &expected).await.expect("the etags are the same");
    // The version numbers and the kinds stay. A later write goes on from them.
    let key = env_key("production");
    let history = store.history(&key, None, 10).await.expect("history");
    assert_eq!(
        history.iter().map(|v| (v.version, v.kind)).collect::<Vec<_>>(),
        vec![(8, VersionKind::External), (7, VersionKind::Save)]
    );
    let next = doc("Three");
    let current = store.get(&key).await.expect("get");
    assert_eq!(
        store.put(&key, &next, &Precondition::Match(current.etag), &meta("carol")).await.expect("save").version,
        9
    );
    // A key with no current document keeps its numbers: a create gives the next number.
    let deleted = DocKey::Common(EnvId::parse("staging").expect("id"));
    assert_eq!(store.get(&deleted).await, Err(StoreError::NotFound));
    assert_eq!(
        store.put(&deleted, &doc("New"), &Precondition::Create, &meta("carol")).await.expect("create").version,
        6
    );
    // Back to memory: the export of PostgreSQL gives the state again (before the two writes above).
    let (_db2, fresh) = ready().await;
    let fresh = AnyStore::Postgres(fresh);
    transfer(original.clone(), fresh.import_begin().await.expect("import")).await.expect("transfer");
    let mut exporter = fresh.export_begin().await.expect("export");
    let mut back = Memory::default();
    for key in exporter.keys().await.expect("keys") {
        back.keys.push(exporter.key(&key).await.expect("key"));
    }
    back.secrets = exporter.secrets().await.expect("secrets");
    for id in exporter.run_ids().await.expect("ids") {
        back.runs.push(exporter.run(&id).await.expect("run"));
    }
    exporter.end().await.expect("end");
    let mut want = original;
    want.keys.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(back, want);
}

/// 2.40: the target MUST be empty. Else the import stops before it writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn an_import_into_a_store_that_is_not_empty_stops_before_it_writes() {
    for fill in ["document", "secret", "run"] {
        let (db, store) = ready().await;
        match fill {
            "document" => {
                store
                    .put(&env_key("production"), &doc("One"), &Precondition::Create, &meta("alice"))
                    .await
                    .expect("create");
            }
            "secret" => {
                let scope = SecretScope::Environment(EnvId::parse("production").expect("id"));
                let name = KeyPath::parse("storage.password").expect("path");
                store
                    .put_secret(&scope, &name, &SecretValue::new(b"a-secret-value".to_vec()), &meta("a"))
                    .await
                    .expect("secret");
            }
            _ => store
                .create_run(&run_record(RunId::from_parts(1_795_514_400_000, [2; 10])), 5, None)
                .await
                .expect("run"),
        }
        let refused = store.import_begin().await;
        assert!(matches!(refused, Err(StoreError::NotEmpty)), "{fill}: {refused:?}");
        // The store keeps working, and nothing was written.
        let admin = db.admin_session().await;
        let n: i64 = admin
            .query_one("SELECT (SELECT count(*) FROM documents) + (SELECT count(*) FROM secrets) + (SELECT count(*) FROM runs)", &[])
            .await
            .expect("count")
            .get(0);
        assert_eq!(n, 1, "{fill}");
    }
}

/// 2.40: an import is one transaction. A failure in the middle leaves the target empty, and a
/// drop before `commit` leaves it empty too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_failed_or_dropped_import_leaves_the_target_empty() {
    let (_db, store) = ready().await;
    let store = AnyStore::Postgres(store);
    let original = files_like();
    let sink_to_pg = store.import_begin().await.expect("import");
    // The source holds a history that is not valid: the second key breaks the rule of 2.18.
    let mut broken = original.clone();
    broken.keys[1].versions[0].meta.etag = Some(Etag::of(b"another"));
    let refused = transfer(broken, sink_to_pg).await;
    assert!(matches!(refused, Err(StoreError::Corrupt(_))), "{refused:?}");
    assert!(store.load().await.expect("load").docs.is_empty());
    // A drop before `commit`.
    let mut importer = store.import_begin().await.expect("import");
    importer.key(&original.keys[0]).await.expect("key");
    drop(importer);
    assert!(store.load().await.expect("load").docs.is_empty());
    let mut again = store.import_begin().await.expect("the target is still empty");
    again.key(&original.keys[0]).await.expect("key");
    let report = again.commit().await.expect("commit");
    assert_eq!(report.keys, 1);
    assert_eq!(store.load().await.expect("load").docs.len(), 1);
}

/// 2.40: the export is a consistent state. A write after the start of the export is not in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_export_is_a_consistent_state() {
    let (_db, store) = ready().await;
    let key = env_key("production");
    let v1 = doc("One");
    store.put(&key, &v1, &Precondition::Create, &meta("alice")).await.expect("create");
    let store = AnyStore::Postgres(store);
    let mut exporter = store.export_begin().await.expect("export");
    assert_eq!(exporter.keys().await.expect("keys"), vec![key.clone()]);
    // A write after the start, by another connection.
    store.put(&key, &doc("Two"), &Precondition::Match(v1.etag), &meta("bob")).await.expect("save");
    store.put(&env_key("staging"), &doc("Three"), &Precondition::Create, &meta("bob")).await.expect("create");
    let seen = exporter.key(&key).await.expect("key");
    assert_eq!(seen.versions.len(), 1, "the later version is not in the export");
    assert_eq!(seen.current.expect("current").etag, v1.etag);
    assert_eq!(exporter.keys().await.expect("keys").len(), 1);
    exporter.end().await.expect("end");
    assert_eq!(store.load().await.expect("load").docs.len(), 2);
}

/// 2.40: the comparison after a copy stops with an error for a missing, an extra or a different document.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_comparison_finds_a_difference() {
    let (_db, store) = ready().await;
    let store = AnyStore::Postgres(store);
    let d = doc("One");
    store.put(&env_key("production"), &d, &Precondition::Create, &meta("alice")).await.expect("create");
    let mut expected: Expected = BTreeMap::new();
    expected.insert(env_key("production"), (d.etag, 1));
    compare_current(&store, &expected).await.expect("equal");
    let mut other_etag = expected.clone();
    other_etag.insert(env_key("production"), (Etag::of(b"x"), 1));
    assert!(matches!(compare_current(&store, &other_etag).await, Err(StoreError::Corrupt(_))));
    let mut other_version = expected.clone();
    other_version.insert(env_key("production"), (d.etag, 2));
    assert!(matches!(compare_current(&store, &other_version).await, Err(StoreError::Corrupt(_))));
    let mut missing = expected.clone();
    missing.insert(env_key("staging"), (d.etag, 1));
    assert!(matches!(compare_current(&store, &missing).await, Err(StoreError::Corrupt(_))));
    assert!(matches!(compare_current(&store, &BTreeMap::new()).await, Err(StoreError::Corrupt(_))));
}

/// 2.40: `export` writes the documents and their versions into an empty folder in the layout
/// of 2.31, and no secret and no run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn export_writes_the_layout_of_2_31() {
    let (_db, store) = ready().await;
    let key = env_key("production");
    let (v1, v2) = (doc("One"), doc("Two"));
    store.put(&key, &v1, &Precondition::Create, &wmeta("alice", 1_795_514_400_123, "first")).await.expect("create");
    store
        .put(&key, &v2, &Precondition::Match(v1.etag), &wmeta("bob", 1_795_514_401_000, "line 1\nline 2"))
        .await
        .expect("save");
    let gone = DocKey::Common(EnvId::parse("production").expect("id"));
    let c = doc("Common");
    store.put(&gone, &c, &Precondition::Create, &wmeta("alice", 1_795_514_402_000, "")).await.expect("create");
    store.delete(&gone, &c.etag, &wmeta("alice", 1_795_514_403_000, "removed")).await.expect("delete");
    let scope = SecretScope::Environment(EnvId::parse("production").expect("id"));
    let name = KeyPath::parse("storage.password").expect("path");
    store.put_secret(&scope, &name, &SecretValue::new(b"a-secret-value".to_vec()), &meta("a")).await.expect("secret");
    store.create_run(&run_record(RunId::from_parts(1_795_514_400_000, [3; 10])), 5, None).await.expect("run");
    let store = AnyStore::Postgres(store);
    let dir = tempfile::tempdir().expect("a folder");
    let target = dir.path().join("documents");
    let report = export_to_dir(&store, &target).await.expect("export");
    assert_eq!((report.keys, report.versions, report.secrets, report.runs), (2, 4, 0, 0));
    let read = |p: &str| std::fs::read(target.join(p)).expect(p);
    assert_eq!(read("pupitre-store.json"), b"{\n  \"format\": 1,\n  \"store_schema\": 1\n}\n");
    assert_eq!(read("environments/production/environment.json"), v2.bytes);
    assert!(!target.join("environments/production/common.json").exists(), "a deleted key has no current file");
    assert_eq!(read(".history/environments/production/0000000001.json"), v1.bytes);
    assert_eq!(read(".history/environments/production/0000000002.json"), v2.bytes);
    let meta2 = String::from_utf8(read(".history/environments/production/0000000002.meta.json")).expect("utf8");
    assert_eq!(
        meta2,
        format!(
            "{{\n  \"etag\": \"{}\",\n  \"kind\": \"save\",\n  \"message\": \"line 1\\nline 2\",\n  \"saved_at\": \"2026-11-24T10:00:01.000Z\",\n  \"saved_by\": \"bob\"\n}}\n",
            v2.etag
        )
    );
    // The delete has a metadata file and no bytes.
    let meta_delete =
        String::from_utf8(read(".history/environments/production/common/0000000002.meta.json")).expect("utf8");
    assert_eq!(
        meta_delete,
        "{\n  \"kind\": \"delete\",\n  \"message\": \"removed\",\n  \"saved_at\": \"2026-11-24T10:00:03.000Z\",\n  \"saved_by\": \"alice\"\n}\n"
    );
    assert!(!target.join(".history/environments/production/common/0000000002.json").exists());
    // The modes of 2.32 for the documents folder: files 0644.
    let mode =
        std::fs::metadata(target.join("environments/production/environment.json")).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o644);
    // No secret and no run is in the folder.
    let mut names = Vec::new();
    let mut stack = vec![target.clone()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(d).expect("dir") {
            let entry = entry.expect("entry");
            names.push(entry.path().to_string_lossy().into_owned());
            if entry.path().is_dir() {
                stack.push(entry.path());
            }
        }
    }
    assert!(
        names.iter().all(|n| !n.contains("secret") && !n.contains("run") && !n.contains("storage.password")),
        "{names:?}"
    );
    // A folder that is not empty stops the export, and an empty one works.
    assert_eq!(export_to_dir(&store, &target).await, Err(StoreError::NotEmpty));
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).expect("folder");
    assert!(export_to_dir(&store, &empty).await.is_ok());
}

/// 6.24: `copy-store` between two PostgreSQL stores in two databases.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn copy_store_moves_the_versions_and_keeps_the_etags() {
    let (_a, one) = ready().await;
    let (_b, two) = ready().await;
    let (one, two): (AnyStore, AnyStore) = (AnyStore::Postgres(one), AnyStore::Postgres(two));
    let mut transferred = files_like();
    transferred = with_run(transferred);
    transfer(transferred.clone(), one.import_begin().await.expect("import")).await.expect("fill");
    let report = copy_store(&one, &two).await.expect("copy");
    assert_eq!(report, ImportReport { keys: 3, versions: 5, secrets: 1, runs: 1 });
}

/// The generic move needs no server: it copies the keys, the secrets and the runs, and it keeps
/// the current document of each key for the comparison. A failure of the sink stops it before `commit`.
#[tokio::test]
async fn the_generic_move_copies_everything_and_stops_on_a_failure() {
    let source = with_run(files_like());
    let (report, expected) = transfer(source.clone(), MemorySink::default()).await.expect("transfer");
    assert_eq!(report, ImportReport { keys: 3, versions: 5, secrets: 1, runs: 1 });
    let keys: Vec<String> = expected.keys().map(DocKey::text).collect();
    assert_eq!(
        keys,
        vec!["environments/production", "environments/staging"],
        "a key with no current document is not expected"
    );
    assert_eq!(expected[&env_key("production")].1, 8);
    let failing = MemorySink { fail_on_key: Some("environments/staging".to_string()), ..MemorySink::default() };
    let stopped = transfer(source, failing).await;
    assert_eq!(stopped.map(|(r, _)| r), Err(StoreError::Unavailable("a fault of the test".to_string())));
}
