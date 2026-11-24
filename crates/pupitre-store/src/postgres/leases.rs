// SPDX-License-Identifier: MIT OR Apache-2.0
//! The leases of the backend `postgres` (2.29). The clock of the database decides.

use std::time::{Duration, SystemTime};

use pupitre_core::time::UtcMillis;

use super::PgStore;
use super::rows::{from_system, to_i64};
use crate::types::{LeaseName, LeaseOutcome, LeaseRenewal, StoreError};

/// The end time of a lease as milliseconds since the epoch of the database clock.
const UNTIL: &str = "(extract(epoch from expires_at) * 1000)::bigint";

fn ttl_ms(ttl: Duration) -> Result<i64, StoreError> {
    to_i64(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX), "ttl")
}

fn until_of(ms: i64) -> Result<UtcMillis, StoreError> {
    let ms = u64::try_from(ms).map_err(|_| StoreError::Corrupt("leases.expires_at".to_string()))?;
    Ok(UtcMillis(ms))
}

impl PgStore {
    /// Take a lease when it has no holder, when it ended, or when the holder is the same.
    pub(crate) async fn acquire_lease_of(
        &self,
        name: &LeaseName,
        holder: &str,
        ttl: Duration,
    ) -> Result<LeaseOutcome, StoreError> {
        let name = name.text();
        let conn = self.conn().await?;
        // The holder can release the lease between the two statements. Then the first statement
        // takes it at the second try. Three tries are enough: a lease that goes on changing hands
        // is held by another holder at the time of the answer.
        for _ in 0..3 {
            let taken = conn
                .query_opt(
                    &format!(
                        "INSERT INTO leases (name, holder, expires_at) \
                         VALUES ($1, $2, clock_timestamp() + $3::bigint * interval '1 millisecond') \
                         ON CONFLICT (name) DO UPDATE SET holder = EXCLUDED.holder, expires_at = EXCLUDED.expires_at \
                         WHERE leases.expires_at <= clock_timestamp() OR leases.holder = EXCLUDED.holder \
                         RETURNING {UNTIL}"
                    ),
                    &[&name, &holder, &ttl_ms(ttl)?],
                )
                .await?;
            if let Some(row) = taken {
                return Ok(LeaseOutcome::Acquired { until: until_of(row.try_get(0)?)? });
            }
            let row = conn.query_opt(&format!("SELECT holder, {UNTIL} FROM leases WHERE name = $1"), &[&name]).await?;
            if let Some(row) = row {
                return Ok(LeaseOutcome::HeldBy { holder: row.try_get(0)?, until: until_of(row.try_get(1)?)? });
            }
        }
        Err(StoreError::Conflict { current: None })
    }

    /// Move the end time of a lease that the holder has and that did not end.
    pub(crate) async fn renew_lease_of(
        &self,
        name: &LeaseName,
        holder: &str,
        ttl: Duration,
    ) -> Result<LeaseRenewal, StoreError> {
        let conn = self.conn().await?;
        let row = conn
            .query_opt(
                &format!(
                    "UPDATE leases SET expires_at = clock_timestamp() + $3::bigint * interval '1 millisecond' \
                     WHERE name = $1 AND holder = $2 AND expires_at > clock_timestamp() RETURNING {UNTIL}"
                ),
                &[&name.text(), &holder, &ttl_ms(ttl)?],
            )
            .await?;
        match row {
            Some(row) => Ok(LeaseRenewal::Renewed { until: until_of(row.try_get(0)?)? }),
            None => Ok(LeaseRenewal::Lost),
        }
    }

    /// Remove a lease that the holder has.
    pub(crate) async fn release_lease_of(&self, name: &LeaseName, holder: &str) -> Result<(), StoreError> {
        let conn = self.conn().await?;
        conn.execute("DELETE FROM leases WHERE name = $1 AND holder = $2", &[&name.text(), &holder]).await?;
        Ok(())
    }

    /// The clock of the database, for the tests.
    #[doc(hidden)]
    pub async fn database_time(&self) -> Result<UtcMillis, StoreError> {
        let conn = self.conn().await?;
        let row = conn.query_one("SELECT clock_timestamp()", &[]).await?;
        let now: SystemTime = row.try_get(0)?;
        Ok(from_system(now))
    }
}
