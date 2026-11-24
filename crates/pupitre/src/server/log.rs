// SPDX-License-Identifier: MIT OR Apache-2.0
//! The log (6.26, 6.27): one JSON object on each line, on the standard error. Each line has
//! `t`, `level`, `event` and the fields of the event. A line never holds a secret, a token, a
//! credential or a value of a document (7.9).

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use pupitre_core::json::{self, Json};
use pupitre_core::time::UtcMillis;

use crate::config::{EffectiveEntry, LogLevel, Warning};

#[derive(Debug, Clone)]
enum Sink {
    Stderr,
    Memory(Arc<Mutex<Vec<u8>>>),
}

/// The log writer.
#[derive(Debug, Clone)]
pub struct Logger {
    level: LogLevel,
    sink: Sink,
}

/// The lines that a memory logger holds. A test reads them.
#[derive(Debug, Clone)]
pub struct MemoryLog(Arc<Mutex<Vec<u8>>>);

impl MemoryLog {
    /// The lines written so far, each one as parsed JSON.
    pub fn lines(&self) -> Vec<Json> {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
        bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .filter_map(|l| json::parse(l, json::ReadLimits::bytes(1 << 20)).ok())
            .collect()
    }

    /// The raw text written so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }

    /// The lines of one event.
    pub fn events(&self, name: &str) -> Vec<Json> {
        self.lines()
            .into_iter()
            .filter(|l| l.as_object().and_then(|m| m.get("event")).and_then(Json::as_str) == Some(name))
            .collect()
    }
}

fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
    }
}

impl Logger {
    /// A logger on the standard error.
    pub fn stderr(level: LogLevel) -> Logger {
        Logger { level, sink: Sink::Stderr }
    }

    /// A logger in memory, for a test.
    pub fn memory(level: LogLevel) -> (Logger, MemoryLog) {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        (Logger { level, sink: Sink::Memory(buffer.clone()) }, MemoryLog(buffer))
    }

    /// Start a line of an event.
    pub fn event(&self, level: LogLevel, name: &'static str) -> Event<'_> {
        let mut pairs = BTreeMap::new();
        pairs.insert("event".to_string(), Json::str(name));
        pairs.insert("level".to_string(), Json::str(level_name(level)));
        pairs.insert("t".to_string(), Json::str(UtcMillis::now().to_string()));
        Event { logger: self, level, pairs }
    }

    fn write(&self, line: &[u8]) {
        match &self.sink {
            Sink::Stderr => {
                let mut err = std::io::stderr().lock();
                let _ = err.write_all(line);
            }
            Sink::Memory(buffer) => buffer.lock().unwrap_or_else(PoisonError::into_inner).extend_from_slice(line),
        }
    }

    /// The warnings of 6.15 and the line `server.config` of 6.22, then `server.start` of 6.27.
    pub fn start_log(&self, effective: &[EffectiveEntry], warnings: &[Warning]) {
        for w in warnings {
            self.event(LogLevel::Warn, w.event).field("message", Json::str(w.message.clone())).emit();
        }
        let mut config = BTreeMap::new();
        let mut sources = BTreeMap::new();
        for entry in effective {
            let value = json::parse(entry.value.as_bytes(), json::ReadLimits::bytes(1 << 16))
                .unwrap_or_else(|_| Json::str(entry.value.clone()));
            config.insert(entry.key.clone(), value);
            if entry.source != "file" {
                sources.insert(entry.key.clone(), Json::str(entry.source.clone()));
            }
        }
        self.event(LogLevel::Info, "server.config")
            .field("config", Json::Obj(config))
            .field("sources", Json::Obj(sources))
            .emit();
    }
}

/// A line that waits for its fields.
#[derive(Debug)]
pub struct Event<'a> {
    logger: &'a Logger,
    level: LogLevel,
    pairs: BTreeMap<String, Json>,
}

impl Event<'_> {
    /// Add a field.
    #[must_use]
    pub fn field(mut self, name: &str, value: Json) -> Self {
        self.pairs.insert(name.to_string(), value);
        self
    }

    /// Add a text field.
    #[must_use]
    pub fn text(self, name: &str, value: &str) -> Self {
        self.field(name, Json::str(value))
    }

    /// Add an integer field.
    #[must_use]
    pub fn int(self, name: &str, value: u64) -> Self {
        self.field(name, Json::Int(value.min(json::MAX_SAFE_INTEGER)))
    }

    /// Add the id of the request (6.26).
    #[must_use]
    pub fn request_id(self, id: &str) -> Self {
        self.text("request_id", id)
    }

    /// Write the line when its level passes `log.level`.
    pub fn emit(self) {
        if self.level > self.logger.level {
            return;
        }
        let mut line = json::to_compact(&Json::Obj(self.pairs));
        line.push(b'\n');
        self.logger.write(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_has_t_level_event_and_its_fields() {
        let (log, mem) = Logger::memory(LogLevel::Info);
        log.event(LogLevel::Info, "http.request").text("method", "GET").int("status", 200).emit();
        let lines = mem.lines();
        assert_eq!(lines.len(), 1);
        let m = lines[0].as_object().unwrap();
        assert_eq!(m["event"], Json::str("http.request"));
        assert_eq!(m["level"], Json::str("info"));
        assert_eq!(m["status"], Json::Int(200));
        assert!(pupitre_core::time::UtcMillis::parse(m["t"].as_str().unwrap()).is_some());
        assert!(mem.text().ends_with('\n'));
        assert_eq!(mem.text().matches('\n').count(), 1);
    }

    #[test]
    fn the_level_is_a_lower_bound() {
        let (log, mem) = Logger::memory(LogLevel::Warn);
        log.event(LogLevel::Info, "a").emit();
        log.event(LogLevel::Debug, "b").emit();
        log.event(LogLevel::Warn, "c").emit();
        log.event(LogLevel::Error, "d").emit();
        let names: Vec<String> =
            mem.lines().iter().map(|l| l.as_object().unwrap()["event"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["c", "d"]);
    }

    #[test]
    fn the_start_log_hides_a_secret_and_names_a_source_that_is_not_the_file_6_22() {
        let (log, mem) = Logger::memory(LogLevel::Info);
        let effective = vec![
            EffectiveEntry { key: "server.listen".into(), value: "\"127.0.0.1:8080\"".into(), source: "file".into() },
            EffectiveEntry { key: "server.max_connections".into(), value: "1024".into(), source: "default".into() },
            EffectiveEntry {
                key: "storage.secret_key".into(),
                value: "<hidden>".into(),
                source: "file:/run/secrets/key".into(),
            },
        ];
        let warnings = vec![Warning { event: "access.mode_open", message: "No token.".into() }];
        log.start_log(&effective, &warnings);
        let lines = mem.lines();
        assert_eq!(mem.events("access.mode_open").len(), 1);
        assert_eq!(lines[0].as_object().unwrap()["level"], Json::str("warn"));
        let config = &mem.events("server.config")[0];
        let m = config.as_object().unwrap();
        let c = m["config"].as_object().unwrap();
        assert_eq!(c["server.listen"], Json::str("127.0.0.1:8080"));
        assert_eq!(c["server.max_connections"], Json::Int(1024));
        assert_eq!(c["storage.secret_key"], Json::str("<hidden>"));
        let s = m["sources"].as_object().unwrap();
        assert!(!s.contains_key("server.listen"));
        assert_eq!(s["server.max_connections"], Json::str("default"));
        assert_eq!(s["storage.secret_key"], Json::str("file:/run/secrets/key"));
    }
}
