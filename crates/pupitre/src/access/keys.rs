// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public keys of the edge (3.4, 6.13): PEM text or a JWK Set with 1 to 16 keys of the
//! algorithms `Ed25519` and `ES256` (P 9.2).
//!
//! The text is a JWK Set when its first character, after spaces, tabs and line ends, is `{`.
//! Else it is PEM text. The check never changes the text and never repairs it.
//!
//! - A PEM text has only blocks `PUBLIC KEY` (SubjectPublicKeyInfo). Text outside the blocks
//!   is an error. Each key gets the RFC 7638 thumbprint as its `kid`.
//! - A JWK Set has public keys only. With more than one key, each key MUST have a `kid`,
//!   because the token MUST name its key (P 9.4).

use procurauth_core::jws::{KeySet, PublicKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The most keys in the text (6.13).
pub const MAX_KEYS: usize = 16;
/// The most bytes of base64 text in one PEM block.
pub const MAX_PEM_BASE64_BYTES: usize = 4096;

const BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
const END: &str = "-----END PUBLIC KEY-----";
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Check the text of `access.identity.public_keys` at stage 2 (6.13).
///
/// The error has the form `expected <rule>, found <detail>`. It never holds the text.
pub fn check_public_keys(text: &str) -> Result<(), String> {
    parse_public_keys(text).map(|_| ())
}

/// Read the text of `access.identity.public_keys` into the key set of the PEP.
pub fn parse_public_keys(text: &str) -> Result<KeySet, String> {
    let first = text.trim_start_matches([' ', '\t', '\r', '\n']).chars().next();
    let set = if first == Some('{') { parse_jwks(text)? } else { parse_pem(text)? };
    if set.is_empty() || set.len() > MAX_KEYS {
        return Err(format!("expected 1 to {MAX_KEYS} public keys, found {}", set.len()));
    }
    Ok(set)
}

fn parse_jwks(text: &str) -> Result<KeySet, String> {
    let set = KeySet::from_jwks_str(text).map_err(|findings| {
        let first = findings.first().map_or_else(String::new, ToString::to_string);
        format!("expected a JWK Set of public keys of Ed25519 and ES256, found: {first}")
    })?;
    if set.len() > 1 && set.keys().iter().any(|k| k.kid().is_none()) {
        return Err(
            "expected a `kid` in each key of a JWK Set with more than one key, found a key with no `kid`".to_string()
        );
    }
    Ok(set)
}

fn parse_pem(text: &str) -> Result<KeySet, String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        match &mut current {
            Some(body) => {
                if line == END {
                    blocks.push(current.take().unwrap_or_default());
                } else if line.starts_with("-----") {
                    return Err(format!("expected a PEM block PUBLIC KEY, found another boundary at line {number}"));
                } else {
                    body.push_str(line);
                    if body.len() > MAX_PEM_BASE64_BYTES {
                        return Err(format!(
                            "expected at most {MAX_PEM_BASE64_BYTES} bytes of base64 text in a PEM block, found more at line {number}"
                        ));
                    }
                }
            }
            None => {
                if line == BEGIN {
                    current = Some(String::new());
                } else if line.starts_with("-----BEGIN ") {
                    return Err(format!("expected a PEM block PUBLIC KEY, found another type at line {number}"));
                } else if !line.is_empty() {
                    return Err(format!(
                        "expected only empty lines outside the PEM blocks, found text at line {number}"
                    ));
                }
            }
        }
        if blocks.len() > MAX_KEYS {
            return Err(format!("expected 1 to {MAX_KEYS} public keys, found more at line {number}"));
        }
    }
    if current.is_some() {
        return Err("expected an end line for each PEM block, found a block with no end".to_string());
    }
    let mut keys: Vec<PublicKey> = Vec::new();
    for (i, body) in blocks.iter().enumerate() {
        let pem = wrap_pem(body);
        let bare = PublicKey::from_spki_pem(&pem, None).map_err(|_| {
            format!(
                "expected an Ed25519 key or a P-256 key in PEM block {}, found another key or text that is not valid",
                i + 1
            )
        })?;
        let kid = thumbprint(&bare);
        let key = PublicKey::from_spki_pem(&pem, Some(&kid))
            .map_err(|_| format!("expected a valid key in PEM block {}, found none", i + 1))?;
        if keys.iter().any(|k| k.kid() == Some(kid.as_str())) {
            return Err(format!("expected a different key in each PEM block, found the same key twice (kid {kid:?})"));
        }
        keys.push(key);
    }
    Ok(KeySet::from_keys(keys))
}

/// The PEM block of a base64 body, with lines of 64 characters.
fn wrap_pem(body: &str) -> String {
    let mut out = String::from(BEGIN);
    out.push('\n');
    for line in body.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(line));
        out.push('\n');
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// The RFC 7638 thumbprint of a key: the SHA-256 of its required JWK members, in base64url.
fn thumbprint(key: &PublicKey) -> String {
    let jwk = key.to_jwk();
    let member = |name: &str| jwk.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
    // The members are in the order of their names, with no space (RFC 7638, 3.1).
    let text = if member("kty") == "OKP" {
        format!("{{\"crv\":\"Ed25519\",\"kty\":\"OKP\",\"x\":\"{}\"}}", member("x"))
    } else {
        format!("{{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"{}\",\"y\":\"{}\"}}", member("x"), member("y"))
    };
    base64url(Sha256::digest(text.as_bytes()).as_slice())
}

/// The base64url text of bytes, with no padding.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

/// The bytes of a base64url text with no padding, or `None` for any other text.
pub(crate) fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = ALPHABET.iter().position(|c| *c == byte)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    // The unused bits MUST be zero, and a text with 1 character more than a multiple of 4 is not valid.
    if text.len() % 4 == 1 || (acc & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips_and_is_strict() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len).map(|i| u8::try_from(i * 7 % 251).unwrap_or(0)).collect();
            let text = base64url(&bytes);
            assert!(!text.contains('='));
            assert_eq!(base64url_decode(&text), Some(bytes));
        }
        assert_eq!(base64url(b"\xfb\xff"), "-_8");
        assert_eq!(base64url_decode("-_8"), Some(b"\xfb\xff".to_vec()));
        // Padding, a character outside the alphabet and a trailing bit are refused.
        assert_eq!(base64url_decode("-_8="), None);
        assert_eq!(base64url_decode("+/8"), None);
        assert_eq!(base64url_decode("-_9"), None);
        assert_eq!(base64url_decode("A"), None);
    }
}
