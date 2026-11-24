// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the configuration: the reference file of the profile local, the defaults, the
//! effective configuration and the contradictions of 6.14.

use std::path::Path;

use super::*;

const SCHEMA_FILE: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
  {"default": "info", "description": "The level.", "kind": "enum", "path": "log.level", "values": ["info", "debug"]},
  {"description": "The password.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true}
]}"#;

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().expect("temporary folder");
        std::fs::write(dir.path().join("relay.json"), SCHEMA_FILE).expect("schema file");
        std::fs::write(dir.path().join("store-key"), format!("{KEY}\n")).expect("key file");
        Fixture { dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).to_string_lossy().into_owned()
    }

    /// The reference file of the profile local, with the paths of the fixture.
    fn local_text(&self) -> String {
        render_reference(ReferenceFormat::Hcl, Profile::Local)
            .replace("/srv/ansible/schemas/relay.json", &self.path("relay.json"))
            .replace("/run/secrets/pupitre/store-key", &self.path("store-key"))
    }

    fn load_text(&self, text: &str) -> Result<LoadedConfig, Vec<ConfigError>> {
        let file = self.dir.path().join("pupitre.hcl");
        std::fs::write(&file, text).expect("configuration file");
        load(&file, &CapturedEnvironment::from_pairs(Vec::<(String, String)>::new()), ReadMode::All)
    }
}

/// Replace the value of the first attribute `name` of the text. The reference aligns the `=`.
fn with_attr(text: &str, name: &str, value: &str) -> String {
    let mut done = false;
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let is_it = trimmed.strip_prefix(name).is_some_and(|rest| rest.trim_start().starts_with('='));
            if is_it && !done {
                done = true;
                let indent = &line[..line.len() - trimmed.len()];
                format!("{indent}{name} = {value}\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

/// Replace the top block `name { ... }` of the text with `new`.
fn replace_block(text: &str, name: &str, new: &str) -> String {
    let start = text.find(&format!("\n{name} {{")).map(|i| i + 1).unwrap_or_else(|| panic!("no block {name}"));
    let end = start + text[start..].find("\n}\n").unwrap_or_else(|| panic!("no end of {name}")) + 3;
    format!("{}{}{}", &text[..start], new, &text[end..])
}

fn messages(errors: &[ConfigError]) -> String {
    errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_reference_of_the_profile_local_passes() {
    let f = Fixture::new();
    let loaded = f.load_text(&f.local_text()).unwrap_or_else(|e| panic!("{}", messages(&e)));
    assert!(matches!(loaded.config.access, AccessConfig::Open));
    assert!(matches!(loaded.config.storage, StorageConfig::Files(_)));
    assert_eq!(loaded.config.capabilities.schemas.len(), 1);
    assert_eq!(loaded.config.capabilities.playbooks.len(), 1);
    assert_eq!(loaded.warnings.iter().map(|w| w.event).collect::<Vec<_>>(), vec!["access.mode_open"]);
    assert_eq!(loaded.config.server.max_page_items, 200);
    assert_eq!(loaded.config.limits.max_document_bytes, 262_144);
}

#[test]
fn print_config_shows_the_defaults_and_hides_the_secret() {
    let f = Fixture::new();
    let loaded = f.load_text(&f.local_text()).unwrap_or_else(|e| panic!("{}", messages(&e)));
    let text = render_print_config(&loaded);
    assert!(text.contains("server.max_connections = 1024  # default\n"), "{text}");
    assert!(text.contains(&format!("storage.secret_key = <hidden>  # file:{}\n", f.path("store-key"))), "{text}");
    assert!(!text.contains(KEY));
    assert!(text.contains("storage.files.history_keep = 100  # default\n"));
    assert!(!text.contains("storage.postgres"));
    assert!(!text.contains("access.pdp"));
    // A default goes after the key before it in the table.
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    assert!(at("server.base_path") < at("server.max_connections"));
    assert!(at("server.max_page_items") < at("log.level"));
}

#[test]
fn the_allowed_hosts_default_to_the_loopback_names_of_a_loopback_listener_7_4() {
    let f = Fixture::new();
    let loaded = f.load_text(&f.local_text()).unwrap_or_else(|e| panic!("{}", messages(&e)));
    let names: Vec<String> = loaded.config.server.allowed_hosts.iter().map(ToString::to_string).collect();
    assert_eq!(names, ["localhost", "127.0.0.1", "[::1]"]);
    // The effective configuration shows the default, after `server.base_path`.
    let text = render_print_config(&loaded);
    assert!(text.contains("server.allowed_hosts = [\"localhost\", \"127.0.0.1\", \"[::1]\"]  # default\n"), "{text}");
    assert!(text.find("server.base_path").unwrap() < text.find("server.allowed_hosts").unwrap());
    assert!(text.find("server.allowed_hosts").unwrap() < text.find("server.max_connections").unwrap());
}

#[test]
fn a_listener_that_is_not_loopback_needs_the_allowed_hosts_7_4() {
    let f = Fixture::new();
    for listen in ["\"0.0.0.0:8080\"", "\"[::]:8080\"", "\"192.0.2.5:8080\""] {
        let text = with_attr(&with_attr(&f.local_text(), "listen", listen), "ops_listen", "\"127.0.0.1:9090\"");
        let errors = f.load_text(&text).unwrap_err();
        let all = messages(&errors);
        assert!(
            all.contains(
                "server.allowed_hosts: missing attribute; necessary when `server.listen` is not a loopback address"
            ),
            "{listen}: {all}"
        );
        // With the key, the start works and the list is the list of the file.
        let with_key = with_attr(
            &text,
            "base_path",
            "\"/\"\n  allowed_hosts = [\"pupitre.example.invalid\", \"edge.example.invalid:8443\"]",
        );
        let loaded = f.load_text(&with_key).unwrap_or_else(|e| panic!("{listen}: {}", messages(&e)));
        let names: Vec<String> = loaded.config.server.allowed_hosts.iter().map(ToString::to_string).collect();
        assert_eq!(names, ["pupitre.example.invalid", "edge.example.invalid:8443"]);
        let shown = render_print_config(&loaded);
        assert!(
            shown.contains(
                "server.allowed_hosts = [\"pupitre.example.invalid\", \"edge.example.invalid:8443\"]  # file\n"
            ),
            "{shown}"
        );
    }
}

#[test]
fn a_bad_entry_of_the_allowed_hosts_stops_the_start_with_the_key_and_the_value_7_4() {
    let f = Fixture::new();
    for (entry, needle) in [
        (
            "\"*\"",
            "expected a host name, an IPv4 address or an IPv6 address in brackets, with an optional port from 1 to 65535, found \"*\"",
        ),
        ("\"*.example.invalid\"", "found \"*.example.invalid\""),
        ("\"example.invalid:0\"", "found \"example.invalid:0\""),
        ("\"::1\"", "found \"::1\""),
        ("\"http://example.invalid\"", "found \"http://example.invalid\""),
    ] {
        let text = with_attr(&f.local_text(), "base_path", &format!("\"/\"\n  allowed_hosts = [{entry}]"));
        let errors = f.load_text(&text).unwrap_err();
        let all = messages(&errors);
        assert!(all.contains("server.allowed_hosts") && all.contains(needle), "{entry}: {all}");
    }
    let dup = with_attr(
        &f.local_text(),
        "base_path",
        "\"/\"\n  allowed_hosts = [\"a.example.invalid\", \"A.example.invalid\"]",
    );
    assert!(messages(&f.load_text(&dup).unwrap_err()).contains("occurs two times"));
    let empty = with_attr(&f.local_text(), "base_path", "\"/\"\n  allowed_hosts = []");
    assert!(f.load_text(&empty).is_err());
    let twice = with_attr(
        &f.local_text(),
        "base_path",
        "\"/\"\n  allowed_hosts = [\"a.example.invalid\"]\n  allowed_hosts = [\"b.example.invalid\"]",
    );
    assert!(messages(&f.load_text(&twice).unwrap_err()).contains("allowed_hosts"));
}

#[test]
fn a_secret_in_clear_text_stops_the_start() {
    let f = Fixture::new();
    let text = f
        .local_text()
        .replace(&format!("secret_key_file = \"{}\"", f.path("store-key")), &format!("secret_key = \"{KEY}\""));
    let errors = f.load_text(&text).unwrap_err();
    assert!(messages(&errors).contains("a secret accepts only `secret_key_env` or `secret_key_file`"));
    assert!(!messages(&errors).contains(KEY));
}

#[test]
fn a_wrong_key_of_the_store_is_hidden() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("store-key"), "Z".repeat(64)).expect("key file");
    let errors = f.load_text(&f.local_text()).unwrap_err();
    assert!(
        messages(&errors).contains("expected 64 lower-case hexadecimal characters, found (value hidden)"),
        "{}",
        messages(&errors)
    );
}

#[test]
fn the_contradictions_of_6_14() {
    let f = Fixture::new();
    let base = f.local_text();
    let cases: Vec<(String, &str)> = vec![
        (
            replace_block(
                &base,
                "runner",
                "runner {\n  enabled = true\n  execute = false\n  allow_apply_in_open_mode = false\n  check_max_age = \"30m\"\n  max_hosts = 100\n}\n",
            ),
            "the backend files has one instance",
        ),
        (with_attr(&base, "ops_listen", "\"127.0.0.2:8080\""), "the port is the port of `server.listen`"),
        (with_attr(&base, "runs_path", "\"/var/lib/pupitre/data/runs\""), "is inside"),
        (
            format!(
                "{base}\ncommand \"restart\" {{\n  label = \"Restart\"\n  playbook = \"deploy-relay\"\n  target = \"unit\"\n  check = false\n}}\n"
            ),
            "expected the id of a block playbook with the kind command",
        ),
    ];
    for (text, needle) in cases {
        let errors = f.load_text(&text).unwrap_err();
        assert!(messages(&errors).contains(needle), "{needle}: {}", messages(&errors));
    }
}

#[test]
fn a_runner_that_is_off_has_no_playbook() {
    let f = Fixture::new();
    let off = replace_block(&f.local_text(), "runner", "runner {\n  enabled = false\n}\n");
    let errors = f.load_text(&off).unwrap_err();
    assert!(
        messages(&errors).contains("playbook \"deploy-relay\": not permitted when `runner.enabled` is false"),
        "{}",
        messages(&errors)
    );
}

#[test]
fn the_keys_of_a_parameter_follow_its_type() {
    let f = Fixture::new();
    let text = format!(
        "{}\nplaybook \"restart\" {{\n  kind = \"command\"\n  path = \"/srv/ansible/playbooks/restart.yml\"\n}}\n\
         command \"restart\" {{\n  label = \"Restart\"\n  playbook = \"restart\"\n  target = \"unit\"\n  check = false\n\
           param \"grace\" {{\n    label = \"Grace\"\n    type = \"duration\"\n    required = true\n    min = 1\n  }}\n}}\n",
        f.local_text()
    );
    let errors = f.load_text(&text).unwrap_err();
    let all = messages(&errors);
    assert!(all.contains("command \"restart\".param \"grace\".min: not permitted for the type duration"), "{all}");
    assert!(all.contains("missing attribute; necessary for the type duration"), "{all}");

    let good = text.replace("    min = 1\n", "    min_duration = \"1s\"\n    max_duration = \"5m\"\n");
    let loaded = f.load_text(&good).unwrap_or_else(|e| panic!("{}", messages(&e)));
    let command = loaded.config.capabilities.commands.values().next().expect("command");
    assert_eq!(command.params[0].ty, pupitre_core::command::ParamType::Duration { min_ms: 1000, max_ms: 300_000 });
}

#[test]
fn the_markdown_reference_is_the_committed_document() {
    let generated = render_reference(ReferenceFormat::Markdown, Profile::Local);
    let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/configuration-reference.md");
    let on_disk = std::fs::read_to_string(&committed).unwrap_or_default();
    assert!(
        on_disk == generated,
        "docs/configuration-reference.md differs: run `pupitre print-reference --format markdown`"
    );
}
