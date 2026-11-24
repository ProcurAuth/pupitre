// SPDX-License-Identifier: MIT OR Apache-2.0
//! The secrets of the backend `postgres` (2.24 to 2.26, 2.39): the cipher, the fingerprint and
//! the events. No log and no error of this module holds a value.

use std::time::SystemTime;

use pupitre_core::ids::{KeyPath, Subject};
use pupitre_core::node::SecretScope;

use super::PgStore;
use super::rows::{from_system, to_system};
use crate::secret::{NONCE_BYTES, Sealed, SecretValue};
use crate::types::{SecretMeta, StoreError, WriteMeta};

const UPSERT_SECRET: &str = "INSERT INTO secrets (scope, name, nonce, ciphertext, fingerprint, updated_by, updated_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (scope, name) DO UPDATE SET nonce = EXCLUDED.nonce, \
    ciphertext = EXCLUDED.ciphertext, fingerprint = EXCLUDED.fingerprint, updated_by = EXCLUDED.updated_by, \
    updated_at = EXCLUDED.updated_at";
const INSERT_EVENT: &str =
    "INSERT INTO secret_events (scope, name, action, saved_by, saved_at) VALUES ($1, $2, $3, $4, $5)";
const PRUNE_EVENTS: &str = "DELETE FROM secret_events WHERE scope = $1 AND name = $2 AND id NOT IN \
    (SELECT id FROM secret_events WHERE scope = $1 AND name = $2 ORDER BY id DESC LIMIT $3)";

/// A new random nonce for one write (2.39).
pub(crate) fn new_nonce() -> Result<[u8; NONCE_BYTES], StoreError> {
    let mut nonce = [0u8; NONCE_BYTES];
    getrandom::fill(&mut nonce)
        .map_err(|_| StoreError::Unavailable("the system cannot give random bytes".to_string()))?;
    Ok(nonce)
}

/// A nonce of a row.
pub(crate) fn nonce_of(bytes: &[u8]) -> Result<[u8; NONCE_BYTES], StoreError> {
    bytes.try_into().map_err(|_| StoreError::Corrupt("secrets.nonce".to_string()))
}

/// The name of a secret as the column `name`.
pub(crate) fn name_of(text: &str) -> Result<KeyPath, StoreError> {
    KeyPath::parse(text).map_err(|_| StoreError::Corrupt("secrets.name".to_string()))
}

/// The scope of a secret as the column `scope`.
pub(crate) fn scope_of(text: &str) -> Result<SecretScope, StoreError> {
    SecretScope::parse(text).map_err(|_| StoreError::Corrupt("secrets.scope".to_string()))
}

impl PgStore {
    /// Write a secret and its event in one transaction. The answer has the new fingerprint.
    pub(crate) async fn put_secret_of(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        meta: &WriteMeta,
    ) -> Result<SecretMeta, StoreError> {
        let nonce = new_nonce()?;
        let Sealed { ciphertext: sealed, fingerprint } = self
            .keys
            .seal(scope, name, value, nonce)
            .map_err(|_| StoreError::Unavailable("the secret value cannot be encrypted".to_string()))?;
        let (scope_text, name_text) = (scope.text(), name.as_str());
        let at = to_system(meta.saved_at);
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        tx.execute(
            UPSERT_SECRET,
            &[&scope_text, &name_text, &nonce.as_slice(), &sealed, &fingerprint, &meta.saved_by.as_str(), &at],
        )
        .await?;
        tx.execute(INSERT_EVENT, &[&scope_text, &name_text, &"set", &meta.saved_by.as_str(), &at]).await?;
        tx.execute(PRUNE_EVENTS, &[&scope_text, &name_text, &i64::from(self.options.history_keep)]).await?;
        tx.commit().await?;
        Ok(SecretMeta {
            scope: scope.clone(),
            name: name.clone(),
            fingerprint,
            updated_at: meta.saved_at,
            updated_by: meta.saved_by.clone(),
        })
    }

    /// Delete a secret and write its event, or `NotFound`.
    pub(crate) async fn delete_secret_of(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        meta: &WriteMeta,
    ) -> Result<(), StoreError> {
        let (scope_text, name_text) = (scope.text(), name.as_str());
        let at = to_system(meta.saved_at);
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        let removed =
            tx.execute("DELETE FROM secrets WHERE scope = $1 AND name = $2", &[&scope_text, &name_text]).await?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.execute(INSERT_EVENT, &[&scope_text, &name_text, &"delete", &meta.saved_by.as_str(), &at]).await?;
        tx.execute(PRUNE_EVENTS, &[&scope_text, &name_text, &i64::from(self.options.history_keep)]).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Give the metadata of the secrets of a scope, in the byte order of the name.
    pub(crate) async fn list_secrets_of(&self, scope: &SecretScope) -> Result<Vec<SecretMeta>, StoreError> {
        let conn = self.conn().await?;
        let rows = conn
            .query(
                "SELECT name, fingerprint, updated_by, updated_at FROM secrets WHERE scope = $1 \
                 ORDER BY name COLLATE \"C\"",
                &[&scope.text()],
            )
            .await?;
        rows.iter()
            .map(|row| {
                let name: String = row.try_get(0)?;
                let fingerprint: String = row.try_get(1)?;
                let by: String = row.try_get(2)?;
                let at: SystemTime = row.try_get(3)?;
                Ok(SecretMeta {
                    scope: scope.clone(),
                    name: name_of(&name)?,
                    fingerprint,
                    updated_at: from_system(at),
                    updated_by: Subject::parse(&by)
                        .map_err(|_| StoreError::Corrupt("secrets.updated_by".to_string()))?,
                })
            })
            .collect()
    }

    /// Give the values of the secrets of the scopes. A value that does not decrypt gives
    /// `SecretUnreadable` (2.39).
    pub(crate) async fn read_secrets_of(
        &self,
        scopes: &[SecretScope],
    ) -> Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError> {
        let texts: Vec<String> = scopes.iter().map(SecretScope::text).collect();
        let conn = self.conn().await?;
        let rows = conn
            .query(
                "SELECT scope, name, nonce, ciphertext, fingerprint FROM secrets WHERE scope = ANY($1) \
                 ORDER BY scope COLLATE \"C\", name COLLATE \"C\"",
                &[&texts],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let scope: String = row.try_get(0)?;
            let name: String = row.try_get(1)?;
            let nonce: Vec<u8> = row.try_get(2)?;
            let sealed: Vec<u8> = row.try_get(3)?;
            let fingerprint: String = row.try_get(4)?;
            let (scope, name) = (scope_of(&scope)?, name_of(&name)?);
            let value = self
                .keys
                .open(&scope, &name, &fingerprint, nonce_of(&nonce)?, &sealed)
                .map_err(|_| StoreError::SecretUnreadable)?;
            out.push((scope, name, value));
        }
        Ok(out)
    }

    /// The number of events that the store keeps for a secret. The tests read it.
    #[doc(hidden)]
    pub async fn secret_event_count(&self, scope: &SecretScope, name: &KeyPath) -> Result<u64, StoreError> {
        let conn = self.conn().await?;
        let row = conn
            .query_one(
                "SELECT count(*) FROM secret_events WHERE scope = $1 AND name = $2",
                &[&scope.text(), &name.as_str()],
            )
            .await?;
        let n: i64 = row.try_get(0)?;
        u64::try_from(n).map_err(|_| StoreError::Corrupt("count".to_string()))
    }
}
