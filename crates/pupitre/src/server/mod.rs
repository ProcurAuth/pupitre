// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTTP server (section 5): the two listeners, the routes, the write guard, the headers,
//! the limits, the log, the metrics and the telemetry.
//!
//! | Module | Paragraphs |
//! | --- | --- |
//! | `routes` | 5.2, 5.3, 3.12 |
//! | `pipeline` | 3.13 |
//! | `handlers` | 5.5 to 5.19, 5.21 to 5.23 |
//! | `guard`, `headers` | 7.4, 7.5, 5.5 |
//! | `conn`, `limits` | 7.10 |
//! | `ops` | 5.20 |
//! | `log`, `metrics`, `telemetry` | 6.26 to 6.29 |
//! | `startup` | the start order of `serve` |
//!
//! The server is generic over the access layer and the store, so a test uses a fake of each. The
//! program uses `AnyAccess` and `AnyStore`.

mod conn;
mod ctx;
pub mod error;
mod guard;
mod handlers;
pub mod headers;
pub mod limits;
pub mod log;
pub mod metrics;
pub mod ops;
pub mod options;
mod pipeline;
pub mod query;
mod response;
pub mod routes;
pub mod startup;
pub mod state;
pub mod telemetry;

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use pupitre_store::Store;
use pupitre_store::types::{Delta, Listing, Marks};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use state::{AppState, Assets, Parts, Settings};

use crate::access::Access;
use crate::config::{LoadedConfig, LogLevel, TelemetryConfig};

/// An error that stops `serve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeError {
    /// The store, the access layer, the runner or a listener could not start. The text names the
    /// cause.
    Start(String),
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServeError::Start(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for ServeError {}

/// A server that runs: the state, the bound addresses and the handles that the stop needs.
pub struct Running<A, S> {
    /// The shared state.
    pub state: Arc<AppState<A, S>>,
    /// The bound address of `server.listen`.
    pub listen_addr: SocketAddr,
    /// The bound address of `server.ops_listen`.
    pub ops_addr: SocketAddr,
    stop_accept: watch::Sender<bool>,
    close_connections: watch::Sender<bool>,
    stop_tasks: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

/// The catalog task (2.41): it polls the store at the period of the backend and puts the delta
/// in the catalog. A failed poll makes the check `store` of `/readyz` fail.
async fn catalog_task<A: Access + 'static, S: Store + 'static>(
    state: Arc<AppState<A, S>>,
    interval: Duration,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = stop.changed() => {
                if *stop.borrow() { return; }
                continue;
            }
        }
        // A catalog that never loaded (the store was above a limit at start) loads again. The
        // other catalogs poll (2.41).
        let (marks, reload) = {
            let catalog = state.catalog();
            let marks = catalog.marks().clone();
            let reload = !catalog.is_ready() && marks == Marks::default();
            (marks, reload)
        };
        let span = state.telemetry.start_trace("store.poll");
        let fetched = if reload {
            state.store.load().await.map(Fetched::Listing)
        } else {
            state.store.poll(&marks).await.map(Fetched::Delta)
        };
        span.end(fetched.is_err());
        match fetched {
            Ok(fetched) => {
                state.set_store_ok(true);
                let applied = match fetched {
                    Fetched::Listing(listing) => state.catalog_mut().apply_listing(listing),
                    Fetched::Delta(delta) => state.catalog_mut().apply_delta(delta),
                };
                if let Err(e) = applied {
                    state.logger.event(LogLevel::Error, "catalog.limit_exceeded").text("limit", e.limit).emit();
                }
            }
            Err(e) => {
                state.set_store_ok(false);
                state.metrics.store_error("poll");
                state.logger.event(LogLevel::Error, "runner.store_error").text("error", &e.to_string()).emit();
            }
        }
    }
}

/// What a round of the catalog task reads from the store.
enum Fetched {
    Listing(Listing),
    Delta(Delta),
}

/// Start the two listeners and the tasks of the server from a state and two bound listeners.
/// The caller made the store, the catalog, the access layer and the runner before (the start
/// order of `serve`).
pub async fn start<A: Access + 'static, S: Store + 'static>(
    state: Arc<AppState<A, S>>,
    main: TcpListener,
    ops: TcpListener,
    poll_interval: Duration,
    telemetry: Option<TelemetryConfig>,
) -> Result<Running<A, S>, ServeError> {
    let listen_addr = main.local_addr().map_err(|e| ServeError::Start(format!("server.listen: {e}")))?;
    let ops_addr = ops.local_addr().map_err(|e| ServeError::Start(format!("server.ops_listen: {e}")))?;
    let (stop_accept, stop_accept_rx) = watch::channel(false);
    let (close_connections, close_rx) = watch::channel(false);
    let (stop_tasks, stop_tasks_rx) = watch::channel(false);
    let mut tasks = vec![
        tokio::spawn(conn::accept_loop(main, state.clone(), stop_accept_rx.clone(), close_rx.clone())),
        tokio::spawn(ops::ops_accept_loop(ops, state.clone(), stop_accept_rx, close_rx)),
        tokio::spawn(catalog_task(state.clone(), poll_interval, stop_tasks_rx.clone())),
    ];
    if let Some(config) = telemetry {
        tasks.push(tokio::spawn(telemetry::export_task(
            state.telemetry.clone(),
            config,
            state.logger.clone(),
            stop_tasks_rx,
        )));
    }
    state
        .logger
        .event(LogLevel::Info, "server.start")
        .text("version", state.settings.version)
        .text("listen", &listen_addr.to_string())
        .text("ops_listen", &ops_addr.to_string())
        .emit();
    Ok(Running { state, listen_addr, ops_addr, stop_accept, close_connections, stop_tasks, tasks })
}

impl<A: Access + 'static, S: Store + 'static> Running<A, S> {
    /// The graceful stop: stop to accept, wait for the open connections at most
    /// `server.shutdown_grace`, then close them and stop the tasks.
    pub async fn stop(self, reason: &str) {
        let _ = self.stop_accept.send(true);
        let deadline = tokio::time::Instant::now() + self.state.settings.shutdown_grace;
        while self.state.open_connections() + self.state.inflight_requests() > 0
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = self.close_connections.send(true);
        let _ = self.stop_tasks.send(true);
        for task in self.tasks {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
        self.state.logger.event(LogLevel::Info, "server.stop").text("reason", reason).emit();
    }
}

/// Start the server from a complete configuration (6.18). The steps are: open the store, load
/// the catalog, build the access layer, start the runner on the runner node, then open the
/// listeners. The function returns after a graceful stop.
pub async fn serve(loaded: LoadedConfig) -> Result<(), ServeError> {
    startup::serve(loaded).await
}
