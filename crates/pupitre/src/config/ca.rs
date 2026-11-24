// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check a CA file (6.13). It is a regular file of 1 MiB at most. It has 1 to 64 PEM blocks
//! `CERTIFICATE` and no other text outside the blocks. Each block is an X.509 certificate.

use std::path::Path;

use hclconf::fsio::{ReadError, read_regular_file};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;

/// The largest CA file (6.25).
pub const MAX_CA_BYTES: u64 = 1_048_576;
/// The most certificates in a CA file (6.25).
pub const MAX_CA_CERTIFICATES: usize = 64;

const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const END: &str = "-----END CERTIFICATE-----";

/// Check the structure of the PEM text: the blocks and the text outside them. Give the
/// number of blocks.
fn scan_blocks(text: &str) -> Result<usize, String> {
    let mut blocks = 0;
    let mut inside = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if inside {
            if line == END {
                inside = false;
            } else if line.starts_with("-----") {
                return Err(format!("a PEM boundary that is not {END} at line {number}"));
            }
        } else if line == BEGIN {
            inside = true;
            blocks += 1;
        } else if line.starts_with("-----BEGIN ") {
            return Err(format!("a PEM block that is not CERTIFICATE at line {number}"));
        } else if !line.is_empty() {
            return Err(format!("text outside the PEM blocks at line {number}"));
        }
    }
    if inside {
        return Err("a PEM block with no end".to_string());
    }
    Ok(blocks)
}

/// Check PEM bytes and give the certificates. The error is the detail for the message.
pub fn check_ca_pem(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the content is not UTF-8".to_string())?;
    let blocks = scan_blocks(text)?;
    if blocks == 0 {
        return Err(format!("no PEM block CERTIFICATE; expected 1 to {MAX_CA_CERTIFICATES}"));
    }
    if blocks > MAX_CA_CERTIFICATES {
        return Err(format!("{blocks} certificates; the maximum is {MAX_CA_CERTIFICATES}"));
    }
    let mut certificates = Vec::new();
    for (index, item) in CertificateDer::pem_slice_iter(bytes).enumerate() {
        let der = item.map_err(|e| format!("block {} is not valid PEM: {e}", index + 1))?;
        certificates.push(der);
    }
    if certificates.len() != blocks {
        return Err(format!("{blocks} PEM blocks but {} certificates", certificates.len()));
    }
    let mut store = rustls::RootCertStore::empty();
    for (index, der) in certificates.iter().enumerate() {
        store.add(der.clone()).map_err(|_| format!("block {} is not an X.509 certificate", index + 1))?;
    }
    Ok(certificates)
}

/// Read a CA file and check it. Give the bytes of the file, or the text of the error
/// without its key path, in the form `cannot read the file <path>: <reason>` or
/// `the CA file <path> is not valid: <detail>`.
pub fn read_ca_file(path: &str) -> Result<Vec<u8>, String> {
    let bytes = match read_regular_file(Path::new(path), MAX_CA_BYTES) {
        Ok(b) => b,
        Err(ReadError::TooLarge(_)) => {
            return Err(format!("cannot read the file {path}: the file has more than {MAX_CA_BYTES} bytes"));
        }
        Err(e) => return Err(format!("cannot read the file {path}: {e}")),
    };
    match check_ca_pem(&bytes) {
        Ok(_) => Ok(bytes),
        Err(detail) => Err(format!("the CA file {path} is not valid: {detail}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway self-signed certificate, made with `openssl req -x509` for these tests.
    const CERT: &str = include_str!("../../../../tests/fixtures/config/ca-a.pem");
    const CERT_B: &str = include_str!("../../../../tests/fixtures/config/ca-b.pem");

    #[test]
    fn one_certificate_passes() {
        assert_eq!(check_ca_pem(CERT.as_bytes()).unwrap().len(), 1);
    }

    #[test]
    fn two_certificates_and_empty_lines_pass() {
        let text = format!("\n{CERT}\n\n{CERT_B}\n");
        assert_eq!(check_ca_pem(text.as_bytes()).unwrap().len(), 2);
    }

    #[test]
    fn sixty_four_pass_and_sixty_five_fail() {
        let text = CERT.repeat(64);
        assert_eq!(check_ca_pem(text.as_bytes()).unwrap().len(), 64);
        let text = CERT.repeat(65);
        assert_eq!(check_ca_pem(text.as_bytes()).unwrap_err(), "65 certificates; the maximum is 64");
    }

    #[test]
    fn no_block_fails() {
        assert!(check_ca_pem(b"").unwrap_err().starts_with("no PEM block CERTIFICATE"));
        assert!(check_ca_pem(b"\n\n").unwrap_err().starts_with("no PEM block CERTIFICATE"));
    }

    #[test]
    fn text_outside_the_blocks_fails() {
        let text = format!("note\n{CERT}");
        assert_eq!(check_ca_pem(text.as_bytes()).unwrap_err(), "text outside the PEM blocks at line 1");
        let text = format!("{CERT}trailer\n");
        assert!(check_ca_pem(text.as_bytes()).unwrap_err().starts_with("text outside the PEM blocks"));
    }

    #[test]
    fn other_block_types_fail() {
        let key = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";
        assert_eq!(check_ca_pem(key.as_bytes()).unwrap_err(), "a PEM block that is not CERTIFICATE at line 1");
        let text = format!("{CERT}{key}");
        assert!(check_ca_pem(text.as_bytes()).is_err());
    }

    #[test]
    fn a_block_with_no_end_fails() {
        let text = CERT.replace(END, "");
        assert!(check_ca_pem(text.as_bytes()).is_err());
    }

    #[test]
    fn bad_certificate_content_fails() {
        let text = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        assert_eq!(check_ca_pem(text.as_bytes()).unwrap_err(), "block 1 is not an X.509 certificate");
        let text = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        assert!(check_ca_pem(text.as_bytes()).unwrap_err().contains("not valid PEM"));
    }

    #[test]
    fn not_utf8_fails() {
        assert_eq!(check_ca_pem(&[0xff, 0xfe]).unwrap_err(), "the content is not UTF-8");
    }

    #[test]
    fn read_ca_file_cases() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("ca.pem");
        std::fs::write(&good, CERT).unwrap();
        let path = good.display().to_string();
        assert_eq!(read_ca_file(&path).unwrap(), CERT.as_bytes());

        let e = read_ca_file(&dir.path().join("none").display().to_string()).unwrap_err();
        assert!(e.starts_with("cannot read the file ") && e.ends_with("No such file or directory"), "{e}");

        let big = dir.path().join("big.pem");
        std::fs::write(&big, vec![b' '; 1_048_577]).unwrap();
        let e = read_ca_file(&big.display().to_string()).unwrap_err();
        assert!(e.ends_with("the file has more than 1048576 bytes"), "{e}");

        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "").unwrap();
        let e = read_ca_file(&empty.display().to_string()).unwrap_err();
        assert!(e.contains("is not valid: no PEM block CERTIFICATE"), "{e}");

        let link = dir.path().join("link.pem");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        let e = read_ca_file(&link.display().to_string()).unwrap_err();
        assert!(e.ends_with("the path is a symbolic link"), "{e}");

        let e = read_ca_file(&dir.path().display().to_string()).unwrap_err();
        assert!(e.ends_with("the path is not a regular file"), "{e}");
    }
}
