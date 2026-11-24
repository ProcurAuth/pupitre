// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of a labelled block inside a labelled block (6.12): `command "<id>" { param "<name>" { } }`.

use hclconf::{
    check_text, render_hcl, render_markdown, BlockDesc, CapturedEnvironment, KeyDesc, Kind, MarkdownSpec, Profile, Ref,
    Schema, TextRule, Value,
};

static BLOCKS: [BlockDesc; 3] = [
    BlockDesc::new("app", "app", "The application."),
    BlockDesc::new("command", "command", "A command.").labelled(0, 4, "restart"),
    BlockDesc::new("command.param", "command.param", "A parameter of a command.").labelled(0, 2, "grace"),
];

static KEYS: [KeyDesc; 4] = [
    KeyDesc::new("app.name", Kind::Text { min: 1, max: 20, rule: TextRule::NoControl }, "The name.")
        .both(Ref::Str("x")),
    KeyDesc::new("command.label", Kind::Text { min: 1, max: 64, rule: TextRule::NoControl }, "The label.")
        .both(Ref::Str("Restart")),
    KeyDesc::new("command.param.type", Kind::Enum(&["text", "integer"]), "The type.").both(Ref::Str("integer")),
    KeyDesc::new("command.param.max", Kind::Integer { min: 0, max: 100 }, "The maximum.")
        .manual("the type is integer", "the type is text")
        .both(Ref::Int(5)),
];

static SCHEMA: Schema = Schema { blocks: &BLOCKS, keys: &KEYS };

fn env() -> CapturedEnvironment {
    CapturedEnvironment::from_pairs(Vec::<(String, String)>::new())
}

#[test]
fn the_table_with_a_nested_labelled_block_is_valid() {
    assert_eq!(SCHEMA.validate(), Ok(()));
}

#[test]
fn a_deeper_labelled_block_is_refused() {
    static DEEP_BLOCKS: [BlockDesc; 3] = [
        BlockDesc::new("command", "command", "A command.").labelled(0, 4, "a"),
        BlockDesc::new("command.param", "command.param", "A parameter.").labelled(0, 2, "b"),
        BlockDesc::new("command.param.choice", "choice", "A choice.").labelled(0, 2, "c"),
    ];
    static DEEP: Schema = Schema { blocks: &DEEP_BLOCKS, keys: &[] };
    assert!(DEEP.validate().is_err());

    static UNDER_PLAIN_BLOCKS: [BlockDesc; 2] = [
        BlockDesc::new("app", "app", "The application."),
        BlockDesc::new("app.item", "item", "An item.").labelled(0, 2, "a"),
    ];
    static UNDER_PLAIN: Schema = Schema { blocks: &UNDER_PLAIN_BLOCKS, keys: &[] };
    assert!(UNDER_PLAIN.validate().is_err());
}

const FILE: &str = r#"
app {
  name = "demo"
}

command "restart" {
  label = "Restart"
  param "grace" {
    type = "integer"
    max  = 30
  }
  param "reason" {
    type = "text"
  }
}

command "reload" {
  label = "Reload"
}
"#;

#[test]
fn the_keys_of_nested_blocks_have_two_labels() {
    let checked = check_text(FILE, &SCHEMA, &env()).expect("valid file");
    assert_eq!(checked.labels("command"), vec!["restart", "reload"]);
    assert_eq!(checked.sub_labels("restart", "command.param"), vec!["grace", "reason"]);
    assert!(checked.sub_labels("reload", "command.param").is_empty());

    let max = checked.get_in_sub("restart", "grace", "command.param.max").expect("max");
    assert_eq!(max.value, Value::Int(30));
    assert_eq!(max.display, "command \"restart\".param \"grace\".max");
    assert_eq!(max.sub_label.as_deref(), Some("grace"));

    let label = checked.get_in("restart", "command.label").expect("label");
    assert_eq!(label.value, Value::Str("Restart".to_string()));
    // A key of an inner block is not a key of the outer block.
    assert!(checked.get_in("restart", "command.param.type").is_none());
    assert!(checked.block_in_sub("restart", "reason", "command.param").is_some());
}

#[test]
fn the_keys_come_in_the_order_of_the_blocks() {
    let checked = check_text(FILE, &SCHEMA, &env()).expect("valid file");
    let order: Vec<&str> = checked.keys().iter().map(|k| k.display.as_str()).collect();
    assert_eq!(
        order,
        vec![
            "app.name",
            "command \"restart\".label",
            "command \"restart\".param \"grace\".type",
            "command \"restart\".param \"grace\".max",
            "command \"restart\".param \"reason\".type",
            "command \"reload\".label",
        ]
    );
}

#[test]
fn a_duplicate_inner_label_is_an_error() {
    let text = r#"
app { name = "demo" }
command "restart" {
  label = "Restart"
  param "grace" { type = "integer" }
  param "grace" { type = "text" }
}
"#;
    let errors = check_text(text, &SCHEMA, &env()).expect_err("duplicate");
    assert!(errors.iter().any(|e| e.to_string().contains("duplicate label")), "{errors:?}");
}

#[test]
fn the_same_inner_label_in_two_outer_blocks_is_valid() {
    let text = r#"
app { name = "demo" }
command "a" {
  label = "A"
  param "grace" { type = "integer" }
}
command "b" {
  label = "B"
  param "grace" { type = "text" }
}
"#;
    let checked = check_text(text, &SCHEMA, &env()).expect("valid");
    assert_eq!(
        checked.get_in_sub("b", "grace", "command.param.type").map(|k| &k.value),
        Some(&Value::Str("text".into()))
    );
}

#[test]
fn too_many_inner_blocks_is_an_error_of_the_outer_block() {
    let text = r#"
app { name = "demo" }
command "a" {
  label = "A"
  param "x" { type = "text" }
  param "y" { type = "text" }
  param "z" { type = "text" }
}
"#;
    let errors = check_text(text, &SCHEMA, &env()).expect_err("too many");
    assert!(
        errors.iter().any(|e| e.key == "command \"a\".param" && e.message.contains("expected 0 to 2 blocks")),
        "{errors:?}"
    );
}

#[test]
fn the_reference_shows_the_two_placeholders() {
    let spec = MarkdownSpec { title: "T", intro: &[], profile_names: ["local", "distributed"] };
    let md = render_markdown(&SCHEMA, &spec);
    assert!(md.contains("`command \"<id>\".param \"<name>\".max`"), "{md}");
    let hcl = render_hcl(&SCHEMA, Profile::Local, &[]);
    assert!(hcl.contains("command \"restart\" {"), "{hcl}");
    assert!(hcl.contains("  param \"grace\" {"), "{hcl}");
}
