// SPDX-License-Identifier: MIT OR Apache-2.0
//! The store schema (2.30): the check at open and the command `migrate`.
//!
//! `migrate` runs in one transaction, under an advisory lock, with the account that owns the
//! tables. It makes the schema, or it moves the schema to the version of the binary, and it
//! writes the key check (2.26). It never runs at start.

use tokio_postgres::{Client, GenericClient};

use super::conn;
use super::{MIGRATION_0001, PgOptions};
use crate::secret::StoreKeys;
use crate::types::{STORE_SCHEMA_VERSION, StoreError};

/// The advisory lock of `migrate`. Two `migrate` commands at the same time run one after the other.
const MIGRATE_LOCK: i64 = 0x5075_7069_7472_6531;

/// One step of the store schema: the SQL that moves the schema from `version - 1` to `version`.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// The store schema after this step.
    pub version: u32,
    /// The SQL. It does not write `pupitre_meta`: `migrate` does it.
    pub sql: &'static str,
}

/// The steps of this binary, in the order of the versions.
pub const MIGRATIONS: &[Migration] = &[Migration { version: 1, sql: MIGRATION_0001 }];

/// The result of `migrate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrateReport {
    /// The version before, or `None` for an empty database.
    pub from: Option<u32>,
    /// The version after.
    pub to: u32,
}

/// Make the store schema, or change it to the version of the binary, and write the key check
/// (2.30). It never runs at start. `migrate` on a store that has the version of the binary
/// changes nothing. A key that differs from the key of the store gives `KeyMismatch`.
pub async fn migrate(options: &PgOptions) -> Result<MigrateReport, StoreError> {
    migrate_with(options, MIGRATIONS).await
}

/// Like `migrate`, with a list of steps. The tests of 2.30 use it to play a binary of another
/// version.
#[doc(hidden)]
pub async fn migrate_with(options: &PgOptions, steps: &[Migration]) -> Result<MigrateReport, StoreError> {
    let target = steps.last().map_or(STORE_SCHEMA_VERSION, |m| m.version);
    let keys = StoreKeys::derive(&options.secret_key)
        .map_err(|_| StoreError::Unavailable("storage.secret_key: the keys cannot be derived".to_string()))?;
    let tls = conn::build_tls(options)?;
    let mut direct = conn::connect_direct(options, &tls).await?;
    check_encoding(&direct.client).await?;
    let tx = direct.client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATE_LOCK]).await?;
    let found = read_meta(&tx).await?;
    let from = found.as_ref().map(|(version, _)| *version);
    if let Some((version, stored_check)) = &found {
        if *version > target {
            return Err(StoreError::StoreNewer { found: *version });
        }
        if !same_check(stored_check, &keys.key_check()) {
            return Err(StoreError::KeyMismatch);
        }
        if *version == target {
            return Ok(MigrateReport { from, to: target });
        }
    }
    for step in steps.iter().filter(|s| from.is_none_or(|v| s.version > v)) {
        tx.batch_execute(step.sql).await.map_err(|e| {
            StoreError::Unavailable(format!("migration {} failed: {}", step.version, conn::describe(&e)))
        })?;
    }
    let version = i32::try_from(target).unwrap_or(i32::MAX);
    if from.is_none() {
        tx.execute(
            "INSERT INTO pupitre_meta (store_schema, key_check) VALUES ($1, $2)",
            &[&version, &keys.key_check()],
        )
        .await?;
    } else {
        tx.execute("UPDATE pupitre_meta SET store_schema = $1", &[&version]).await?;
    }
    tx.commit().await?;
    Ok(MigrateReport { from, to: target })
}

/// Read the row of `pupitre_meta`: the version and the key check. `None` when the table is
/// absent. A table with no row is a defect of the store.
async fn read_meta(client: &impl GenericClient) -> Result<Option<(u32, String)>, StoreError> {
    let exists = client.query_one("SELECT to_regclass('pupitre_meta') IS NOT NULL", &[]).await?;
    if !exists.try_get::<_, bool>(0)? {
        return Ok(None);
    }
    let row = client.query_opt("SELECT store_schema, key_check FROM pupitre_meta", &[]).await?;
    let Some(row) = row else {
        return Err(StoreError::Corrupt("pupitre_meta has no row".to_string()));
    };
    let version: i32 = row.try_get(0)?;
    let check: String = row.try_get(1)?;
    let version = u32::try_from(version).map_err(|_| StoreError::Corrupt("pupitre_meta.store_schema".to_string()))?;
    Ok(Some((version, check)))
}

/// Compare the stored key check with the check of the key of the configuration. Both are
/// public values of an HMAC: no secret is in the comparison.
fn same_check(stored: &str, own: &str) -> bool {
    stored == own
}

/// The database encoding MUST be `UTF8`: the text columns hold ids and messages in UTF-8.
pub(crate) async fn check_encoding(client: &Client) -> Result<(), StoreError> {
    let row = client.query_one("SELECT current_setting('server_encoding')", &[]).await?;
    let encoding: String = row.try_get(0)?;
    if encoding.eq_ignore_ascii_case("UTF8") {
        Ok(())
    } else {
        Err(StoreError::Unavailable(format!("the database encoding is {encoding}; Pupitre needs UTF8")))
    }
}

/// The check of 2.30 and 2.26 at open: the store schema and the key check.
pub(crate) async fn check_store(client: &Client, binary_schema: u32, keys: &StoreKeys) -> Result<(), StoreError> {
    let Some((version, stored_check)) = read_meta(client).await? else {
        return Err(StoreError::MigrateNeeded { found: None });
    };
    if version < binary_schema {
        return Err(StoreError::MigrateNeeded { found: Some(version) });
    }
    if version > binary_schema {
        return Err(StoreError::StoreNewer { found: version });
    }
    if !same_check(&stored_check, &keys.key_check()) {
        return Err(StoreError::KeyMismatch);
    }
    Ok(())
}
