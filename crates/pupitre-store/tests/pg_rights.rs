// SPDX-License-Identifier: MIT OR Apache-2.0
//! 2.36 on PostgreSQL: the account of `serve` has `SELECT`, `INSERT`, `UPDATE` and `DELETE` on
//! the tables and `USAGE` on the sequence, and no right to change a table. Each test needs the
//! server of `tests/ci/store-pg.sh`.
#![cfg(feature = "conformance")]

mod common;

use common::{doc, env_key, meta, ready};
use pupitre_store::{Precondition, Store};

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

/// 2.36: the rights of the account of `serve`, read from the catalog of the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_account_of_serve_has_the_rights_of_2_36_and_no_more() {
    let (db, _store) = ready().await;
    let admin = db.admin_session().await;
    let app = db.app_options(&common::key()).user;
    let row = admin
        .query_one("SELECT rolsuper, rolcreatedb, rolcreaterole FROM pg_roles WHERE rolname = $1", &[&app])
        .await
        .expect("role");
    assert!(!row.get::<_, bool>(0) && !row.get::<_, bool>(1) && !row.get::<_, bool>(2), "a plain account");
    for table in TABLES {
        for right in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
            let has: bool = admin
                .query_one("SELECT has_table_privilege($1, $2, $3)", &[&app, &table, &right])
                .await
                .expect("privilege")
                .get(0);
            assert!(has, "{table}: {right}");
        }
        for right in ["TRUNCATE", "REFERENCES", "TRIGGER"] {
            let has: bool = admin
                .query_one("SELECT has_table_privilege($1, $2, $3)", &[&app, &table, &right])
                .await
                .expect("privilege")
                .get(0);
            assert!(!has, "{table}: {right} is not part of 2.36");
        }
        let owner: String = admin
            .query_one("SELECT tableowner FROM pg_tables WHERE tablename = $1", &[&table])
            .await
            .expect("owner")
            .get(0);
        assert_ne!(owner, app, "the account of serve does not own {table}");
    }
    let usage: bool = admin
        .query_one("SELECT has_sequence_privilege($1, 'pupitre_change_seq', 'USAGE')", &[&app])
        .await
        .expect("privilege")
        .get(0);
    assert!(usage);
}

/// 2.36: the account of `serve` cannot make, change or remove a table, and cannot empty one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_account_of_serve_cannot_change_a_table() {
    let (db, store) = ready().await;
    store.put(&env_key("production"), &doc("One"), &Precondition::Create, &meta("alice")).await.expect("create");
    let app = db.app_session().await;
    for sql in [
        "CREATE TABLE evil (id integer)",
        "ALTER TABLE documents ADD COLUMN evil integer",
        "DROP TABLE documents",
        "TRUNCATE documents",
        "DROP SEQUENCE pupitre_change_seq",
        "CREATE INDEX evil ON documents (version)",
    ] {
        let error = app.batch_execute(sql).await.expect_err(sql);
        let code = error.code().map(|c| c.code().to_string());
        assert_eq!(code.as_deref(), Some("42501"), "{sql}: {error}");
    }
    // The store works with these rights: the contract suite runs with them too.
    assert_eq!(store.get(&env_key("production")).await.expect("get").version, 1);
}
