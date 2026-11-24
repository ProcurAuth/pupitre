// SPDX-License-Identifier: MIT OR Apache-2.0
//! 2.39 on PostgreSQL: the cipher of the secrets. Each test needs the server of
//! `tests/ci/store-pg.sh`.
#![cfg(feature = "conformance")]

mod common;

use common::{meta, ready};
use pupitre_core::ids::{EnvId, KeyPath, UnitId};
use pupitre_core::node::SecretScope;
use pupitre_store::secret::SecretValue;
use pupitre_store::{Store, StoreError};

fn unit() -> SecretScope {
    SecretScope::Unit(EnvId::parse("production").expect("id"), UnitId::parse("relay").expect("id"))
}

fn environment() -> SecretScope {
    SecretScope::Environment(EnvId::parse("production").expect("id"))
}

fn name(text: &str) -> KeyPath {
    KeyPath::parse(text).expect("a key path")
}

fn value(text: &str) -> SecretValue {
    SecretValue::new(text.as_bytes().to_vec())
}

const PLAIN: &str = "plain-text-secret-value";

/// 2.39: a value that moves to another row does not decrypt. The scope and the name are the
/// associated data of the cipher.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn a_value_moved_to_another_row_does_not_decrypt() {
    let (db, store) = ready().await;
    let admin = db.admin_session().await;
    store.put_secret(&unit(), &name("storage.password"), &value(PLAIN), &meta("alice")).await.expect("write");
    store
        .put_secret(&environment(), &name("storage.password"), &value("another-value-1"), &meta("alice"))
        .await
        .expect("write");
    store.put_secret(&unit(), &name("storage.token"), &value("another-value-2"), &meta("alice")).await.expect("write");
    assert_eq!(store.read_secrets(&[unit(), environment()]).await.expect("read").len(), 3);
    // Move the sealed value of (unit, password) to the row (environment, password): another scope.
    admin
        .batch_execute(
            "UPDATE secrets SET nonce = s.nonce, ciphertext = s.ciphertext FROM secrets s \
             WHERE s.scope = 'environment:production/unit:relay' AND s.name = 'storage.password' \
             AND secrets.scope = 'environment:production' AND secrets.name = 'storage.password'",
        )
        .await
        .expect("move");
    assert_eq!(store.read_secrets(&[environment()]).await, Err(StoreError::SecretUnreadable));
    // The row of the unit still reads.
    assert!(store.read_secrets(&[unit()]).await.is_ok());
    // Move it to another name in the same scope.
    admin
        .batch_execute(
            "UPDATE secrets SET nonce = s.nonce, ciphertext = s.ciphertext FROM secrets s \
             WHERE s.scope = 'environment:production/unit:relay' AND s.name = 'storage.password' \
             AND secrets.scope = 'environment:production/unit:relay' AND secrets.name = 'storage.token'",
        )
        .await
        .expect("move");
    assert_eq!(store.read_secrets(&[unit()]).await, Err(StoreError::SecretUnreadable));
}

/// 2.39: a changed ciphertext, a changed tag and a changed nonce do not decrypt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn changed_bytes_do_not_decrypt() {
    let (db, store) = ready().await;
    let admin = db.admin_session().await;
    store.put_secret(&unit(), &name("storage.password"), &value(PLAIN), &meta("alice")).await.expect("write");
    for change in [
        "ciphertext = set_byte(ciphertext, 0, get_byte(ciphertext, 0) # 1)",
        "ciphertext = set_byte(ciphertext, octet_length(ciphertext) - 1, get_byte(ciphertext, octet_length(ciphertext) - 1) # 1)",
        "nonce = set_byte(nonce, 0, get_byte(nonce, 0) # 1)",
    ] {
        admin.batch_execute(&format!("UPDATE secrets SET {change}")).await.expect("change");
        assert_eq!(store.read_secrets(&[unit()]).await, Err(StoreError::SecretUnreadable), "{change}");
        admin.batch_execute(&format!("UPDATE secrets SET {change}")).await.expect("change back");
        let ok = store.read_secrets(&[unit()]).await.expect("the value reads again");
        assert_eq!(ok[0].2.expose(), PLAIN.as_bytes());
    }
    let text = StoreError::SecretUnreadable.to_string();
    assert!(!text.contains(PLAIN));
}

/// 2.39: the database holds no value in clear text. Each write has a new nonce.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_database_holds_no_value_and_each_write_has_a_new_nonce() {
    let (db, store) = ready().await;
    let admin = db.admin_session().await;
    let (scope, secret) = (unit(), name("storage.password"));
    store.put_secret(&scope, &secret, &value(PLAIN), &meta("alice")).await.expect("write");
    let first = admin.query_one("SELECT nonce, ciphertext, fingerprint FROM secrets", &[]).await.expect("row");
    store.put_secret(&scope, &secret, &value(PLAIN), &meta("bob")).await.expect("write");
    let second = admin.query_one("SELECT nonce, ciphertext, fingerprint FROM secrets", &[]).await.expect("row");
    let (n1, c1): (Vec<u8>, Vec<u8>) = (first.get(0), first.get(1));
    let (n2, c2): (Vec<u8>, Vec<u8>) = (second.get(0), second.get(1));
    assert_eq!(n1.len(), 12);
    assert_ne!(n1, n2, "a new nonce for each write");
    assert_ne!(c1, c2, "the same value gives another ciphertext");
    assert_eq!(first.get::<_, String>(2), second.get::<_, String>(2), "the fingerprint of one value is stable");
    assert_eq!(c1.len(), PLAIN.len() + 16, "the ciphertext has the tag of 16 bytes");
    // No column of any table holds the value, in clear text.
    let found: i64 = admin
        .query_one(
            "SELECT (SELECT count(*) FROM secrets WHERE position($1::bytea in ciphertext) > 0) \
             + (SELECT count(*) FROM secret_events WHERE scope || name || action || saved_by LIKE '%' || $2 || '%') \
             + (SELECT count(*) FROM secrets WHERE fingerprint LIKE '%' || $2 || '%')",
            &[&PLAIN.as_bytes(), &PLAIN],
        )
        .await
        .expect("count")
        .get(0);
    assert_eq!(found, 0);
    let events: i64 = admin.query_one("SELECT count(*) FROM secret_events", &[]).await.expect("count").get(0);
    assert_eq!(events, 2);
}

/// 2.24: the events keep the newest `history_keep` of each secret, also after a delete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_events_of_a_secret_are_bounded_by_history_keep() {
    let (_db, mut options) = common::migrated().await;
    options.history_keep = 3;
    let store = pupitre_store::postgres::PgStore::open(options).await.expect("open");
    let (scope, secret) = (unit(), name("storage.password"));
    for n in 0..7 {
        store.put_secret(&scope, &secret, &value(&format!("value-number-{n}")), &meta("alice")).await.expect("write");
    }
    assert_eq!(store.secret_event_count(&scope, &secret).await, Ok(3));
    store.delete_secret(&scope, &secret, &meta("bob")).await.expect("delete");
    assert_eq!(store.secret_event_count(&scope, &secret).await, Ok(3), "the delete event replaces the oldest");
}

/// 2.39 (L7): the fingerprint is part of the associated data. A database writer who puts an old
/// nonce and ciphertext under the fingerprint of a new value makes the read fail. A change of the
/// column `fingerprint` alone fails too. A whole old row (the three columns) is a rollback, and
/// 7.11 says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn the_fingerprint_is_bound_to_the_ciphertext() {
    let (db, store) = ready().await;
    let admin = db.admin_session().await;
    let (scope, secret) = (unit(), name("storage.password"));
    store.put_secret(&scope, &secret, &value("old-value-0001"), &meta("alice")).await.expect("write");
    let old = admin.query_one("SELECT nonce, ciphertext, fingerprint FROM secrets", &[]).await.expect("row");
    let (old_nonce, old_cipher, old_fp): (Vec<u8>, Vec<u8>, String) = (old.get(0), old.get(1), old.get(2));
    store.put_secret(&scope, &secret, &value("new-value-0002"), &meta("alice")).await.expect("write");
    // The old nonce and ciphertext under the fingerprint of the new value.
    admin
        .execute("UPDATE secrets SET nonce = $1, ciphertext = $2", &[&old_nonce, &old_cipher])
        .await
        .expect("restore the old pair");
    assert_eq!(store.read_secrets(std::slice::from_ref(&scope)).await, Err(StoreError::SecretUnreadable));
    // The metadata shows the fingerprint of the new value: the runner would have deployed the old value under it.
    let listed = store.list_secrets(&scope).await.expect("list");
    assert_ne!(listed[0].fingerprint, old_fp);
    // The whole old row is a consistent rollback: it reads as the old value, with the old fingerprint.
    admin.execute("UPDATE secrets SET fingerprint = $1", &[&old_fp]).await.expect("restore the old row");
    let rolled = store.read_secrets(std::slice::from_ref(&scope)).await.expect("a consistent old row reads");
    assert_eq!(rolled[0].2.expose(), b"old-value-0001");
    assert_eq!(store.list_secrets(&scope).await.expect("list")[0].fingerprint, old_fp);
    // A fingerprint that does not match the sealed value fails.
    admin.execute("UPDATE secrets SET fingerprint = '0123456789abcdef'", &[]).await.expect("change");
    assert_eq!(store.read_secrets(std::slice::from_ref(&scope)).await, Err(StoreError::SecretUnreadable));
}
