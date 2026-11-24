// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of `check_public_keys` (6.13): PEM text and JWK Sets of 1 to 16 keys of `Ed25519` and
//! `ES256`, and nothing else.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod access_support;

use access_support::*;
use procurauth_core::jws::{SigningKey, ed25519_public_pem};
use pupitre::access::{check_public_keys, parse_public_keys};
use serde_json::{Value, json};

fn ed_pem(seed: u8) -> String {
    ed25519_public_pem(&SigningKey::from_ed25519_seed(&[seed; 32]).public_key(None)).unwrap()
}

fn ed_jwk(seed: u8, kid: Option<&str>) -> Value {
    SigningKey::from_ed25519_seed(&[seed; 32]).public_key(kid).to_jwk()
}

fn set(keys: &[Value]) -> String {
    json!({"keys": keys}).to_string()
}

fn error(text: &str) -> String {
    check_public_keys(text).unwrap_err()
}

#[test]
fn a_pem_text_with_one_key_of_each_algorithm_passes() {
    assert!(check_public_keys(&ed_pem(1)).is_ok());
    assert!(check_public_keys(P256_PUBLIC_PEM).is_ok());
    let both = format!("{}\n{P256_PUBLIC_PEM}", ed_pem(1));
    assert_eq!(parse_public_keys(&both).unwrap().len(), 2);
}

#[test]
fn a_jwk_set_with_keys_of_each_algorithm_passes() {
    assert!(check_public_keys(&jwks_two()).is_ok());
    assert_eq!(parse_public_keys(&jwks_two()).unwrap().len(), 2);
    // One key needs no `kid`. A space before the object is allowed.
    assert!(check_public_keys(&format!(" \n\t{}", set(&[ed_jwk(1, None)]))).is_ok());
}

#[test]
fn from_1_to_16_keys_pass_and_17_keys_stop() {
    let sixteen: Vec<Value> = (1..=16u8).map(|i| ed_jwk(i, Some(&format!("k{i}")))).collect();
    assert_eq!(parse_public_keys(&set(&sixteen)).unwrap().len(), 16);
    let mut seventeen = sixteen.clone();
    seventeen.push(ed_jwk(17, Some("k17")));
    assert!(error(&set(&seventeen)).starts_with("expected 1 to 16 public keys, found 17"));
    let pem16: String = (1..=16u8).map(ed_pem).collect();
    assert_eq!(parse_public_keys(&pem16).unwrap().len(), 16);
    let pem17: String = (1..=17u8).map(ed_pem).collect();
    assert!(error(&pem17).starts_with("expected 1 to 16 public keys"));
}

#[test]
fn no_key_stops() {
    assert!(error("").starts_with("expected 1 to 16 public keys, found 0"));
    assert!(error("  \n").starts_with("expected 1 to 16 public keys, found 0"));
    assert!(error(r#"{"keys":[]}"#).contains("expected a JWK Set"));
}

#[test]
fn a_pem_text_that_is_not_only_public_keys_stops() {
    assert!(error(&format!("note\n{}", ed_pem(1))).contains("text at line 1"));
    assert!(error("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n").contains("another type"));
    assert!(error("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n").contains("another type"));
    assert!(error(&ed_pem(1).replace("-----END PUBLIC KEY-----", "")).contains("no end"));
    assert!(
        error("-----BEGIN PUBLIC KEY-----\nMAAA\n-----END PUBLIC KEY-----\n").contains("Ed25519 key or a P-256 key")
    );
    let long = format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", "A".repeat(4100));
    assert!(error(&long).contains("4096 bytes"));
    // The same key two times.
    assert!(error(&ed_pem(1).repeat(2)).contains("twice"));
}

#[test]
fn a_key_of_another_algorithm_stops() {
    // An RSA key and an Ed448 key in a JWK Set. The text of the members is of the right form.
    let rsa = json!({"kty": "RSA", "n": "AQAB", "e": "AQAB", "kid": "a"});
    assert!(error(&set(&[rsa])).contains("JWK Set"));
    let ed448 = json!({"kty": "OKP", "crv": "Ed448", "x": "AAAA", "kid": "a"});
    assert!(error(&set(&[ed448])).contains("Ed25519"));
    let p384 = json!({"kty": "EC", "crv": "P-384", "x": "AAAA", "y": "AAAA", "kid": "a"});
    assert!(error(&set(&[p384])).contains("P-256"));
    // The algorithm of the key says another one.
    let mut wrong = ed_jwk(1, Some("a"));
    wrong["alg"] = json!("ES256");
    assert!(error(&set(&[wrong])).contains("alg"));
    let mut edsa = ed_jwk(1, Some("a"));
    edsa["alg"] = json!("EdDSA");
    assert!(check_public_keys(&set(&[edsa])).is_err(), "the name EdDSA is deprecated (P 9.2)");
}

#[test]
fn a_private_key_in_a_jwk_set_stops() {
    let mut key = ed_jwk(1, Some("a"));
    key["d"] = json!("AAAA");
    assert!(error(&set(&[key])).contains("`d`"));
}

#[test]
fn a_point_that_is_not_on_the_curve_stops() {
    let bad = json!({"kty": "EC", "crv": "P-256", "kid": "a",
        "x": "zLnzLhZKGQpAjaN3vyE01nakXnIMc1rEX05_bPP0qNw", "y": "zLnzLhZKGQpAjaN3vyE01nakXnIMc1rEX05_bPP0qNw"});
    assert!(check_public_keys(&set(&[bad])).is_err());
}

#[test]
fn a_set_with_more_than_one_key_needs_a_kid_in_each_key() {
    let e = error(&set(&[ed_jwk(1, Some("a")), ed_jwk(2, None)]));
    assert!(e.contains("`kid`"), "{e}");
    // The same `kid` two times stops too.
    assert!(check_public_keys(&set(&[ed_jwk(1, Some("a")), ed_jwk(2, Some("a"))])).is_err());
    // An empty `kid` stops.
    assert!(check_public_keys(&set(&[ed_jwk(1, Some(""))])).is_err());
}

#[test]
fn a_json_text_that_is_not_strict_stops() {
    assert!(check_public_keys("{").is_err());
    assert!(check_public_keys(r#"{"keys":[],"keys":[]}"#).is_err());
    let one = set(&[ed_jwk(1, Some("a"))]);
    assert!(check_public_keys(&format!("{one} x")).is_err());
    assert!(check_public_keys(r#"{"other":[]}"#).is_err());
}

#[test]
fn a_pem_key_gets_the_thumbprint_as_its_kid() {
    // The thumbprint of RFC 7638 for the Ed25519 key of the seed [1; 32]: a token for the edge
    // names it in `kid`, or has no `kid` when the text holds one key.
    let key = SigningKey::from_ed25519_seed(&[1; 32]);
    let set = parse_public_keys(&ed_pem(1)).unwrap();
    let kid = set.keys()[0].kid().unwrap().to_owned();
    assert_eq!(kid.len(), 43);
    assert!(kid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    let jwk = key.public_key(None).to_jwk();
    let expected = {
        use sha2::{Digest, Sha256};
        let text = format!("{{\"crv\":\"Ed25519\",\"kty\":\"OKP\",\"x\":\"{}\"}}", jwk["x"].as_str().unwrap());
        let digest = Sha256::digest(text.as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    // Compare through the hexadecimal form of the decoded thumbprint.
    let decoded = procurauth_core::jws::b64url_decode(&kid).unwrap();
    assert_eq!(decoded.iter().map(|b| format!("{b:02x}")).collect::<String>(), expected);
}

#[test]
fn the_error_never_holds_the_key_text() {
    let text = ed_pem(1).replace("-----END PUBLIC KEY-----", "");
    let e = error(&text);
    let body: String = text.lines().nth(1).unwrap().to_owned();
    assert!(!e.contains(&body), "{e}");
}
