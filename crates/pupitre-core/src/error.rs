// SPDX-License-Identifier: MIT OR Apache-2.0
//! The error of one field: the item of the list `errors` of 5.4.

use std::fmt;

use crate::json::Json;

/// The most items of one list of errors (5.4).
pub const MAX_ERRORS: usize = 100;

/// One error of a document, a value, a parameter or a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// A key path, a member or a parameter name. Empty for an error of the whole item.
    pub path: String,
    /// The message: the expected type and range, and the value. Never a secret value.
    pub message: String,
    /// The layer of the value, when the error belongs to one (2.14).
    pub layer: Option<&'static str>,
}

impl FieldError {
    /// Make an error with no layer.
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> FieldError {
        FieldError { path: path.into(), message: message.into(), layer: None }
    }

    /// The item of 5.4.
    pub fn to_json(&self) -> Json {
        let mut pairs = vec![("message", Json::str(self.message.clone())), ("path", Json::str(self.path.clone()))];
        if let Some(layer) = self.layer {
            pairs.push(("layer", Json::str(layer)));
        }
        Json::object(pairs)
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() { write!(f, "{}", self.message) } else { write!(f, "{}: {}", self.path, self.message) }
    }
}

/// The list `errors` of 5.4: 100 items at most, then one item `and <n> more errors`.
pub fn errors_json(errors: &[FieldError]) -> Json {
    let mut items: Vec<Json> = errors.iter().take(MAX_ERRORS).map(FieldError::to_json).collect();
    if errors.len() > MAX_ERRORS {
        items.push(FieldError::new("", format!("and {} more errors", errors.len() - MAX_ERRORS)).to_json());
    }
    Json::Arr(items)
}
