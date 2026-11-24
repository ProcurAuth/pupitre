// SPDX-License-Identifier: MIT OR Apache-2.0
//! 6.10 on PostgreSQL: the TLS link with `verify-full`. Each test needs the server of
//! `tests/ci/store-pg.sh`.
#![cfg(feature = "conformance")]

mod common;

use common::{env, migrated, ready, unavailable};
use pupitre_store::Store;
use pupitre_store::postgres::{PgStore, PgTls, SslMode};

/// The link has TLS with `verify-full`, and the server sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_link_uses_tls_1_2_or_1_3() {
    let (db, store) = ready().await;
    store.history(&common::env_key("production"), None, 1).await.expect("a call opens a connection");
    let admin = db.admin_session().await;
    let rows = admin
        .query(
            "SELECT s.version FROM pg_stat_ssl s JOIN pg_stat_activity a USING (pid) \
             WHERE a.application_name = 'pupitre' AND a.datname = $1 AND s.ssl",
            &[&db.name],
        )
        .await
        .expect("stat");
    assert!(!rows.is_empty(), "no TLS connection of the store");
    for row in rows {
        let version: String = row.get(0);
        assert!(version == "TLSv1.2" || version == "TLSv1.3", "{version}");
    }
}

/// The certificate must come from the CA of the file. Another CA gives an error at the open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_certificate_from_another_ca_is_refused() {
    let (_db, mut options) = migrated().await;
    options.tls = Some(PgTls { ca_pem: env().wrong_ca_pem.clone(), server_name: env().server_name.clone() });
    let text = unavailable(PgStore::open(options).await);
    assert!(text.to_lowercase().contains("certificate") || text.to_lowercase().contains("issuer"), "{text}");
}

/// The certificate must hold the name `server_name`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_wrong_server_name_is_refused() {
    let (_db, mut options) = migrated().await;
    options.tls = Some(PgTls { ca_pem: env().ca_pem.clone(), server_name: "other.example.invalid".to_string() });
    let text = unavailable(PgStore::open(options).await);
    assert!(text.to_lowercase().contains("certificate") || text.to_lowercase().contains("name"), "{text}");
}

/// The root store holds only the CA of the file: a file with no certificate is an error, and a
/// `verify-full` link with no block `tls` is an error. `sslmode = "disable"` makes a plain link.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_ca_file_and_the_mode_are_checked() {
    let (db, options) = migrated().await;
    let mut empty = options.clone();
    empty.tls = Some(PgTls { ca_pem: b"not a certificate\n".to_vec(), server_name: env().server_name.clone() });
    let text = unavailable(PgStore::open(empty).await);
    assert!(text.contains("storage.postgres.tls.ca_file"), "{text}");
    let mut missing = options.clone();
    missing.tls = None;
    let text = unavailable(PgStore::open(missing).await);
    assert!(text.contains("storage.postgres.tls"), "{text}");
    let mut plain = options.clone();
    plain.sslmode = SslMode::Disable;
    plain.tls = None;
    let store = PgStore::open(plain).await.expect("a plain link");
    store.history(&common::env_key("production"), None, 1).await.expect("a call");
    let admin = db.admin_session().await;
    let ssl: Vec<bool> = admin
        .query(
            "SELECT s.ssl FROM pg_stat_ssl s JOIN pg_stat_activity a USING (pid) \
             WHERE a.application_name = 'pupitre' AND a.datname = $1",
            &[&db.name],
        )
        .await
        .expect("stat")
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert!(!ssl.is_empty() && ssl.iter().all(|s| !s), "the plain link has no TLS");
}

/// A wrong password is an error at the open. The error names the SQLSTATE and never the password.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_wrong_password_stops_the_open_without_showing_it() {
    let (_db, mut options) = migrated().await;
    options.password = pupitre_store::postgres::PgPassword("wrong-password-value".to_string());
    let text = unavailable(PgStore::open(options.clone()).await);
    assert!(text.contains("28P01") || text.to_lowercase().contains("password"), "{text}");
    assert!(!text.contains("wrong-password-value"));
    assert!(!format!("{options:?}").contains("wrong-password-value"), "Debug hides the password");
}
