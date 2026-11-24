// SPDX-License-Identifier: MIT OR Apache-2.0
//! Helpers of the PostgreSQL tests. Each test file includes this module.
#![allow(dead_code)]

use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::ids::{EnvId, Subject};
use pupitre_core::json::{self, Json};
use pupitre_core::time::UtcMillis;
use pupitre_store::postgres::testing::{PgTestEnv, TestDb};
use pupitre_store::postgres::{PgOptions, PgStore};
use pupitre_store::secret::SecretKey;
use pupitre_store::{Message, StoreError, WriteMeta};

/// The key of the tests: the vector of S12.
pub fn key() -> SecretKey {
    pupitre_store::conformance::test_key()
}

pub fn env() -> PgTestEnv {
    PgTestEnv::require()
}

/// A new database with the store schema, the rights of 2.36 and an open store as the account of `serve`.
pub async fn ready() -> (TestDb, PgStore) {
    let db = env().new_database().await;
    let store = db.ready(&key()).await;
    (db, store)
}

/// A new database with the store schema and the rights, and the options of the account of `serve`.
pub async fn migrated() -> (TestDb, PgOptions) {
    let db = env().new_database().await;
    pupitre_store::postgres::migrate(&db.owner_options(&key())).await.expect("migrate");
    db.grant_app().await;
    let options = db.app_options(&key());
    (db, options)
}

pub fn meta(by: &str) -> WriteMeta {
    WriteMeta {
        saved_by: Subject::parse(by).expect("a subject"),
        saved_at: UtcMillis::now(),
        message: Message::parse("").expect("a message"),
    }
}

pub fn env_key(id: &str) -> DocKey {
    DocKey::Environment(EnvId::parse(id).expect("an environment id"))
}

pub fn doc(label: &str) -> CanonicalDocument {
    CanonicalDocument::from_bytes(json::to_canonical(&Json::object([
        ("format", Json::Int(1)),
        ("label", Json::str(label)),
    ])))
}

/// The text of an error that holds `Unavailable`, or a panic.
pub fn unavailable(result: Result<impl std::fmt::Debug, StoreError>) -> String {
    match result {
        Err(StoreError::Unavailable(text)) => text,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}
