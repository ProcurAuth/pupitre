// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `pupitre` binary: the server and the command line tools (1.1, 6.24).
//!
//! `main` captures and clears the environment first (6.23). No other code reads it.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let env = pupitre::config::CapturedEnvironment::capture_and_clear();
    ExitCode::from(pupitre::cli::run(std::env::args_os().skip(1), &env))
}
