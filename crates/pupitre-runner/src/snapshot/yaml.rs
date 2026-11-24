// SPDX-License-Identifier: MIT OR Apache-2.0
//! The YAML form of the variable files of a snapshot (4.10, 4.12, 4.14).
//!
//! Ansible evaluates a template in the value of a variable when a playbook reads it (7.3). A
//! value with the tag `!unsafe` is data: Ansible never evaluates a string in it. JSON has no
//! tags, so the runner writes these files in YAML. The form is the canonical form of 2.4, with
//! two changes:
//!
//! - Each object and each array that holds values of a document or of a request has the tag
//!   `!unsafe`, at each depth. The tag is never on a string: from ansible-core 2.19, a tag on a
//!   string makes Ansible resolve its type again, so `"true"` becomes a boolean and `""` becomes
//!   `null`. A string in a quoted form inside a tagged collection stays a string.
//! - Each character outside the printable ASCII range is an escape of YAML: `\xXX`, `\uXXXX` or
//!   `\UXXXXXXXX`. So the file is ASCII. No reader of YAML sees a line break, a byte order mark
//!   or a surrogate pair in a string.
//!
//! ansible-core 2.15 to 2.18 build a collection with the tag at once only when each collection
//! in it also has the tag: [`Yaml::untrusted`] puts the tag on each one.
//!
//! The form is a function of the value, so the hash of the snapshot stays stable (4.16).

use std::collections::BTreeMap;

use pupitre_core::json::Json;

/// The tag of a collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// No tag: Pupitre builds the content. Ansible can evaluate a string in it.
    Plain,
    /// `!unsafe`: the content comes from a document or from a request.
    Unsafe,
}

/// A value of a variable file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Yaml {
    /// `null`. A document has none (2.4). The writer has it for a complete form.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer from 0 to 2^53-1.
    Int(u64),
    /// A string in the double-quoted form. It is data when a collection with the tag `!unsafe`
    /// holds it.
    Str(String),
    /// A sequence.
    Seq(Tag, Vec<Yaml>),
    /// A mapping. The names are ids of 2.1, key path segments or fixed names: no name holds a
    /// template marker.
    Map(Tag, BTreeMap<String, Yaml>),
}

impl Yaml {
    /// An object of a document or of a request: it and each collection in it get the tag
    /// `!unsafe`. The argument is an object, so that no string of a document is outside a tagged
    /// collection.
    pub fn untrusted(object: &BTreeMap<String, Json>) -> Yaml {
        Yaml::Map(Tag::Unsafe, object.iter().map(|(k, v)| (k.clone(), untrusted_item(v))).collect())
    }

    /// A mapping with no tag, from pairs.
    pub fn map<I, K>(pairs: I) -> Yaml
    where
        I: IntoIterator<Item = (K, Yaml)>,
        K: Into<String>,
    {
        Yaml::Map(Tag::Plain, pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// The bytes of the file: the layout of 2.4, the tags and the escapes of this module, and one
    /// U+000A at the end.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        write_value(&mut out, self, 0);
        out.push('\n');
        out.into_bytes()
    }
}

fn untrusted_item(value: &Json) -> Yaml {
    match value {
        Json::Null => Yaml::Null,
        Json::Bool(b) => Yaml::Bool(*b),
        Json::Int(n) => Yaml::Int(*n),
        Json::Str(s) => Yaml::Str(s.clone()),
        Json::Arr(items) => Yaml::Seq(Tag::Unsafe, items.iter().map(untrusted_item).collect()),
        Json::Obj(map) => Yaml::untrusted(map),
    }
}

/// Write a string as a double-quoted scalar of YAML. Only the printable ASCII characters stay
/// as they are.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        let code = u32::from(c);
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            ' '..='~' => out.push(c),
            _ if code <= 0xff => out.push_str(&format!("\\x{code:02x}")),
            _ if code <= 0xffff => out.push_str(&format!("\\u{code:04x}")),
            _ => out.push_str(&format!("\\U{code:08x}")),
        }
    }
    out.push('"');
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_tag(out: &mut String, tag: Tag) {
    if tag == Tag::Unsafe {
        out.push_str("!unsafe ");
    }
}

fn write_value(out: &mut String, value: &Yaml, level: usize) {
    match value {
        Yaml::Null => out.push_str("null"),
        Yaml::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Yaml::Int(n) => out.push_str(&n.to_string()),
        Yaml::Str(s) => write_string(out, s),
        Yaml::Seq(tag, items) => {
            write_tag(out, *tag);
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(out, level + 1);
                write_value(out, item, level + 1);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(out, level);
            out.push(']');
        }
        Yaml::Map(tag, map) => {
            write_tag(out, *tag);
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, (name, item)) in map.iter().enumerate() {
                indent(out, level + 1);
                write_string(out, name);
                out.push_str(": ");
                write_value(out, item, level + 1);
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            indent(out, level);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pupitre_core::json;

    fn text(value: &Yaml) -> String {
        String::from_utf8(value.to_bytes()).unwrap()
    }

    fn object(text: &str) -> BTreeMap<String, Json> {
        json::parse(text.as_bytes(), json::ReadLimits::bytes(4096)).unwrap().as_object().unwrap().clone()
    }

    #[test]
    fn each_collection_of_a_document_is_tagged_and_a_built_one_is_not() {
        let values = object(
            r#"{"banner": "{{ lookup('pipe','id') }}", "limits": {"max_clients": 500}, "names": ["a", "{% x %}"], "on": true, "none": {}, "list": []}"#,
        );
        let file = Yaml::map([
            ("relay_config", Yaml::untrusted(&values)),
            ("pupitre_secret_files", Yaml::map([("relay", Yaml::Str("{{ pupitre_secrets_dir }}/x".into()))])),
            ("pupitre_units", Yaml::Seq(Tag::Plain, vec![])),
            ("empty", Yaml::map(Vec::<(String, Yaml)>::new())),
        ]);
        assert_eq!(
            text(&file),
            concat!(
                "{\n",
                "  \"empty\": {},\n",
                "  \"pupitre_secret_files\": {\n",
                "    \"relay\": \"{{ pupitre_secrets_dir }}/x\"\n",
                "  },\n",
                "  \"pupitre_units\": [],\n",
                "  \"relay_config\": !unsafe {\n",
                "    \"banner\": \"{{ lookup('pipe','id') }}\",\n",
                "    \"limits\": !unsafe {\n",
                "      \"max_clients\": 500\n",
                "    },\n",
                "    \"list\": !unsafe [],\n",
                "    \"names\": !unsafe [\n",
                "      \"a\",\n",
                "      \"{% x %}\"\n",
                "    ],\n",
                "    \"none\": !unsafe {},\n",
                "    \"on\": true\n",
                "  }\n",
                "}\n",
            )
        );
    }

    #[test]
    fn each_character_outside_printable_ascii_is_an_escape() {
        let cases = [
            ("", "\"\""),
            ("a\"b\\c", "\"a\\\"b\\\\c\""),
            ("l1\nl2\r\tx", "\"l1\\nl2\\r\\tx\""),
            ("\u{1}\u{1f}\u{7f}", "\"\\x01\\x1f\\x7f\""),
            ("\u{85}\u{a0}\u{e9}", "\"\\x85\\xa0\\xe9\""),
            ("\u{2028}\u{2029}\u{feff}\u{fffe}\u{202e}", "\"\\u2028\\u2029\\ufeff\\ufffe\\u202e\""),
            ("日本", "\"\\u65e5\\u672c\""),
            ("\u{1F600}\u{10FFFF}", "\"\\U0001f600\\U0010ffff\""),
            ("{{ x }} {% y %} {# z #} !unsafe # : - ? ~", "\"{{ x }} {% y %} {# z #} !unsafe # : - ? ~\""),
        ];
        for (input, expected) in cases {
            assert_eq!(text(&Yaml::Str(input.to_string())), format!("{expected}\n"), "{input:?}");
        }
        // The file is ASCII and has one line for each member, also for a long string.
        let long = "é\n".repeat(100_000);
        let bytes = Yaml::map([("a", Yaml::Str(long))]).to_bytes();
        assert!(bytes.is_ascii());
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 3);
    }

    #[test]
    fn the_form_is_stable() {
        let value = object(r#"{"b": [1, "x"], "a": {"c": false}}"#);
        assert_eq!(Yaml::untrusted(&value).to_bytes(), Yaml::untrusted(&value.clone()).to_bytes());
        // With no tag, the form is the canonical JSON of 2.4.
        let numbers =
            json::parse(br#"{"b": [1, 2], "a": {"c": false, "d": "x"}}"#, json::ReadLimits::bytes(4096)).unwrap();
        let plain = Yaml::map([
            ("a", Yaml::map([("c", Yaml::Bool(false)), ("d", Yaml::Str("x".into()))])),
            ("b", Yaml::Seq(Tag::Plain, vec![Yaml::Int(1), Yaml::Int(2)])),
        ]);
        assert_eq!(plain.to_bytes(), json::to_canonical(&numbers));
        assert_eq!(text(&Yaml::Null), "null\n");
    }
}
