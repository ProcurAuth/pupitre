// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contract suite of 2.43, behind the feature `conformance`. Each case is a function that
//! takes a `Harness` of one backend. The harness makes a fresh store for each case. A test of
//! the backend calls the case, so each backend passes the same cases.
//!
//! A case fails with a panic, as a test does. This module is test support: no server code
//! calls it.
//!
//! The groups of cases:
//!
//! | Module | Cases |
//! |---|---|
//! | `documents` | S01 to S10 |
//! | `secrets` | S11 to S13 |
//! | `runs` | S14 to S16, S17, S18 |
//! | `transfer` | S19 |
//! | `schema` | S20 |

mod documents;
mod harness;
mod runs;
mod schema;
mod secrets;
mod support;
mod transfer;

pub use documents::*;
pub use harness::{CrashOutcome, FreshOptions, Harness};
pub use runs::*;
pub use schema::*;
pub use secrets::*;
pub use support::{TEST_KEY_HEX, other_key, test_key};
pub use transfer::*;

/// The cases of 2.43, with their one-line rule.
pub const CASES: [(&str, &str); 20] = [
    ("S01", "a create then a get give the same bytes, etag and version 1"),
    ("S02", "a save with an old etag gives Conflict with the current etag and changes nothing"),
    ("S03", "two saves at the same time with one base etag: exactly one succeeds"),
    ("S04", "a create on a current key gives AlreadyExists; a create after a delete gives the next version"),
    ("S05", "a delete writes a version delete; get gives NotFound; a poll reports the key as gone"),
    ("S06", "history gives the versions newest first with pages; version gives the bytes"),
    ("S07", "history_keep removes the oldest versions and keeps the numbers"),
    ("S08", "a crash between the version and the document is repaired at the next open"),
    ("S09", "a poll reports the write of another handle and nothing else"),
    ("S10", "read_environment never shows half of a write"),
    ("S11", "secret write, list, read for a run, delete; no value in the list or the events"),
    ("S12", "the fingerprint of a known key, scope, name and value equals the vector"),
    ("S13", "a wrong storage.secret_key stops the open"),
    ("S14", "create_run above max_queued gives QueueFull; one check gives one apply"),
    ("S15", "update_run with a wrong expected status gives Conflict; a transition outside 4.39 is refused"),
    ("S16", "append_output and append_events with a wrong offset or number give Conflict and add nothing"),
    ("S17", "acquire, held, renew, end, second holder, Lost"),
    ("S18", "prune_runs removes the oldest final runs and never a queued or running run"),
    ("S19", "an export from one backend and an import into the other give the same etags and versions"),
    ("S20", "an old store schema stops the open; migrate makes it current"),
];
