// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runs of the backend `postgres`: the records, the output and the events (2.27, 2.28).

use pupitre_core::ids::{EnvId, RunId};
use pupitre_core::json::{self, ReadLimits};
use pupitre_core::run::{RunRecord, RunStatus};
use tokio_postgres::error::SqlState;

use super::PgStore;
use super::conn::has_code;
use super::rows::{to_i64, to_u64};
use crate::types::{EventLine, OutputChunk, RunFilter, StoreError};

/// The advisory lock that makes the count of the queue exact (2.27).
const QUEUE_LOCK: i64 = 0x5075_7069_7472_7131;
/// The largest record that the store reads: much more than the bound of a record (4.38).
const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
/// The statuses of 4.39 that end a run.
const FINAL_STATUSES: [&str; 4] = ["passed", "failed", "canceled", "lost"];

/// The canonical bytes of a record (2.4).
pub(crate) fn encode(record: &RunRecord) -> Vec<u8> {
    json::to_canonical(&record.to_json())
}

/// Read the canonical bytes of a record. A record that fails is a defect of the store.
pub(crate) fn decode(bytes: &[u8]) -> Result<RunRecord, StoreError> {
    let value = json::parse(bytes, ReadLimits::bytes(MAX_RECORD_BYTES))
        .map_err(|e| StoreError::Corrupt(format!("runs.record: {e}")))?;
    RunRecord::from_json(&value).map_err(|_| StoreError::Corrupt("runs.record: a member is not valid".to_string()))
}

/// The run id of a row.
pub(crate) fn run_id_of(text: &str) -> Result<RunId, StoreError> {
    RunId::parse(text).map_err(|_| StoreError::Corrupt("runs.id".to_string()))
}

impl PgStore {
    /// Write a new run with the status `queued` (2.27). The count of the queue and the
    /// `applied_by` of a consumed check are exact: the transaction takes a lock for each.
    pub(crate) async fn create_run_of(
        &self,
        record: &RunRecord,
        max_queued: u32,
        consumes: Option<&RunId>,
    ) -> Result<(), StoreError> {
        if record.status != RunStatus::Queued {
            return Err(StoreError::BadTransition { from: RunStatus::Queued, to: record.status });
        }
        let id = record.id.to_string();
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        tx.execute("SELECT pg_advisory_xact_lock($1)", &[&QUEUE_LOCK]).await?;
        let queued: i64 = tx.query_one("SELECT count(*) FROM runs WHERE status = 'queued'", &[]).await?.try_get(0)?;
        if queued >= i64::from(max_queued) {
            return Err(StoreError::QueueFull);
        }
        if let Some(check) = consumes {
            let row = tx.query_opt("SELECT record FROM runs WHERE id = $1 FOR UPDATE", &[&check.to_string()]).await?;
            let Some(row) = row else { return Err(StoreError::NotFound) };
            let bytes: Vec<u8> = row.try_get(0)?;
            let mut consumed = decode(&bytes)?;
            if consumed.applied_by.is_some() {
                return Err(StoreError::CheckUsed);
            }
            consumed.applied_by = Some(record.id);
            tx.execute("UPDATE runs SET record = $2 WHERE id = $1", &[&check.to_string(), &encode(&consumed)]).await?;
        }
        let inserted = tx
            .execute(
                "INSERT INTO runs (id, environment, status, record) VALUES ($1, $2, 'queued', $3)",
                &[&id, &record.environment.as_str(), &encode(record)],
            )
            .await;
        match inserted {
            Ok(_) => {}
            Err(e) if has_code(&e, &SqlState::UNIQUE_VIOLATION) => return Err(StoreError::AlreadyExists),
            Err(e) => return Err(e.into()),
        }
        tx.commit().await?;
        Ok(())
    }

    /// Give a run record, or `NotFound`.
    pub(crate) async fn get_run_of(&self, id: &RunId) -> Result<RunRecord, StoreError> {
        let conn = self.conn().await?;
        let row = conn.query_opt("SELECT record FROM runs WHERE id = $1", &[&id.to_string()]).await?;
        let Some(row) = row else { return Err(StoreError::NotFound) };
        let bytes: Vec<u8> = row.try_get(0)?;
        decode(&bytes)
    }

    /// Give the runs of the filter, newest first, with an id below `before`, at most `limit`.
    pub(crate) async fn list_runs_of(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        self.list_runs_page_of(filter, before, 0, limit).await
    }

    /// Like `list_runs_of`, with the first `skip` runs left out by the database.
    pub(crate) async fn list_runs_page_of(
        &self,
        filter: &RunFilter,
        before: Option<&RunId>,
        skip: u32,
        limit: u32,
    ) -> Result<Vec<RunRecord>, StoreError> {
        let environment = filter.environment.as_ref().map(|e| e.as_str().to_string());
        let status = filter.status.map(|s| s.name().to_string());
        let before = before.map(ToString::to_string);
        let conn = self.conn().await?;
        let rows = conn
            .query(
                "SELECT record FROM runs WHERE ($1::text IS NULL OR environment = $1) \
                 AND ($2::text IS NULL OR status = $2) AND ($3::text IS NULL OR id COLLATE \"C\" < $3) \
                 ORDER BY id COLLATE \"C\" DESC LIMIT $4 OFFSET $5",
                &[&environment, &status, &before, &i64::from(limit), &i64::from(skip)],
            )
            .await?;
        rows.iter().map(|r| decode(&r.try_get::<_, Vec<u8>>(0)?)).collect()
    }

    /// Give the oldest queued runs whose environment is not in `busy`, at most `limit`.
    pub(crate) async fn next_queued_of(&self, busy: &[EnvId], limit: u32) -> Result<Vec<RunRecord>, StoreError> {
        let busy: Vec<String> = busy.iter().map(|e| e.as_str().to_string()).collect();
        let conn = self.conn().await?;
        let rows = conn
            .query(
                "SELECT record FROM runs WHERE status = 'queued' AND NOT (environment = ANY($1)) \
                 ORDER BY id COLLATE \"C\" ASC LIMIT $2",
                &[&busy, &i64::from(limit)],
            )
            .await?;
        rows.iter().map(|r| decode(&r.try_get::<_, Vec<u8>>(0)?)).collect()
    }

    /// Replace a record when its stored status is `expected` (2.27, 4.39).
    pub(crate) async fn update_run_of(&self, expected: RunStatus, record: &RunRecord) -> Result<(), StoreError> {
        let id = record.id.to_string();
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        let row = tx.query_opt("SELECT status FROM runs WHERE id = $1 FOR UPDATE", &[&id]).await?;
        let Some(row) = row else { return Err(StoreError::NotFound) };
        let stored: String = row.try_get(0)?;
        if stored != expected.name() {
            return Err(StoreError::Conflict { current: None });
        }
        if record.status != expected && !expected.can_move_to(record.status) {
            return Err(StoreError::BadTransition { from: expected, to: record.status });
        }
        tx.execute(
            "UPDATE runs SET status = $2, environment = $3, record = $4 WHERE id = $1",
            &[&id, &record.status.name(), &record.environment.as_str(), &encode(record)],
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Remove the oldest final runs above `keep`, with their output and events (2.27). A run
    /// with the status `queued` or `running` stays.
    pub(crate) async fn prune_runs_of(&self, keep: u32) -> Result<Vec<RunId>, StoreError> {
        let finals: Vec<String> = FINAL_STATUSES.iter().map(ToString::to_string).collect();
        let conn = self.conn().await?;
        let rows = conn
            .query(
                "DELETE FROM runs WHERE id IN (SELECT id FROM runs WHERE status = ANY($1) \
                 ORDER BY id COLLATE \"C\" DESC OFFSET $2) RETURNING id",
                &[&finals, &i64::from(keep)],
            )
            .await?;
        let mut ids =
            rows.iter().map(|r| run_id_of(&r.try_get::<_, String>(0)?)).collect::<Result<Vec<RunId>, StoreError>>()?;
        ids.sort();
        Ok(ids)
    }

    /// Add output bytes at `offset`, the current length (2.28). Give the new length.
    pub(crate) async fn append_output_of(&self, id: &RunId, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        let id = id.to_string();
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        lock_run(&tx, &id).await?;
        let last = tx
            .query_opt(
                "SELECT seq, byte_offset + octet_length(bytes) FROM run_output WHERE run_id = $1 \
                 ORDER BY seq DESC LIMIT 1",
                &[&id],
            )
            .await?;
        let (seq, length) = match last {
            Some(row) => (row.try_get::<_, i64>(0)?, to_u64(row.try_get::<_, i64>(1)?, "run_output.length")?),
            None => (0, 0),
        };
        if offset != length {
            return Err(StoreError::Conflict { current: None });
        }
        if bytes.is_empty() {
            return Ok(length);
        }
        let new_length = length
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| StoreError::Corrupt("the output is too long".to_string()))?;
        tx.execute(
            "INSERT INTO run_output (run_id, seq, byte_offset, bytes) VALUES ($1, $2, $3, $4)",
            &[&id, &(seq + 1), &to_i64(length, "offset")?, &bytes],
        )
        .await?;
        tx.commit().await?;
        Ok(new_length)
    }

    /// Give output bytes from `offset`, at most `max` bytes, and the current length.
    pub(crate) async fn read_output_of(&self, id: &RunId, offset: u64, max: u32) -> Result<OutputChunk, StoreError> {
        let id = id.to_string();
        let end = offset.saturating_add(u64::from(max));
        let mut conn = self.conn().await?;
        let tx = conn
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .await?;
        if tx.query_opt("SELECT 1 FROM runs WHERE id = $1", &[&id]).await?.is_none() {
            return Err(StoreError::NotFound);
        }
        let last = tx
            .query_opt(
                "SELECT byte_offset + octet_length(bytes) FROM run_output WHERE run_id = $1 \
                 ORDER BY seq DESC LIMIT 1",
                &[&id],
            )
            .await?;
        let length = match last {
            Some(row) => to_u64(row.try_get::<_, i64>(0)?, "run_output.length")?,
            None => 0,
        };
        let rows = tx
            .query(
                "SELECT byte_offset, bytes FROM run_output WHERE run_id = $1 \
                 AND byte_offset + octet_length(bytes) > $2 AND byte_offset < $3 ORDER BY seq",
                &[&id, &to_i64(offset, "offset")?, &to_i64(end.min(length), "offset")?],
            )
            .await?;
        tx.commit().await?;
        let mut out = Vec::new();
        for row in &rows {
            let start = to_u64(row.try_get::<_, i64>(0)?, "run_output.byte_offset")?;
            let chunk: Vec<u8> = row.try_get(1)?;
            let skip = usize::try_from(offset.saturating_sub(start)).unwrap_or(usize::MAX).min(chunk.len());
            let room = usize::try_from(max).unwrap_or(usize::MAX).saturating_sub(out.len());
            out.extend_from_slice(&chunk[skip..chunk.len().min(skip.saturating_add(room))]);
        }
        Ok(OutputChunk { bytes: out, length })
    }

    /// Add event lines with the numbers from `first_seq`, the next number (2.28).
    pub(crate) async fn append_events_of(
        &self,
        id: &RunId,
        first_seq: u64,
        lines: &[Vec<u8>],
    ) -> Result<(), StoreError> {
        let id = id.to_string();
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await?;
        lock_run(&tx, &id).await?;
        let last: i64 =
            tx.query_one("SELECT coalesce(max(seq), 0) FROM run_events WHERE run_id = $1", &[&id]).await?.try_get(0)?;
        if to_i64(first_seq, "first_seq")? != last + 1 {
            return Err(StoreError::Conflict { current: None });
        }
        if lines.is_empty() {
            return Ok(());
        }
        let mut seqs = Vec::with_capacity(lines.len());
        for n in 0..lines.len() {
            seqs.push(last + 1 + to_i64(u64::try_from(n).unwrap_or(u64::MAX), "line")?);
        }
        tx.execute(
            "INSERT INTO run_events (run_id, seq, line) SELECT $1, s, l FROM unnest($2::bigint[], $3::bytea[]) AS t(s, l)",
            &[&id, &seqs, &lines],
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Give the event lines with a number above `after`, at most `limit`.
    pub(crate) async fn read_events_of(
        &self,
        id: &RunId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<EventLine>, StoreError> {
        let id = id.to_string();
        let conn = self.conn().await?;
        if conn.query_opt("SELECT 1 FROM runs WHERE id = $1", &[&id]).await?.is_none() {
            return Err(StoreError::NotFound);
        }
        let rows = conn
            .query(
                "SELECT seq, line FROM run_events WHERE run_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
                &[&id, &to_i64(after, "after")?, &i64::from(limit)],
            )
            .await?;
        rows.iter()
            .map(|row| {
                let seq: i64 = row.try_get(0)?;
                Ok(EventLine { seq: to_u64(seq, "run_events.seq")?, bytes: row.try_get(1)? })
            })
            .collect()
    }
}

/// Lock the row of a run, so two appends to one run run one after the other. `NotFound` when
/// the run does not exist.
async fn lock_run(tx: &deadpool_postgres::Transaction<'_>, id: &str) -> Result<(), StoreError> {
    let row = tx.query_opt("SELECT 1 FROM runs WHERE id = $1 FOR NO KEY UPDATE", &[&id]).await?;
    if row.is_none() {
        return Err(StoreError::NotFound);
    }
    Ok(())
}
