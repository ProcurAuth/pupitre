// SPDX-License-Identifier: MIT OR Apache-2.0
//! The strict JSON reader (2.5) and the two writers: the canonical form of the stored
//! documents (2.4) and the compact form of the API answers (5.3).

use std::collections::BTreeMap;
use std::fmt;

/// The largest integer of a document: 2^53-1 (2.4, rule 6).
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// The default depth bound of the reader (2.5).
pub const MAX_DEPTH: usize = 16;

/// A JSON value. An object keeps its members in the byte order of their names, which is the
/// order of the canonical form. `Null` exists only for the writer: the reader refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    /// `null`. Only the options route writes it (5.3).
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer from 0 to `MAX_SAFE_INTEGER`.
    Int(u64),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object.
    Obj(BTreeMap<String, Json>),
}

/// An error of the reader: the byte offset and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    /// The byte offset of the error in the input.
    pub offset: usize,
    /// The reason, a fixed English text.
    pub reason: &'static str,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.reason, self.offset)
    }
}

impl std::error::Error for JsonError {}

/// The bounds of one read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// The most bytes of the input.
    pub max_bytes: usize,
    /// The most levels of objects and arrays.
    pub max_depth: usize,
}

impl ReadLimits {
    /// Bounds with the default depth of 2.5.
    pub const fn bytes(max_bytes: usize) -> Self {
        ReadLimits { max_bytes, max_depth: MAX_DEPTH }
    }
}

impl Json {
    /// Make an object from pairs. A later pair with the same name replaces an earlier one.
    pub fn object<I, K>(pairs: I) -> Json
    where
        I: IntoIterator<Item = (K, Json)>,
        K: Into<String>,
    {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Make a string value.
    pub fn str(text: impl Into<String>) -> Json {
        Json::Str(text.into())
    }

    /// The object, when the value is one.
    pub fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Obj(map) => Some(map),
            _ => None,
        }
    }

    /// The text, when the value is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The integer, when the value is one.
    pub fn as_int(&self) -> Option<u64> {
        match self {
            Json::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// The boolean, when the value is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The items, when the value is an array.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// The name of the JSON type, for an error message.
    pub fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "a boolean",
            Json::Int(_) => "an integer",
            Json::Str(_) => "a string",
            Json::Arr(_) => "an array",
            Json::Obj(_) => "an object",
        }
    }
}

/// Read a JSON text with the rules of 2.5.
pub fn parse(input: &[u8], limits: ReadLimits) -> Result<Json, JsonError> {
    if input.len() > limits.max_bytes {
        return Err(JsonError { offset: limits.max_bytes, reason: "the text is larger than its bound" });
    }
    if input.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(JsonError { offset: 0, reason: "the text starts with a byte order mark" });
    }
    let text = match std::str::from_utf8(input) {
        Ok(t) => t,
        Err(e) => return Err(JsonError { offset: e.valid_up_to(), reason: "the text is not UTF-8" }),
    };
    let mut reader = Reader { bytes: text.as_bytes(), text, pos: 0, max_depth: limits.max_depth };
    reader.skip_space();
    let value = reader.value(0)?;
    reader.skip_space();
    if reader.pos != reader.bytes.len() {
        return Err(reader.error("text after the value"));
    }
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
    max_depth: usize,
}

impl Reader<'_> {
    fn error(&self, reason: &'static str) -> JsonError {
        JsonError { offset: self.pos, reason }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    fn expect_word(&mut self, word: &'static [u8], reason: &'static str) -> Result<(), JsonError> {
        if self.bytes[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(self.error(reason))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        match self.peek() {
            None => Err(self.error("the text ends before a value")),
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.expect_word(b"true", "an unknown word").map(|()| Json::Bool(true)),
            Some(b'f') => self.expect_word(b"false", "an unknown word").map(|()| Json::Bool(false)),
            Some(b'n') => Err(self.error("null is not permitted")),
            Some(b'-') => Err(self.error("a number with a sign is not permitted")),
            Some(b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("an unexpected character")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > self.max_depth {
            return Err(self.error("the depth is above its bound"));
        }
        self.pos += 1;
        let mut map = BTreeMap::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(map));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected the name of a member"));
            }
            let name_at = self.pos;
            let name = self.string()?;
            self.skip_space();
            if self.peek() != Some(b':') {
                return Err(self.error("expected a colon"));
            }
            self.pos += 1;
            self.skip_space();
            let value = self.value(depth)?;
            if map.insert(name, value).is_some() {
                return Err(JsonError { offset: name_at, reason: "a member name occurs two times" });
            }
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(map));
                }
                _ => return Err(self.error("expected a comma or the end of the object")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > self.max_depth {
            return Err(self.error("the depth is above its bound"));
        }
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_space();
            items.push(self.value(depth)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.error("expected a comma or the end of the array")),
            }
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;
        while let Some(b'0'..=b'9') = self.peek() {
            self.pos += 1;
        }
        if let Some(b'.' | b'e' | b'E') = self.peek() {
            return Err(self.error("a number with a fraction or an exponent is not permitted"));
        }
        let digits = &self.text[start..self.pos];
        if digits.len() > 1 && digits.starts_with('0') {
            return Err(JsonError { offset: start, reason: "a number with a zero at the start is not permitted" });
        }
        match digits.parse::<u64>() {
            Ok(n) if n <= MAX_SAFE_INTEGER => Ok(Json::Int(n)),
            _ => Err(JsonError { offset: start, reason: "an integer above 2^53-1 is not permitted" }),
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let Some(chunk) = self.text.get(self.pos..self.pos + 4) else {
            return Err(self.error("an escape that is not valid"));
        };
        if !chunk.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(self.error("an escape that is not valid"));
        }
        let value = u32::from_str_radix(chunk, 16).map_err(|_| self.error("an escape that is not valid"))?;
        self.pos += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let run_start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(&self.text[run_start..self.pos]);
            match self.peek() {
                None => return Err(self.error("the text ends in a string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.error("the text ends in a string"));
                    };
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first) {
                                if !self.bytes[self.pos..].starts_with(b"\\u") {
                                    return Err(self.error("a lone surrogate is not permitted"));
                                }
                                self.pos += 2;
                                let second = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&second) {
                                    return Err(self.error("a lone surrogate is not permitted"));
                                }
                                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&first) {
                                return Err(self.error("a lone surrogate is not permitted"));
                            } else {
                                first
                            };
                            match char::from_u32(code) {
                                Some(c) => out.push(c),
                                None => return Err(self.error("an escape that is not valid")),
                            }
                        }
                        _ => return Err(self.error("an escape that is not valid")),
                    }
                }
                Some(_) => return Err(self.error("a control character in a string")),
            }
        }
    }
}

/// Write a string with the escapes of 2.4, rule 7.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Write a string in the JSON form of 2.4, with its quotes. The scrubber of 4.21 uses it.
pub fn string_form(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    write_string(&mut out, text);
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_pretty(out: &mut String, value: &Json, level: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(n) => out.push_str(&n.to_string()),
        Json::Str(s) => write_string(out, s),
        Json::Arr(items) if items.is_empty() => out.push_str("[]"),
        Json::Arr(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(out, level + 1);
                write_pretty(out, item, level + 1);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(out, level);
            out.push(']');
        }
        Json::Obj(map) if map.is_empty() => out.push_str("{}"),
        Json::Obj(map) => {
            out.push_str("{\n");
            for (i, (name, item)) in map.iter().enumerate() {
                indent(out, level + 1);
                write_string(out, name);
                out.push_str(": ");
                write_pretty(out, item, level + 1);
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            indent(out, level);
            out.push('}');
        }
    }
}

fn write_compact(out: &mut String, value: &Json) {
    match value {
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(out, item);
            }
            out.push(']');
        }
        Json::Obj(map) => {
            out.push('{');
            for (i, (name, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, name);
                out.push(':');
                write_compact(out, item);
            }
            out.push('}');
        }
        other => write_pretty(out, other, 0),
    }
}

/// The canonical bytes of a value (2.4): two spaces of indent, members in byte order, one
/// U+000A at the end.
pub fn to_canonical(value: &Json) -> Vec<u8> {
    let mut out = String::new();
    write_pretty(&mut out, value, 0);
    out.push('\n');
    out.into_bytes()
}

/// The compact bytes of a value (5.3): the rules of 2.4 with no space and no line end.
pub fn to_compact(value: &Json) -> Vec<u8> {
    let mut out = String::new();
    write_compact(&mut out, value);
    out.into_bytes()
}

/// The offset of the first byte where `stored` differs from the canonical form of its value,
/// or `None` when the bytes are canonical (2.5).
pub fn canonical_difference(stored: &[u8], value: &Json) -> Option<usize> {
    let canonical = to_canonical(value);
    if canonical == stored {
        return None;
    }
    Some(stored.iter().zip(canonical.iter()).position(|(a, b)| a != b).unwrap_or(stored.len().min(canonical.len())))
}

/// A reader of the members of a closed object. Each `take` removes a member. `finish` gives
/// an error for each member that no `take` read.
#[derive(Debug)]
pub struct Members {
    map: BTreeMap<String, Json>,
}

impl Members {
    /// Start from a value. A value that is not an object gives an error.
    pub fn of(value: &Json) -> Result<Members, String> {
        match value {
            Json::Obj(map) => Ok(Members { map: map.clone() }),
            other => Err(format!("expected an object, found {}", other.type_name())),
        }
    }

    /// Take a member.
    pub fn take(&mut self, name: &str) -> Option<Json> {
        self.map.remove(name)
    }

    /// The names of the members that no `take` read, in byte order.
    pub fn finish(self) -> Vec<String> {
        self.map.into_keys().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Result<Json, JsonError> {
        parse(text.as_bytes(), ReadLimits::bytes(1 << 20))
    }

    #[test]
    fn reads_and_writes_the_canonical_form() {
        let value = read(r#"{"values": {"b": 2, "a": "xé"}, "format": 1, "list": [true, false], "e": {}}"#).unwrap();
        let bytes = to_canonical(&value);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert_eq!(
            text,
            "{\n  \"e\": {},\n  \"format\": 1,\n  \"list\": [\n    true,\n    false\n  ],\n  \"values\": {\n    \"a\": \"x\u{e9}\",\n    \"b\": 2\n  }\n}\n"
        );
        assert_eq!(parse(&bytes, ReadLimits::bytes(1 << 20)).unwrap(), value);
        assert_eq!(canonical_difference(&bytes, &value), None);
        let compact = String::from_utf8(to_compact(&value)).unwrap();
        assert_eq!(compact, "{\"e\":{},\"format\":1,\"list\":[true,false],\"values\":{\"a\":\"x\u{e9}\",\"b\":2}}");
    }

    #[test]
    fn the_escapes_of_rule_7() {
        let value = Json::str("a\"b\\c\n\u{1}\u{7f}/\u{2028}");
        assert_eq!(String::from_utf8(to_compact(&value)).unwrap(), "\"a\\\"b\\\\c\\n\\u0001\u{7f}/\u{2028}\"");
        assert_eq!(string_form("x\"y"), "\"x\\\"y\"");
    }

    #[test]
    fn refuses_the_forms_of_2_5() {
        let cases: &[(&[u8], &str)] = &[
            (b"null", "null is not permitted"),
            (b"-1", "a number with a sign is not permitted"),
            (b"1.5", "a number with a fraction or an exponent is not permitted"),
            (b"1e2", "a number with a fraction or an exponent is not permitted"),
            (b"01", "a number with a zero at the start is not permitted"),
            (b"9007199254740992", "an integer above 2^53-1 is not permitted"),
            (br#"{"a":1,"a":2}"#, "a member name occurs two times"),
            (br#""\ud800""#, "a lone surrogate is not permitted"),
            (br#""\udc00""#, "a lone surrogate is not permitted"),
            (br#""\x""#, "an escape that is not valid"),
            (b"\"a\nb\"", "a control character in a string"),
            (b"\xEF\xBB\xBF{}", "the text starts with a byte order mark"),
            (b"{} {}", "text after the value"),
            (b"\xff", "the text is not UTF-8"),
            (b"tru", "an unknown word"),
        ];
        for (input, reason) in cases {
            let error = parse(input, ReadLimits::bytes(1 << 20)).unwrap_err();
            assert_eq!(error.reason, *reason, "{:?}", String::from_utf8_lossy(input));
        }
    }

    #[test]
    fn the_bounds_of_size_and_depth() {
        assert!(parse(b"[1]", ReadLimits::bytes(2)).is_err());
        let deep = format!("{}{}", "[".repeat(16), "]".repeat(16));
        assert!(parse(deep.as_bytes(), ReadLimits::bytes(1 << 20)).is_ok());
        let deeper = format!("{}{}", "[".repeat(17), "]".repeat(17));
        assert_eq!(
            parse(deeper.as_bytes(), ReadLimits::bytes(1 << 20)).unwrap_err().reason,
            "the depth is above its bound"
        );
    }

    #[test]
    fn a_surrogate_pair_gives_one_character() {
        assert_eq!(read(r#""😀""#).unwrap(), Json::str("\u{1F600}"));
    }

    #[test]
    fn a_non_canonical_text_gives_the_first_offset() {
        let value = read("{\"a\":1}").unwrap();
        assert_eq!(canonical_difference(b"{\"a\":1}", &value), Some(1));
    }

    #[test]
    fn members_finds_the_unknown_names() {
        let value = read(r#"{"a": 1, "b": 2, "c": 3}"#).unwrap();
        let mut members = Members::of(&value).unwrap();
        assert_eq!(members.take("a"), Some(Json::Int(1)));
        assert_eq!(members.finish(), vec!["b".to_string(), "c".to_string()]);
        assert!(Members::of(&Json::Int(1)).is_err());
    }
}
