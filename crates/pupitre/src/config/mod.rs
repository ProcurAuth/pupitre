// SPDX-License-Identifier: MIT OR Apache-2.0
//! The configuration of Pupitre (section 6): the table of keys, the three stages of checks and
//! the typed `Config`.
//!
//! This module is the only code that uses the engine `hclconf` (6.2, rule 4). It parses the
//! file one time at start into one typed structure. The rest of the code reads only this
//! structure.

mod ca;
mod checks;
pub mod hosts;
mod keys;
mod render;
mod url;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use hclconf::env::CapturedEnvironment;
use hclconf::{Checked, ResolvedKey, Value};
pub use hclconf::{ConfigError, Profile, Secret};
use pupitre_core::capability::Capabilities;
use pupitre_runner::RunnerSettings;
use pupitre_store::files::FilesOptions;
use pupitre_store::postgres::PgOptions;

pub use render::{ReferenceFormat, render_print_config, render_reference};
pub use url::HttpUrl;

/// The typed configuration (6.2, rule 4).
///
/// A configuration of `ReadMode::Storage` holds placeholders for the `_env` and `_file` values
/// outside the block `storage`: an empty text and an empty secret. The server MUST NEVER start
/// from it (6.18).
#[derive(Debug, Clone)]
pub struct Config {
    /// The block `server`.
    pub server: ServerConfig,
    /// The key `log.level`.
    pub log_level: LogLevel,
    /// The block `telemetry`.
    pub telemetry: TelemetryConfig,
    /// The block `storage`.
    pub storage: StorageConfig,
    /// The block `access`.
    pub access: AccessConfig,
    /// The block `runner`.
    pub runner: RunnerConfig,
    /// The block `limits`, with the defaults.
    pub limits: LimitsConfig,
    /// The blocks `schema`, `playbook` and `command` (4.2).
    pub capabilities: Capabilities,
}

/// The block `server` (6.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// The main listener.
    pub listen: SocketAddr,
    /// The ops listener.
    pub ops_listen: SocketAddr,
    /// The prefix of the routes of the main listener.
    pub base_path: String,
    /// The values of the header `Host` that the main listener serves (7.4).
    pub allowed_hosts: Vec<hosts::HostPattern>,
    /// The most open connections.
    pub max_connections: u32,
    /// The idle time of a kept connection.
    pub idle_timeout: Duration,
    /// The longest graceful stop.
    pub shutdown_grace: Duration,
    /// The longest read of the request line and the headers.
    pub header_read_timeout: Duration,
    /// The longest read of a body.
    pub body_read_timeout: Duration,
    /// The largest request body.
    pub max_request_bytes: u64,
    /// The most request bodies in flight.
    pub max_inflight_bodies: u32,
    /// The most items of a list page.
    pub max_page_items: u32,
}

/// The log level (6.26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Information, warnings and errors.
    Info,
    /// All levels.
    Debug,
}

/// The block `telemetry` (6.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryConfig {
    /// `enabled = false`
    Disabled,
    /// `enabled = true`
    Enabled {
        /// The OTLP endpoint.
        otlp_endpoint: String,
        /// The bytes of the CA file of an `https` endpoint.
        ca_pem: Option<Vec<u8>>,
        /// `service.name`
        service_name: String,
        /// The period of the export.
        export_interval: Duration,
        /// The most spans that wait.
        max_queued_spans: u32,
    },
}

/// The block `storage` (6.10).
#[derive(Debug, Clone)]
pub enum StorageConfig {
    /// The backend `files`.
    Files(FilesOptions),
    /// The backend `postgres`.
    Postgres(PgOptions),
}

/// The block `access` (6.11).
#[derive(Debug, Clone)]
pub enum AccessConfig {
    /// The mode `open`.
    Open,
    /// The mode `authzen`.
    Authzen(Box<AuthzenConfig>),
}

/// The keys of the mode `authzen` (6.11).
#[derive(Debug, Clone)]
pub struct AuthzenConfig {
    /// `access.identity.header`
    pub header: String,
    /// `access.identity.issuer`
    pub issuer: String,
    /// `access.identity.audience`: the service id.
    pub audience: String,
    /// `access.identity.public_keys`: PEM text or a JWK Set.
    pub public_keys: String,
    /// `access.identity.max_age`
    pub max_age: Duration,
    /// `access.identity.clock_skew`
    pub clock_skew: Duration,
    /// `access.pdp.url`
    pub pdp_url: String,
    /// `access.pdp.token`
    pub pdp_token: Secret,
    /// `access.pdp.plain_http`
    pub plain_http: bool,
    /// The bytes of `access.pdp.ca_file`, with `plain_http = false`.
    pub pdp_ca_pem: Option<Vec<u8>>,
    /// `access.pdp.timeout`
    pub pdp_timeout: Duration,
    /// `access.pdp.cache_ttl`
    pub cache_ttl: Duration,
    /// `access.pdp.max_connections`
    pub pdp_max_connections: u32,
    /// `access.pdp.cache_entries`
    pub cache_entries: u32,
    /// `access.options_max_questions`
    pub options_max_questions: u32,
    /// `access.options_page_items`
    pub options_page_items: u32,
}

/// The block `runner` (6.12).
#[derive(Debug, Clone)]
pub enum RunnerConfig {
    /// `runner.enabled = false`
    Disabled,
    /// `runner.enabled = true`
    Enabled(Box<RunnerEnabled>),
}

/// The keys of `runner.enabled = true`.
#[derive(Debug, Clone)]
pub struct RunnerEnabled {
    /// `runner.allow_apply_in_open_mode`, in the mode `open` only.
    pub allow_apply_in_open_mode: Option<bool>,
    /// `runner.check_max_age`
    pub check_max_age: Duration,
    /// `runner.max_hosts`
    pub max_hosts: u32,
    /// `runner.max_queued_runs`
    pub max_queued_runs: u32,
    /// The settings of the runner node, with `runner.execute = true`.
    pub executor: Option<RunnerSettings>,
}

/// The block `limits` (2.42), with the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitsConfig {
    /// `limits.max_environments`
    pub max_environments: u32,
    /// `limits.max_units_per_environment`
    pub max_units_per_environment: u32,
    /// `limits.max_hosts_per_environment`
    pub max_hosts_per_environment: u32,
    /// `limits.max_document_bytes`
    pub max_document_bytes: u64,
    /// `limits.max_store_bytes`
    pub max_store_bytes: u64,
}

/// A warning (6.15). It never stops the start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// The event of the log line, for example `access.mode_open`.
    pub event: &'static str,
    /// The message.
    pub message: String,
}

/// One line of the effective configuration (6.20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveEntry {
    /// The key path, for example `command "restart".param "grace".type`.
    pub key: String,
    /// The value in the form of `print-config`; `<hidden>` for a secret.
    pub value: String,
    /// The source: `file`, `env:VAR`, `file:/path` or `default`.
    pub source: String,
}

/// Which `_env` and `_file` values a load reads (6.18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// Each value. The mode of `serve`, `check-config` and `print-config`.
    All,
    /// The values of the block `storage` only. The mode of `migrate`, `export`, `import` and
    /// `copy-store`.
    Storage,
}

impl ReadMode {
    fn engine(self) -> hclconf::ReadMode<'static> {
        match self {
            ReadMode::All => hclconf::ReadMode::All,
            ReadMode::Storage => hclconf::ReadMode::Blocks(&["storage"]),
        }
    }
}

/// The result of a load.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// The read mode of the load. The server starts only from `ReadMode::All`.
    pub mode: ReadMode,
    /// The typed configuration.
    pub config: Config,
    /// The warnings of 6.15.
    pub warnings: Vec<Warning>,
    /// The effective configuration, in the order of the table of keys (6.20).
    pub effective: Vec<EffectiveEntry>,
}

/// The text that stands for a secret in `print-config` (6.20).
pub const HIDDEN_VALUE: &str = "<hidden>";

/// A view of the keys of stage 1 with typed getters. A key that the engine did not read gives
/// `None`.
pub(crate) struct View<'a> {
    pub(crate) checked: &'a Checked,
}

impl<'a> View<'a> {
    pub(crate) fn key(&self, path: &str) -> Option<&'a ResolvedKey> {
        self.checked.get(path)
    }

    fn value(&self, path: &str) -> Option<&'a Value> {
        self.key(path).map(|k| &k.value)
    }

    pub(crate) fn text(&self, path: &str) -> Option<&'a str> {
        match self.value(path)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn int(&self, path: &str) -> Option<u64> {
        match self.value(path)? {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub(crate) fn bool(&self, path: &str) -> Option<bool> {
        match self.value(path)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn duration(&self, path: &str) -> Option<Duration> {
        match self.value(path)? {
            Value::DurationMs(ms) => Some(Duration::from_millis(*ms)),
            _ => None,
        }
    }

    /// The value of a key, or its default of `keys::DEFAULTS`.
    fn int_or_default(&self, path: &str) -> u64 {
        self.int(path).or_else(|| default_of(path).and_then(|t| t.parse().ok())).unwrap_or(0)
    }

    fn duration_or_default(&self, path: &str) -> Duration {
        self.duration(path)
            .or_else(|| {
                default_of(path)
                    .and_then(|t| hclconf::value::parse_duration(t.trim_matches('"')))
                    .map(Duration::from_millis)
            })
            .unwrap_or_default()
    }
}

fn default_of(path: &str) -> Option<&'static str> {
    keys::DEFAULTS.iter().find(|(k, _)| *k == path).map(|(_, v)| *v)
}

fn u32_of(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Read the file, check it in three stages (6.19) and build the typed configuration.
pub fn load(path: &Path, env: &CapturedEnvironment, mode: ReadMode) -> Result<LoadedConfig, Vec<ConfigError>> {
    let checked = hclconf::check_file_with(path, &keys::SCHEMA, env, mode.engine())?;
    let view = View { checked: &checked };
    let stage2 = checks::stage2(&view, mode)?;
    let contradictions = checks::contradictions(&view, &stage2);
    if !contradictions.is_empty() {
        return Err(hclconf::error::limit_errors(contradictions));
    }
    let warnings = checks::warnings(&view);
    let config = build(&view, stage2);
    let effective = effective_entries(&view);
    Ok(LoadedConfig { mode, config, warnings, effective })
}

/// The entries of `server.allowed_hosts`, or the default of a loopback listener (7.4). Stage 2
/// checked each entry, and it refused a file that has no key and no loopback listener.
pub(crate) fn allowed_hosts(v: &View<'_>) -> Vec<hosts::HostPattern> {
    if let Some(Value::List(items)) = v.value("server.allowed_hosts") {
        return items
            .iter()
            .filter_map(|i| match i {
                Value::Str(t) => hosts::HostPattern::parse(t).ok(),
                _ => None,
            })
            .collect();
    }
    v.text("server.listen")
        .and_then(|t| t.parse::<SocketAddr>().ok())
        .and_then(hosts::default_hosts)
        .unwrap_or_default()
}

fn build(v: &View<'_>, stage2: checks::Stage2) -> Config {
    let text = |p: &str| v.text(p).unwrap_or_default().to_string();
    let addr = |p: &str| v.text(p).and_then(|t| t.parse().ok()).unwrap_or(SocketAddr::from(([127, 0, 0, 1], 0)));
    let server = ServerConfig {
        listen: addr("server.listen"),
        ops_listen: addr("server.ops_listen"),
        base_path: text("server.base_path"),
        allowed_hosts: allowed_hosts(v),
        max_connections: u32_of(v.int_or_default("server.max_connections")),
        idle_timeout: v.duration_or_default("server.idle_timeout"),
        shutdown_grace: v.duration_or_default("server.shutdown_grace"),
        header_read_timeout: v.duration_or_default("server.header_read_timeout"),
        body_read_timeout: v.duration_or_default("server.body_read_timeout"),
        max_request_bytes: v.int_or_default("server.max_request_bytes"),
        max_inflight_bodies: u32_of(v.int_or_default("server.max_inflight_bodies")),
        max_page_items: u32_of(v.int_or_default("server.max_page_items")),
    };
    let log_level = match v.text("log.level") {
        Some("error") => LogLevel::Error,
        Some("warn") => LogLevel::Warn,
        Some("debug") => LogLevel::Debug,
        _ => LogLevel::Info,
    };
    let telemetry = if v.bool("telemetry.enabled") == Some(true) {
        TelemetryConfig::Enabled {
            otlp_endpoint: text("telemetry.otlp_endpoint"),
            ca_pem: stage2.telemetry_ca.clone(),
            service_name: text("telemetry.service_name"),
            export_interval: v.duration_or_default("telemetry.export_interval"),
            max_queued_spans: u32_of(v.int_or_default("telemetry.max_queued_spans")),
        }
    } else {
        TelemetryConfig::Disabled
    };
    let limits = LimitsConfig {
        max_environments: u32_of(v.int_or_default("limits.max_environments")),
        max_units_per_environment: u32_of(v.int_or_default("limits.max_units_per_environment")),
        max_hosts_per_environment: u32_of(v.int_or_default("limits.max_hosts_per_environment")),
        max_document_bytes: v.int_or_default("limits.max_document_bytes"),
        max_store_bytes: v.int_or_default("limits.max_store_bytes"),
    };
    let max_document_bytes = usize::try_from(limits.max_document_bytes).unwrap_or(usize::MAX);
    let secret_key = stage2.secret_key.clone();
    let storage = if v.text("storage.backend") == Some("postgres") {
        StorageConfig::Postgres(PgOptions {
            host: text("storage.postgres.host"),
            port: u16::try_from(v.int("storage.postgres.port").unwrap_or(0)).unwrap_or(0),
            database: text("storage.postgres.database"),
            user: text("storage.postgres.user"),
            password: pupitre_store::postgres::PgPassword(text("storage.postgres.password")),
            sslmode: if v.text("storage.postgres.sslmode") == Some("disable") {
                pupitre_store::postgres::SslMode::Disable
            } else {
                pupitre_store::postgres::SslMode::VerifyFull
            },
            tls: stage2.pg_ca.clone().map(|ca_pem| pupitre_store::postgres::PgTls {
                ca_pem,
                server_name: text("storage.postgres.tls.server_name"),
            }),
            max_connections: u32_of(v.int_or_default("storage.postgres.max_connections")),
            connect_timeout: v.duration_or_default("storage.postgres.connect_timeout"),
            statement_timeout: v.duration_or_default("storage.postgres.statement_timeout"),
            poll_interval: v.duration_or_default("storage.postgres.poll_interval"),
            history_keep: u32_of(v.int_or_default("storage.postgres.history_keep")),
            secret_key,
            max_document_bytes,
        })
    } else {
        StorageConfig::Files(FilesOptions {
            path: PathBuf::from(text("storage.files.path")),
            secrets_path: PathBuf::from(text("storage.files.secrets_path")),
            runs_path: PathBuf::from(text("storage.files.runs_path")),
            rescan_interval: v.duration_or_default("storage.files.rescan_interval"),
            history_keep: u32_of(v.int_or_default("storage.files.history_keep")),
            secret_key,
            max_document_bytes,
        })
    };
    let access = if v.text("access.mode") == Some("authzen") {
        AccessConfig::Authzen(Box::new(AuthzenConfig {
            header: text("access.identity.header"),
            issuer: text("access.identity.issuer"),
            audience: text("access.identity.audience"),
            public_keys: text("access.identity.public_keys"),
            max_age: v.duration_or_default("access.identity.max_age"),
            clock_skew: v.duration_or_default("access.identity.clock_skew"),
            pdp_url: text("access.pdp.url"),
            pdp_token: Secret::new(text("access.pdp.token")),
            plain_http: v.bool("access.pdp.plain_http").unwrap_or(false),
            pdp_ca_pem: stage2.pdp_ca.clone(),
            pdp_timeout: v.duration_or_default("access.pdp.timeout"),
            cache_ttl: v.duration_or_default("access.pdp.cache_ttl"),
            pdp_max_connections: u32_of(v.int_or_default("access.pdp.max_connections")),
            cache_entries: u32_of(v.int_or_default("access.pdp.cache_entries")),
            options_max_questions: u32_of(v.int_or_default("access.options_max_questions")),
            options_page_items: u32_of(v.int_or_default("access.options_page_items")),
        }))
    } else {
        AccessConfig::Open
    };
    let runner = if v.bool("runner.enabled") == Some(true) {
        let executor = (v.bool("runner.execute") == Some(true)).then(|| RunnerSettings {
            work_dir: PathBuf::from(text("runner.work_dir")),
            secrets_dir: PathBuf::from(text("runner.secrets_dir")),
            ansible_playbook: PathBuf::from(text("runner.ansible_playbook")),
            path: text("runner.path"),
            ssh_user: text("runner.ssh_user"),
            ssh_key_file: PathBuf::from(text("runner.ssh_key_file")),
            known_hosts_file: PathBuf::from(text("runner.known_hosts_file")),
            releases_dir: v.text("runner.releases_dir").map(PathBuf::from),
            forks: u32_of(v.int_or_default("runner.forks")),
            max_parallel_runs: u32_of(v.int_or_default("runner.max_parallel_runs")),
            check_max_age: v.duration_or_default("runner.check_max_age"),
            run_timeout: v.duration_or_default("runner.run_timeout"),
            cancel_grace: v.duration_or_default("runner.cancel_grace"),
            lease_ttl: v.duration_or_default("runner.lease_ttl"),
            max_log_bytes: v.int_or_default("runner.max_log_bytes"),
            max_events: v.int_or_default("runner.max_events"),
            max_event_bytes: v.int_or_default("runner.max_event_bytes"),
            runs_keep: u32_of(v.int_or_default("runner.runs_keep")),
            poll_interval: match &storage {
                StorageConfig::Files(f) => f.rescan_interval,
                StorageConfig::Postgres(p) => p.poll_interval,
            },
        });
        RunnerConfig::Enabled(Box::new(RunnerEnabled {
            allow_apply_in_open_mode: v.bool("runner.allow_apply_in_open_mode"),
            check_max_age: v.duration_or_default("runner.check_max_age"),
            max_hosts: u32_of(v.int_or_default("runner.max_hosts")),
            max_queued_runs: u32_of(v.int_or_default("runner.max_queued_runs")),
            executor,
        }))
    } else {
        RunnerConfig::Disabled
    };
    Config { server, log_level, telemetry, storage, access, runner, limits, capabilities: stage2.capabilities }
}

/// True when the terms of a key with a default hold: the default then applies (6.7).
fn default_applies(v: &View<'_>, path: &str) -> bool {
    let Some(desc) = keys::SCHEMA.key(path) else { return false };
    match desc.applies {
        hclconf::Applies::WhenOptional(terms) | hclconf::Applies::When(terms) => terms
            .iter()
            .all(|t| v.key(t.key).and_then(|k| k.value.as_condition_text()).is_some_and(|text| text == t.equals)),
        hclconf::Applies::Always | hclconf::Applies::Manual => true,
    }
}

fn effective_entries(v: &View<'_>) -> Vec<EffectiveEntry> {
    let mut entries: Vec<EffectiveEntry> = v
        .checked
        .keys()
        .iter()
        .map(|key| {
            let value = if !key.is_read() {
                key.value.print()
            } else if key.secret {
                HIDDEN_VALUE.to_string()
            } else {
                key.value.print()
            };
            EffectiveEntry { key: key.display.clone(), value, source: key.source.text() }
        })
        .collect();
    // Each default that applies goes after the key before it in the table (6.20).
    let mut defaults: Vec<(&str, String)> = keys::DEFAULTS
        .iter()
        .filter(|(path, _)| v.key(path).is_none() && default_applies(v, path))
        .map(|(path, value)| (*path, (*value).to_string()))
        .collect();
    // The default of `server.allowed_hosts` comes from `server.listen` (7.4).
    if v.key("server.allowed_hosts").is_none()
        && v.key("server.listen").is_some_and(ResolvedKey::is_read)
        && let Some(listen) = v.text("server.listen").and_then(|t| t.parse::<SocketAddr>().ok())
        && let Some(patterns) = hosts::default_hosts(listen)
    {
        let items: Vec<String> = patterns.iter().map(|p| hclconf::value::json_string(&p.to_string())).collect();
        defaults.push(("server.allowed_hosts", format!("[{}]", items.join(", "))));
    }
    for (path, value) in defaults {
        let index = keys::SCHEMA.key_index(path).unwrap_or(usize::MAX);
        let at = entries
            .iter()
            .position(|e| keys::SCHEMA.key_index(&base_path(&e.key)).is_some_and(|i| i > index))
            .unwrap_or(entries.len());
        entries.insert(at, EffectiveEntry { key: path.to_string(), value, source: "default".to_string() });
    }
    entries
}

/// The path of the table of a displayed key: `command "x".label` gives `command.label`.
fn base_path(display: &str) -> String {
    let mut out = String::new();
    let mut in_quotes = false;
    for c in display.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            _ if in_quotes => {}
            ' ' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
