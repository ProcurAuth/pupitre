// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the engine with a small key table. They cover the nine rules of 6.2 and the
//! error forms of 6.14 that belong to the engine.

use hclconf::{
    check_file, check_file_with, check_text, check_text_with, Applies, BlockDesc, CapturedEnvironment, Forms, KeyDesc,
    Kind, ReadMode, Ref, Schema, Source, Term, TextRule, Value,
};

const MODE_BLOCK: &[Term] = &[Term { key: "app.mode", equals: "full" }];
const FLAG_ON: &[Term] = &[Term { key: "app.flag", equals: "true" }];

static BLOCKS: [BlockDesc; 5] = [
    BlockDesc::new("app", "app", "The application."),
    BlockDesc::new("app.extra", "extra", "The extra block.").when(MODE_BLOCK),
    BlockDesc::new("app.flagged", "flagged", "The block of the flag.").when(FLAG_ON),
    BlockDesc::new("source", "source", "A source.").labelled(1, 2, "main"),
    BlockDesc::new("source.tls", "tls", "TLS of a source.").manual("a secure URL", "a plain URL"),
];

static KEYS: [KeyDesc; 13] = [
    KeyDesc::new("app.mode", Kind::Enum(&["full", "lite"]), "The mode.").both(Ref::Str("full")),
    KeyDesc::new("app.flag", Kind::Bool, "The flag.").both(Ref::Bool(true)),
    KeyDesc::new("app.port", Kind::Integer { min: 1, max: 100 }, "The port.").both(Ref::Int(10)),
    KeyDesc::new("app.wait", Kind::Duration { min_ms: 1000, max_ms: 60_000 }, "The wait.").both(Ref::Str("5s")),
    KeyDesc::new("app.name", Kind::Text { min: 1, max: 20, rule: TextRule::NoControl }, "The name.")
        .forms(Forms::Vef)
        .refs(Ref::Str("x"), Ref::File("/run/name")),
    KeyDesc::new("app.password", Kind::Text { min: 1, max: 20, rule: TextRule::Any }, "The secret.")
        .secret()
        .both(Ref::File("/run/pw")),
    KeyDesc::new("app.extra.level", Kind::Integer { min: 0, max: 9 }, "The level.").both(Ref::Int(1)),
    KeyDesc::new("app.flagged.note", Kind::Text { min: 1, max: 9, rule: TextRule::Any }, "A note."),
    KeyDesc::new("source.url", Kind::Text { min: 1, max: 99, rule: TextRule::Any }, "The URL."),
    KeyDesc::new("source.tls.ca", Kind::Path, "The CA file."),
    KeyDesc::new("source.limit", Kind::Size { min: 0, max: 1000 }, "The limit."),
    KeyDesc::new("source.token", Kind::Text { min: 1, max: 9, rule: TextRule::Any }, "A token.")
        .secret()
        .forms(Forms::Ef),
    KeyDesc::new("source.addr", Kind::SocketAddr, "The address."),
];

fn schema() -> Schema {
    Schema { blocks: &BLOCKS, keys: &KEYS }
}

const GOOD: &str = r#"
app {
  mode = "full"
  flag = false
  port = 10
  wait = "5s"
  name = "n"
  password_env = "PW"

  extra {
    level = 1
  }
}

source "a" {
  url = "u"
  limit = 5
  token_env = "TOK"
  addr = "127.0.0.1:80"
}
"#;

fn env() -> CapturedEnvironment {
    CapturedEnvironment::from_pairs([("PW".to_string(), "secret".to_string()), ("TOK".to_string(), "t".to_string())])
}

fn errors(text: &str) -> Vec<String> {
    match check_text(text, &schema(), &env()) {
        Ok(_) => Vec::new(),
        Err(list) => list.iter().map(ToString::to_string).collect(),
    }
}

fn one_error(text: &str) -> String {
    let e = errors(text);
    assert_eq!(e.len(), 1, "{e:#?}");
    e[0].clone()
}

#[test]
fn the_table_is_consistent() {
    schema().validate().unwrap();
}

#[test]
fn good_file_passes_and_keys_are_typed() {
    let checked = check_text(GOOD, &schema(), &env()).unwrap();
    assert_eq!(checked.get("app.port").unwrap().value.print(), "10");
    assert_eq!(checked.get("app.password").unwrap().source, Source::Env("PW".into()));
    assert_eq!(checked.get_in("a", "source.url").unwrap().display, "source \"a\".url");
    assert!(checked.has_block("app.extra"));
    assert_eq!(checked.labels("source"), vec!["a"]);
    assert!(format!("{:?}", checked.get("app.password").unwrap()).contains("(value hidden)"));
    assert!(!format!("{:?}", checked.keys()).contains("secret"));
}

// Rule 1: closed schema.
#[test]
fn rule1_unknown_attribute_block_and_duplicates() {
    assert_eq!(one_error(&GOOD.replace("port = 10", "port = 10\n  prot = 1")), "app.prot: unknown attribute (line 6)");
    assert_eq!(one_error(&format!("{GOOD}\nother {{\n}}\n")), "other: unknown block (line 22)");
    assert_eq!(
        one_error(&GOOD.replace("port = 10", "port = 10\n  port = 11")),
        "app.port: duplicate attribute; the first one is at line 5 (line 6)"
    );
    let dup_block = GOOD
        .replace("  extra {\n    level = 1\n  }\n", "  extra {\n    level = 1\n  }\n  extra {\n    level = 2\n  }\n");
    assert_eq!(one_error(&dup_block), "app.extra: duplicate block; the first one is at line 10 (line 13)");
    let dup_label = format!(
        "{GOOD}\nsource \"a\" {{\n  url = \"u\"\n  limit = 5\n  token_env = \"TOK\"\n  addr = \"127.0.0.1:80\"\n}}\n"
    );
    assert_eq!(
        one_error(&dup_label),
        "source \"a\": duplicate label; the first block with this label is at line 15 (line 22)"
    );
}

#[test]
fn rule1_attribute_at_the_top_is_unknown() {
    assert!(errors(&format!("x = 1\n{GOOD}")).contains(&"x: unknown attribute (line 1)".to_string()));
}

// Rule 2 and 3: messages name key, type and value; no coercion.
#[test]
fn rule2_and_3_type_errors() {
    assert_eq!(
        one_error(&GOOD.replace("port = 10", "port = \"10\"")),
        "app.port: expected an integer from 1 to 100, found \"10\" (line 5)"
    );
    assert_eq!(
        one_error(&GOOD.replace("port = 10", "port = 1.0")),
        "app.port: expected an integer from 1 to 100, found 1.0 (line 5)"
    );
    assert_eq!(
        one_error(&GOOD.replace("port = 10", "port = 101")),
        "app.port: expected an integer from 1 to 100, found 101 (line 5)"
    );
    assert_eq!(
        one_error(&GOOD.replace("flag = false", "flag = \"true\"")),
        "app.flag: expected a boolean, found \"true\" (line 4)"
    );
    assert_eq!(
        one_error(&GOOD.replace("wait = \"5s\"", "wait = \"1m30s\"")),
        "app.wait: expected a duration from 1s to 1m, found \"1m30s\" (line 6)"
    );
    assert_eq!(
        one_error(&GOOD.replace("mode = \"full\"", "mode = \"Full\"")),
        "app.mode: expected \"full\" or \"lite\", found \"Full\" (line 3)"
    );
}

#[test]
fn not_a_literal_names_the_key() {
    assert_eq!(
        one_error(&GOOD.replace("port = 10", "port = 1 + 2")),
        "app.port: expected an integer from 1 to 100, found expression: 1 + 2; write a literal value (line 5)"
    );
    assert!(
        one_error(&GOOD.replace("port = 10", "port = var.x")).contains("found variable: var.x; write a literal value")
    );
    assert!(one_error(&GOOD.replace("name = \"n\"", "name = \"${x}\""))
        .contains("found template: \"${x}\"; write a literal value"));
    assert!(
        one_error(&GOOD.replace("name = \"n\"", "name = upper(\"n\")")).contains("found function call: upper(\"n\")")
    );
    assert!(one_error(&GOOD.replace("name = \"n\"", "name = null")).contains("found null: null"));
    assert!(one_error(&GOOD.replace("name = \"n\"", "name = {a = 1}")).contains("found object"));
    assert!(one_error(&GOOD.replace("name = \"n\"", "name = <<EOT\nx\nEOT")).contains("found heredoc"));
}

#[test]
fn syntax_error_stops_at_once() {
    let e = errors("app {\n  port = \n}\nbad bad\n");
    assert_eq!(e.len(), 1);
    assert!(e[0].starts_with("the configuration file is not valid HCL at line 2, column"));
}

// Rule 6: forms.
#[test]
fn rule6_two_forms_none_and_refused_indirection() {
    assert_eq!(
        one_error(&GOOD.replace("name = \"n\"", "name = \"n\"\n  name_env = \"PW\"")),
        "app.name: the file gives `name` at line 7 and `name_env`; give exactly one form (line 8)"
    );
    assert_eq!(
        one_error(&GOOD.replace("  password_env = \"PW\"\n", "")),
        "app.password: missing secret; give `password_env` or `password_file` (line 2)"
    );
    assert_eq!(
        one_error(&GOOD.replace("  name = \"n\"\n", "")),
        "app.name: missing attribute; give `name`, `name_env` or `name_file` (a text of 1 to 20 bytes with no control character) (line 2)"
    );
    assert_eq!(
        one_error(&GOOD.replace("port = 10", "port_env = \"PW\"")),
        "app.port: `port` accepts only a value in the file, not `_env` or `_file` (line 5)"
    );
    assert_eq!(
        one_error(&GOOD.replace("port = 10", "port_file = \"/x\"")),
        "app.port: `port` accepts only a value in the file, not `_env` or `_file` (line 5)"
    );
    assert_eq!(
        one_error(&GOOD.replace("  limit = 5\n", "")),
        "source \"a\".limit: missing attribute; expected a size in bytes from 0 to 1000 (line 15)"
    );
}

#[test]
fn rule6_secret_in_clear_text_never_shows_the_value() {
    let e = one_error(&GOOD.replace("password_env = \"PW\"", "password = \"hunter2\""));
    assert_eq!(
        e,
        "app.password: a secret accepts only `password_env` or `password_file`, but the file gives it in clear text (value hidden) (line 8)"
    );
    assert!(!e.contains("hunter2"));
    // Also together with another form: one error, and the other form is not read.
    let e = one_error(&GOOD.replace("password_env = \"PW\"", "password = \"hunter2\"\n  password_env = \"NOPE\""));
    assert!(e.contains("in clear text"), "{e}");
}

#[test]
fn rule6_env_cases() {
    let t = |pw: Option<&str>, var: &str| -> Vec<String> {
        let text = GOOD.replace("password_env = \"PW\"", &format!("password_env = \"{var}\""));
        let env = CapturedEnvironment::from_pairs(
            pw.map(|v| ("V".to_string(), v.to_string())).into_iter().chain([("TOK".to_string(), "t".to_string())]),
        );
        match check_text(&text, &schema(), &env) {
            Ok(_) => vec![],
            Err(l) => l.iter().map(ToString::to_string).collect(),
        }
    };
    assert!(t(Some("good"), "V").is_empty());
    assert_eq!(t(None, "V"), vec!["app.password: the environment variable V is not set (line 8)"]);
    assert_eq!(t(Some(""), "V"), vec!["app.password: the value of the environment variable V is empty (line 8)"]);
    for bad in [" x", "x ", "x\n", "\tx"] {
        let e = t(Some(bad), "V");
        assert!(e[0].contains("starts or ends with a space or a line end"), "{bad:?}: {e:?}");
        assert!(!e[0].contains('x'), "no value in the message");
    }
    assert!(t(Some("good"), "1BAD")[0].contains("expected an environment variable name that matches"));
    assert!(t(Some("good"), "A-B")[0].contains("expected an environment variable name that matches"));
    assert!(t(Some("good"), "SSL_CERT_FILE")[0].contains("other than SSL_CERT_FILE and SSL_CERT_DIR"));
    assert!(t(Some("good"), "SSL_CERT_DIR")[0].contains("other than SSL_CERT_FILE and SSL_CERT_DIR"));
    // A value that is too long for a secret never shows.
    let e = t(Some(&"z".repeat(30)), "V");
    assert_eq!(e, vec!["app.password: expected a text of 1 to 20 bytes, found (value hidden) (line 8)"]);
}

#[test]
fn rule6_env_not_utf8() {
    use std::os::unix::ffi::OsStringExt;
    let env = CapturedEnvironment::from_os_pairs([
        ("PW".to_string(), std::ffi::OsString::from_vec(vec![0xff, 0xfe])),
        ("TOK".to_string(), "t".into()),
    ]);
    let e = check_text(GOOD, &schema(), &env).unwrap_err();
    assert_eq!(e[0].to_string(), "app.password: the value of the environment variable PW is not UTF-8 (line 8)");
}

fn file_case(content: &[u8]) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("pw");
    std::fs::write(&p, content).unwrap();
    let text = GOOD.replace("password_env = \"PW\"", &format!("password_file = \"{}\"", p.display()));
    match check_text(&text, &schema(), &env()) {
        Ok(c) => vec![format!("ok:{}", c.get("app.password").unwrap().value.print())],
        Err(l) => l.iter().map(|e| e.to_string().replace(&p.display().to_string(), "<P>")).collect(),
    }
}

#[test]
fn rule6_file_cases() {
    assert_eq!(file_case(b"abc"), vec!["ok:\"abc\""]);
    // One last line end is removed, and no more.
    assert_eq!(file_case(b"abc\n"), vec!["ok:\"abc\""]);
    assert_eq!(file_case(b"abc\r\n"), vec!["ok:\"abc\""]);
    assert_eq!(file_case(b"abc\n\n"), vec!["ok:\"abc\\n\""]);
    assert_eq!(file_case(b"abc\r\r\n"), vec!["ok:\"abc\\r\""]);
    assert_eq!(file_case(b" abc "), vec!["ok:\" abc \""]);
    assert_eq!(file_case(b""), vec!["app.password: the file <P> is empty (line 8)"]);
    assert_eq!(file_case(b"\n"), vec!["app.password: the file <P> is empty (line 8)"]);
    assert_eq!(
        file_case(b"\xef\xbb\xbfabc"),
        vec!["app.password: cannot read the file <P>: the content starts with a byte order mark (line 8)"]
    );
    assert_eq!(
        file_case(b"\xff\xfe"),
        vec!["app.password: cannot read the file <P>: the content is not UTF-8 (line 8)"]
    );
    let big = vec![b'a'; 65_537];
    assert_eq!(
        file_case(&big),
        vec!["app.password: cannot read the file <P>: the content has more than 65536 bytes (line 8)"]
    );
    // 65536 bytes are read, then the range of the key refuses the length without showing it.
    let max = vec![b'a'; 65_536];
    assert_eq!(file_case(&max), vec!["app.password: expected a text of 1 to 20 bytes, found (value hidden) (line 8)"]);
}

#[test]
fn rule6_file_not_readable_relative_and_symlink() {
    let text = |p: &str| GOOD.replace("password_env = \"PW\"", &format!("password_file = \"{p}\""));
    let e = check_text(&text("/nonexistent/file"), &schema(), &env()).unwrap_err();
    assert_eq!(
        e[0].to_string(),
        "app.password: cannot read the file /nonexistent/file: No such file or directory (line 8)"
    );
    let e = check_text(&text("relative/file"), &schema(), &env()).unwrap_err();
    assert_eq!(
        e[0].to_string(),
        "app.password: expected an absolute path of 1 to 4096 bytes, found \"relative/file\" (line 8)"
    );
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::write(&real, "abc").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let e = check_text(&text(&link.display().to_string()), &schema(), &env()).unwrap_err();
    assert!(e[0].message.contains("the path is a symbolic link"), "{e:?}");
    let e = check_text(&text(&dir.path().display().to_string()), &schema(), &env()).unwrap_err();
    assert!(e[0].message.contains("not a regular file"), "{e:?}");
}

#[test]
fn rule6_two_forms_read_neither_the_variable_nor_the_file() {
    // The file of `_file` does not exist, and the variable does not exist: only the two-forms error shows.
    let text = GOOD.replace("password_env = \"PW\"", "password_env = \"NOPE\"\n  password_file = \"/nonexistent\"");
    let e = errors(&text);
    assert_eq!(e.len(), 1);
    assert!(e[0].contains("give exactly one form"));
}

// Rule 9 and the modes.
#[test]
fn modes_forbidden_and_missing() {
    // The block `extra` exists only with mode "full".
    let lite = GOOD.replace("mode = \"full\"", "mode = \"lite\"");
    assert_eq!(
        one_error(&lite),
        "app.extra: not permitted when `app.mode` is \"lite\"; the key is only for `app.mode` = \"full\" (line 10)"
    );
    let missing = GOOD.replace("  extra {\n    level = 1\n  }\n", "");
    assert_eq!(one_error(&missing), "app.extra: missing block; expected when `app.mode` is \"full\" (line 2)");
    // A boolean mode key drives a block (behaviour 3 of 6.1).
    let flagged = GOOD.replace("flag = false", "flag = true");
    assert_eq!(one_error(&flagged), "app.flagged: missing block; expected when `app.flag` is true (line 2)");
    let flagged = flagged.replace("  extra {", "  flagged {\n    note = \"n\"\n  }\n  extra {");
    assert!(errors(&flagged).is_empty());
    let unflagged = GOOD.replace("  extra {", "  flagged {\n    note = \"n\"\n  }\n  extra {");
    assert_eq!(
        one_error(&unflagged),
        "app.flagged: not permitted when `app.flag` is false; the key is only for `app.flag` = true (line 10)"
    );
}

#[test]
fn keys_inside_a_forbidden_block_are_not_checked() {
    let lite = GOOD.replace("mode = \"full\"", "mode = \"lite\"").replace("level = 1", "level = 99\n    nonsense = 1");
    assert_eq!(errors(&lite).len(), 1);
}

#[test]
fn a_broken_mode_key_hides_the_dependent_checks() {
    // `mode` has a bad value: only that error shows, not a second one for the block `extra`.
    let e = errors(&GOOD.replace("mode = \"full\"", "mode = 1"));
    assert_eq!(e.len(), 1);
    assert!(e[0].starts_with("app.mode: expected"));
}

#[test]
fn counts_of_labelled_blocks() {
    let none = GOOD.split("source \"a\"").next().unwrap().to_string();
    assert_eq!(one_error(&none), "source: expected 1 to 2 blocks `source \"<label>\"`, found 0");
    let block = |l: &str| {
        format!("source \"{l}\" {{\n  url = \"u\"\n  limit = 5\n  token_env = \"TOK\"\n  addr = \"127.0.0.1:80\"\n}}\n")
    };
    let three = format!("{none}{}{}{}", block("a"), block("b"), block("c"));
    assert_eq!(one_error(&three), "source: expected 1 to 2 blocks `source \"<label>\"`, found 3");
    let two = format!("{none}{}{}", block("b"), block("a"));
    let checked = check_text(&two, &schema(), &env()).unwrap();
    assert_eq!(checked.labels("source"), vec!["b", "a"], "file order");
    // Keys come in table order, with the labelled blocks in file order.
    let displays: Vec<&str> = checked.keys().iter().map(|k| k.display.as_str()).collect();
    let b_url = displays.iter().position(|d| *d == "source \"b\".url").unwrap();
    let a_url = displays.iter().position(|d| *d == "source \"a\".url").unwrap();
    assert!(b_url < a_url);
}

#[test]
fn labels_must_be_one() {
    let e = errors("source {\n}\n");
    assert!(e.iter().any(|m| m.contains("expected one label")), "{e:?}");
    let e = errors("app \"x\" {\n}\n");
    assert!(e.iter().any(|m| m.contains("expected a block with no label")), "{e:?}");
}

#[test]
fn missing_top_block() {
    let e = errors("");
    assert!(e.contains(&"app: missing block".to_string()), "{e:?}");
}

#[test]
fn manual_rules_are_optional_for_the_engine() {
    let with_tls =
        GOOD.replace("  addr = \"127.0.0.1:80\"\n", "  addr = \"127.0.0.1:80\"\n\n  tls {\n    ca = \"/x\"\n  }\n");
    let checked = check_text(&with_tls, &schema(), &env()).unwrap();
    assert!(checked.block_in("a", "source.tls").is_some());
    assert_eq!(checked.get_in("a", "source.tls.ca").unwrap().display, "source \"a\".tls.ca");
}

#[test]
fn errors_come_in_file_order() {
    let text =
        GOOD.replace("limit = 5", "limit = -1").replace("port = 10", "port = 0").replace("flag = false", "flag = 3");
    let lines: Vec<u32> = check_text(&text, &schema(), &env()).unwrap_err().iter().map(|e| e.line.unwrap()).collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
    assert_eq!(lines.len(), 3);
}

#[test]
fn limit_of_ten_errors_for_one_key_does_not_apply_to_distinct_keys() {
    // 12 blocks `source` with distinct labels but the maximum is 2: one count error only.
    let text: String = (0..12).map(|i| format!("source \"s{i}\" {{\n}}\n")).collect();
    let e = errors(&format!("app {{}}\n{text}"));
    assert!(e.iter().any(|m| m.contains("found 12")));
}

#[test]
fn file_limits() {
    let big = format!("# {}\n", "x".repeat(1_048_577));
    let e = check_text(&big, &schema(), &env()).unwrap_err();
    assert_eq!(
        e[0].to_string(),
        format!("the configuration file has {} bytes; the maximum is 1048576 bytes", big.len())
    );
    let e = check_text("\u{feff}app {}\n", &schema(), &env()).unwrap_err();
    assert!(e[0].message.contains("byte order mark"));

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.hcl");
    std::fs::write(&p, &big).unwrap();
    let e = check_file(&p, &schema(), &env()).unwrap_err();
    assert!(e[0].message.contains("the maximum is 1048576 bytes"));
    std::fs::write(&p, b"\xff\xfe").unwrap();
    assert_eq!(check_file(&p, &schema(), &env()).unwrap_err()[0].message, "the configuration file is not UTF-8");
    let e = check_file(&dir.path().join("none"), &schema(), &env()).unwrap_err();
    assert!(e[0].message.starts_with("cannot read the configuration file"));
    std::fs::write(&p, GOOD).unwrap();
    assert!(check_file(&p, &schema(), &env()).is_ok());
}

#[test]
fn rule4_checked_values_are_typed() {
    let checked = check_text(GOOD, &schema(), &env()).unwrap();
    assert_eq!(checked.get("app.wait").unwrap().value, hclconf::Value::DurationMs(5000));
    assert_eq!(checked.get("app.flag").unwrap().value, hclconf::Value::Bool(false));
    assert_eq!(checked.get_in("a", "source.limit").unwrap().value, hclconf::Value::Int(5));
}

#[test]
fn rule7_hcl_reference_uses_the_table() {
    let hcl = hclconf::render_hcl(&schema(), hclconf::Profile::Local, &["Header."]);
    assert!(hcl.starts_with("# Header.\n\napp {\n  mode          = \"full\"\n"), "{hcl}");
    assert!(hcl.contains("  password_file = \"/run/pw\"\n"));
    let md = hclconf::render_markdown(
        &schema(),
        &hclconf::MarkdownSpec { title: "T", intro: &["Intro."], profile_names: ["local", "distributed"] },
    );
    assert!(
        md.contains("| `app.port` | an integer from 1 to 100 | no | V | always | never | `10` | The port. |"),
        "{md}"
    );
    assert!(md.contains("| `app.name` | a text of 1 to 20 bytes with no control character | no | V, E, F |"));
    assert!(
        md.contains("`name = \"x\"` (local)")
            || md.contains("`\"x\"` (local), `name_file = \"/run/name\"` (distributed)"),
        "{md}"
    );
    assert!(md.contains("| `source \"<id>\".url` |"));
    assert!(md.contains("| `app.extra` | 0 or 1 | `app.mode = \"full\"` | `app.mode = \"lite\"` |"), "{md}");
}

#[test]
fn applies_variants_are_used() {
    // Sanity check of the public enum.
    assert!(matches!(KEYS[0].applies, Applies::Always));
}

// The read modes of 6.18.

fn empty_env() -> CapturedEnvironment {
    CapturedEnvironment::default()
}

fn mode_errors(text: &str, mode: ReadMode<'_>) -> Vec<String> {
    match check_text_with(text, &schema(), &empty_env(), mode) {
        Ok(_) => Vec::new(),
        Err(list) => list.iter().map(ToString::to_string).collect(),
    }
}

#[test]
fn read_mode_reads_by_block_path() {
    assert!(ReadMode::All.reads("app.password"));
    assert!(!ReadMode::None.reads("app.password"));
    let only_source = ReadMode::Blocks(&["source"]);
    assert!(only_source.reads("source.token"));
    assert!(only_source.reads("source.tls.ca"));
    assert!(!only_source.reads("app.password"));
    // A block path ends at a dot: `sour` is not a prefix of the block `source`.
    assert!(!ReadMode::Blocks(&["sour"]).reads("source.token"));
    assert!(!ReadMode::Blocks(&["source.token"]).reads("source.token"));
}

#[test]
fn read_none_keeps_source_and_skips_the_read() {
    // Neither variable is set, so the mode All fails and the mode None passes.
    assert_eq!(mode_errors(GOOD, ReadMode::All).len(), 2);
    let checked = check_text_with(GOOD, &schema(), &empty_env(), ReadMode::None).unwrap();
    let pw = checked.get("app.password").unwrap();
    assert!(!pw.is_read());
    assert_eq!(pw.value, Value::NotRead);
    assert_eq!(pw.value.print(), "<not read>");
    assert_eq!(pw.source, Source::Env("PW".into()));
    assert!(format!("{pw:?}").contains("<not read>"));
    assert!(!format!("{pw:?}").contains("(value hidden)"));
    let tok = checked.get_in("a", "source.token").unwrap();
    assert_eq!(tok.source, Source::Env("TOK".into()));
    assert!(!tok.is_read());
    // A value in the file is read in each mode.
    let port = checked.get("app.port").unwrap();
    assert!(port.is_read());
    assert_eq!(port.value.print(), "10");
}

#[test]
fn read_blocks_reads_only_the_named_blocks() {
    // PW is not set (outside the block `source`); TOK is set.
    let env = CapturedEnvironment::from_pairs([("TOK".to_string(), "t".to_string())]);
    let mode = ReadMode::Blocks(&["source"]);
    let checked = check_text_with(GOOD, &schema(), &env, mode).unwrap();
    assert!(!checked.get("app.password").unwrap().is_read());
    let tok = checked.get_in("a", "source.token").unwrap();
    assert!(tok.is_read());
    assert_eq!(tok.value.print(), "\"t\"");
    // A variable inside the block that the mode reads is still necessary.
    let e = check_text_with(GOOD, &schema(), &empty_env(), mode).unwrap_err();
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].to_string(), "source \"a\".token: the environment variable TOK is not set (line 18)");
}

#[test]
fn read_mode_skips_the_file_of_a_file_form() {
    let text = GOOD.replace("password_env = \"PW\"", "password_file = \"/nonexistent/secret\"");
    assert!(mode_errors(&text, ReadMode::All)[0].contains("cannot read the file /nonexistent/secret"));
    let checked = check_text_with(&text, &schema(), &empty_env(), ReadMode::Blocks(&["source"]));
    // `source.token` is read and its variable is not set: that is the only error.
    assert_eq!(checked.unwrap_err().len(), 1);
    let env = CapturedEnvironment::from_pairs([("TOK".to_string(), "t".to_string())]);
    let checked = check_text_with(&text, &schema(), &env, ReadMode::Blocks(&["source"])).unwrap();
    let pw = checked.get("app.password").unwrap();
    assert_eq!(pw.source, Source::FileOf("/nonexistent/secret".into()));
    assert_eq!(pw.value, Value::NotRead);
}

#[test]
fn read_mode_keeps_the_form_rules_for_an_unread_key() {
    let none = ReadMode::None;
    // Two forms.
    let text = GOOD.replace("password_env = \"PW\"", "password_env = \"A\"\n  password_file = \"/x\"");
    assert!(mode_errors(&text, none)[0].contains("give exactly one form"));
    // A secret in clear text.
    let text = GOOD.replace("password_env = \"PW\"", "password = \"clear\"");
    let e = mode_errors(&text, none);
    assert!(e[0].contains("a secret accepts only `password_env` or `password_file`"), "{e:?}");
    assert!(!e[0].contains("clear\""));
    // The name of the variable.
    for bad in ["1BAD", "A-B", "SSL_CERT_FILE", "SSL_CERT_DIR", ""] {
        let text = GOOD.replace("password_env = \"PW\"", &format!("password_env = \"{bad}\""));
        assert_eq!(mode_errors(&text, none).len(), 1, "{bad:?}");
    }
    let text = GOOD.replace("password_env = \"PW\"", "password_env = 5");
    assert!(mode_errors(&text, none)[0].contains("expected an environment variable name"));
    // The absolute path.
    let text = GOOD.replace("password_env = \"PW\"", "password_file = \"relative/file\"");
    assert_eq!(
        mode_errors(&text, none),
        vec!["app.password: expected an absolute path of 1 to 4096 bytes, found \"relative/file\" (line 8)"]
    );
    // A form that the key refuses.
    let text = GOOD.replace("port = 10", "port_env = \"P\"");
    assert!(mode_errors(&text, none)[0].contains("accepts only a value in the file"));
    // A key that no form gives.
    let text = GOOD.replace("  password_env = \"PW\"\n", "");
    assert!(mode_errors(&text, none)[0].contains("missing secret"));
    // The other checks of the key table still run: an unknown key, a wrong type.
    assert!(
        mode_errors(&GOOD.replace("port = 10", "port = 1000"), none)[0].contains("expected an integer from 1 to 100")
    );
}

#[test]
fn read_mode_checks_a_value_in_the_file_of_an_indirect_key() {
    // The form V of a key that accepts the indirection is read and checked in each mode.
    let text = GOOD.replace("name = \"n\"", "name = \"\"");
    assert!(mode_errors(&text, ReadMode::None)[0].contains("expected a text of 1 to 20 bytes"));
}

#[test]
fn read_mode_all_is_the_default_of_check_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.hcl");
    std::fs::write(&path, GOOD).unwrap();
    assert!(check_file(&path, &schema(), &empty_env()).is_err());
    assert!(check_file_with(&path, &schema(), &empty_env(), ReadMode::All).is_err());
    let checked = check_file_with(&path, &schema(), &empty_env(), ReadMode::None).unwrap();
    assert!(!checked.get("app.password").unwrap().is_read());
}
