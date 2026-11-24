// SPDX-License-Identifier: MIT OR Apache-2.0
//! 2.30 and 2.26 on PostgreSQL: the store schema, `migrate` and the key check. Each test needs
//! the server of `tests/ci/store-pg.sh`.
#![cfg(feature = "conformance")]

mod common;

use common::{doc, env, env_key, key, meta, migrated, unavailable};
use pupitre_store::postgres::{MIGRATIONS, MigrateReport, Migration, PgStore, migrate, migrate_with};
use pupitre_store::secret::{SecretKey, StoreKeys};
use pupitre_store::{Precondition, Store, StoreError};

const TABLES: [&str; 9] = [
    "pupitre_meta",
    "documents",
    "document_versions",
    "secrets",
    "secret_events",
    "runs",
    "run_output",
    "run_events",
    "leases",
];

async fn tables(db: &pupitre_store::postgres::testing::TestDb) -> Vec<String> {
    let admin = db.admin_session().await;
    let rows = admin
        .query("SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename", &[])
        .await
        .expect("tables");
    rows.iter().map(|r| r.get(0)).collect()
}

/// 2.30: a store with no schema stops the start with `run pupitre migrate`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_database_with_no_schema_stops_the_open() {
    let db = env().new_database().await;
    let refused = PgStore::open(db.app_options(&key())).await;
    assert!(matches!(refused, Err(StoreError::MigrateNeeded { found: None })), "{refused:?}");
    assert_eq!(StoreError::MigrateNeeded { found: None }.to_string(), "run pupitre migrate");
    assert!(tables(&db).await.is_empty(), "the open never makes a table");
}

/// 2.30: an older schema stops the start with `run pupitre migrate`. A newer schema stops it
/// with `the store is newer than this binary`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn an_old_schema_and_a_newer_schema_stop_the_open() {
    let (db, options) = migrated().await;
    // A binary of the schema 2 opens the store of the schema 1.
    let old = PgStore::open_for(options.clone(), 2).await;
    assert!(matches!(old, Err(StoreError::MigrateNeeded { found: Some(1) })), "{old:?}");
    // The store of the schema 2 stops a binary of the schema 1.
    let owner = db.owner_session().await;
    owner.batch_execute("UPDATE pupitre_meta SET store_schema = 2").await.expect("update");
    let newer = PgStore::open(options.clone()).await;
    assert!(matches!(newer, Err(StoreError::StoreNewer { found: 2 })), "{newer:?}");
    assert_eq!(StoreError::StoreNewer { found: 2 }.to_string(), "the store is newer than this binary");
    // `migrate` of the older binary stops too, and changes nothing.
    let refused = migrate(&db.owner_options(&key())).await;
    assert!(matches!(refused, Err(StoreError::StoreNewer { found: 2 })), "{refused:?}");
    owner.batch_execute("UPDATE pupitre_meta SET store_schema = 1").await.expect("update");
    assert!(PgStore::open(options).await.is_ok());
}

/// 2.30: `migrate` makes the nine tables and the sequence, and writes the key check. A second
/// `migrate` changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn migrate_makes_the_schema_and_a_second_run_changes_nothing() {
    let db = env().new_database().await;
    let options = db.owner_options(&key());
    let report = migrate(&options).await.expect("migrate");
    assert_eq!(report, MigrateReport { from: None, to: 1 });
    let mut expected: Vec<String> = TABLES.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(tables(&db).await, expected);
    let admin = db.admin_session().await;
    let row = admin.query_one("SELECT store_schema, key_check FROM pupitre_meta", &[]).await.expect("meta");
    assert_eq!(row.get::<_, i32>(0), 1);
    let check = StoreKeys::derive(&key()).expect("keys").key_check();
    assert_eq!(row.get::<_, String>(1), check, "the key check of 2.26");
    let sequence: i64 = admin
        .query_one("SELECT count(*) FROM pg_class WHERE relkind = 'S' AND relname = 'pupitre_change_seq'", &[])
        .await
        .expect("sequence")
        .get(0);
    assert_eq!(sequence, 1);
    // A second run is a success with no change. The same holds with data in the store.
    db.grant_app().await;
    let store = PgStore::open(db.app_options(&key())).await.expect("open");
    store.put(&env_key("production"), &doc("One"), &Precondition::Create, &meta("alice")).await.expect("create");
    let again = migrate(&options).await.expect("migrate again");
    assert_eq!(again, MigrateReport { from: Some(1), to: 1 });
    assert_eq!(store.get(&env_key("production")).await.expect("get").version, 1);
    assert_eq!(tables(&db).await, expected);
}

/// 2.26: a key that differs from the key of the store stops `migrate` and the open. The check stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_wrong_key_stops_migrate_and_the_open() {
    let (db, options) = migrated().await;
    let wrong = SecretKey::parse(&"cd".repeat(32)).expect("a key");
    assert_eq!(migrate(&db.owner_options(&wrong)).await, Err(StoreError::KeyMismatch));
    let mut other = options.clone();
    other.secret_key = wrong;
    let refused = PgStore::open(other).await;
    assert!(matches!(refused, Err(StoreError::KeyMismatch)), "{refused:?}");
    let admin = db.admin_session().await;
    let stored: String = admin.query_one("SELECT key_check FROM pupitre_meta", &[]).await.expect("meta").get(0);
    assert_eq!(stored, StoreKeys::derive(&key()).expect("keys").key_check(), "the check is not changed");
}

/// 2.30: `migrate` moves an older store to the version of the binary, in one transaction. A
/// failed step leaves the store at its old version, with its data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_failed_step_leaves_the_old_schema_and_its_data() {
    let (db, options) = migrated().await;
    let store = PgStore::open(options.clone()).await.expect("open");
    store.put(&env_key("production"), &doc("One"), &Precondition::Create, &meta("alice")).await.expect("create");
    let bad = Migration { version: 2, sql: "CREATE TABLE half_made (id integer); SELECT 1 / 0" };
    let steps = [MIGRATIONS[0], bad];
    let failed = migrate_with(&db.owner_options(&key()), &steps).await;
    let text = unavailable(failed);
    assert!(text.contains("migration 2 failed"), "{text}");
    assert!(!tables(&db).await.contains(&"half_made".to_string()), "the step is rolled back");
    let admin = db.admin_session().await;
    let version: i32 = admin.query_one("SELECT store_schema FROM pupitre_meta", &[]).await.expect("meta").get(0);
    assert_eq!(version, 1);
    assert_eq!(store.get(&env_key("production")).await.expect("get").version, 1);
    // A good step moves the schema to 2.
    let good = Migration { version: 2, sql: "CREATE TABLE added_by_step_2 (id integer PRIMARY KEY)" };
    let report = migrate_with(&db.owner_options(&key()), &[MIGRATIONS[0], good]).await.expect("migrate");
    assert_eq!(report, MigrateReport { from: Some(1), to: 2 });
    assert!(tables(&db).await.contains(&"added_by_step_2".to_string()));
    assert!(PgStore::open_for(options, 2).await.is_ok());
    // A failed first run makes no table.
    let fresh = env().new_database().await;
    let first_failure = [Migration { version: 1, sql: "CREATE TABLE half_made (id integer); SELECT 1 / 0" }];
    assert!(migrate_with(&fresh.owner_options(&key()), &first_failure).await.is_err());
    assert!(tables(&fresh).await.is_empty());
}

/// 2.30: two `migrate` commands at the same time run one after the other. One makes the schema, the other changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn two_migrates_at_the_same_time_make_the_schema_one_time() {
    let db = env().new_database().await;
    let options = db.owner_options(&key());
    let (a, b) = tokio::join!(migrate(&options), migrate(&options));
    let (a, b) = (a.expect("migrate a"), b.expect("migrate b"));
    let firsts = [a, b].iter().filter(|r| r.from.is_none()).count();
    assert_eq!(firsts, 1, "{a:?} {b:?}");
    assert_eq!(a.to, 1);
    assert_eq!(b.to, 1);
}

/// 2.36: the account of `serve` has no right to change a table, so `migrate` with it fails and makes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_account_of_serve_cannot_migrate() {
    let db = env().new_database().await;
    db.grant_app().await;
    let text = unavailable(migrate(&db.app_options(&key())).await);
    assert!(text.contains("42501"), "permission denied: {text}");
    assert!(tables(&db).await.is_empty());
}
