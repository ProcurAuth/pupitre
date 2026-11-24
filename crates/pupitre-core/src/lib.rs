// SPDX-License-Identifier: MIT OR Apache-2.0
//! The core of Pupitre: the formats and the rules that the store, the runner and the server
//! share. The crate has no I/O.
//!
//! | Module | Paragraphs |
//! | --- | --- |
//! | [`ids`] | 2.1 |
//! | [`json`] | 2.4, 2.5 |
//! | [`etag`] | 2.6 |
//! | [`doc`] | 2.2, 2.3, 2.7 to 2.10 |
//! | [`schema`] | 2.11, 2.12 |
//! | [`validate`] | 2.10, 2.15, 2.25 |
//! | [`layers`] | 2.13, 2.14, 4.12 |
//! | [`node`] | 2.24, 3.7, 3.8 |
//! | [`action`] | 3.9, 3.10 |
//! | [`command`] | 4.32 to 4.34 |
//! | [`capability`] | 4.2, 5.15 |
//! | [`run`] | 4.3, 4.4, 4.38, 4.39 |
//! | [`manifest`] | 4.16 |
//! | [`time`] | 5.3 |
//! | [`text`] | 2.1, 2.12, 7.3 |
#![forbid(unsafe_code)]

pub mod action;
pub mod capability;
pub mod command;
pub mod doc;
pub mod error;
pub mod etag;
pub mod ids;
pub mod json;
pub mod layers;
pub mod manifest;
pub mod node;
pub mod run;
pub mod schema;
pub mod text;
pub mod time;
pub mod validate;

pub use error::FieldError;
pub use etag::Etag;
pub use json::Json;
