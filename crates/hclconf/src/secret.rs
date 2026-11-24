// SPDX-License-Identifier: MIT OR Apache-2.0
//! A secret text that never shows in `Debug` or `Display`.

use std::fmt;

/// The text that stands for a hidden value in messages and logs.
pub const HIDDEN: &str = "(value hidden)";

/// A secret value. `Debug` and `Display` print `(value hidden)`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a secret text.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Give the secret text. Use it only to build a credential.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(HIDDEN)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(HIDDEN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_hide_the_value() {
        let s = Secret::new("hunter2".to_string());
        assert_eq!(format!("{s:?}"), "(value hidden)");
        assert_eq!(format!("{s}"), "(value hidden)");
        assert_eq!(s.expose(), "hunter2");
    }
}
