// SPDX-License-Identifier: MIT OR Apache-2.0
//! The cases S11 to S13 of 2.43: the secrets.

use pupitre_core::ids::KeyPath;

use super::harness::{FreshOptions, Harness};
use super::support::{TEST_KEY_HEX, env_scope, meta, other_key, test_key, unit_scope};
use crate::secret::{SecretKey, SecretValue, StoreKeys};
use crate::store::Store;
use crate::transfer::SecretAction;
use crate::types::StoreError;

fn name(text: &str) -> KeyPath {
    KeyPath::parse(text).expect("a valid key path")
}

fn value(text: &str) -> SecretValue {
    SecretValue::new(text.as_bytes().to_vec())
}

/// S11: a secret write, a list, a read for a run and a delete. The list never holds the value.
/// The events have no value.
pub async fn s11_secrets<H: Harness>(h: &H) {
    let store = h.fresh(FreshOptions { history_keep: 2, ..FreshOptions::default() }).await.expect("a fresh store");
    let (unit, env) = (unit_scope("production", "relay"), env_scope("production"));
    let (password, token) = (name("storage.password"), name("storage.token"));
    let (one, two) = ("first-secret-value-1", "second-secret-value-2");
    let saved = store.put_secret(&unit, &password, &value(one), &meta("alice", "")).await.expect("write");
    assert_eq!(saved.fingerprint.len(), 16);
    assert!(saved.fingerprint.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    assert_eq!((saved.scope.clone(), saved.name.clone()), (unit.clone(), password.clone()));
    store.put_secret(&unit, &token, &value("token-value-3"), &meta("alice", "")).await.expect("write");
    store.put_secret(&env, &password, &value("environment-value-4"), &meta("bob", "")).await.expect("write");
    // The list: the byte order of the name, one scope only, never a value.
    let listed = store.list_secrets(&unit).await.expect("list");
    let names: Vec<&str> = listed.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["storage.password", "storage.token"]);
    assert_eq!(listed[0].fingerprint, saved.fingerprint);
    assert_eq!(listed[0].updated_by.as_str(), "alice");
    assert_eq!(store.list_secrets(&env).await.expect("list").len(), 1);
    let text = format!("{listed:?}");
    for secret in [one, two, "token-value-3", "environment-value-4"] {
        assert!(!text.contains(secret), "the list holds a value");
    }
    // A read for a run gives the values of the asked scopes only.
    let read = store.read_secrets(std::slice::from_ref(&unit)).await.expect("read");
    assert_eq!(read.len(), 2);
    let got = read.iter().find(|(_, n, _)| *n == password).expect("the password");
    assert_eq!(got.2.expose(), one.as_bytes());
    let both = store.read_secrets(&[unit.clone(), env.clone()]).await.expect("read");
    assert_eq!(both.len(), 3);
    assert!(store.read_secrets(&[]).await.expect("read").is_empty());
    // A new value replaces the old one. The fingerprint changes. There is no version.
    let again = store.put_secret(&unit, &password, &value(two), &meta("carol", "")).await.expect("write");
    assert_ne!(again.fingerprint, saved.fingerprint);
    let read = store.read_secrets(std::slice::from_ref(&unit)).await.expect("read");
    assert_eq!(read.iter().find(|(_, n, _)| *n == password).expect("the password").2.expose(), two.as_bytes());
    // The same value in another scope has another fingerprint (2.26).
    let same = store.put_secret(&env, &password, &value(two), &meta("carol", "")).await.expect("write");
    assert_ne!(same.fingerprint, again.fingerprint);
    // The events: the action, the subject and the time. No value. At most `history_keep`.
    let mut exporter = store.export_begin().await.expect("export");
    let exported = exporter.secrets().await.expect("secrets");
    exporter.end().await.expect("end");
    let unit_password = exported.iter().find(|s| s.scope == unit && s.name == password).expect("the secret");
    assert_eq!(unit_password.value.as_ref().map(SecretValue::expose), Some(two.as_bytes()));
    assert_eq!(unit_password.events.len(), 2, "history_keep is 2");
    assert!(unit_password.events.iter().all(|e| e.action == SecretAction::Set));
    assert_eq!(unit_password.events.last().map(|e| e.by.as_str()), Some("carol"));
    assert!(!format!("{:?}", unit_password.events).contains(two));
    // A delete removes the secret. A second delete gives `NotFound`.
    store.delete_secret(&unit, &password, &meta("alice", "")).await.expect("delete");
    assert_eq!(store.delete_secret(&unit, &password, &meta("alice", "")).await, Err(StoreError::NotFound));
    let left = store.list_secrets(&unit).await.expect("list");
    assert_eq!(left.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["storage.token"]);
    assert_eq!(store.read_secrets(std::slice::from_ref(&unit)).await.expect("read").len(), 1);
    assert_eq!(store.delete_secret(&unit, &name("storage.other"), &meta("alice", "")).await, Err(StoreError::NotFound));
    // The events of a deleted secret stay in an export: no value, the delete event last (L10).
    let mut exporter = store.export_begin().await.expect("export");
    let exported = exporter.secrets().await.expect("secrets");
    exporter.end().await.expect("end");
    let gone = exported.iter().find(|s| s.scope == unit && s.name == password).expect("the deleted secret");
    assert!(gone.value.is_none(), "a deleted secret has no value");
    assert_eq!(gone.events.last().map(|e| (e.action, e.by.as_str())), Some((SecretAction::Delete, "alice")));
}

/// S12: the fingerprint of a known key, scope, name and value equals the vector of the suite.
/// The vector comes from an independent computation of HKDF-SHA256 and HMAC-SHA256.
pub async fn s12_fingerprint_vector<H: Harness>(h: &H) {
    const SCOPE: &str = "environment:production/unit:relay";
    const FINGERPRINT: &str = "59b50c54cf977b23";
    const KEY_CHECK: &str = "ce03a3f53a0a890ed2ff2c22961a12a9fe01b4f47a1c7a9e993f4e4194684dc9";
    let keys = StoreKeys::derive(&SecretKey::parse(TEST_KEY_HEX).expect("the key")).expect("the keys");
    let (unit, password) = (unit_scope("production", "relay"), name("storage.password"));
    assert_eq!(unit.text(), SCOPE);
    let secret = value("correct horse battery staple");
    assert_eq!(keys.fingerprint(&unit, &password, &secret), FINGERPRINT);
    assert_eq!(keys.key_check(), KEY_CHECK);
    // The store gives the same fingerprint, whatever its cipher.
    let store = h.fresh(FreshOptions::default()).await.expect("a fresh store");
    let saved = store.put_secret(&unit, &password, &secret, &meta("alice", "")).await.expect("write");
    assert_eq!(saved.fingerprint, FINGERPRINT, "{}", h.name());
    assert_eq!(store.list_secrets(&unit).await.expect("list")[0].fingerprint, FINGERPRINT);
    // Two equal values in two scopes have two fingerprints.
    let env = store.put_secret(&env_scope("production"), &password, &secret, &meta("alice", "")).await.expect("write");
    assert_ne!(env.fingerprint, FINGERPRINT);
    // The wrong key gives another fingerprint.
    let wrong = StoreKeys::derive(&other_key()).expect("the keys");
    assert_ne!(wrong.fingerprint(&unit, &password, &secret), FINGERPRINT);
    assert_eq!(test_key(), SecretKey::parse(TEST_KEY_HEX).expect("the key"));
}

/// S13: a wrong `storage.secret_key` stops the open.
pub async fn s13_wrong_key_stops_the_open<H: Harness>(h: &H) {
    let store = h.fresh(FreshOptions::default()).await.expect("a fresh store");
    let (unit, password) = (unit_scope("production", "relay"), name("storage.password"));
    store.put_secret(&unit, &password, &value("a-value-of-8-bytes"), &meta("alice", "")).await.expect("write");
    let refused = h.reopen_with_key(store, other_key()).await;
    assert!(matches!(refused, Err(StoreError::KeyMismatch)), "{}: {refused:?}", h.name());
    assert_eq!(StoreError::KeyMismatch.to_string(), "storage.secret_key does not match the key of the store");
}

/// The key check does not stop the open of the same key: the secret is still readable after a
/// reopen. This is the other half of S13.
pub async fn s13_same_key_opens<H: Harness>(h: &H) {
    let store = h.fresh(FreshOptions::default()).await.expect("a fresh store");
    let (unit, password) = (unit_scope("production", "relay"), name("storage.password"));
    store.put_secret(&unit, &password, &value("a-value-of-8-bytes"), &meta("alice", "")).await.expect("write");
    let store = h.reopen(store).await.expect("the same key opens");
    let read = store.read_secrets(&[unit]).await.expect("read");
    assert_eq!(read[0].2.expose(), b"a-value-of-8-bytes");
}
