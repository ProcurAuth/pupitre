// SPDX-License-Identifier: MIT OR Apache-2.0
//! The profile distributed (6.13, 6.17) with the mode `authzen`: the reference file passes the
//! check of the configuration with test keys and CA files, and a bad key text stops it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod access_support;

use std::path::Path;

use access_support::*;
use pupitre::config::{
    AccessConfig, CapturedEnvironment, ConfigError, Profile, ReadMode, ReferenceFormat, load, render_reference,
};

const SCHEMA: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
  {"default": "info", "description": "The level.", "kind": "enum", "path": "log.level", "values": ["info", "debug"]}
]}"#;
const STORE_KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(jwks: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let w = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
        w("relay.json", SCHEMA);
        w("store-key", &format!("{STORE_KEY}\n"));
        w("jwks.json", jwks);
        w("pdp-token", &format!("{CREDENTIAL}\n"));
        w("pdp-ca.pem", CA_A);
        w("pg-ca.crt", CA_B);
        w("pg-user", "pupitre\n");
        w("pg-password", "a-long-test-password\n");
        w("ssh-key", "not used by the check\n");
        w("known_hosts", "host-a ssh-ed25519 AAAA\n");
        Fixture { dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).to_string_lossy().into_owned()
    }

    fn text(&self) -> String {
        render_reference(ReferenceFormat::Hcl, Profile::Distributed)
            .replace("/srv/ansible/schemas/relay.json", &self.path("relay.json"))
            .replace("/run/secrets/pupitre/store-key", &self.path("store-key"))
            .replace("/run/secrets/edge/jwks.json", &self.path("jwks.json"))
            .replace("/run/secrets/pdp/token", &self.path("pdp-token"))
            .replace("/run/secrets/pdp/ca.pem", &self.path("pdp-ca.pem"))
            .replace("/run/secrets/pg-ca/ca.crt", &self.path("pg-ca.crt"))
            .replace("/run/secrets/pg/username", &self.path("pg-user"))
            .replace("/run/secrets/pg/password", &self.path("pg-password"))
            .replace("/run/secrets/ssh/id_ed25519", &self.path("ssh-key"))
            .replace("/run/secrets/ssh/known_hosts", &self.path("known_hosts"))
    }

    fn load(&self, text: &str) -> Result<pupitre::config::LoadedConfig, Vec<ConfigError>> {
        let file = self.dir.path().join("pupitre.hcl");
        std::fs::write(&file, text).unwrap();
        load(Path::new(&file), &CapturedEnvironment::from_pairs(Vec::<(String, String)>::new()), ReadMode::All)
    }
}

fn messages(errors: &[ConfigError]) -> String {
    errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_profile_distributed_passes_with_a_jwk_set_and_test_ca_files() {
    let f = Fixture::new(&jwks_two());
    let loaded = f.load(&f.text()).unwrap_or_else(|e| panic!("{}", messages(&e)));
    let AccessConfig::Authzen(a) = &loaded.config.access else { panic!("the mode is authzen") };
    assert_eq!(a.audience, "pupitre");
    assert_eq!(a.pdp_ca_pem.as_deref(), Some(CA_A.as_bytes()));
    assert_eq!(a.public_keys, jwks_two());
    // The access layer builds from the typed configuration, and the layer sends nothing.
    let access = pupitre::access::AuthzenAccess::new(a, true, 100).unwrap();
    assert!(pupitre::access::Access::action_document(&access).is_some());
}

#[test]
fn the_profile_distributed_passes_with_pem_keys() {
    let pem =
        format!("{}{P256_PUBLIC_PEM}", procurauth_core::jws::ed25519_public_pem(&ed_key().public_key(None)).unwrap());
    let f = Fixture::new(&pem);
    f.load(&f.text()).unwrap_or_else(|e| panic!("{}", messages(&e)));
}

#[test]
fn a_bad_key_text_stops_the_check_with_the_key_the_rule_and_no_key_text() {
    for bad in ["not keys", r#"{"keys":[]}"#, "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"] {
        let f = Fixture::new(bad);
        let errors = f.load(&f.text()).unwrap_err();
        let text = messages(&errors);
        assert!(text.contains("access.identity.public_keys"), "{bad:?}: {text}");
        assert!(text.contains("expected"), "{text}");
    }
}

#[test]
fn a_budget_that_cannot_end_one_item_is_a_contradiction_3_16() {
    let f = Fixture::new(&jwks_two());
    let text = f.text();
    assert!(text.contains("access {\n  mode = \"authzen\"\n"), "the profile has an access block");
    let with = |n: u32| {
        text.replacen(
            "access {\n  mode = \"authzen\"\n",
            &format!("access {{\n  mode = \"authzen\"\n  options_max_questions = {n}\n"),
            1,
        )
    };
    // The runner is on in the profile distributed: the least budget is 8.
    let errors = f.load(&with(7)).unwrap_err();
    assert!(messages(&errors).contains("access.options_max_questions"), "{}", messages(&errors));
    assert!(messages(&errors).contains("expected at least 8"), "{}", messages(&errors));
    f.load(&with(8)).unwrap_or_else(|e| panic!("{}", messages(&e)));
}
