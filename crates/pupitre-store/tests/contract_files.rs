// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contract suite of 2.43 on the backend `files`. It runs the same cases as the backend
//! `postgres`, with a `Harness` that makes each store in a new temporary folder.
#![cfg(feature = "conformance")]

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_store::conformance::{self as suite, CrashOutcome, FreshOptions, Harness, test_key};
use pupitre_store::files::{FaultPoint, Faults, FilesOptions, FilesStore};
use pupitre_store::secret::SecretKey;
use pupitre_store::{AnyStore, Precondition, Store, StoreError, WriteMeta};
use tempfile::TempDir;

/// One folder for the three storage paths of a store, and the options of the store.
struct Place {
    _dir: TempDir,
    options: FilesOptions,
}

fn place(options: &FreshOptions) -> Place {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let root: PathBuf = dir.path().to_path_buf();
    let options = FilesOptions {
        path: root.join("docs"),
        secrets_path: root.join("secrets"),
        runs_path: root.join("runs"),
        rescan_interval: Duration::from_secs(5),
        history_keep: options.history_keep,
        secret_key: options.secret_key.clone(),
        max_document_bytes: 262_144,
    };
    Place { _dir: dir, options }
}

/// The harness of the backend `files`. It keeps the folders of every store that it made, so a
/// folder stays until the harness ends.
#[derive(Default)]
struct FilesHarness {
    places: Mutex<Vec<Place>>,
    old: Mutex<Option<FilesOptions>>,
}

fn files(store: AnyStore) -> Result<FilesStore, StoreError> {
    match store {
        AnyStore::Files(s) => Ok(s),
        AnyStore::Postgres(_) => Err(StoreError::Corrupt("the harness got a store of another backend".to_string())),
    }
}

impl FilesHarness {
    fn keep(&self, p: Place) -> FilesOptions {
        let options = p.options.clone();
        self.places.lock().expect("lock").push(p);
        options
    }

    /// The options that the harness gave to the last store that has the same folders.
    fn options_of(&self, store: &FilesStore) -> FilesOptions {
        let _ = store;
        self.places.lock().expect("lock").last().expect("a store").options.clone()
    }
}

impl Harness for FilesHarness {
    fn name(&self) -> &'static str {
        "files"
    }

    async fn fresh(&self, options: FreshOptions) -> Result<AnyStore, StoreError> {
        let options = self.keep(place(&options));
        FilesStore::migrate(&options)?;
        Ok(AnyStore::Files(FilesStore::open(options)?))
    }

    async fn reopen(&self, store: AnyStore) -> Result<AnyStore, StoreError> {
        let store = files(store)?;
        let options = self.options_of(&store);
        drop(store);
        Ok(AnyStore::Files(FilesStore::open(options)?))
    }

    async fn reopen_with_key(&self, store: AnyStore, key: SecretKey) -> Result<AnyStore, StoreError> {
        let store = files(store)?;
        let options = FilesOptions { secret_key: key, ..self.options_of(&store) };
        drop(store);
        Ok(AnyStore::Files(FilesStore::open(options)?))
    }

    async fn other_handle(&self, _store: &AnyStore) -> Result<Option<AnyStore>, StoreError> {
        // A second open of the same folders stops at the lock (2.32).
        Ok(None)
    }

    async fn crash_after_version(
        &self,
        store: AnyStore,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<(AnyStore, CrashOutcome), StoreError> {
        let options = self.options_of(&files(store)?);
        let faults = Faults::new();
        {
            let faulty = FilesStore::open_with_faults(options.clone(), faults.clone())?;
            faults.fail_once(FaultPoint::AfterHistory, 5);
            let crashed = faulty.put(key, doc, precondition, meta).await;
            assert!(crashed.is_err(), "the fault must stop the write");
        }
        Ok((AnyStore::Files(FilesStore::open(options)?), CrashOutcome::Repaired))
    }

    async fn prepare_old_schema(&self) -> Result<(), StoreError> {
        let options = self.keep(place(&FreshOptions { history_keep: 100, secret_key: test_key() }));
        FilesStore::migrate(&options)?;
        {
            let store = FilesStore::open(options.clone())?;
            store.put(&suite::old_key(), &suite::old_document(), &Precondition::Create, &suite::old_meta()).await?;
        }
        std::fs::write(options.path.join("pupitre-store.json"), "{\n  \"format\": 1,\n  \"store_schema\": 0\n}\n")
            .map_err(|e| StoreError::Unavailable(e.to_string()))?;
        *self.old.lock().expect("lock") = Some(options);
        Ok(())
    }

    async fn open_prepared(&self) -> Result<AnyStore, StoreError> {
        let options = self.old.lock().expect("lock").clone().expect("a prepared store");
        Ok(AnyStore::Files(FilesStore::open(options)?))
    }

    async fn migrate_prepared(&self) -> Result<(), StoreError> {
        let options = self.old.lock().expect("lock").clone().expect("a prepared store");
        FilesStore::migrate(&options).map(|_| ())
    }
}

macro_rules! cases {
    ($($name:ident => $case:path),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $name() {
                $case(&FilesHarness::default()).await;
            }
        )*
    };
}

cases! {
    s01_create_and_get => suite::s01_create_and_get,
    s02_save_and_conflict => suite::s02_save_and_conflict,
    s03_two_writers_one_winner => suite::s03_two_writers_one_winner,
    s04_create_exists_and_after_delete => suite::s04_create_exists_and_after_delete,
    s05_delete_and_poll => suite::s05_delete_and_poll,
    s06_history_and_version => suite::s06_history_and_version,
    s07_history_keep => suite::s07_history_keep,
    s08_crash_between_version_and_document => suite::s08_crash_between_version_and_document,
    s09_poll => suite::s09_poll,
    s10_read_environment_is_consistent => suite::s10_read_environment_is_consistent,
    s11_secrets => suite::s11_secrets,
    s12_fingerprint_vector => suite::s12_fingerprint_vector,
    s13_wrong_key_stops_the_open => suite::s13_wrong_key_stops_the_open,
    s13_same_key_opens => suite::s13_same_key_opens,
    s14_queue_and_check_use => suite::s14_queue_and_check_use,
    s15_update_run_transitions => suite::s15_update_run_transitions,
    s16_output_and_events => suite::s16_output_and_events,
    s17_leases => suite::s17_leases,
    s18_prune_runs => suite::s18_prune_runs,
    s20_old_schema_and_migrate => suite::s20_old_schema_and_migrate,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s19_transfer_files_to_files() {
    suite::s19_transfer(&FilesHarness::default(), &FilesHarness::default()).await;
}

/// Each case of the table `CASES` has a test in this file, so a new case cannot be left out.
#[test]
fn each_case_of_2_43_has_a_test() {
    let source = include_str!("contract_files.rs");
    for (id, _) in suite::CASES {
        let prefix = id.to_lowercase();
        assert!(
            source.lines().any(|l| {
                let l = l.trim_start();
                l.starts_with(&format!("{prefix}_")) || l.starts_with(&format!("async fn {prefix}_"))
            }),
            "no test for the case {id}"
        );
    }
}
