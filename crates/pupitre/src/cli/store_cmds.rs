// SPDX-License-Identifier: MIT OR Apache-2.0
//! The store commands of 6.24: `migrate` (2.30), `export` and `copy-store` (2.40).
//!
//! Each function reads the block `storage` of a configuration that the command line loaded in
//! the read mode Storage (6.18), does its work, writes one line on the standard output, and gives
//! the exit code of 6.24: 0 for a success and 1 for a failure. A failure writes its message on
//! the standard error. No command starts a server, and no command runs at start of `serve`.

use std::io::Write;
use std::path::Path;

use pupitre_store::files::{FilesStore, MigrateOutcome};
use pupitre_store::postgres::{self, PgStore};
use pupitre_store::transfer::{ImportReport, copy_store as copy_stores, export_to_dir};
use pupitre_store::{AnyStore, STORE_SCHEMA_VERSION, StoreError};

use super::{EXIT_OK, failure};
use crate::config::{LoadedConfig, StorageConfig};

fn say(line: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

/// Open the store that the block `storage` names. The backend `postgres` checks its schema and
/// its key check. The backend `files` takes its locks.
async fn open(storage: &StorageConfig) -> Result<AnyStore, StoreError> {
    match storage {
        StorageConfig::Files(options) => Ok(AnyStore::Files(FilesStore::open(options.clone())?)),
        StorageConfig::Postgres(options) => Ok(AnyStore::Postgres(PgStore::open(options.clone()).await?)),
    }
}

fn counts(report: &ImportReport) -> String {
    format!("{} keys, {} versions, {} secrets, {} runs", report.keys, report.versions, report.secrets, report.runs)
}

/// `migrate --config <path>`: make or change the store schema, and write the key check (2.30).
pub fn migrate(loaded: &LoadedConfig) -> u8 {
    match &loaded.config.storage {
        StorageConfig::Postgres(options) => {
            let runtime = match super::runtime() {
                Ok(r) => r,
                Err(e) => return failure(format!("migrate: {e}")),
            };
            match runtime.block_on(postgres::migrate(options)) {
                Ok(report) if report.from == Some(report.to) => {
                    say(&format!("migrate: the store schema is {}; nothing to change", report.to));
                    EXIT_OK
                }
                Ok(report) => {
                    let from = report.from.map_or_else(|| "none".to_string(), |v| v.to_string());
                    say(&format!("migrate: the store schema is now {} (it was {from})", report.to));
                    EXIT_OK
                }
                Err(e) => failure(format!("migrate: {e}")),
            }
        }
        StorageConfig::Files(options) => match FilesStore::migrate(options) {
            Ok(MigrateOutcome::Current) => {
                say(&format!("migrate: the store schema is {STORE_SCHEMA_VERSION}; nothing to change"));
                EXIT_OK
            }
            Ok(_) => {
                say(&format!(
                    "migrate: the folders of the backend files are ready (store schema {STORE_SCHEMA_VERSION})"
                ));
                EXIT_OK
            }
            Err(e) => failure(format!("migrate: {e}")),
        },
    }
}

/// `export --config <path> --to <folder>`: write the documents and their versions into an empty
/// folder, in the layout of 2.31 (2.40). It writes no secret and no run.
pub fn export(loaded: &LoadedConfig, to: &Path) -> u8 {
    let runtime = match super::runtime() {
        Ok(r) => r,
        Err(e) => return failure(format!("export: {e}")),
    };
    let result = runtime.block_on(async {
        let store = open(&loaded.config.storage).await?;
        export_to_dir(&store, to).await
    });
    match result {
        Ok(report) => {
            say(&format!("export: {} keys and {} versions written", report.keys, report.versions));
            EXIT_OK
        }
        Err(StoreError::NotEmpty) => failure("export: the folder of --to is not empty"),
        Err(e) => failure(format!("export: {e}")),
    }
}

/// `copy-store --from <path> --to <path>`: copy a store into an empty store, then compare the
/// etags of the current documents (2.40). The secrets go through the memory of the process.
pub fn copy_store(from: &LoadedConfig, to: &LoadedConfig) -> u8 {
    let runtime = match super::runtime() {
        Ok(r) => r,
        Err(e) => return failure(format!("copy-store: {e}")),
    };
    let result = runtime.block_on(async {
        let source = open(&from.config.storage).await.map_err(|e| format!("--from: {e}"))?;
        let target = open(&to.config.storage).await.map_err(|e| format!("--to: {e}"))?;
        copy_stores(&source, &target).await.map_err(|e| match e {
            StoreError::NotEmpty => "the store of --to is not empty".to_string(),
            other => other.to_string(),
        })
    });
    match result {
        Ok(report) => {
            say(&format!("copy-store: copied {}; the etags are the same", counts(&report)));
            EXIT_OK
        }
        Err(message) => failure(format!("copy-store: {message}")),
    }
}
