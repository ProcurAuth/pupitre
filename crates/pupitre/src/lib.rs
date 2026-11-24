// SPDX-License-Identifier: MIT OR Apache-2.0
//! The program Pupitre (section 1): the configuration, the command line, the access layer and
//! the HTTP server. The binary `pupitre` calls `cli::run`. The library form lets the tests of
//! `tests/` use the modules.
//!
//! | Module | Paragraphs |
//! | --- | --- |
//! | [`config`] | section 6 |
//! | [`cli`] | 6.24 |
//! | [`access`] | section 3 |
//! | [`server`] | section 5 |
//! | [`web_assets`] | 5.21 to 5.23 |
#![forbid(unsafe_code)]

pub mod access;
pub mod cli;
pub mod config;
pub mod server;
pub mod web_assets;
