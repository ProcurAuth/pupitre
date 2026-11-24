// SPDX-License-Identifier: MIT OR Apache-2.0
//! The key of the store, the derived keys, the fingerprint and the key check (2.26), and
//! the cipher of the backend `postgres` (2.39).

use std::fmt;

use pupitre_core::etag::{parse_hex32, to_hex};
use pupitre_core::ids::KeyPath;
use pupitre_core::node::SecretScope;
use ring::{aead, hkdf, hmac};

/// The salt of HKDF (2.26).
const SALT: &[u8] = b"pupitre";
/// The `info` of the encryption key (2.26).
const INFO_ENCRYPTION: &[u8] = b"pupitre secret encryption v1";
/// The `info` of the fingerprint key (2.26).
const INFO_FINGERPRINT: &[u8] = b"pupitre secret fingerprint v1";
/// The input of the key check (2.26).
const KEY_CHECK_INPUT: &[u8] = b"pupitre key check v1";
/// The length of a nonce of ChaCha20-Poly1305 (2.39).
pub const NONCE_BYTES: usize = 12;

/// The key `storage.secret_key`: 32 bytes. `Debug` never shows them.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretKey([u8; 32]);

/// Overwrite a buffer with zeros. `black_box` stops the compiler from removing the stores of a
/// buffer that nobody reads again. This is a best effort: copies that a dependency or the
/// system made are not reached (7.11).
fn wipe(bytes: &mut [u8]) {
    bytes.fill(0);
    std::hint::black_box(&*bytes);
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl SecretKey {
    /// Read the form of 2.26: exactly 64 lower-case hexadecimal characters.
    pub fn parse(text: &str) -> Option<SecretKey> {
        parse_hex32(text).map(SecretKey)
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(<hidden>)")
    }
}

/// A secret value. `Debug` never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    /// Wrap the bytes of a value.
    pub fn new(bytes: Vec<u8>) -> SecretValue {
        SecretValue(bytes)
    }

    /// The bytes. Use them only to write the private folder of a run (4.13) or to encrypt.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<hidden>)")
    }
}

/// A value that does not decrypt (2.39).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CipherError;

impl fmt::Display for CipherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the secret value does not decrypt")
    }
}

impl std::error::Error for CipherError {}

struct Len32;

impl hkdf::KeyType for Len32 {
    fn len(&self) -> usize {
        32
    }
}

fn derive(ikm: &[u8; 32], info: &[u8]) -> Result<[u8; 32], CipherError> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, SALT).extract(ikm);
    let mut out = [0u8; 32];
    let infos = [info];
    prk.expand(&infos, Len32).and_then(|okm| okm.fill(&mut out)).map_err(|_| CipherError)?;
    Ok(out)
}

/// The two keys that HKDF derives from the key of the store (2.26).
pub struct StoreKeys {
    encryption: aead::LessSafeKey,
    fingerprint: hmac::Key,
}

impl fmt::Debug for StoreKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StoreKeys(<hidden>)")
    }
}

fn aad(scope: &SecretScope, name: &KeyPath) -> Vec<u8> {
    format!("{}\n{}", scope.text(), name.as_str()).into_bytes()
}

/// The associated data of the cipher (2.39): the scope, the name and the fingerprint.
fn bound_aad(scope: &SecretScope, name: &KeyPath, fingerprint: &str) -> Vec<u8> {
    format!("{}\n{}\n{fingerprint}", scope.text(), name.as_str()).into_bytes()
}

/// A sealed value (2.39): the ciphertext with its tag, and the fingerprint that the ciphertext binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    /// The ciphertext with its tag.
    pub ciphertext: Vec<u8>,
    /// The fingerprint of the value (2.26). It is part of the associated data.
    pub fingerprint: String,
}

impl StoreKeys {
    /// Derive the encryption key and the fingerprint key. `ring` refuses no key of 32 bytes,
    /// so an error here is a defect of the library; the start then stops.
    pub fn derive(key: &SecretKey) -> Result<StoreKeys, CipherError> {
        let enc = derive(&key.0, INFO_ENCRYPTION)?;
        let fp = derive(&key.0, INFO_FINGERPRINT)?;
        let unbound = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &enc).map_err(|_| CipherError)?;
        Ok(StoreKeys {
            encryption: aead::LessSafeKey::new(unbound),
            fingerprint: hmac::Key::new(hmac::HMAC_SHA256, &fp),
        })
    }

    /// The fingerprint of a secret: the first 16 hexadecimal characters of the HMAC (2.26).
    pub fn fingerprint(&self, scope: &SecretScope, name: &KeyPath, value: &SecretValue) -> String {
        let mut input = aad(scope, name);
        input.push(b'\n');
        input.extend_from_slice(value.expose());
        let tag = hmac::sign(&self.fingerprint, &input);
        to_hex(&tag.as_ref()[..8])
    }

    /// The key check of 2.26: 64 hexadecimal characters.
    pub fn key_check(&self) -> String {
        to_hex(hmac::sign(&self.fingerprint, KEY_CHECK_INPUT).as_ref())
    }

    /// Encrypt a value (2.39). The caller gives a new random nonce for each write. The
    /// answer has the ciphertext with its tag and the fingerprint, which the associated data
    /// holds. The cipher fails only for a value above its limit of about 256 GiB, and a value
    /// has 65,536 bytes at most (2.25).
    pub fn seal(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        nonce: [u8; NONCE_BYTES],
    ) -> Result<Sealed, CipherError> {
        let fingerprint = self.fingerprint(scope, name, value);
        let mut in_out = value.expose().to_vec();
        let nonce = aead::Nonce::assume_unique_for_key(nonce);
        let result = self.encryption.seal_in_place_append_tag(
            nonce,
            aead::Aad::from(bound_aad(scope, name, &fingerprint)),
            &mut in_out,
        );
        result.map_err(|_| CipherError)?;
        Ok(Sealed { ciphertext: in_out, fingerprint })
    }

    /// Decrypt a value (2.39). The fingerprint is the one of the row. A wrong key, a wrong
    /// scope, name or fingerprint, or changed bytes give `CipherError`. The function also
    /// computes the fingerprint of the value again: a difference gives `CipherError`.
    pub fn open(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        fingerprint: &str,
        nonce: [u8; NONCE_BYTES],
        sealed: &[u8],
    ) -> Result<SecretValue, CipherError> {
        let mut in_out = sealed.to_vec();
        let nonce = aead::Nonce::assume_unique_for_key(nonce);
        let opened = self
            .encryption
            .open_in_place(nonce, aead::Aad::from(bound_aad(scope, name, fingerprint)), &mut in_out)
            .map(|plain| SecretValue(plain.to_vec()));
        wipe(&mut in_out);
        let value = opened.map_err(|_| CipherError)?;
        if self.fingerprint(scope, name, &value) != fingerprint {
            return Err(CipherError);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pupitre_core::ids::{EnvId, UnitId};

    fn keys() -> StoreKeys {
        StoreKeys::derive(
            &SecretKey::parse("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f").unwrap(),
        )
        .unwrap()
    }

    fn scope() -> SecretScope {
        SecretScope::Unit(EnvId::parse("production").unwrap(), UnitId::parse("relay").unwrap())
    }

    #[test]
    fn the_vectors_of_2_26() {
        // The vectors come from an independent computation with the HMAC and SHA-256 of Python.
        let k = keys();
        let name = KeyPath::parse("storage.password").unwrap();
        let value = SecretValue::new(b"correct horse battery staple".to_vec());
        assert_eq!(k.fingerprint(&scope(), &name, &value), "59b50c54cf977b23");
        assert_eq!(k.key_check(), "ce03a3f53a0a890ed2ff2c22961a12a9fe01b4f47a1c7a9e993f4e4194684dc9");
    }

    #[test]
    fn seal_and_open() {
        let k = keys();
        let name = KeyPath::parse("storage.password").unwrap();
        let value = SecretValue::new(b"correct horse battery staple".to_vec());
        let sealed = k.seal(&scope(), &name, &value, [7; 12]).unwrap();
        let fp = sealed.fingerprint.as_str();
        assert_eq!(fp, k.fingerprint(&scope(), &name, &value));
        assert_eq!(sealed.ciphertext.len(), value.expose().len() + 16);
        assert_eq!(k.open(&scope(), &name, fp, [7; 12], &sealed.ciphertext).unwrap(), value);
        let other = KeyPath::parse("storage.user").unwrap();
        assert_eq!(k.open(&scope(), &other, fp, [7; 12], &sealed.ciphertext), Err(CipherError));
        let env_scope = SecretScope::Environment(EnvId::parse("production").unwrap());
        assert_eq!(k.open(&env_scope, &name, fp, [7; 12], &sealed.ciphertext), Err(CipherError));
        let mut changed = sealed.ciphertext.clone();
        changed[0] ^= 1;
        assert_eq!(k.open(&scope(), &name, fp, [7; 12], &changed), Err(CipherError));
        let wrong = StoreKeys::derive(&SecretKey::parse(&"ab".repeat(32)).unwrap()).unwrap();
        assert_eq!(wrong.open(&scope(), &name, fp, [7; 12], &sealed.ciphertext), Err(CipherError));
    }

    /// L7: the fingerprint is in the associated data. An old ciphertext with the fingerprint of
    /// a new value does not open, and a ciphertext whose value has another fingerprint is refused.
    #[test]
    fn the_fingerprint_is_bound_to_the_ciphertext() {
        let k = keys();
        let name = KeyPath::parse("storage.password").unwrap();
        let old = k.seal(&scope(), &name, &SecretValue::new(b"old value".to_vec()), [1; 12]).unwrap();
        let new = k.seal(&scope(), &name, &SecretValue::new(b"new value".to_vec()), [2; 12]).unwrap();
        assert_ne!(old.fingerprint, new.fingerprint);
        // A database writer puts the old nonce and ciphertext under the fingerprint of the new value.
        assert_eq!(k.open(&scope(), &name, &new.fingerprint, [1; 12], &old.ciphertext), Err(CipherError));
        // The consistent old row still opens: a rollback of the whole row is not a forgery of the fingerprint.
        assert_eq!(k.open(&scope(), &name, &old.fingerprint, [1; 12], &old.ciphertext).unwrap().expose(), b"old value");
        // A fingerprint that is not the one of the value is refused, even if the cipher accepted it.
        let wrong_fp = "0000000000000000";
        let forged = {
            let mut in_out = b"old value".to_vec();
            let nonce = aead::Nonce::assume_unique_for_key([3; 12]);
            k.encryption
                .seal_in_place_append_tag(nonce, aead::Aad::from(bound_aad(&scope(), &name, wrong_fp)), &mut in_out)
                .unwrap();
            in_out
        };
        assert_eq!(k.open(&scope(), &name, wrong_fp, [3; 12], &forged), Err(CipherError));
    }

    #[test]
    fn the_key_has_one_form_and_never_shows() {
        assert!(SecretKey::parse(&"AB".repeat(32)).is_none());
        assert!(SecretKey::parse("00").is_none());
        let key = SecretKey::parse(&"ab".repeat(32)).unwrap();
        assert_eq!(format!("{key:?}"), "SecretKey(<hidden>)");
        assert_eq!(format!("{:?}", SecretValue::new(b"x".to_vec())), "SecretValue(<hidden>)");
    }
}
