// SPDX-License-Identifier: MIT OR Apache-2.0
//! Strict HCL configuration engine (section 13).
//!
//! The engine reads a file of the HCL subset of 6.3 and checks it against a key table: a
//! closed schema, no default value, the forms `_env` and `_file`, and typed values. The
//! same key table generates the reference files (6.21).
#![forbid(unsafe_code)]

pub mod check;
pub mod env;
pub mod error;
pub mod fsio;
pub mod kinds;
pub mod reference;
pub mod schema;
pub mod secret;
pub mod syntax;
pub mod value;

pub use check::{
    check_file, check_file_with, check_text, check_text_with, BlockInstance, Checked, ReadMode, ResolvedKey, Source,
};
pub use env::CapturedEnvironment;
pub use error::ConfigError;
pub use reference::{render_hcl, render_markdown, MarkdownSpec};
pub use schema::{Applies, BlockDesc, Forms, KeyDesc, Kind, Profile, Ref, Schema, Term, TextRule};
pub use secret::Secret;
pub use value::Value;
