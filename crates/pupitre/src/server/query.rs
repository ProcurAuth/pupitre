// SPDX-License-Identifier: MIT OR Apache-2.0
//! The query string (5.3) and the cursor of a list (5.6).

use super::error::ApiError;
use super::limits::MAX_CURSOR_BYTES;

/// Decode each `%XX` one time. A `+` stays a `+`. A bad escape, or a result that is not UTF-8,
/// gives `None`.
pub fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            let (hi, lo) = (hex(*bytes.get(i + 1)?)?, hex(*bytes.get(i + 2)?)?);
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The parameters of a query string. A parameter that the route does not name, a parameter two
/// times and a bad escape give `400 bad_request` (5.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query(Vec<(String, String)>);

impl Query {
    /// Read the query string of a route that names the parameters `allowed`.
    pub fn parse(raw: Option<&str>, allowed: &[&str]) -> Result<Query, ApiError> {
        let mut pairs: Vec<(String, String)> = Vec::new();
        let Some(raw) = raw else { return Ok(Query(pairs)) };
        if raw.is_empty() {
            return Ok(Query(pairs));
        }
        for part in raw.split('&') {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            let (Some(name), Some(value)) = (percent_decode(name), percent_decode(value)) else {
                return Err(ApiError::bad_request());
            };
            if !allowed.contains(&name.as_str()) || pairs.iter().any(|(n, _)| *n == name) {
                return Err(ApiError::bad_request());
            }
            pairs.push((name, value));
        }
        Ok(Query(pairs))
    }

    /// The value of a parameter.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// An integer parameter from `min` to `max`. A value out of range gives `400 bad_request`.
    /// A value with a sign, a fraction or a zero at the start gives the same error. The server
    /// never changes a value (5.6).
    pub fn integer(&self, name: &str, min: u64, max: u64) -> Result<Option<u64>, ApiError> {
        let Some(text) = self.get(name) else { return Ok(None) };
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) || (text.len() > 1 && text.starts_with('0')) {
            return Err(ApiError::bad_request());
        }
        match text.parse::<u64>() {
            Ok(n) if (min..=max).contains(&n) => Ok(Some(n)),
            _ => Err(ApiError::bad_request()),
        }
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url text with no padding.
pub fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let take = chunk.len() + 1;
        for i in 0..take {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

/// Read base64url text with no padding. Another character, a padding character or a length
/// that no byte string makes gives `None`.
pub fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in text.bytes() {
        let v = ALPHABET.iter().position(|a| *a == b)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    // The bits that remain are zero in a text that the encoder made. A length of 1 mod 4 never
    // comes from the encoder.
    if text.len() % 4 == 1 || (acc & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

/// The most hidden items that a cursor can skip. It bounds the work of a resume.
pub const MAX_SKIP: u32 = 10_000;

/// The cursor of a list (5.6). It holds the id of the last item that the page shows. It also
/// holds the count of the items after it that the display filter hid (3.15, 3.16). It never
/// holds the id of an item that the subject cannot see.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cursor {
    /// The last item that the previous page showed. Absent before the first item.
    pub last: Option<String>,
    /// The hidden items after it, that the next page skips.
    pub skip: u32,
}

impl Cursor {
    /// The text of the cursor for a route. The tag is the name of the list.
    pub fn encode(&self, tag: &str) -> String {
        let text = format!("{tag}\n{}\n{}", self.last.as_deref().unwrap_or(""), self.skip);
        base64url_encode(text.as_bytes())
    }

    /// Read a cursor of a route. A cursor that the route did not make gives `400`.
    pub fn decode(tag: &str, text: &str) -> Result<Cursor, ApiError> {
        if text.len() > MAX_CURSOR_BYTES {
            return Err(ApiError::bad_request());
        }
        let bytes = base64url_decode(text).ok_or_else(ApiError::bad_request)?;
        let text = String::from_utf8(bytes).map_err(|_| ApiError::bad_request())?;
        let mut parts = text.split('\n');
        let (Some(t), Some(last), Some(skip), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err(ApiError::bad_request());
        };
        let count: u32 = skip.parse().map_err(|_| ApiError::bad_request())?;
        if t != tag || count > MAX_SKIP || last.len() > 256 || count.to_string() != skip {
            return Err(ApiError::bad_request());
        }
        Ok(Cursor { last: (!last.is_empty()).then(|| last.to_string()), skip: count })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_follows_5_3() {
        let q = Query::parse(Some("limit=5&after=abc"), &["limit", "after"]).unwrap();
        assert_eq!(q.get("limit"), Some("5"));
        assert_eq!(q.get("after"), Some("abc"));
        assert!(Query::parse(None, &[]).unwrap().get("x").is_none());
        assert!(Query::parse(Some(""), &[]).is_ok());
        for bad in ["other=1", "limit=1&limit=2", "limit=%zz", "limit=%f"] {
            assert!(Query::parse(Some(bad), &["limit"]).is_err(), "{bad}");
        }
        // A `+` is not a space, and an escape is decoded one time.
        let q = Query::parse(Some("host=a%2Bb+c%2541"), &["host"]).unwrap();
        assert_eq!(q.get("host"), Some("a+b+c%41"));
    }

    #[test]
    fn an_integer_parameter_is_never_changed_5_6() {
        let q = |s: &str| Query::parse(Some(s), &["limit"]).unwrap();
        assert_eq!(q("limit=5").integer("limit", 1, 10), Ok(Some(5)));
        assert_eq!(q("").integer("limit", 1, 10), Ok(None));
        for bad in [
            "limit=0",
            "limit=11",
            "limit=-1",
            "limit=05",
            "limit=",
            "limit=1.5",
            "limit=%2B1",
            "limit=99999999999999999999",
        ] {
            assert!(q(bad).integer("limit", 1, 10).is_err(), "{bad}");
        }
    }

    #[test]
    fn base64url_round_trips_and_refuses_the_rest() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let text = base64url_encode(&bytes);
            assert!(!text.contains('='));
            assert_eq!(base64url_decode(&text), Some(bytes));
        }
        assert_eq!(base64url_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url_decode("Zm9vYg"), Some(b"foob".to_vec()));
        for bad in ["Zm9vYg==", "Zm9v+g", "Zm9v/g", "Z", "Zm9vYh"] {
            assert_eq!(base64url_decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_cursor_belongs_to_its_route_5_6() {
        let c = Cursor { last: Some("host-a".into()), skip: 3 };
        let text = c.encode("hosts");
        assert_eq!(Cursor::decode("hosts", &text), Ok(c));
        assert!(Cursor::decode("units", &text).is_err());
        assert!(Cursor::decode("hosts", "not a cursor").is_err());
        assert!(Cursor::decode("hosts", &base64url_encode(b"hosts\nx")).is_err());
        assert!(Cursor::decode("hosts", &base64url_encode(b"hosts\nx\n99999")).is_err());
        assert!(Cursor::decode("hosts", &base64url_encode(b"hosts\nx\n007")).is_err());
        assert!(Cursor::decode("hosts", &"A".repeat(MAX_CURSOR_BYTES + 1)).is_err());
        let first = Cursor::default().encode("hosts");
        assert_eq!(Cursor::decode("hosts", &first), Ok(Cursor::default()));
    }
}
