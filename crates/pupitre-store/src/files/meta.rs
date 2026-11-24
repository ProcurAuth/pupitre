// SPDX-License-Identifier: MIT OR Apache-2.0
//! The small canonical files of the backend `files` (2.31): the metadata of a version, the
//! meta file of the documents folder, and the meta file of the secrets folder.

use pupitre_core::etag::Etag;
use pupitre_core::ids::Subject;
use pupitre_core::json::{self, Json, Members, ReadLimits};
use pupitre_core::time::UtcMillis;

use crate::types::{Message, STORE_SCHEMA_VERSION, StoreError, VersionKind, WriteMeta};

/// The largest size of a small file that the backend reads.
pub(crate) const MAX_SMALL_FILE: usize = 65_536;

/// The kind, the etag and the metadata of one version (2.18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionRecord {
    pub(crate) kind: VersionKind,
    pub(crate) etag: Option<Etag>,
    pub(crate) meta: WriteMeta,
}

/// The kind of a name.
pub(crate) fn kind_from_name(name: &str) -> Option<VersionKind> {
    [VersionKind::Create, VersionKind::Save, VersionKind::Delete, VersionKind::Import, VersionKind::External]
        .into_iter()
        .find(|k| k.name() == name)
}

fn ten_digits(text: &str) -> Option<u64> {
    (text.len() == 10 && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

/// The file name of the metadata of a version (2.31).
pub(crate) fn meta_file(version: u64) -> String {
    format!("{version:010}.meta.json")
}

/// The number of a name `<10 digits>.meta.json`.
pub(crate) fn parse_meta_name(name: &str) -> Option<u64> {
    ten_digits(name.strip_suffix(".meta.json")?)
}

/// The number of a name `<10 digits>.json`.
pub(crate) fn parse_bytes_name(name: &str) -> Option<u64> {
    ten_digits(name.strip_suffix(".json")?)
}

/// The canonical bytes of the metadata of a version.
pub(crate) fn encode_version(rec: &VersionRecord) -> Vec<u8> {
    let mut pairs = vec![
        ("kind", Json::str(rec.kind.name())),
        ("message", Json::str(rec.meta.message.as_str())),
        ("saved_at", Json::str(rec.meta.saved_at.to_string())),
        ("saved_by", Json::str(rec.meta.saved_by.as_str())),
    ];
    if let Some(etag) = &rec.etag {
        pairs.push(("etag", Json::str(etag.to_hex())));
    }
    json::to_canonical(&Json::object(pairs))
}

fn corrupt(place: &str, what: &str) -> StoreError {
    StoreError::Corrupt(format!("{place}: {what}"))
}

fn read_object(bytes: &[u8], place: &str) -> Result<Members, StoreError> {
    let value = json::parse(bytes, ReadLimits::bytes(MAX_SMALL_FILE)).map_err(|e| corrupt(place, &e.to_string()))?;
    Members::of(&value).map_err(|e| corrupt(place, &e))
}

fn text(m: &mut Members, name: &str, place: &str) -> Result<String, StoreError> {
    match m.take(name) {
        Some(Json::Str(t)) => Ok(t),
        _ => Err(corrupt(place, &format!("the member {name} is not a string"))),
    }
}

fn no_more(m: Members, place: &str) -> Result<(), StoreError> {
    match m.finish().first() {
        Some(name) => Err(corrupt(place, &format!("the member {name} is not known"))),
        None => Ok(()),
    }
}

/// Read the metadata of a version. `place` names the file for the error.
pub(crate) fn decode_version(bytes: &[u8], place: &str) -> Result<VersionRecord, StoreError> {
    let mut m = read_object(bytes, place)?;
    let kind = kind_from_name(&text(&mut m, "kind", place)?).ok_or_else(|| corrupt(place, "the kind is not known"))?;
    let message = Message::parse(&text(&mut m, "message", place)?).ok_or_else(|| corrupt(place, "the message"))?;
    let saved_at = UtcMillis::parse(&text(&mut m, "saved_at", place)?).ok_or_else(|| corrupt(place, "saved_at"))?;
    let saved_by = Subject::parse(&text(&mut m, "saved_by", place)?).map_err(|_| corrupt(place, "saved_by"))?;
    let etag = match m.take("etag") {
        None => None,
        Some(Json::Str(t)) => Some(Etag::parse_hex(&t).map_err(|_| corrupt(place, "etag"))?),
        Some(_) => return Err(corrupt(place, "the member etag is not a string")),
    };
    no_more(m, place)?;
    if (kind == VersionKind::Delete) != etag.is_none() {
        return Err(corrupt(place, "a version delete has no etag, and each other version has one"));
    }
    Ok(VersionRecord { kind, etag, meta: WriteMeta { saved_by, saved_at, message } })
}

/// The bytes of the meta file of the documents folder (2.31).
pub(crate) fn encode_store_meta(schema: u32) -> Vec<u8> {
    json::to_canonical(&Json::object([("format", Json::Int(1)), ("store_schema", Json::Int(u64::from(schema)))]))
}

/// Read the meta file of the documents folder. Give the store schema.
pub(crate) fn decode_store_meta(bytes: &[u8], place: &str) -> Result<u32, StoreError> {
    let mut m = read_object(bytes, place)?;
    if m.take("format") != Some(Json::Int(1)) {
        return Err(corrupt(place, "expected format 1"));
    }
    let schema = match m.take("store_schema") {
        Some(Json::Int(n)) => u32::try_from(n).map_err(|_| corrupt(place, "store_schema"))?,
        _ => return Err(corrupt(place, "the member store_schema is not an integer")),
    };
    no_more(m, place)?;
    Ok(schema)
}

/// The bytes of the meta file of the secrets folder (2.31).
pub(crate) fn encode_secrets_meta(key_check: &str) -> Vec<u8> {
    json::to_canonical(&Json::object([("format", Json::Int(1)), ("key_check", Json::str(key_check))]))
}

/// Read the meta file of the secrets folder. Give the key check.
pub(crate) fn decode_secrets_meta(bytes: &[u8], place: &str) -> Result<String, StoreError> {
    let mut m = read_object(bytes, place)?;
    if m.take("format") != Some(Json::Int(1)) {
        return Err(corrupt(place, "expected format 1"));
    }
    let check = text(&mut m, "key_check", place)?;
    no_more(m, place)?;
    Ok(check)
}

/// The store schema of this binary, for the checks at open.
pub(crate) const CURRENT_SCHEMA: u32 = STORE_SCHEMA_VERSION;

/// The metadata of a secret: the time and the subject of the last write.
pub(crate) fn encode_secret_meta(at: UtcMillis, by: &Subject) -> Vec<u8> {
    json::to_canonical(&Json::object([("saved_at", Json::str(at.to_string())), ("saved_by", Json::str(by.as_str()))]))
}

/// Read the metadata of a secret.
pub(crate) fn decode_secret_meta(bytes: &[u8], place: &str) -> Result<(UtcMillis, Subject), StoreError> {
    let mut m = read_object(bytes, place)?;
    let at = UtcMillis::parse(&text(&mut m, "saved_at", place)?).ok_or_else(|| corrupt(place, "saved_at"))?;
    let by = Subject::parse(&text(&mut m, "saved_by", place)?).map_err(|_| corrupt(place, "saved_by"))?;
    no_more(m, place)?;
    Ok((at, by))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_meta_round_trip_and_bytes() {
        let rec = VersionRecord {
            kind: VersionKind::Save,
            etag: Some(Etag::of(b"x")),
            meta: WriteMeta {
                saved_by: Subject::parse("alice").unwrap(),
                saved_at: UtcMillis::parse("2026-11-24T10:00:00.000Z").unwrap(),
                message: Message::parse("why").unwrap(),
            },
        };
        let bytes = encode_version(&rec);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert_eq!(
            text,
            format!(
                "{{\n  \"etag\": \"{}\",\n  \"kind\": \"save\",\n  \"message\": \"why\",\n  \"saved_at\": \"2026-11-24T10:00:00.000Z\",\n  \"saved_by\": \"alice\"\n}}\n",
                Etag::of(b"x")
            )
        );
        assert_eq!(decode_version(&bytes, "m").unwrap(), rec);
    }

    #[test]
    fn a_delete_has_no_etag() {
        let rec = VersionRecord {
            kind: VersionKind::Delete,
            etag: None,
            meta: WriteMeta {
                saved_by: Subject::anonymous(),
                saved_at: UtcMillis(0),
                message: Message::parse("").unwrap(),
            },
        };
        assert!(!String::from_utf8(encode_version(&rec)).unwrap().contains("etag"));
        let mut bad = rec.clone();
        bad.etag = Some(Etag::of(b"x"));
        assert!(decode_version(&encode_version(&bad), "m").is_err());
        assert!(decode_version(b"{}\n", "m").is_err());
    }

    #[test]
    fn names() {
        assert_eq!(meta_file(12), "0000000012.meta.json");
        assert_eq!(parse_meta_name("0000000012.meta.json"), Some(12));
        assert_eq!(parse_meta_name("12.meta.json"), None);
        assert_eq!(parse_bytes_name("0000000012.json"), Some(12));
        assert_eq!(parse_bytes_name("0000000012.meta.json"), None);
        assert_eq!(parse_bytes_name("00000000a2.json"), None);
    }

    #[test]
    fn store_meta() {
        assert_eq!(encode_store_meta(1), b"{\n  \"format\": 1,\n  \"store_schema\": 1\n}\n");
        assert_eq!(decode_store_meta(&encode_store_meta(7), "p").unwrap(), 7);
        assert!(decode_store_meta(b"{\"format\":1}", "p").is_err());
        let check = "ab".repeat(32);
        assert_eq!(decode_secrets_meta(&encode_secrets_meta(&check), "p").unwrap(), check);
    }
}
