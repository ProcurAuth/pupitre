// SPDX-License-Identifier: MIT OR Apache-2.0
//! The start order of `serve`: the store, the catalog, the access layer, the runner on the
//! runner node, then the listeners. A failure stops the start with the cause, before the server
//! serves (6.2, rule 2).

use std::sync::Arc;
use std::time::Duration;

use pupitre_runner::{RunnerHandle, Wake, start_executor};
use pupitre_store::catalog::Catalog;
use pupitre_store::files::FilesStore;
use pupitre_store::postgres::PgStore;
use pupitre_store::{AnyStore, Store};
use tokio::net::TcpListener;

use super::ServeError;
use super::log::Logger;
use super::state::{AppState, Assets, Parts, Settings};
use super::telemetry::Telemetry;
use crate::access::{AnyAccess, AuthzenAccess, OpenAccess};
use crate::config::{
    AccessConfig, Config, LoadedConfig, LogLevel, ReadMode, RunnerConfig, StorageConfig, TelemetryConfig,
};
use crate::web_assets::{ASSETS, SHELL};

fn start_error(step: &str, cause: impl std::fmt::Display) -> ServeError {
    ServeError::Start(format!("{step}: {cause}"))
}

/// Open the store of the configuration (2.30): the backend `files` or the backend `postgres`.
pub async fn open_store(storage: &StorageConfig) -> Result<AnyStore, ServeError> {
    match storage {
        StorageConfig::Files(options) => {
            let options = options.clone();
            let opened = tokio::task::spawn_blocking(move || FilesStore::open(options))
                .await
                .map_err(|e| start_error("storage", e))?;
            opened.map(AnyStore::Files).map_err(|e| start_error("storage", e))
        }
        StorageConfig::Postgres(options) => {
            PgStore::open(options.clone()).await.map(AnyStore::Postgres).map_err(|e| start_error("storage", e))
        }
    }
}

/// Build the access layer of the configuration (3.2). The mode `authzen` builds the action
/// document of 3.11 once, at start.
pub fn build_access(config: &Config) -> Result<AnyAccess, ServeError> {
    match &config.access {
        AccessConfig::Open => Ok(AnyAccess::Open(OpenAccess)),
        AccessConfig::Authzen(a) => {
            let (runner_enabled, max_hosts) = match &config.runner {
                RunnerConfig::Enabled(r) => (true, u64::from(r.max_hosts)),
                RunnerConfig::Disabled => (false, 1),
            };
            AuthzenAccess::new(a, runner_enabled, max_hosts)
                .map(|a| AnyAccess::Authzen(Box::new(a)))
                .map_err(|e| start_error("access", e))
        }
    }
}

/// The period of the poll of the catalog: the period of the backend (2.22, 4.6).
pub fn poll_interval(storage: &StorageConfig) -> Duration {
    match storage {
        StorageConfig::Files(f) => f.rescan_interval,
        StorageConfig::Postgres(p) => p.poll_interval,
    }
}

/// Load the catalog from the store (2.41). A load above a limit does not stop the start: the
/// catalog stays empty and not ready (`Catalog::limit_error` names the limit), and the catalog
/// task loads again at each period. A store that does not answer stops the start.
pub async fn load_catalog<S: Store>(store: &S, settings: &Settings) -> Result<Catalog, ServeError> {
    let listing = store.load().await.map_err(|e| start_error("catalog", e))?;
    let mut catalog = Catalog::new(settings.limits);
    // An error keeps the empty state; the catalog keeps the limit for `/readyz` and the caller
    // writes the event.
    let _ = catalog.apply_listing(listing);
    Ok(catalog)
}

/// The events that the store holds for the log (6.27): `store.entry_ignored`,
/// `store.external_change` and `store.repaired`.
fn log_store_events(store: &AnyStore, logger: &Logger) {
    use pupitre_store::types::StoreEvent;
    for event in store.drain_events() {
        match event {
            StoreEvent::EntryIgnored { path, reason } => {
                logger.event(LogLevel::Warn, "store.entry_ignored").text("path", &path).text("reason", &reason).emit();
            }
            StoreEvent::ExternalChange { key, before, after } => {
                let mut line = logger.event(LogLevel::Warn, "store.external_change").text("key", &key.text());
                if let Some(e) = before {
                    line = line.text("etag_before", &e.to_hex());
                }
                if let Some(e) = after {
                    line = line.text("etag_after", &e.to_hex());
                }
                line.emit();
            }
            StoreEvent::Repaired { key, version } => {
                logger.event(LogLevel::Warn, "store.repaired").text("key", &key.text()).int("version", version).emit();
            }
        }
    }
}

async fn wait_for_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
        // No handler: the process waits for a kill.
        std::future::pending::<()>().await;
        return "none";
    };
    tokio::select! {
        _ = term.recv() => "SIGTERM",
        _ = int.recv() => "SIGINT",
    }
}

/// The start order of `serve` (the task of L5): store, catalog, access, runner, listeners.
pub async fn serve(loaded: LoadedConfig) -> Result<(), ServeError> {
    if loaded.mode != ReadMode::All {
        return Err(ServeError::Start(
            "serve starts only from a configuration that read each value (6.18)".to_string(),
        ));
    }
    let config = loaded.config;
    let logger = Logger::stderr(config.log_level);
    logger.start_log(&loaded.effective, &loaded.warnings);
    let settings = Settings::from_config(&config);
    // 1. The store.
    let store = Arc::new(open_store(&config.storage).await?);
    // 2. The catalog.
    let catalog = load_catalog(&*store, &settings).await?;
    if let Some(limit) = catalog.limit_error() {
        logger.event(LogLevel::Error, "catalog.limit_exceeded").text("limit", limit).emit();
    }
    // 3. The access layer.
    let access = build_access(&config)?;
    // 4. The runner, on the runner node.
    let wake = Wake::default();
    let capabilities = Arc::new(config.capabilities.clone());
    let mut runner: Option<RunnerHandle> = None;
    let mut execute = false;
    if let RunnerConfig::Enabled(r) = &config.runner
        && let Some(executor) = &r.executor
    {
        execute = true;
        let handle = start_executor(executor.clone(), capabilities.clone(), store.clone(), wake.clone())
            .await
            .map_err(|e| start_error("runner", e))?;
        runner = Some(handle);
    }
    // 5. The listeners.
    let main = TcpListener::bind(config.server.listen).await.map_err(|e| start_error("server.listen", e))?;
    let ops = TcpListener::bind(config.server.ops_listen).await.map_err(|e| start_error("server.ops_listen", e))?;
    let telemetry = match &config.telemetry {
        TelemetryConfig::Disabled => Telemetry::disabled(),
        TelemetryConfig::Enabled { service_name, max_queued_spans, .. } => {
            Telemetry::enabled(service_name, *max_queued_spans)
        }
    };
    let assets = Assets::new(ASSETS, SHELL, &settings.base_path);
    let state = Arc::new(AppState::new(Parts {
        settings,
        access,
        store: store.clone(),
        catalog,
        capabilities,
        wake,
        logger: logger.clone(),
        telemetry,
        assets,
    }));
    state.set_runner_lease(execute);
    let telemetry_config =
        matches!(config.telemetry, TelemetryConfig::Enabled { .. }).then(|| config.telemetry.clone());
    let interval = poll_interval(&config.storage);
    let running = super::start(state.clone(), main, ops, interval, telemetry_config).await?;
    // The log of the events of the store, at the period of the poll.
    let events_store = store.clone();
    let events_logger = logger.clone();
    let events = tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            log_store_events(&events_store, &events_logger);
        }
    });
    let reason = wait_for_signal().await;
    events.abort();
    running.stop(reason).await;
    if let Some(handle) = runner {
        handle.stop(config.server.shutdown_grace).await;
    }
    Ok(())
}
