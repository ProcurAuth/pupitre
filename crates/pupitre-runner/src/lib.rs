// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runner of Pupitre (section 4): the loop of the runner node, the snapshot builder, the
//! child process, the events of the callback plugin and the scrubber.
//!
//! | Module | Paragraphs |
//! | --- | --- |
//! | [`executor`] | 4.1, 4.5 to 4.8, 4.27 to 4.31, 4.37 |
//! | [`snapshot`] | 4.9 to 4.16 |
//! | [`child`] | 4.15, 4.17, 4.18 |
//! | [`process`] | 4.19, 4.29 |
//! | [`events`] | 4.20, 4.22 |
//! | [`scrub`] | 4.21 |
//! | [`settings`] | 6.12 |
#![forbid(unsafe_code)]

pub mod child;
pub mod events;
pub mod executor;
pub mod process;
pub mod scrub;
pub mod settings;
pub mod snapshot;

pub use executor::{RunnerError, RunnerHandle, Wake, start_executor};
pub use process::{ProcessControl, SystemProcess};
pub use settings::RunnerSettings;
