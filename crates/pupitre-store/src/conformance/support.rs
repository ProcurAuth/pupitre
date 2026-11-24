// SPDX-License-Identifier: MIT OR Apache-2.0
//! Builders for the cases of the suite: keys, documents, metadata and run records.

use std::sync::atomic::{AtomicU64, Ordering};

use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::ids::{EnvId, HostId, PlaybookId, RunId, Subject, UnitId};
use pupitre_core::json::{self, Json};
use pupitre_core::node::{Node, SecretScope};
use pupitre_core::run::{RunKind, RunMode, RunRecord, RunStatus};
use pupitre_core::time::UtcMillis;

use crate::secret::SecretKey;
use crate::types::{Message, WriteMeta};

/// The key of the suite: the vector of S12.
pub const TEST_KEY_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// The key of the suite.
pub fn test_key() -> SecretKey {
    SecretKey::parse(TEST_KEY_HEX).expect("the test key has the form of 2.26")
}

/// A key that is not the key of the suite.
pub fn other_key() -> SecretKey {
    SecretKey::parse(&"ab".repeat(32)).expect("the other key has the form of 2.26")
}

pub(super) fn env(id: &str) -> EnvId {
    EnvId::parse(id).expect("a valid environment id")
}

pub(super) fn env_key(id: &str) -> DocKey {
    DocKey::Environment(env(id))
}

pub(super) fn common_key(id: &str) -> DocKey {
    DocKey::Common(env(id))
}

/// The scope `environment:<env>/unit:<unit>`.
pub(super) fn unit_scope(e: &str, u: &str) -> SecretScope {
    SecretScope::Unit(env(e), UnitId::parse(u).expect("a valid unit id"))
}

/// The scope `environment:<env>`.
pub(super) fn env_scope(e: &str) -> SecretScope {
    SecretScope::Environment(env(e))
}

/// A canonical document `environment` with a label.
pub(super) fn env_doc(label: &str) -> CanonicalDocument {
    CanonicalDocument::from_bytes(json::to_canonical(&Json::object([
        ("format", Json::Int(1)),
        ("label", Json::str(label)),
    ])))
}

/// A canonical object with a number. The store never parses it, so the cases use it for any key.
pub(super) fn numbered_doc(n: u64) -> CanonicalDocument {
    CanonicalDocument::from_bytes(json::to_canonical(&Json::object([("format", Json::Int(1)), ("n", Json::Int(n))])))
}

/// The number of a document of `numbered_doc`.
pub(super) fn number_of(bytes: &[u8]) -> u64 {
    let value = json::parse(bytes, json::ReadLimits::bytes(1024)).expect("the bytes are canonical JSON");
    value.as_object().and_then(|o| o.get("n")).and_then(Json::as_int).expect("the document has a number")
}

/// The metadata of a write.
pub(super) fn meta(by: &str, message: &str) -> WriteMeta {
    WriteMeta {
        saved_by: Subject::parse(by).expect("a valid subject"),
        saved_at: UtcMillis::now(),
        message: Message::parse(message).expect("a valid message"),
    }
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A new run id. The ids rise with the calls, so the order of creation is the order of the ids.
pub(super) fn run_id() -> RunId {
    let n = RUN_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut random = [0u8; 10];
    random[2..].copy_from_slice(&n.to_be_bytes());
    RunId::from_parts(1_795_514_400_000 + n, random)
}

/// A run record with the status `queued` for an environment.
pub(super) fn run_record(environment: &str, mode: RunMode) -> RunRecord {
    RunRecord {
        id: run_id(),
        kind: RunKind::Deploy,
        mode,
        environment: env(environment),
        node: Node::Environment(env(environment)),
        hosts: vec![HostId::parse("host-a").expect("a valid host id")],
        playbook: PlaybookId::parse("deploy-relay").expect("a valid playbook id"),
        command: None,
        params: None,
        check_run: None,
        applied_by: None,
        message: None,
        subject: Subject::parse("alice").expect("a valid subject"),
        created_at: UtcMillis::now(),
        started_at: None,
        finished_at: None,
        status: RunStatus::Queued,
        reason: None,
        snapshot: None,
        runner: None,
        exit_code: None,
        stats: None,
        cancel: None,
        output_truncated: false,
        events_truncated: false,
    }
}

/// The record with a new status.
pub(super) fn with_status(record: &RunRecord, status: RunStatus) -> RunRecord {
    let mut next = record.clone();
    next.status = status;
    if status != RunStatus::Queued {
        next.started_at = Some(UtcMillis::now());
    }
    if status.is_final() {
        next.finished_at = Some(UtcMillis::now());
    }
    next
}
