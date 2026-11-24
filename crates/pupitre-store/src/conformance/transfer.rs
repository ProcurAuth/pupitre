// SPDX-License-Identifier: MIT OR Apache-2.0
//! The case S19 of 2.43: an export from one backend and an import into another.

use std::collections::BTreeMap;

use pupitre_core::doc::DocKey;
use pupitre_core::etag::Etag;
use pupitre_core::ids::KeyPath;
use pupitre_core::node::SecretScope;
use pupitre_core::run::{RunMode, RunRecord, RunStatus};

use super::harness::{FreshOptions, Harness};
use super::support::{
    common_key, env_doc, env_key, env_scope, meta, numbered_doc, other_key, run_record, test_key, unit_scope,
    with_status,
};
use crate::secret::{SecretValue, StoreKeys};
use crate::store::{AnyStore, Store};
use crate::transfer::{ImportReport, SecretAction, copy_store};
use crate::types::{Precondition, RunFilter, SecretMeta, StoreError, StoredDoc, Version};

/// One run with its output and its numbered event lines.
type CapturedRun = (RunRecord, Vec<u8>, Vec<(u64, Vec<u8>)>);

/// Everything that a store holds, read through its public methods.
#[derive(Debug, PartialEq)]
struct Captured {
    docs: Vec<StoredDoc>,
    versions: BTreeMap<DocKey, Vec<Version>>,
    secrets: Vec<(SecretScope, KeyPath, Vec<u8>)>,
    secret_meta: Vec<SecretMeta>,
    runs: Vec<CapturedRun>,
}

async fn capture(store: &AnyStore, scopes: &[SecretScope]) -> Captured {
    let mut docs = store.load().await.expect("load").docs;
    docs.sort_by(|a, b| a.key.cmp(&b.key));
    let mut exporter = store.export_begin().await.expect("export");
    let keys = exporter.keys().await.expect("keys");
    exporter.end().await.expect("end");
    let mut versions = BTreeMap::new();
    for key in keys {
        let metas = store.history(&key, None, 1000).await.expect("history");
        let mut all = Vec::new();
        for m in metas.iter().rev() {
            all.push(store.version(&key, m.version).await.expect("version"));
        }
        versions.insert(key, all);
    }
    let mut secrets = Vec::new();
    let mut secret_meta = Vec::new();
    for scope in scopes {
        for (s, n, v) in store.read_secrets(std::slice::from_ref(scope)).await.expect("read") {
            secrets.push((s, n, v.expose().to_vec()));
        }
        secret_meta.extend(store.list_secrets(scope).await.expect("list"));
    }
    let mut runs = Vec::new();
    for record in store.list_runs(&RunFilter::default(), None, 1000).await.expect("runs") {
        let output = store.read_output(&record.id, 0, 1 << 20).await.expect("output").bytes;
        let events = store.read_events(&record.id, 0, 1000).await.expect("events");
        runs.push((record, output, events.into_iter().map(|e| (e.seq, e.bytes)).collect()));
    }
    Captured { docs, versions, secrets, secret_meta, runs }
}

fn scopes() -> Vec<SecretScope> {
    vec![env_scope("production"), unit_scope("production", "relay"), unit_scope("staging", "relay")]
}

/// Fill a store with documents, versions, a delete, secrets with events and runs with output.
async fn populate(store: &AnyStore) -> ImportReport {
    let mut expected = ImportReport::default();
    // Three versions of one key.
    let key = env_key("production");
    let (v1, v2, v3) = (env_doc("One"), env_doc("Two"), env_doc("Three"));
    store.put(&key, &v1, &Precondition::Create, &meta("alice", "first")).await.expect("create");
    store.put(&key, &v2, &Precondition::Match(v1.etag), &meta("bob", "second")).await.expect("save");
    store.put(&key, &v3, &Precondition::Match(v2.etag), &meta("carol", "line one\nline two")).await.expect("save");
    // A key that was deleted and created again, and a key that is deleted.
    let common = common_key("production");
    let c1 = numbered_doc(1);
    store.put(&common, &c1, &Precondition::Create, &meta("alice", "")).await.expect("create");
    store.delete(&common, &c1.etag, &meta("alice", "removed")).await.expect("delete");
    store.put(&common, &numbered_doc(2), &Precondition::Create, &meta("alice", "again")).await.expect("create");
    let gone = common_key("staging");
    let g1 = numbered_doc(9);
    store.put(&gone, &g1, &Precondition::Create, &meta("alice", "")).await.expect("create");
    store.delete(&gone, &g1.etag, &meta("alice", "")).await.expect("delete");
    store
        .put(&env_key("staging"), &env_doc("Staging"), &Precondition::Create, &meta("alice", ""))
        .await
        .expect("create");
    expected.keys = 4;
    expected.versions = 3 + 3 + 2 + 1;
    // Secrets with events.
    let (unit, env) = (unit_scope("production", "relay"), env_scope("production"));
    let password = KeyPath::parse("storage.password").expect("a key path");
    let value = |t: &str| SecretValue::new(t.as_bytes().to_vec());
    store.put_secret(&unit, &password, &value("first-unit-secret"), &meta("alice", "")).await.expect("secret");
    store.put_secret(&unit, &password, &value("second-unit-secret"), &meta("bob", "")).await.expect("secret");
    store.put_secret(&env, &password, &value("environment-secret"), &meta("carol", "")).await.expect("secret");
    let token = KeyPath::parse("storage.token").expect("a key path");
    store.put_secret(&unit, &token, &value("short-lived-token"), &meta("carol", "")).await.expect("secret");
    store.delete_secret(&unit, &token, &meta("carol", "")).await.expect("secret");
    expected.secrets = 2;
    // Runs: a check that an apply consumed, a running run with output, a queued run.
    let check = run_record("production", RunMode::Check);
    store.create_run(&check, 10, None).await.expect("run");
    let running = with_status(&check, RunStatus::Running);
    store.update_run(RunStatus::Queued, &running).await.expect("run");
    let passed = with_status(&running, RunStatus::Passed);
    store.update_run(RunStatus::Running, &passed).await.expect("run");
    let first = b"PLAY [all]\n";
    store.append_output(&check.id, 0, first).await.expect("output");
    let offset = u64::try_from(first.len()).expect("a small length");
    store.append_output(&check.id, offset, b"ok: [host-a]\n\xe2\x82\xac\n").await.expect("output");
    store.append_events(&check.id, 1, &[b"{\"event\":\"start\"}".to_vec()]).await.expect("events");
    store
        .append_events(&check.id, 2, &[b"{\"event\":\"stats\"}".to_vec(), b"{\"event\":\"end\"}".to_vec()])
        .await
        .expect("events");
    let apply = run_record("production", RunMode::Apply);
    store.create_run(&apply, 10, Some(&check.id)).await.expect("run");
    let live = run_record("staging", RunMode::Check);
    store.create_run(&live, 10, None).await.expect("run");
    store.update_run(RunStatus::Queued, &with_status(&live, RunStatus::Running)).await.expect("run");
    store.append_output(&live.id, 0, b"half").await.expect("output");
    expected.runs = 3;
    expected
}

/// S19: an export from the backend of `from` and an import into the backend of `to` give the same
/// etags, versions, metadata, secret values, runs, output and events. The target has its own key:
/// the values are the same, and the fingerprints are the ones of the key of the target. The
/// copy back into a store of the first backend gives the first state again. A target that is
/// not empty stops the copy before it writes.
pub async fn s19_transfer<A: Harness, B: Harness>(from: &A, to: &B) {
    let source = from.fresh(FreshOptions::default()).await.expect("the source");
    let wanted = populate(&source).await;
    let before = capture(&source, &scopes()).await;
    let target_key = other_key();
    let target =
        to.fresh(FreshOptions { secret_key: target_key.clone(), ..FreshOptions::default() }).await.expect("the target");
    let report = copy_store(&source, &target).await.expect("copy");
    assert_eq!(report, wanted, "the counts of the copy ({} to {})", from.name(), to.name());
    let after = capture(&target, &scopes()).await;
    assert_eq!(after.docs, before.docs, "documents, etags and versions");
    assert_eq!(after.versions, before.versions, "each version with its bytes and its metadata");
    assert_eq!(after.secrets, before.secrets, "the values of the secrets");
    assert_eq!(after.runs, before.runs, "runs, output and events");
    // The metadata of the secrets: the fingerprint is the one of the key of the target.
    assert_eq!(after.secret_meta.len(), before.secret_meta.len());
    let target_keys = StoreKeys::derive(&target_key).expect("the keys");
    for (got, was) in after.secret_meta.iter().zip(&before.secret_meta) {
        assert_eq!((&got.scope, &got.name, &got.updated_by), (&was.scope, &was.name, &was.updated_by));
        assert_eq!(got.updated_at, was.updated_at);
        let value = after.secrets.iter().find(|(s, n, _)| *s == got.scope && *n == got.name).expect("the value");
        assert_eq!(got.fingerprint, target_keys.fingerprint(&got.scope, &got.name, &SecretValue::new(value.2.clone())));
        assert_ne!(got.fingerprint, was.fingerprint, "another key gives another fingerprint");
    }
    // The events of a secret come with it: two writes of one secret, two events.
    let mut exporter = target.export_begin().await.expect("export");
    let secrets = exporter.secrets().await.expect("secrets");
    exporter.end().await.expect("end");
    let unit = secrets.iter().find(|s| s.scope == unit_scope("production", "relay")).expect("the secret");
    assert_eq!(unit.events.iter().map(|e| e.by.as_str()).collect::<Vec<_>>(), vec!["alice", "bob"]);
    // L10: the events of a deleted secret go with the transfer, with no value and no row of the secret.
    let token = KeyPath::parse("storage.token").expect("a key path");
    let deleted = secrets
        .iter()
        .find(|s| s.scope == unit_scope("production", "relay") && s.name == token)
        .expect("the deleted secret keeps its events");
    assert!(deleted.value.is_none());
    assert_eq!(
        deleted.events.iter().map(|e| (e.action, e.by.as_str())).collect::<Vec<_>>(),
        vec![(SecretAction::Set, "carol"), (SecretAction::Delete, "carol")]
    );
    assert_eq!(secrets.len(), 3, "two live secrets and one deleted secret");
    assert_eq!(
        target.list_secrets(&unit_scope("production", "relay")).await.expect("list").len(),
        1,
        "the deleted secret is not listed"
    );
    // A target that is not empty stops the copy and keeps its state.
    let again = copy_store(&source, &target).await;
    assert_eq!(again, Err(StoreError::NotEmpty), "{} to {}", from.name(), to.name());
    assert_eq!(capture(&target, &scopes()).await.docs, before.docs);
    // The copy back: the same state, with the first key.
    let back = from.fresh(FreshOptions { secret_key: test_key(), ..FreshOptions::default() }).await.expect("back");
    copy_store(&target, &back).await.expect("copy back");
    assert_eq!(capture(&back, &scopes()).await, before, "the round trip keeps everything");
    // A later write on the copy goes on from the version numbers of the source.
    let key = env_key("production");
    let current: Etag = target.get(&key).await.expect("get").etag;
    let saved =
        target.put(&key, &env_doc("Four"), &Precondition::Match(current), &meta("dave", "")).await.expect("save");
    assert_eq!(saved.version, 4);
}
