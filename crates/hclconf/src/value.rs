// SPDX-License-Identifier: MIT OR Apache-2.0
//! Typed values and their text forms (6.4, 6.20).

/// A value that passed the check of its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A text, or a path.
    Str(String),
    /// A whole number, or a size in bytes.
    Int(u64),
    /// A boolean.
    Bool(bool),
    /// A duration in milliseconds.
    DurationMs(u64),
    /// A list.
    List(Vec<Value>),
    /// The value of a key whose `_env` or `_file` source the read mode did not read (6.18).
    /// The key keeps its form and its source. The program must not use this value.
    NotRead,
}

/// The text that stands for a value that was not read (6.18).
pub const NOT_READ: &str = "<not read>";

impl Value {
    /// The text form of `print-config` (6.20). A text is in double quotes, with the escapes
    /// of JSON. An integer is in decimal. A duration is in canonical form, in quotes. A list is
    /// in brackets.
    pub fn print(&self) -> String {
        match self {
            Value::Str(s) => json_string(s),
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::DurationMs(ms) => json_string(&format_duration(*ms)),
            Value::List(items) => {
                let parts: Vec<String> = items.iter().map(Value::print).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::NotRead => NOT_READ.to_string(),
        }
    }

    /// True when the read mode did not read the value.
    pub fn is_not_read(&self) -> bool {
        matches!(self, Value::NotRead)
    }

    /// The text of the value for a comparison in a mode condition: the text itself, or
    /// `true` or `false`.
    pub fn as_condition_text(&self) -> Option<String> {
        match self {
            Value::Str(s) => Some(s.clone()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

/// Write a text as a JSON string, with quotes and escapes.
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parse a duration of 6.4: one whole number and one unit among `ms`, `s`, `m`, `h`. The
/// number has no sign, no fraction and no leading zero (except `0`). The result is in
/// milliseconds. A result that does not fit in 64 bits is an error.
pub fn parse_duration(text: &str) -> Option<u64> {
    let digits_end = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(digits_end);
    if number.is_empty() || (number.len() > 1 && number.starts_with('0')) {
        return None;
    }
    let factor: u64 = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    };
    number.parse::<u64>().ok()?.checked_mul(factor)
}

/// Write a duration in canonical form: one integer and the largest unit that gives a whole
/// number. Thus 60000 gives `1m`, and 1500 gives `1500ms`. Zero gives `0s`.
pub fn format_duration(ms: u64) -> String {
    if ms == 0 {
        "0s".to_string()
    } else if ms.is_multiple_of(3_600_000) {
        format!("{}h", ms / 3_600_000)
    } else if ms.is_multiple_of(60_000) {
        format!("{}m", ms / 60_000)
    } else if ms.is_multiple_of(1_000) {
        format!("{}s", ms / 1_000)
    } else {
        format!("{ms}ms")
    }
}

/// Write the path of a block with its labels, for example `clickhouse "prod"`.
pub fn block_display(name: &str, labels: &[String]) -> String {
    let mut out = name.to_string();
    for label in labels {
        out.push(' ');
        out.push_str(&json_string(label));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_with_one_unit() {
        assert_eq!(parse_duration("500ms"), Some(500));
        assert_eq!(parse_duration("30s"), Some(30_000));
        assert_eq!(parse_duration("5m"), Some(300_000));
        assert_eq!(parse_duration("1h"), Some(3_600_000));
        assert_eq!(parse_duration("0s"), Some(0));
    }

    #[test]
    fn durations_refuse_other_forms() {
        for bad in [
            "1m30s",
            "1m 30s",
            "1.5s",
            "-1s",
            "05s",
            "30",
            "30 s",
            "1d",
            "",
            "s",
            "1us",
            "1ns",
            "+1s",
            "1S",
            "99999999999999999999h",
        ] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn canonical_durations() {
        assert_eq!(format_duration(60_000), "1m");
        assert_eq!(format_duration(1_500), "1500ms");
        assert_eq!(format_duration(75_000), "75s");
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(5_400_000), "90m");
        assert_eq!(format_duration(0), "0s");
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json_string("a\"b\\c\n\t\u{1}"), "\"a\\\"b\\\\c\\n\\t\\u0001\"");
    }

    #[test]
    fn print_forms() {
        assert_eq!(Value::Str("x".into()).print(), "\"x\"");
        assert_eq!(Value::Int(7).print(), "7");
        assert_eq!(Value::Bool(false).print(), "false");
        assert_eq!(Value::DurationMs(60_000).print(), "\"1m\"");
        assert_eq!(Value::List(vec![Value::Int(1), Value::Str("a".into())]).print(), "[1, \"a\"]");
        assert_eq!(Value::NotRead.print(), "<not read>");
        assert!(Value::NotRead.is_not_read());
        assert!(!Value::Int(1).is_not_read());
    }

    #[test]
    fn block_display_with_labels() {
        assert_eq!(block_display("clickhouse", &["prod".to_string()]), "clickhouse \"prod\"");
        assert_eq!(block_display("server", &[]), "server");
    }
}
