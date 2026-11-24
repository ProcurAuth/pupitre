// SPDX-License-Identifier: MIT OR Apache-2.0
//! The log lines of the runner (6.26, 6.27): one JSON object on each line of the standard
//! error, with `t`, `level` and `event`. A line never holds a secret or a value (7.9).

use std::io::Write;

use pupitre_core::json::{self, Json};
use pupitre_core::time::UtcMillis;

/// The level of a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// The bytes of a line, with a line end.
pub(crate) fn line(level: Level, event: &str, fields: &[(&str, Json)]) -> Vec<u8> {
    let mut pairs: Vec<(String, Json)> = vec![
        ("event".to_string(), Json::str(event)),
        ("level".to_string(), Json::str(level.name())),
        ("t".to_string(), Json::str(UtcMillis::now().to_string())),
    ];
    pairs.extend(fields.iter().map(|(k, v)| ((*k).to_string(), v.clone())));
    let mut out = json::to_compact(&Json::object(pairs));
    out.push(b'\n');
    out
}

/// Write a line on the standard error. A failed write is ignored: a log never stops a run.
pub(crate) fn emit(level: Level, event: &str, fields: &[(&str, Json)]) {
    let bytes = line(level, event, fields);
    let _ = std::io::stderr().lock().write_all(&bytes);
}

/// `runner.store_error` (6.27).
pub(crate) fn store_error(what: &str, error: &dyn std::fmt::Display) {
    emit(Level::Error, "runner.store_error", &[("error", Json::str(format!("{what}: {error}")))]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_has_time_level_and_event() {
        let bytes = line(Level::Warn, "run.lost", &[("run", Json::str("r1")), ("reason", Json::str("process_gone"))]);
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.starts_with(r#"{"event":"run.lost","level":"warn","reason":"process_gone","run":"r1","t":"20"#),
            "{text}"
        );
        assert!(text.ends_with("}\n"));
    }
}
