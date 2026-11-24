// SPDX-License-Identifier: MIT OR Apache-2.0
//! The types of the store contract (2.17 to 2.30) and its errors.

use std::fmt;

use pupitre_core::doc::DocKey;
use pupitre_core::etag::Etag;
use pupitre_core::ids::{EnvId, KeyPath, Subject};
use pupitre_core::node::SecretScope;
use pupitre_core::run::RunStatus;
use pupitre_core::time::UtcMillis;

/// The largest message of a write (2.18).
pub const MAX_MESSAGE_CHARS: usize = 500;

/// The version of the store schema of this binary (2.30).
pub const STORE_SCHEMA_VERSION: u32 = 1;

/// The error of a store call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The key, the run, the secret or the version does not exist.
    NotFound,
    /// The precondition failed (2.20), or a compare and set of a run status failed (2.27), or
    /// an offset or a number of 2.28 is wrong. `current` is the current etag of a document,
    /// when it has one.
    Conflict {
        /// The current etag of the document.
        current: Option<Etag>,
    },
    /// A create of a key with a current document (2.20).
    AlreadyExists,
    /// The queue holds `max_queued` runs (2.27).
    QueueFull,
    /// The check run has `applied_by` (2.27).
    CheckUsed,
    /// A transition outside 4.39.
    BadTransition {
        /// The current status.
        from: RunStatus,
        /// The asked status.
        to: RunStatus,
    },
    /// The store schema is older than this binary, or absent (2.30).
    MigrateNeeded {
        /// The version of the store, or `None` when it has none.
        found: Option<u32>,
    },
    /// The store schema is newer than this binary (2.30).
    StoreNewer {
        /// The version of the store.
        found: u32,
    },
    /// The key check of the store differs from `storage.secret_key` (2.26).
    KeyMismatch,
    /// A folder of the backend `files` is locked by another process (2.32).
    Locked {
        /// The path of the folder.
        path: String,
    },
    /// The target of an import is not empty (2.40).
    NotEmpty,
    /// A secret value does not decrypt (2.39).
    SecretUnreadable,
    /// The store did not answer, or an I/O call failed. The text names the cause, never a
    /// value of a document or a secret.
    Unavailable(String),
    /// The stored bytes break the format of the store. The text names the place.
    Corrupt(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::NotFound => f.write_str("not found"),
            StoreError::Conflict { current: Some(e) } => write!(f, "conflict: the current etag is {e}"),
            StoreError::Conflict { current: None } => f.write_str("conflict"),
            StoreError::AlreadyExists => f.write_str("already exists"),
            StoreError::QueueFull => f.write_str("the queue of runs is full"),
            StoreError::CheckUsed => f.write_str("the check run is already used by an apply"),
            StoreError::BadTransition { from, to } => {
                write!(f, "the status {} cannot change to {}", from.name(), to.name())
            }
            StoreError::MigrateNeeded { .. } => f.write_str("run pupitre migrate"),
            StoreError::StoreNewer { .. } => f.write_str("the store is newer than this binary"),
            StoreError::KeyMismatch => f.write_str("storage.secret_key does not match the key of the store"),
            StoreError::Locked { path } => write!(f, "the folder {path} is locked by another process"),
            StoreError::NotEmpty => f.write_str("the target store is not empty"),
            StoreError::SecretUnreadable => f.write_str("a secret value does not decrypt"),
            StoreError::Unavailable(cause) => write!(f, "the store is not available: {cause}"),
            StoreError::Corrupt(place) => write!(f, "the store has bytes that break its format: {place}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// A message of a write: 0 to 500 characters with no control character except U+000A (2.18).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message(String);

impl Message {
    /// Check the message.
    pub fn parse(text: &str) -> Option<Message> {
        let ok = text.chars().count() <= MAX_MESSAGE_CHARS && !text.chars().any(|c| c.is_control() && c != '\n');
        ok.then(|| Message(text.to_string()))
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The metadata of a write (2.18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteMeta {
    /// The subject.
    pub saved_by: Subject,
    /// The time of the instance.
    pub saved_at: UtcMillis,
    /// The message.
    pub message: Message,
}

/// The kind of a version (2.18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionKind {
    /// `create`
    Create,
    /// `save`
    Save,
    /// `delete`
    Delete,
    /// `import`
    Import,
    /// `external` (2.33)
    External,
}

impl VersionKind {
    /// The name of the kind.
    pub fn name(self) -> &'static str {
        match self {
            VersionKind::Create => "create",
            VersionKind::Save => "save",
            VersionKind::Delete => "delete",
            VersionKind::Import => "import",
            VersionKind::External => "external",
        }
    }
}

/// The precondition of a `put` (2.20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// `If-None-Match: *`
    Create,
    /// `If-Match: "<etag>"`
    Match(Etag),
}

/// The result of a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    /// The new version.
    pub version: u64,
    /// The new etag, or `None` after a delete.
    pub etag: Option<Etag>,
}

/// A current document of the store, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDoc {
    /// The key.
    pub key: DocKey,
    /// The bytes. The catalog checks them (2.41).
    pub bytes: Vec<u8>,
    /// The SHA-256 of the bytes.
    pub etag: Etag,
    /// The version.
    pub version: u64,
}

/// The metadata of one version (2.18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMeta {
    /// The number.
    pub version: u64,
    /// The kind.
    pub kind: VersionKind,
    /// The etag, absent for a delete.
    pub etag: Option<Etag>,
    /// The metadata of the write.
    pub meta: WriteMeta,
}

/// One version with its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// The metadata.
    pub meta: VersionMeta,
    /// The bytes, absent for a delete.
    pub bytes: Option<Vec<u8>>,
}

/// The marks of a state of the store (2.22). They are opaque for the caller.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Marks {
    pub(crate) inner: MarksInner,
}

// The two backends build these marks.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum MarksInner {
    /// No state yet.
    #[default]
    Empty,
    /// The backend `files`: the signature of each file (2.33).
    Files(std::collections::BTreeMap<String, FileSig>),
    /// The backend `postgres`: the highest change number and the time of the poll (2.38).
    Postgres {
        /// The highest `change_seq`.
        change_seq: i64,
    },
}

/// The signature of a file (2.33).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileSig {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) size: u64,
    pub(crate) mtime_ns: i128,
    pub(crate) ctime_ns: i128,
}

/// The result of `load` (2.22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// Each current document.
    pub docs: Vec<StoredDoc>,
    /// The marks of this state.
    pub marks: Marks,
}

/// The result of `poll` (2.22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    /// The documents that are new or changed.
    pub changed: Vec<StoredDoc>,
    /// The keys that are gone.
    pub removed: Vec<DocKey>,
    /// The new marks.
    pub marks: Marks,
}

/// The metadata of a secret (2.24). It never holds the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    /// The scope.
    pub scope: SecretScope,
    /// The name.
    pub name: KeyPath,
    /// The fingerprint (2.26).
    pub fingerprint: String,
    /// The time of the last write.
    pub updated_at: UtcMillis,
    /// The subject of the last write.
    pub updated_by: Subject,
}

/// The filter of `list_runs` (2.27).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunFilter {
    /// Only the runs of this environment.
    pub environment: Option<EnvId>,
    /// Only the runs with this status.
    pub status: Option<RunStatus>,
}

/// A part of the output of a run (2.28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputChunk {
    /// The bytes from the asked offset.
    pub bytes: Vec<u8>,
    /// The current length of the output.
    pub length: u64,
}

/// One stored event line (2.28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLine {
    /// The number of the line, from 1.
    pub seq: u64,
    /// The canonical compact bytes of the line, scrubbed (4.20, 4.21).
    pub bytes: Vec<u8>,
}

/// The name of a lease (2.29).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LeaseName {
    /// `runner` (4.8)
    Runner,
    /// `environment:<env>` (4.7)
    Environment(EnvId),
}

impl LeaseName {
    /// The text of the name.
    pub fn text(&self) -> String {
        match self {
            LeaseName::Runner => "runner".to_string(),
            LeaseName::Environment(e) => format!("environment:{e}"),
        }
    }
}

/// The result of `acquire_lease` (2.29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseOutcome {
    /// The caller holds the lease until the time.
    Acquired {
        /// The end time.
        until: UtcMillis,
    },
    /// Another holder has the lease.
    HeldBy {
        /// The holder.
        holder: String,
        /// The end time.
        until: UtcMillis,
    },
}

/// The result of `renew_lease` (2.29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseRenewal {
    /// The caller holds the lease until the time.
    Renewed {
        /// The end time.
        until: UtcMillis,
    },
    /// The caller does not hold the lease.
    Lost,
}

/// An event that the store gives to the server for its log (6.27).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreEvent {
    /// `store.entry_ignored`
    EntryIgnored {
        /// The path relative to the folder.
        path: String,
        /// The reason.
        reason: String,
    },
    /// `store.external_change` (2.33)
    ExternalChange {
        /// The key.
        key: DocKey,
        /// The etag before the change.
        before: Option<Etag>,
        /// The etag after the change.
        after: Option<Etag>,
    },
    /// `store.repaired`: a version newer than the document at open or at a poll (2.19).
    Repaired {
        /// The key.
        key: DocKey,
        /// The version that the store wrote again.
        version: u64,
    },
}
