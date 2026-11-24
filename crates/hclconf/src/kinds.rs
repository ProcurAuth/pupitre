// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check a literal against the type and the range of a key (6.4, 6.6).

use crate::schema::{Kind, TextRule};
use crate::secret::HIDDEN;
use crate::syntax::Literal;
use crate::value::{json_string, parse_duration, Value};

/// A DNS name: labels of 1 to 63 characters from `A-Z a-z 0-9 -`, none that starts or ends
/// with `-`, and at most 253 bytes in all.
pub fn is_dns_name(text: &str) -> bool {
    if text.is_empty() || text.len() > 253 {
        return false;
    }
    text.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

fn rule_holds(rule: TextRule, text: &str) -> bool {
    match rule {
        TextRule::Any => true,
        TextRule::NoControl => !text.chars().any(char::is_control),
        TextRule::PrintableAscii => text.bytes().all(|b| (0x20..=0x7e).contains(&b)),
        TextRule::NoControlNoSpace => !text.chars().any(|c| c.is_control() || c == ' '),
        TextRule::DnsName => is_dns_name(text),
        TextRule::HostOrIp => is_dns_name(text) || text.parse::<std::net::IpAddr>().is_ok(),
    }
}

/// True for a path of 6.6: absolute, 1 to 4096 bytes, no NUL byte.
pub fn is_absolute_path(text: &str) -> bool {
    text.starts_with('/') && text.len() <= 4096 && !text.contains('\0')
}

fn found(secret: bool, shown: impl FnOnce() -> String) -> String {
    if secret {
        HIDDEN.to_string()
    } else {
        shown()
    }
}

/// Parse a whole number of 6.4: decimal digits, no sign, no leading zero, at most
/// 18446744073709551615.
fn parse_whole(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.parse().ok()
}

/// Check a text that came from the file, an environment variable or a file.
pub fn check_string(kind: &Kind, secret: bool, text: &str) -> Result<Value, String> {
    let bad = || format!("expected {}, found {}", kind.expected(), found(secret, || json_string(text)));
    match kind {
        Kind::Text { min, max, rule } => {
            if text.len() < *min || text.len() > *max || !rule_holds(*rule, text) {
                return Err(bad());
            }
            Ok(Value::Str(text.to_string()))
        }
        Kind::Enum(values) => {
            if values.contains(&text) {
                Ok(Value::Str(text.to_string()))
            } else {
                Err(bad())
            }
        }
        Kind::Path => {
            if is_absolute_path(text) {
                Ok(Value::Str(text.to_string()))
            } else {
                Err(bad())
            }
        }
        Kind::SocketAddr => match text.parse::<std::net::SocketAddr>() {
            Ok(addr) if addr.to_string() == text => Ok(Value::Str(text.to_string())),
            _ => Err(bad()),
        },
        Kind::Duration { min_ms, max_ms } => match parse_duration(text) {
            Some(ms) if ms >= *min_ms && ms <= *max_ms => Ok(Value::DurationMs(ms)),
            _ => Err(bad()),
        },
        Kind::Integer { .. } | Kind::Size { .. } | Kind::Bool | Kind::TextList { .. } => Err(bad()),
    }
}

/// Check a literal of the file against a kind. An error is the message of 6.14.
pub fn check_literal(kind: &Kind, secret: bool, literal: &Literal) -> Result<Value, String> {
    let expected = kind.expected();
    match literal {
        Literal::Other { what, text } => {
            Err(format!("expected {expected}, found {what}: {text}; write a literal value"))
        }
        Literal::List(items) => match kind {
            Kind::TextList { min_items, max_items, item_min, item_max, rule } => {
                let bad = || format!("expected {expected}, found {}", found(secret, || literal.describe()));
                if items.len() < *min_items || items.len() > *max_items {
                    return Err(bad());
                }
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Literal::Str(text)
                            if text.len() >= *item_min && text.len() <= *item_max && rule_holds(*rule, text) =>
                        {
                            values.push(Value::Str(text.clone()));
                        }
                        Literal::Other { what, text } => {
                            return Err(format!("expected {expected}, found {what}: {text}; write a literal value"));
                        }
                        _ => return Err(bad()),
                    }
                }
                Ok(Value::List(values))
            }
            _ => Err(format!("expected {expected}, found {}", found(secret, || literal.describe()))),
        },
        Literal::Str(text) => check_string(kind, secret, text),
        Literal::Bool(b) => match kind {
            Kind::Bool => Ok(Value::Bool(*b)),
            _ => Err(format!("expected {expected}, found {}", found(secret, || b.to_string()))),
        },
        Literal::Number(text) => {
            let range = match kind {
                Kind::Integer { min, max } | Kind::Size { min, max } => Some((*min, *max)),
                _ => None,
            };
            match (range, parse_whole(text)) {
                (Some((min, max)), Some(n)) if n >= min && n <= max => Ok(Value::Int(n)),
                _ => Err(format!("expected {expected}, found {}", found(secret, || text.clone()))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INT: Kind = Kind::Integer { min: 16, max: 65536 };
    const DUR: Kind = Kind::Duration { min_ms: 1_000, max_ms: 600_000 };

    fn num(t: &str) -> Literal {
        Literal::Number(t.to_string())
    }

    #[test]
    fn integers_accept_the_range_and_refuse_other_forms() {
        assert_eq!(check_literal(&INT, false, &num("16")), Ok(Value::Int(16)));
        assert_eq!(check_literal(&INT, false, &num("65536")), Ok(Value::Int(65536)));
        for bad in ["15", "65537", "1.0", "1e3", "-1", "+16", "016", "18446744073709551616", "0x10"] {
            let e = check_literal(&INT, false, &num(bad)).unwrap_err();
            assert_eq!(e, format!("expected an integer from 16 to 65536, found {bad}"));
        }
        let e = check_literal(&INT, false, &Literal::Str("8080".into())).unwrap_err();
        assert_eq!(e, "expected an integer from 16 to 65536, found \"8080\"");
        let e = check_literal(&INT, false, &Literal::Bool(true)).unwrap_err();
        assert_eq!(e, "expected an integer from 16 to 65536, found true");
    }

    #[test]
    fn largest_u64_is_a_valid_number_but_out_of_range() {
        let k = Kind::Size { min: 0, max: u64::MAX };
        assert_eq!(check_literal(&k, false, &num("18446744073709551615")), Ok(Value::Int(u64::MAX)));
    }

    #[test]
    fn durations_accept_text_with_one_unit_only() {
        assert_eq!(check_literal(&DUR, false, &Literal::Str("1s".into())), Ok(Value::DurationMs(1000)));
        assert_eq!(check_literal(&DUR, false, &Literal::Str("10m".into())), Ok(Value::DurationMs(600_000)));
        for bad in ["999ms", "11m", "1m30s", "1.5s", "-1s", "05s", "30", "30 s", "1d"] {
            let e = check_literal(&DUR, false, &Literal::Str(bad.into())).unwrap_err();
            assert_eq!(e, format!("expected a duration from 1s to 10m, found \"{bad}\""), "{bad}");
        }
        assert!(check_literal(&DUR, false, &num("30")).is_err());
    }

    #[test]
    fn booleans_are_not_text_or_numbers() {
        assert_eq!(check_literal(&Kind::Bool, false, &Literal::Bool(false)), Ok(Value::Bool(false)));
        assert!(check_literal(&Kind::Bool, false, &Literal::Str("true".into())).is_err());
        assert!(check_literal(&Kind::Bool, false, &num("1")).is_err());
    }

    #[test]
    fn text_length_and_rules() {
        let k = Kind::Text { min: 1, max: 4, rule: TextRule::NoControl };
        assert!(check_literal(&k, false, &Literal::Str("abcd".into())).is_ok());
        assert!(check_literal(&k, false, &Literal::Str("".into())).is_err());
        assert!(check_literal(&k, false, &Literal::Str("abcde".into())).is_err());
        assert!(check_literal(&k, false, &Literal::Str("a\u{1}".into())).is_err());
        let ns = Kind::Text { min: 1, max: 9, rule: TextRule::NoControlNoSpace };
        assert!(check_literal(&ns, false, &Literal::Str("a b".into())).is_err());
        let pa = Kind::Text { min: 1, max: 9, rule: TextRule::PrintableAscii };
        assert!(check_literal(&pa, false, &Literal::Str("caf\u{e9}".into())).is_err());
    }

    #[test]
    fn dns_names_and_hosts() {
        assert!(is_dns_name("store-pg-rw"));
        assert!(is_dns_name("a.b.example"));
        for bad in ["", "-a", "a-", "a..b", "a_b", "a.b.", &"a".repeat(64), &format!("{}.a", "a.".repeat(127))] {
            assert!(!is_dns_name(bad), "{bad}");
        }
        let k = Kind::Text { min: 1, max: 253, rule: TextRule::HostOrIp };
        assert!(check_string(&k, false, "10.0.0.1").is_ok());
        assert!(check_string(&k, false, "::1").is_ok());
        assert!(check_string(&k, false, "bad host").is_err());
    }

    #[test]
    fn enums_and_paths() {
        let k = Kind::Enum(&["error", "warn", "info", "debug"]);
        assert!(check_string(&k, false, "info").is_ok());
        assert_eq!(
            check_string(&k, false, "INFO").unwrap_err(),
            "expected \"error\", \"warn\", \"info\" or \"debug\", found \"INFO\""
        );
        assert!(check_string(&Kind::Path, false, "/a/b").is_ok());
        assert!(check_string(&Kind::Path, false, "a/b").is_err());
        assert!(check_string(&Kind::Path, false, "/a\0b").is_err());
        assert!(check_string(&Kind::Path, false, &format!("/{}", "a".repeat(4096))).is_err());
    }

    #[test]
    fn socket_addresses_need_the_canonical_form() {
        let k = Kind::SocketAddr;
        assert!(check_string(&k, false, "0.0.0.0:8080").is_ok());
        assert!(check_string(&k, false, "[::1]:8080").is_ok());
        for bad in ["localhost:80", "0.0.0.0", "0.0.0.0:08080", "[0:0:0:0:0:0:0:1]:80", ":80"] {
            assert!(check_string(&k, false, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn text_lists_check_the_count_and_each_item() {
        let k = Kind::TextList { min_items: 1, max_items: 2, item_min: 1, item_max: 3, rule: TextRule::NoControl };
        let list = |items: &[&str]| Literal::List(items.iter().map(|i| Literal::Str(i.to_string())).collect());
        assert_eq!(
            check_literal(&k, false, &list(&["a", "bcd"])),
            Ok(Value::List(vec![Value::Str("a".into()), Value::Str("bcd".into())]))
        );
        let expected = "a list of 1 to 2 texts, each of 1 to 3 bytes with no control character";
        assert_eq!(k.expected(), expected);
        for bad in [list(&[]), list(&["a", "b", "c"]), list(&["abcd"]), list(&[""]), list(&["a\u{1}"])] {
            let e = check_literal(&k, false, &bad).unwrap_err();
            assert_eq!(e, format!("expected {expected}, found {}", bad.describe()));
        }
        assert!(check_literal(&k, false, &Literal::Str("a".into())).is_err());
        assert!(check_literal(&k, false, &Literal::List(vec![Literal::Number("1".into())])).is_err());
        let e = check_literal(&k, false, &Literal::List(vec![Literal::Other { what: "variable", text: "x".into() }]))
            .unwrap_err();
        assert!(e.ends_with("found variable: x; write a literal value"), "{e}");
        assert!(check_string(&k, false, "a").is_err());
    }

    #[test]
    fn non_literals_say_write_a_literal_value() {
        let e = check_literal(&INT, false, &Literal::Other { what: "function call", text: "max(1, 2)".into() })
            .unwrap_err();
        assert_eq!(e, "expected an integer from 16 to 65536, found function call: max(1, 2); write a literal value");
    }

    #[test]
    fn secrets_are_never_shown() {
        let k = Kind::Text { min: 1, max: 3, rule: TextRule::Any };
        let e = check_literal(&k, true, &Literal::Str("toolong-secret".into())).unwrap_err();
        assert_eq!(e, "expected a text of 1 to 3 bytes, found (value hidden)");
        assert!(!e.contains("toolong"));
    }
}
