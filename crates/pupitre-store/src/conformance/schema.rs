// SPDX-License-Identifier: MIT OR Apache-2.0
//! The case S20 of 2.43: the store schema (2.30).

use pupitre_core::doc::{CanonicalDocument, DocKey};

use super::harness::Harness;
use super::support::{env_doc, env_key, meta};
use crate::store::Store;
use crate::types::{Precondition, StoreError, WriteMeta};

/// The key of the document that `prepare_old_schema` writes into the old storage.
pub fn old_key() -> DocKey {
    env_key("old")
}

/// The document that `prepare_old_schema` writes into the old storage, as version 1.
pub fn old_document() -> CanonicalDocument {
    env_doc("Old")
}

/// The metadata of that write.
pub fn old_meta() -> WriteMeta {
    meta("alice", "before the migration")
}

/// S20: an old store schema stops the open with `run pupitre migrate`. `migrate` then makes it
/// current, and the data of the old store is still there.
pub async fn s20_old_schema_and_migrate<H: Harness>(h: &H) {
    h.prepare_old_schema().await.expect("an old store");
    let refused = h.open_prepared().await;
    match refused {
        Err(StoreError::MigrateNeeded { found }) => {
            assert!(found.is_some(), "{}: the store has an old version, not none", h.name());
        }
        other => panic!("{}: the open of an old store must give MigrateNeeded: {other:?}", h.name()),
    }
    assert_eq!(StoreError::MigrateNeeded { found: Some(1) }.to_string(), "run pupitre migrate");
    h.migrate_prepared().await.expect("migrate");
    let store = h.open_prepared().await.expect("the open after migrate");
    // The old data stays. The store works.
    let old = store.get(&old_key()).await.expect("the old document");
    assert_eq!((old.version, old.bytes), (1, old_document().bytes));
    let saved = store.put(&env_key("new"), &env_doc("New"), &Precondition::Create, &meta("alice", "")).await;
    assert_eq!(saved.expect("write").version, 1);
    assert_eq!(store.get(&old_key()).await.expect("get").version, 1);
    // `migrate` on a current store changes nothing and succeeds. A backend with a lock needs the
    // store to be closed first.
    drop(store);
    h.migrate_prepared().await.expect("migrate again");
    let again = h.open_prepared().await.expect("the open after the second migrate");
    assert_eq!(again.get(&old_key()).await.expect("get").version, 1);
}
