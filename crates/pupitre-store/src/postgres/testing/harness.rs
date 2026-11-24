// SPDX-License-Identifier: MIT OR Apache-2.0
//! The harness of the backend `postgres` for the contract suite (2.43).

use std::sync::Mutex;

use pupitre_core::doc::{CanonicalDocument, DocKey};

use super::{PgTestEnv, TestDb};
use crate::conformance::{CrashOutcome, FreshOptions, Harness, test_key};
use crate::postgres::{MIGRATIONS, Migration, PgStore, migrate, migrate_with};
use crate::secret::SecretKey;
use crate::store::{AnyStore, Store};
use crate::types::{Precondition, StoreError, WriteMeta};

/// The step 2 that S20 adds to the steps of this binary. It makes a binary of the next version.
const STEP_2: Migration = Migration { version: 2, sql: "CREATE TABLE pupitre_schema_probe (id integer PRIMARY KEY)" };

/// The harness: each fresh store has its own database in the test server. The account of
/// `serve` has the rights of 2.36 only, so each case runs with these rights.
pub struct PgHarness {
    env: PgTestEnv,
    old: Mutex<Option<TestDb>>,
}

impl PgHarness {
    /// Read the variables `PUPITRE_TEST_PG_*`.
    pub fn from_env() -> PgHarness {
        PgHarness { env: PgTestEnv::require(), old: Mutex::new(None) }
    }

    /// The server of the tests.
    pub fn env(&self) -> &PgTestEnv {
        &self.env
    }

    fn prepared(&self) -> Result<TestDb, StoreError> {
        let guard = self.old.lock().map_err(|_| StoreError::Corrupt("the harness lock".to_string()))?;
        guard.clone().ok_or_else(|| StoreError::Corrupt("no prepared database".to_string()))
    }
}

fn postgres(store: AnyStore) -> Result<PgStore, StoreError> {
    match store {
        AnyStore::Postgres(s) => Ok(s),
        AnyStore::Files(_) => Err(StoreError::Corrupt("the harness of postgres got another backend".to_string())),
    }
}

impl Harness for PgHarness {
    fn name(&self) -> &'static str {
        "postgres"
    }

    async fn fresh(&self, options: FreshOptions) -> Result<AnyStore, StoreError> {
        let db = self.env.new_database().await;
        migrate(&db.owner_options(&options.secret_key)).await?;
        db.grant_app().await;
        let mut app = db.app_options(&options.secret_key);
        app.history_keep = options.history_keep;
        Ok(AnyStore::Postgres(PgStore::open(app).await?))
    }

    async fn reopen(&self, store: AnyStore) -> Result<AnyStore, StoreError> {
        let options = postgres(store)?.options().clone();
        Ok(AnyStore::Postgres(PgStore::open(options).await?))
    }

    async fn reopen_with_key(&self, store: AnyStore, key: SecretKey) -> Result<AnyStore, StoreError> {
        let mut options = postgres(store)?.options().clone();
        options.secret_key = key;
        Ok(AnyStore::Postgres(PgStore::open(options).await?))
    }

    async fn other_handle(&self, store: &AnyStore) -> Result<Option<AnyStore>, StoreError> {
        let AnyStore::Postgres(s) = store else { return Ok(None) };
        Ok(Some(AnyStore::Postgres(PgStore::open(s.options().clone()).await?)))
    }

    async fn crash_after_version(
        &self,
        store: AnyStore,
        key: &DocKey,
        doc: &CanonicalDocument,
        precondition: &Precondition,
        meta: &WriteMeta,
    ) -> Result<(AnyStore, CrashOutcome), StoreError> {
        let store = postgres(store)?;
        store.faults().crash_after_version();
        let crashed = store.put(key, doc, precondition, meta).await;
        assert!(crashed.is_err(), "the fault must stop the write");
        let reopened = self.reopen(AnyStore::Postgres(store)).await?;
        // One transaction holds the version and the document: the crash leaves nothing.
        Ok((reopened, CrashOutcome::RolledBack))
    }

    async fn prepare_old_schema(&self) -> Result<(), StoreError> {
        let db = self.env.new_database().await;
        let key = test_key();
        migrate(&db.owner_options(&key)).await?;
        db.grant_app().await;
        let store = PgStore::open(db.app_options(&key)).await?;
        let doc = crate::conformance::old_document();
        store.put(&crate::conformance::old_key(), &doc, &Precondition::Create, &crate::conformance::old_meta()).await?;
        *self.old.lock().map_err(|_| StoreError::Corrupt("the harness lock".to_string()))? = Some(db);
        Ok(())
    }

    async fn open_prepared(&self) -> Result<AnyStore, StoreError> {
        // The binary of S20 has the store schema 2.
        let db = self.prepared()?;
        Ok(AnyStore::Postgres(PgStore::open_for(db.app_options(&test_key()), 2).await?))
    }

    async fn migrate_prepared(&self) -> Result<(), StoreError> {
        let db = self.prepared()?;
        let steps = [MIGRATIONS[0], STEP_2];
        migrate_with(&db.owner_options(&test_key()), &steps).await?;
        Ok(())
    }
}
