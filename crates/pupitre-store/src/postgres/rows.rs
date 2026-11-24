// SPDX-License-Identifier: MIT OR Apache-2.0
//! Conversions between the rows of the store schema and the types of the store contract.
//! Each stored value that breaks its form gives `Corrupt` with the place and never the value.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pupitre_core::etag::Etag;
use pupitre_core::ids::Subject;
use pupitre_core::time::UtcMillis;
use tokio_postgres::Row;

use crate::types::{Message, StoreError, VersionKind, VersionMeta, WriteMeta};

/// The time of an instance as the value of a `timestamptz` column.
pub(crate) fn to_system(time: UtcMillis) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(time.0)
}

/// The value of a `timestamptz` column as a time of an instance.
pub(crate) fn from_system(time: SystemTime) -> UtcMillis {
    UtcMillis::from_system(time)
}

/// A number of the contract as a `bigint`. The formats bound each number far below 2^63.
pub(crate) fn to_i64(value: u64, what: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt(format!("{what} is above the range of the store")))
}

/// A `bigint` column as a number of the contract.
pub(crate) fn to_u64(value: i64, what: &str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt(format!("{what} is negative")))
}

/// A `bytea` column of 32 bytes as an etag.
pub(crate) fn etag_of(bytes: &[u8], what: &str) -> Result<Etag, StoreError> {
    let raw: [u8; 32] = bytes.try_into().map_err(|_| StoreError::Corrupt(format!("{what} is not 32 bytes")))?;
    Ok(Etag::from_bytes(raw))
}

/// A nullable `bytea` column of 32 bytes as an optional etag.
pub(crate) fn opt_etag(bytes: Option<Vec<u8>>, what: &str) -> Result<Option<Etag>, StoreError> {
    bytes.map(|b| etag_of(&b, what)).transpose()
}

/// The name of a version kind in the column `kind`.
pub(crate) fn kind_from_name(name: &str) -> Result<VersionKind, StoreError> {
    match name {
        "create" => Ok(VersionKind::Create),
        "save" => Ok(VersionKind::Save),
        "delete" => Ok(VersionKind::Delete),
        "import" => Ok(VersionKind::Import),
        "external" => Ok(VersionKind::External),
        _ => Err(StoreError::Corrupt("document_versions.kind".to_string())),
    }
}

/// The write metadata of a row with the columns `saved_by`, `saved_at` and `message`.
pub(crate) fn write_meta(by: &str, at: SystemTime, message: &str) -> Result<WriteMeta, StoreError> {
    Ok(WriteMeta {
        saved_by: Subject::parse(by).map_err(|_| StoreError::Corrupt("saved_by".to_string()))?,
        saved_at: from_system(at),
        message: Message::parse(message).ok_or_else(|| StoreError::Corrupt("message".to_string()))?,
    })
}

/// The columns of `document_versions` that `version_meta` reads, in this order.
pub(crate) const VERSION_COLUMNS: &str = "version, kind, etag, saved_by, saved_at, message";

/// A row of the columns `VERSION_COLUMNS`.
pub(crate) fn version_meta(row: &Row) -> Result<VersionMeta, StoreError> {
    let version: i64 = row.try_get(0)?;
    let kind: String = row.try_get(1)?;
    let etag: Option<Vec<u8>> = row.try_get(2)?;
    let by: String = row.try_get(3)?;
    let at: SystemTime = row.try_get(4)?;
    let message: String = row.try_get(5)?;
    Ok(VersionMeta {
        version: to_u64(version, "document_versions.version")?,
        kind: kind_from_name(&kind)?,
        etag: opt_etag(etag, "document_versions.etag")?,
        meta: write_meta(&by, at, &message)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_round_trip() {
        let t = UtcMillis(1_795_514_400_123);
        assert_eq!(from_system(to_system(t)), t);
    }

    #[test]
    fn numbers_and_etags_keep_their_range() {
        assert_eq!(to_i64(7, "x"), Ok(7));
        assert!(to_i64(u64::MAX, "x").is_err());
        assert!(to_u64(-1, "x").is_err());
        assert!(etag_of(&[0u8; 31], "x").is_err());
        assert!(etag_of(&[0u8; 32], "x").is_ok());
        assert!(kind_from_name("other").is_err());
        assert_eq!(kind_from_name("import"), Ok(VersionKind::Import));
    }
}
