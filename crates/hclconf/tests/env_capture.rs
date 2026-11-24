// SPDX-License-Identifier: MIT OR Apache-2.0
//! The capture test runs alone in its own process, because it clears the environment.

use hclconf::env::CapturedEnvironment;
use std::ffi::OsStr;

#[test]
fn capture_copies_clears_and_keeps_the_two_ssl_names() {
    std::env::set_var("HCLCONF_TEST_CAPTURE_VAR", "value");
    std::env::set_var("SSL_CERT_FILE", "/nonexistent");
    let env = CapturedEnvironment::capture_and_clear();
    assert_eq!(env.get("HCLCONF_TEST_CAPTURE_VAR"), Some(OsStr::new("value")));
    assert!(std::env::var_os("HCLCONF_TEST_CAPTURE_VAR").is_none());
    assert!(std::env::var_os("PATH").is_none());
    // 6.23: the process keeps SSL_CERT_FILE and SSL_CERT_DIR.
    assert_eq!(std::env::var_os("SSL_CERT_FILE"), Some("/nonexistent".into()));
}
