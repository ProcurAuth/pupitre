// SPDX-License-Identifier: MIT OR Apache-2.0
//! The limits of the HTTP server (7.10) that are formats or protocol values, not keys of the
//! configuration. The limits that are keys are in `Settings`.

use std::time::Duration;

/// The longest request target (7.10): 8,192 bytes.
pub const MAX_TARGET_BYTES: usize = 8_192;
/// The most headers of one request (7.10).
pub const MAX_HEADERS: usize = 100;
/// The most bytes of all headers of one request (7.10).
pub const MAX_HEADER_BYTES: usize = 16_384;
/// The buffer of the HTTP parser. A head above the limits of 7.10 but below this value gets a
/// JSON answer. A larger head gets the bare answer of the parser.
pub const PARSER_HEAD_BYTES: usize = 65_536;
/// The most headers that the parser reads. One more than `MAX_HEADERS` shows the fault.
pub const PARSER_MAX_HEADERS: usize = 256;
/// The most bytes of one read of the output of a run (5.17).
pub const MAX_OUTPUT_READ: u32 = 1_048_576;
/// The longest cursor of a list (5.6).
pub const MAX_CURSOR_BYTES: usize = 1_024;
/// The most connections of the ops listener.
pub const OPS_MAX_CONNECTIONS: usize = 64;
/// The time to read the headers on the ops listener.
pub const OPS_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// The number of refusal answers that the server writes at the same time above
/// `server.max_connections`.
pub const MAX_REFUSAL_TASKS: usize = 64;
/// The time to write one refusal answer.
pub const REFUSAL_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// The time that the sampling of the run gauges waits for the store at a scrape of `/metrics`.
pub const SCRAPE_STORE_TIMEOUT: Duration = Duration::from_secs(2);
/// The most runs that a scrape counts for the gauge `pupitre_runs_running`.
pub const SCRAPE_MAX_RUNS: u32 = 1_000;
