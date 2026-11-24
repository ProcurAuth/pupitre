// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one reader of the process environment (6.23).
//!
//! `main` calls `CapturedEnvironment::capture_and_clear` first, before any other thread
//! starts. The engine then reads the variables that `_env` attributes name from the copy.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};

/// The two variables that stay in the process, as 6.23 says. No code of the program reads them,
/// and an `_env` attribute cannot name them.
pub const KEPT_VARIABLES: [&str; 2] = ["SSL_CERT_FILE", "SSL_CERT_DIR"];

/// A copy of the environment of the process, taken at start.
#[derive(Debug, Clone, Default)]
pub struct CapturedEnvironment {
    vars: HashMap<String, OsString>,
}

impl CapturedEnvironment {
    /// Copy every variable of the process, then remove each one from the process, except
    /// the two names of `KEPT_VARIABLES`. A variable whose name is not UTF-8 is removed and
    /// not copied: no `_env` attribute can name it.
    pub fn capture_and_clear() -> CapturedEnvironment {
        let all: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let mut vars = HashMap::new();
        for (name, value) in all {
            let keep = name.to_str().is_some_and(|n| KEPT_VARIABLES.contains(&n));
            if !keep {
                std::env::remove_var(&name);
            }
            if let Some(name) = name.to_str() {
                vars.insert(name.to_string(), value);
            }
        }
        CapturedEnvironment { vars }
    }

    /// Give the value of a variable of the copy.
    pub fn get(&self, name: &str) -> Option<&OsStr> {
        self.vars.get(name).map(OsString::as_os_str)
    }

    /// Build a copy from pairs. This is for tests.
    pub fn from_pairs<I: IntoIterator<Item = (String, String)>>(pairs: I) -> Self {
        CapturedEnvironment { vars: pairs.into_iter().map(|(k, v)| (k, OsString::from(v))).collect() }
    }

    /// Build a copy from pairs whose values are raw. This is for tests of the UTF-8 rule.
    pub fn from_os_pairs<I: IntoIterator<Item = (String, OsString)>>(pairs: I) -> Self {
        CapturedEnvironment { vars: pairs.into_iter().collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_pairs_gives_the_values() {
        let env = CapturedEnvironment::from_pairs([("A".to_string(), "1".to_string())]);
        assert_eq!(env.get("A"), Some(OsStr::new("1")));
        assert_eq!(env.get("B"), None);
    }
}
