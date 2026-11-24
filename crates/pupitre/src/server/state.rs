// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared state of the server: the settings, the access layer, the store, the catalog and
//! the other parts that each request uses.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use bytes::Bytes;
use http::header::HeaderName;
use pupitre_core::capability::Capabilities;
use pupitre_runner::Wake;
use pupitre_store::Store;
use pupitre_store::catalog::{Catalog, CatalogLimits};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};

use super::log::Logger;
use super::metrics::Metrics;
use super::telemetry::Telemetry;
use crate::access::Access;
use crate::config::hosts::HostPattern;
use crate::config::{AccessConfig, Config, RunnerConfig};
use crate::web_assets::Asset;

/// The access mode, with the keys that the server reads (3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeSettings {
    /// The mode `open`. `allow_apply` is `runner.allow_apply_in_open_mode` (3.22).
    Open {
        /// The value of `runner.allow_apply_in_open_mode`, `false` without runner.
        allow_apply: bool,
    },
    /// The mode `authzen`.
    Authzen {
        /// The header of the identity token (3.4).
        header: HeaderName,
        /// `access.options_max_questions`
        options_max_questions: u32,
        /// `access.options_page_items`
        options_page_items: u32,
    },
}

/// The keys of the block `runner` that the server reads (4.4, 4.5, 4.24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerSettings {
    /// `runner.max_hosts` (3.10)
    pub max_hosts: u32,
    /// `runner.check_max_age` (4.24)
    pub check_max_age: Duration,
    /// `runner.max_queued_runs` (4.5)
    pub max_queued_runs: u32,
    /// True on the runner node (`runner.execute`).
    pub execute: bool,
}

/// The typed keys that the server reads. The configuration module builds the structure at
/// start, and the rest of the server reads only this structure (6.2, rule 4).
#[derive(Debug, Clone)]
pub struct Settings {
    /// `server.base_path` (5.2)
    pub base_path: String,
    /// `server.allowed_hosts`, or its default (7.4)
    pub allowed_hosts: Vec<HostPattern>,
    /// `server.max_connections`
    pub max_connections: u32,
    /// `server.idle_timeout`
    pub idle_timeout: Duration,
    /// `server.shutdown_grace`
    pub shutdown_grace: Duration,
    /// `server.header_read_timeout`
    pub header_read_timeout: Duration,
    /// `server.body_read_timeout`
    pub body_read_timeout: Duration,
    /// `server.max_request_bytes`
    pub max_request_bytes: u64,
    /// `server.max_inflight_bodies`
    pub max_inflight_bodies: u32,
    /// `server.max_page_items`
    pub max_page_items: u32,
    /// The access mode.
    pub mode: ModeSettings,
    /// The runner keys, with `runner.enabled = true`.
    pub runner: Option<RunnerSettings>,
    /// The limits of 2.42.
    pub limits: CatalogLimits,
    /// The version of the binary (5.7).
    pub version: &'static str,
}

fn usize_of(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

impl Settings {
    /// Read the keys of the server from the typed configuration.
    pub fn from_config(config: &Config) -> Settings {
        let s = &config.server;
        let mode = match &config.access {
            AccessConfig::Open => ModeSettings::Open {
                allow_apply: match &config.runner {
                    RunnerConfig::Enabled(r) => r.allow_apply_in_open_mode.unwrap_or(false),
                    RunnerConfig::Disabled => false,
                },
            },
            AccessConfig::Authzen(a) => ModeSettings::Authzen {
                // Stage 2 checked that the text is a header name (6.13). A text that fails here
                // cannot happen; the server then reads no token and refuses each request.
                header: HeaderName::from_bytes(a.header.as_bytes())
                    .unwrap_or_else(|_| HeaderName::from_static("x-identity-token-invalid")),
                options_max_questions: a.options_max_questions,
                options_page_items: a.options_page_items,
            },
        };
        let runner = match &config.runner {
            RunnerConfig::Disabled => None,
            RunnerConfig::Enabled(r) => Some(RunnerSettings {
                max_hosts: r.max_hosts,
                check_max_age: r.check_max_age,
                max_queued_runs: r.max_queued_runs,
                execute: r.executor.is_some(),
            }),
        };
        Settings {
            base_path: s.base_path.clone(),
            allowed_hosts: s.allowed_hosts.clone(),
            max_connections: s.max_connections,
            idle_timeout: s.idle_timeout,
            shutdown_grace: s.shutdown_grace,
            header_read_timeout: s.header_read_timeout,
            body_read_timeout: s.body_read_timeout,
            max_request_bytes: s.max_request_bytes,
            max_inflight_bodies: s.max_inflight_bodies,
            max_page_items: s.max_page_items,
            mode,
            runner,
            limits: CatalogLimits {
                max_environments: usize_of(u64::from(config.limits.max_environments)),
                max_units_per_environment: usize_of(u64::from(config.limits.max_units_per_environment)),
                max_hosts_per_environment: usize_of(u64::from(config.limits.max_hosts_per_environment)),
                max_document_bytes: usize_of(config.limits.max_document_bytes),
                max_store_bytes: usize_of(config.limits.max_store_bytes),
            },
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// True in the mode `open`.
    pub fn is_open(&self) -> bool {
        matches!(self.mode, ModeSettings::Open { .. })
    }

    /// The name of the mode (5.7).
    pub fn mode_name(&self) -> &'static str {
        if self.is_open() { "open" } else { "authzen" }
    }

    /// The most questions of one list or options request (3.15, 6.11). The mode `open` asks
    /// none.
    pub fn question_budget(&self) -> u32 {
        match &self.mode {
            ModeSettings::Open { .. } => u32::MAX,
            ModeSettings::Authzen { options_max_questions, .. } => *options_max_questions,
        }
    }

    /// `runner.enabled`
    pub fn runner_enabled(&self) -> bool {
        self.runner.is_some()
    }
}

/// The static files of the page, the shell and the build id (5.23).
#[derive(Debug, Clone)]
pub struct Assets {
    /// The shell with the markers `{{base_path}}` and `{{build}}` replaced.
    pub shell: Bytes,
    /// The first 12 hexadecimal characters of the SHA-256 of all static files.
    pub build: String,
    files: BTreeMap<&'static str, &'static Asset>,
}

impl Assets {
    /// Build the set from the files, the shell and the base path. The build id is the start of
    /// a SHA-256. The hash reads each file in the order of the paths. For each file it reads the
    /// path, a zero byte, the bytes and a zero byte.
    pub fn new(assets: &'static [Asset], shell: &str, base_path: &str) -> Assets {
        let mut hash = Sha256::new();
        for a in assets {
            hash.update(a.path.as_bytes());
            hash.update([0u8]);
            hash.update(a.bytes);
            hash.update([0u8]);
        }
        let digest = hash.finalize();
        let build = pupitre_core::etag::to_hex(&digest)[..12].to_string();
        let shell = shell.replace("{{base_path}}", base_path).replace("{{build}}", &build);
        Assets { shell: Bytes::from(shell), build, files: assets.iter().map(|a| (a.path, a)).collect() }
    }

    /// The file of a path below `/static/<build>/`.
    pub fn file(&self, path: &str) -> Option<&'static Asset> {
        self.files.get(path).copied()
    }
}

/// The shared state of the server.
pub struct AppState<A, S> {
    /// The typed keys.
    pub settings: Settings,
    /// The access layer (section 3).
    pub access: A,
    /// The store.
    pub store: Arc<S>,
    /// The catalog (2.41). A lock never stays held across an `await`.
    catalog: RwLock<Catalog>,
    /// The catalog of capabilities (4.2).
    pub capabilities: Arc<Capabilities>,
    /// One write of a document at a time in this instance: the check of the rules across
    /// documents and the write are one step.
    pub write_lock: Mutex<()>,
    /// The signal of the runner loop (4.6).
    pub wake: Wake,
    /// The log.
    pub logger: Logger,
    /// The metrics.
    pub metrics: Metrics,
    /// The spans.
    pub telemetry: Telemetry,
    /// The static files and the shell.
    pub assets: Assets,
    /// The places of the request bodies in flight (7.10).
    pub bodies: Arc<Semaphore>,
    connections: AtomicU64,
    inflight: AtomicU64,
    store_ok: AtomicBool,
    runner_lease: AtomicBool,
}

/// The parts that `AppState::new` takes.
pub struct Parts<A, S> {
    /// The typed keys.
    pub settings: Settings,
    /// The access layer.
    pub access: A,
    /// The store.
    pub store: Arc<S>,
    /// The catalog, loaded.
    pub catalog: Catalog,
    /// The catalog of capabilities.
    pub capabilities: Arc<Capabilities>,
    /// The signal of the runner loop.
    pub wake: Wake,
    /// The log.
    pub logger: Logger,
    /// The spans.
    pub telemetry: Telemetry,
    /// The static files and the shell.
    pub assets: Assets,
}

impl<A: Access, S: Store> AppState<A, S> {
    /// Build the state.
    pub fn new(parts: Parts<A, S>) -> AppState<A, S> {
        let places =
            usize::try_from(parts.settings.max_inflight_bodies).unwrap_or(usize::MAX).min(Semaphore::MAX_PERMITS);
        AppState {
            settings: parts.settings,
            access: parts.access,
            store: parts.store,
            catalog: RwLock::new(parts.catalog),
            capabilities: parts.capabilities,
            write_lock: Mutex::new(()),
            wake: parts.wake,
            logger: parts.logger,
            metrics: Metrics::default(),
            telemetry: parts.telemetry,
            assets: parts.assets,
            bodies: Arc::new(Semaphore::new(places)),
            connections: AtomicU64::new(0),
            inflight: AtomicU64::new(0),
            store_ok: AtomicBool::new(true),
            runner_lease: AtomicBool::new(true),
        }
    }

    /// Read the catalog. The guard never crosses an `await`.
    pub fn catalog(&self) -> RwLockReadGuard<'_, Catalog> {
        self.catalog.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Change the catalog. The guard never crosses an `await`.
    pub fn catalog_mut(&self) -> RwLockWriteGuard<'_, Catalog> {
        self.catalog.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Count a new connection and give the count after it.
    pub fn connection_opened(&self) -> u64 {
        self.connections.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Count a closed connection.
    pub fn connection_closed(&self) {
        self.connections.fetch_sub(1, Ordering::SeqCst);
    }

    /// The open connections of the main listener.
    pub fn open_connections(&self) -> u64 {
        self.connections.load(Ordering::SeqCst)
    }

    /// Count a request that starts. A request runs to its end also when its connection closes
    /// (7.10), so the server counts it apart from the connection.
    pub fn request_begun(&self) {
        self.inflight.fetch_add(1, Ordering::SeqCst);
    }

    /// Count a request that ended.
    pub fn request_ended(&self) {
        self.inflight.fetch_sub(1, Ordering::SeqCst);
    }

    /// The requests of the main listener that run now.
    pub fn inflight_requests(&self) -> u64 {
        self.inflight.load(Ordering::SeqCst)
    }

    /// The result of the last poll of the store: the check `store` of `/readyz` (5.20).
    pub fn set_store_ok(&self, ok: bool) {
        self.store_ok.store(ok, Ordering::SeqCst);
    }

    /// True when the last poll of the store worked.
    pub fn store_ok(&self) -> bool {
        self.store_ok.load(Ordering::SeqCst)
    }

    /// Tell if the runner node holds the lease `runner`: the check `runner_lease` of `/readyz`.
    pub fn set_runner_lease(&self, held: bool) {
        self.runner_lease.store(held, Ordering::SeqCst);
    }

    /// True when the runner node holds the lease `runner`.
    pub fn runner_lease(&self) -> bool {
        self.runner_lease.load(Ordering::SeqCst)
    }
}
