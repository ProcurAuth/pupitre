// SPDX-License-Identifier: MIT OR Apache-2.0
//! The store of Pupitre: the `Store` trait (2.17), the enum `AnyStore`, the backends `files`
//! (2.31 to 2.35) and `postgres` (2.36 to 2.39), the key of the store and the cipher (2.26,
//! 2.39), the catalog (2.41) and the transfer (2.40). The contract suite of 2.43 is behind the
//! feature `conformance`.
#![forbid(unsafe_code)]

pub mod catalog;
pub mod files;
pub mod postgres;
pub mod secret;
pub mod store;
pub mod transfer;
pub mod types;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use store::{AnyStore, Store};
pub use types::*;
