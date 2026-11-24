// SPDX-License-Identifier: MIT OR Apache-2.0
//! Errors of the configuration (6.14).

use std::fmt;

/// One configuration error. `Display` gives the line form of 6.14:
/// `<key>: <message> (line <n>)`. An empty `key` means an error of the whole file: the line
/// then holds the message only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// The key path in the form of `print-config`, or an empty text.
    pub key: String,
    /// The message. It never holds the value of a secret.
    pub message: String,
    /// The line of the file, when the error has one.
    pub line: Option<u32>,
}

impl ConfigError {
    /// Make an error with an optional line.
    pub fn new(key: impl Into<String>, message: impl Into<String>, line: Option<u32>) -> Self {
        Self { key: key.into(), message: message.into(), line }
    }

    /// Make an error for a line of the file.
    pub fn at(key: impl Into<String>, message: impl Into<String>, line: u32) -> Self {
        Self::new(key, message, Some(line))
    }

    /// Make an error of the whole file, with no key and no line.
    pub fn file(message: impl Into<String>) -> Self {
        Self::new("", message, None)
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}", self.message)?;
        } else {
            write!(f, "{}: {}", self.key, self.message)?;
        }
        if let Some(line) = self.line {
            write!(f, " (line {line})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

/// The most errors for one key (6.25).
pub const MAX_ERRORS_PER_KEY: usize = 10;
/// The most errors in one list (6.25).
pub const MAX_ERRORS_PER_LIST: usize = 100;

/// Apply the limits of 6.25: 10 errors for one key and 100 in the list. Each cut gives one
/// line `and <n> more errors`.
pub fn limit_errors(errors: Vec<ConfigError>) -> Vec<ConfigError> {
    let mut per_key: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for error in &errors {
        *per_key.entry(error.key.as_str()).or_insert(0) += 1;
    }
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut kept: Vec<ConfigError> = Vec::new();
    for error in &errors {
        let count = seen.entry(error.key.as_str()).or_insert(0);
        *count += 1;
        if *count <= MAX_ERRORS_PER_KEY {
            kept.push(error.clone());
        }
        if *count == MAX_ERRORS_PER_KEY + 1 {
            let more = per_key.get(error.key.as_str()).copied().unwrap_or(0) - MAX_ERRORS_PER_KEY;
            kept.push(ConfigError::file(format!("and {more} more errors")));
        }
    }
    let real = kept.iter().filter(|e| !e.key.is_empty() || !e.message.starts_with("and ")).count();
    if real <= MAX_ERRORS_PER_LIST {
        return kept;
    }
    let mut out: Vec<ConfigError> = kept.into_iter().take(MAX_ERRORS_PER_LIST).collect();
    out.push(ConfigError::file(format!("and {} more errors", real - MAX_ERRORS_PER_LIST)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_with_line() {
        let e = ConfigError::at("server.listen", "unknown attribute", 3);
        assert_eq!(e.to_string(), "server.listen: unknown attribute (line 3)");
    }

    #[test]
    fn display_without_line_and_without_key() {
        assert_eq!(ConfigError::new("a.b", "m", None).to_string(), "a.b: m");
        assert_eq!(ConfigError::file("whole file").to_string(), "whole file");
    }

    #[test]
    fn ten_errors_for_one_key() {
        let errors: Vec<_> = (0..13).map(|i| ConfigError::at("k", format!("e{i}"), i)).collect();
        let out = limit_errors(errors);
        assert_eq!(out.len(), 11);
        assert_eq!(out[10].to_string(), "and 3 more errors");
    }

    #[test]
    fn one_hundred_errors_in_one_list() {
        let errors: Vec<_> = (0..130).map(|i| ConfigError::at(format!("k{i}"), "e", 1)).collect();
        let out = limit_errors(errors);
        assert_eq!(out.len(), 101);
        assert_eq!(out[100].to_string(), "and 30 more errors");
    }

    #[test]
    fn short_list_is_unchanged() {
        let errors = vec![ConfigError::at("a", "x", 1), ConfigError::at("b", "y", 2)];
        assert_eq!(limit_errors(errors.clone()), errors);
    }
}
