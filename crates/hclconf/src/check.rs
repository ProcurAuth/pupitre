// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check a file against a key table: the closed schema, the forms, the reads of `_env` and
//! `_file`, the type and range of each value, and the modes (stage 1 of 6.19).

use std::collections::HashMap;
use std::path::Path;

use crate::env::{CapturedEnvironment, KEPT_VARIABLES};
use crate::error::{limit_errors, ConfigError};
use crate::fsio::{read_regular_file, ReadError};
use crate::kinds::{check_literal, check_string, is_absolute_path};
use crate::schema::{Applies, BlockDesc, Forms, KeyDesc, Kind, Schema};
use crate::syntax::{self, Attr, Body, Literal};
use crate::value::{block_display, Value};

/// The largest configuration file (6.25).
pub const MAX_CONFIG_BYTES: u64 = 1_048_576;
/// The largest content of a `_file` value (6.25).
pub const MAX_FILE_VALUE_BYTES: u64 = 65_536;

/// Which `_env` and `_file` values the check reads (6.18).
///
/// A value that the mode does not read keeps its form and its source, and its value is
/// `Value::NotRead`. The checks that need the value do not run for it. The checks of the
/// form (6.5) still run: one form only, a secret only in `_env` or `_file`, the rules of the
/// variable name and of the absolute path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode<'a> {
    /// Read each `_env` and `_file` value.
    All,
    /// Read only the `_env` and `_file` values of the keys under these block paths of the key
    /// table, for example `storage`. A block path covers its sub-blocks.
    Blocks(&'a [&'a str]),
    /// Read no `_env` and no `_file` value.
    None,
}

impl ReadMode<'_> {
    /// True when the mode reads the `_env` and `_file` value of the key `key_path`.
    pub fn reads(&self, key_path: &str) -> bool {
        match self {
            ReadMode::All => true,
            ReadMode::None => false,
            ReadMode::Blocks(blocks) => {
                blocks.iter().any(|b| key_path.strip_prefix(b).is_some_and(|rest| rest.starts_with('.')))
            }
        }
    }
}

/// Where the value of a key comes from (6.20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The value is in the configuration file.
    File,
    /// The value is in this environment variable.
    Env(String),
    /// The value is the content of this file.
    FileOf(String),
}

impl Source {
    /// The text of `print-config`: `file`, `env:VAR` or `file:/path`.
    pub fn text(&self) -> String {
        match self {
            Source::File => "file".to_string(),
            Source::Env(var) => format!("env:{var}"),
            Source::FileOf(path) => format!("file:{path}"),
        }
    }
}

/// A key that has a value, after all checks of stage 1.
#[derive(Clone)]
pub struct ResolvedKey {
    /// The path in the key table, for example `clickhouse.tls.ca_file`.
    pub path: &'static str,
    /// The label of the labelled block that holds the key.
    pub label: Option<String>,
    /// The label of the labelled block inside the labelled block, for example `grace` in
    /// `command "restart".param "grace".type` (6.12).
    pub sub_label: Option<String>,
    /// The key path in the form of `print-config`, for example `clickhouse "prod".url`.
    pub display: String,
    /// The value.
    pub value: Value,
    /// Where the value comes from.
    pub source: Source,
    /// The line of the attribute that gave the value.
    pub line: u32,
    /// True for a secret. A secret never shows in `Debug`.
    pub secret: bool,
}

impl ResolvedKey {
    /// True when the read mode read the value. False for a value of `_env` or `_file` that
    /// the mode skipped: its value is `Value::NotRead`.
    pub fn is_read(&self) -> bool {
        !self.value.is_not_read()
    }
}

impl std::fmt::Debug for ResolvedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = if self.value.is_not_read() {
            self.value.print()
        } else if self.secret {
            "(value hidden)".to_string()
        } else {
            self.value.print()
        };
        write!(f, "ResolvedKey({} = {} # {})", self.display, value, self.source.text())
    }
}

/// A block of the file that the checks kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInstance {
    /// The path in the key table.
    pub path: &'static str,
    /// The label of the labelled block that holds this block, or of this block.
    pub label: Option<String>,
    /// The label of the labelled block inside the labelled block that holds this block, or of
    /// this block (6.12).
    pub sub_label: Option<String>,
    /// The path in the form of `print-config`.
    pub display: String,
    /// The line of the block.
    pub line: u32,
}

/// The result of stage 1: the keys that have a value and the blocks that exist.
#[derive(Debug, Clone, Default)]
pub struct Checked {
    keys: Vec<ResolvedKey>,
    blocks: Vec<BlockInstance>,
}

impl Checked {
    /// All keys, in the order of the key table. The labelled blocks come in file order.
    pub fn keys(&self) -> &[ResolvedKey] {
        &self.keys
    }

    /// All blocks that exist.
    pub fn blocks(&self) -> &[BlockInstance] {
        &self.blocks
    }

    /// Find a key outside a labelled block.
    pub fn get(&self, path: &str) -> Option<&ResolvedKey> {
        self.keys.iter().find(|k| k.label.is_none() && k.path == path)
    }

    /// Find a key of a labelled block, outside its inner labelled blocks.
    pub fn get_in(&self, label: &str, path: &str) -> Option<&ResolvedKey> {
        self.keys.iter().find(|k| k.label.as_deref() == Some(label) && k.sub_label.is_none() && k.path == path)
    }

    /// Find a key of an inner labelled block: the block `sub_label` inside the block `label`.
    pub fn get_in_sub(&self, label: &str, sub_label: &str, path: &str) -> Option<&ResolvedKey> {
        self.keys
            .iter()
            .find(|k| k.label.as_deref() == Some(label) && k.sub_label.as_deref() == Some(sub_label) && k.path == path)
    }

    /// True when a block outside a labelled block exists.
    pub fn has_block(&self, path: &str) -> bool {
        self.blocks.iter().any(|b| b.label.is_none() && b.path == path)
    }

    /// Find a block of a labelled block (or the labelled block itself), outside its inner
    /// labelled blocks.
    pub fn block_in(&self, label: &str, path: &str) -> Option<&BlockInstance> {
        self.blocks.iter().find(|b| b.label.as_deref() == Some(label) && b.sub_label.is_none() && b.path == path)
    }

    /// Find a block of an inner labelled block (or the inner labelled block itself).
    pub fn block_in_sub(&self, label: &str, sub_label: &str, path: &str) -> Option<&BlockInstance> {
        self.blocks
            .iter()
            .find(|b| b.label.as_deref() == Some(label) && b.sub_label.as_deref() == Some(sub_label) && b.path == path)
    }

    /// The labels of the labelled block `path` at the top of the file, in file order.
    pub fn labels(&self, path: &str) -> Vec<&str> {
        self.blocks
            .iter()
            .filter(|b| b.path == path && b.sub_label.is_none())
            .filter_map(|b| b.label.as_deref())
            .collect()
    }

    /// The labels of the inner labelled block `path` inside the block `label`, in file order.
    pub fn sub_labels(&self, label: &str, path: &str) -> Vec<&str> {
        self.blocks
            .iter()
            .filter(|b| b.path == path && b.label.as_deref() == Some(label))
            .filter_map(|b| b.sub_label.as_deref())
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Value,
    Env,
    File,
}

struct KeyRes {
    value: Option<(Value, Source)>,
    errors: Vec<ConfigError>,
    line: u32,
}

struct Inst {
    desc: &'static BlockDesc,
    label: Option<String>,
    sub_label: Option<String>,
    display: String,
    line: u32,
    keys: HashMap<&'static str, KeyRes>,
    errors: Vec<ConfigError>,
    children: Vec<Inst>,
}

fn join_display(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

fn valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Check the name of the variable that an `_env` attribute gives. This check needs no read.
fn env_name(var_literal: &Literal) -> Result<String, String> {
    let Literal::Str(var) = var_literal else {
        return Err(format!("expected an environment variable name, found {}", var_literal.describe()));
    };
    if !valid_variable_name(var) {
        return Err(format!(
            "expected an environment variable name that matches ^[A-Za-z_][A-Za-z0-9_]*$, found {}",
            crate::value::json_string(var)
        ));
    }
    if KEPT_VARIABLES.contains(&var.as_str()) {
        return Err(format!(
            "expected an environment variable name other than SSL_CERT_FILE and SSL_CERT_DIR, found {}",
            crate::value::json_string(var)
        ));
    }
    Ok(var.clone())
}

fn read_env(var_literal: &Literal, env: &CapturedEnvironment) -> Result<(String, String), String> {
    let var = env_name(var_literal)?;
    let var = &var;
    let Some(raw) = env.get(var) else {
        return Err(format!("the environment variable {var} is not set"));
    };
    let Some(text) = raw.to_str() else {
        return Err(format!("the value of the environment variable {var} is not UTF-8"));
    };
    if text.is_empty() {
        return Err(format!("the value of the environment variable {var} is empty"));
    }
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        return Err(format!("the value of the environment variable {var} starts or ends with a space or a line end"));
    }
    Ok((var.clone(), text.to_string()))
}

/// Check the path that an `_file` attribute gives. This check does not touch the file.
fn file_path(path_literal: &Literal) -> Result<String, String> {
    let Literal::Str(path) = path_literal else {
        return Err(format!("expected an absolute path of 1 to 4096 bytes, found {}", path_literal.describe()));
    };
    if !is_absolute_path(path) {
        return Err(format!("expected an absolute path of 1 to 4096 bytes, found {}", crate::value::json_string(path)));
    }
    Ok(path.clone())
}

fn read_file_value(path_literal: &Literal) -> Result<(String, String), String> {
    let path = &file_path(path_literal)?;
    let cannot = |reason: &str| format!("cannot read the file {path}: {reason}");
    let bytes = match read_regular_file(Path::new(path), MAX_FILE_VALUE_BYTES) {
        Ok(b) => b,
        Err(ReadError::TooLarge(_)) => {
            return Err(cannot(&format!("the content has more than {MAX_FILE_VALUE_BYTES} bytes")));
        }
        Err(e) => return Err(cannot(&e.to_string())),
    };
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(cannot("the content starts with a byte order mark"));
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Err(cannot("the content is not UTF-8"));
    };
    let text = text.strip_suffix('\n').map(|t| t.strip_suffix('\r').unwrap_or(t)).unwrap_or(&text).to_string();
    if text.is_empty() {
        return Err(format!("the file {path} is empty"));
    }
    Ok((path.clone(), text))
}

fn resolve_key(
    key: &KeyDesc,
    key_display: &str,
    entries: &[(Form, &Attr)],
    env: &CapturedEnvironment,
    mode: ReadMode<'_>,
) -> KeyRes {
    let name = key.name();
    let form_name = |f: Form| match f {
        Form::Value => name.to_string(),
        Form::Env => format!("{name}_env"),
        Form::File => format!("{name}_file"),
    };
    let first_line = entries[0].1.line;
    let mut res = KeyRes { value: None, errors: Vec::new(), line: first_line };
    let mut err = |message: String, line: u32| res.errors.push(ConfigError::at(key_display, message, line));

    // A secret in clear text stops the start, also together with another form (6.5).
    if key.secret {
        if let Some((_, attr)) = entries.iter().find(|(f, _)| *f == Form::Value) {
            err(
                format!(
                    "a secret accepts only `{name}_env` or `{name}_file`, but the file gives it in clear text (value hidden)"
                ),
                attr.line,
            );
            return res;
        }
    }
    let mut refused = false;
    for (form, attr) in entries {
        let accepted = match (key.forms, form) {
            (Forms::V, Form::Value) | (Forms::Vef, _) | (Forms::Ef, Form::Env | Form::File) => true,
            (Forms::V, _) | (Forms::Ef, Form::Value) => false,
        };
        if !accepted {
            refused = true;
            let message = if key.forms == Forms::V {
                format!("`{name}` accepts only a value in the file, not `_env` or `_file`")
            } else {
                format!("`{name}` accepts only `{name}_env` or `{name}_file`, not a value in the file")
            };
            err(message, attr.line);
        }
    }
    if refused {
        return res;
    }
    if entries.len() > 1 {
        let (f1, a1) = entries[0];
        let (f2, a2) = entries[1];
        err(
            format!(
                "the file gives `{}` at line {} and `{}`; give exactly one form",
                form_name(f1),
                a1.line,
                form_name(f2)
            ),
            a2.line,
        );
        return res;
    }
    let (form, attr) = entries[0];
    let reads = mode.reads(key.path);
    let outcome: Result<(Value, Source), String> = match form {
        Form::Value => check_literal(&key.kind, key.secret, &attr.value).map(|v| (v, Source::File)),
        // A value that the mode does not read keeps its source. Only the form is checked.
        Form::Env if !reads => env_name(&attr.value).map(|var| (Value::NotRead, Source::Env(var))),
        Form::File if !reads => file_path(&attr.value).map(|path| (Value::NotRead, Source::FileOf(path))),
        Form::Env => read_env(&attr.value, env)
            .and_then(|(var, text)| check_string(&key.kind, key.secret, &text).map(|v| (v, Source::Env(var)))),
        Form::File => read_file_value(&attr.value)
            .and_then(|(path, text)| check_string(&key.kind, key.secret, &text).map(|v| (v, Source::FileOf(path)))),
    };
    match outcome {
        Ok(pair) => res.value = Some(pair),
        Err(message) => err(message, attr.line),
    }
    res
}

/// Classify the name of an attribute against the keys of a block.
fn classify(schema: &Schema, block_path: &str, name: &str) -> Option<(&'static KeyDesc, Form)> {
    let keys: Vec<&'static KeyDesc> = schema.keys_of(block_path).collect();
    if let Some(k) = keys.iter().find(|k| k.name() == name) {
        return Some((k, Form::Value));
    }
    for (suffix, form) in [("_env", Form::Env), ("_file", Form::File)] {
        if let Some(base) = name.strip_suffix(suffix) {
            if let Some(k) = keys.iter().find(|k| k.name() == base) {
                return Some((k, form));
            }
        }
    }
    None
}

/// What the build of the blocks needs to resolve a key: the environment and the read mode.
#[derive(Clone, Copy)]
struct Reads<'a> {
    env: &'a CapturedEnvironment,
    mode: ReadMode<'a>,
}

/// The labels of a new block: the label of its labelled block and the label of its inner
/// labelled block (6.12).
struct Labels {
    label: Option<String>,
    sub_label: Option<String>,
}

fn build_instance(
    schema: &Schema,
    desc: &'static BlockDesc,
    labels: Labels,
    display: String,
    line: u32,
    body: &Body,
    reads: Reads<'_>,
) -> Inst {
    let mut inst = Inst {
        desc,
        label: labels.label,
        sub_label: labels.sub_label,
        display,
        line,
        keys: HashMap::new(),
        errors: Vec::new(),
        children: Vec::new(),
    };
    // Group the attributes by key, in file order.
    let mut groups: Vec<(&'static KeyDesc, Vec<(Form, &Attr)>)> = Vec::new();
    for attr in &body.attrs {
        match classify(schema, desc.path, &attr.name) {
            Some((key, form)) => match groups.iter_mut().find(|(k, _)| k.path == key.path) {
                Some((_, entries)) => entries.push((form, attr)),
                None => groups.push((key, vec![(form, attr)])),
            },
            None => inst.errors.push(ConfigError::at(
                join_display(&inst.display, &attr.name),
                "unknown attribute",
                attr.line,
            )),
        }
    }
    for (key, entries) in groups {
        let key_display = join_display(&inst.display, key.name());
        let res = resolve_key(key, &key_display, &entries, reads.env, reads.mode);
        inst.keys.insert(key.path, res);
    }
    for node in &body.blocks {
        let shown = if desc.path.is_empty() {
            block_display(&node.name, &node.labels)
        } else {
            join_display(&inst.display, &block_display(&node.name, &node.labels))
        };
        let child_path = if desc.path.is_empty() { node.name.clone() } else { format!("{}.{}", desc.path, node.name) };
        let Some(child_desc) = schema.block(&child_path) else {
            inst.errors.push(ConfigError::at(shown, "unknown block", node.line));
            continue;
        };
        if child_desc.labelled && node.labels.len() != 1 {
            inst.errors.push(ConfigError::at(
                shown,
                format!("expected one label: `{} \"<label>\"`", node.name),
                node.line,
            ));
            continue;
        }
        if !child_desc.labelled && !node.labels.is_empty() {
            inst.errors.push(ConfigError::at(shown, "expected a block with no label", node.line));
            continue;
        }
        // A labelled block inside a labelled block keeps the outer label and gets an inner label (6.12).
        let labels = match (child_desc.labelled, &inst.label) {
            (true, Some(outer)) => Labels { label: Some(outer.clone()), sub_label: node.labels.first().cloned() },
            (true, None) => Labels { label: node.labels.first().cloned(), sub_label: None },
            (false, _) => Labels { label: inst.label.clone(), sub_label: inst.sub_label.clone() },
        };
        let earlier = inst.children.iter().find(|c| {
            c.desc.path == child_desc.path
                && (!child_desc.labelled || (c.label == labels.label && c.sub_label == labels.sub_label))
        });
        if let Some(first) = earlier {
            let message = if child_desc.labelled {
                format!("duplicate label; the first block with this label is at line {}", first.line)
            } else {
                format!("duplicate block; the first one is at line {}", first.line)
            };
            inst.errors.push(ConfigError::at(shown, message, node.line));
            continue;
        }
        let child = build_instance(schema, child_desc, labels, shown, node.line, &node.body, reads);
        inst.children.push(child);
    }
    inst
}

enum Decision {
    Yes,
    No(usize),
    Unknown,
}

/// The order of a key in the result: the rank of its top block, the index of its labelled
/// block, the index of its inner labelled block (0 outside one), and its index in the table.
type Order = (usize, usize, usize, usize);

/// The place of a block instance: the rank of its top block, the index of its labelled block
/// and the index of its inner labelled block.
#[derive(Clone, Copy)]
struct Place {
    rank: usize,
    instance: usize,
    sub: usize,
}

struct Finalizer<'a> {
    schema: &'a Schema,
    lookup: HashMap<&'static str, Value>,
    errors: Vec<ConfigError>,
    keys: Vec<(ResolvedKey, Order)>,
    blocks: Vec<BlockInstance>,
}

impl Finalizer<'_> {
    fn decide(&self, applies: &Applies) -> Decision {
        let (Applies::When(terms) | Applies::WhenOptional(terms)) = applies else {
            return Decision::Yes;
        };
        let mut unknown = false;
        for (i, term) in terms.iter().enumerate() {
            match self.lookup.get(term.key).and_then(Value::as_condition_text) {
                Some(text) if text == term.equals => {}
                Some(_) => return Decision::No(i),
                None => unknown = true,
            }
        }
        if unknown {
            Decision::Unknown
        } else {
            Decision::Yes
        }
    }

    fn required_text(&self, term_key: &str, equals: &str) -> String {
        match self.schema.key(term_key).map(|k| k.kind) {
            Some(Kind::Bool) => equals.to_string(),
            _ => crate::value::json_string(equals),
        }
    }

    /// The message for something that a mode forbids.
    fn forbidden_message(&self, applies: &Applies, failed: usize) -> String {
        if let Applies::When(terms) | Applies::WhenOptional(terms) = applies {
            let term = &terms[failed];
            let current = self.lookup.get(term.key).map(Value::print).unwrap_or_default();
            return format!(
                "not permitted when `{}` is {}; the key is only for `{}` = {}",
                term.key,
                current,
                term.key,
                self.required_text(term.key, term.equals)
            );
        }
        "not permitted".to_string()
    }

    fn missing_block_message(&self, applies: &Applies) -> String {
        if let Applies::When(terms) | Applies::WhenOptional(terms) = applies {
            if let Some(term) = terms.first() {
                return format!(
                    "missing block; expected when `{}` is {}",
                    term.key,
                    self.required_text(term.key, term.equals)
                );
            }
        }
        "missing block".to_string()
    }

    fn finalize(&mut self, inst: Inst, place: Place, parent_line: Option<u32>) {
        self.errors.extend(inst.errors);
        self.blocks.push(BlockInstance {
            path: inst.desc.path,
            label: inst.label.clone(),
            sub_label: inst.sub_label.clone(),
            display: inst.display.clone(),
            line: inst.line,
        });
        let mut keys = inst.keys;
        for key in self.schema.keys_of(inst.desc.path) {
            let present = keys.remove(key.path);
            let key_display = join_display(&inst.display, key.name());
            let schema_idx = self.schema.key_index(key.path).unwrap_or(usize::MAX);
            match (self.decide(&key.applies), present) {
                (Decision::No(i), Some(res)) => {
                    self.errors.push(ConfigError::at(key_display, self.forbidden_message(&key.applies, i), res.line));
                }
                (Decision::No(_), None) => {}
                (_, Some(res)) => {
                    self.errors.extend(res.errors);
                    if let Some((value, source)) = res.value {
                        let resolved = ResolvedKey {
                            path: key.path,
                            label: inst.label.clone(),
                            sub_label: inst.sub_label.clone(),
                            display: key_display,
                            value,
                            source,
                            line: res.line,
                            secret: key.secret,
                        };
                        self.keys.push((resolved, (place.rank, place.instance, place.sub, schema_idx)));
                    }
                }
                (Decision::Yes, None) if matches!(key.applies, Applies::Always | Applies::When(_)) => {
                    self.errors.push(ConfigError::at(key_display, missing_key_message(key), inst.line));
                }
                (_, None) => {}
            }
        }
        let mut children = inst.children;
        for child_desc in self.schema.child_blocks(inst.desc.path) {
            let (mine, rest): (Vec<Inst>, Vec<Inst>) =
                std::mem::take(&mut children).into_iter().partition(|c| c.desc.path == child_desc.path);
            children = rest;
            let shown = join_display(&inst.display, child_desc.name());
            let line = (!inst.desc.path.is_empty()).then_some(inst.line).or(parent_line);
            self.finalize_children(child_desc, mine, &shown, line, place);
        }
    }

    fn finalize_children(
        &mut self,
        desc: &'static BlockDesc,
        instances: Vec<Inst>,
        shown: &str,
        line: Option<u32>,
        place: Place,
    ) {
        if desc.labelled {
            let nested = !desc.parent_path().is_empty();
            if instances.len() < desc.min || instances.len() > desc.max {
                self.errors.push(ConfigError::new(
                    shown,
                    format!(
                        "expected {} to {} blocks `{} \"<label>\"`, found {}",
                        desc.min,
                        desc.max,
                        desc.name(),
                        instances.len()
                    ),
                    None,
                ));
            }
            for (i, inst) in instances.into_iter().enumerate() {
                let inner = if nested { Place { sub: i + 1, ..place } } else { Place { instance: i, sub: 0, ..place } };
                self.finalize(inst, inner, line);
            }
            return;
        }
        match (self.decide(&desc.applies), instances.is_empty()) {
            (Decision::No(i), false) => {
                let first = &instances[0];
                self.errors.push(ConfigError::at(
                    first.display.clone(),
                    self.forbidden_message(&desc.applies, i),
                    first.line,
                ));
            }
            (Decision::No(_), true) | (Decision::Unknown, true) => {}
            (Decision::Yes, true) => {
                if matches!(desc.applies, Applies::Always | Applies::When(_)) {
                    let message = self.missing_block_message(&desc.applies);
                    self.errors.push(ConfigError::new(shown, message, line));
                }
            }
            (_, false) => {
                for inst in instances {
                    self.finalize(inst, place, line);
                }
            }
        }
    }
}

fn missing_key_message(key: &KeyDesc) -> String {
    let name = key.name();
    let expected = key.kind.expected();
    match key.forms {
        Forms::V => format!("missing attribute; expected {expected}"),
        Forms::Vef => {
            format!("missing attribute; give `{name}`, `{name}_env` or `{name}_file` ({expected})")
        }
        Forms::Ef if key.secret => format!("missing secret; give `{name}_env` or `{name}_file`"),
        Forms::Ef => format!("missing attribute; give `{name}_env` or `{name}_file` ({expected})"),
    }
}

fn collect_lookup(inst: &Inst, out: &mut HashMap<&'static str, Value>) {
    for (path, res) in &inst.keys {
        if let Some((value, _)) = &res.value {
            out.insert(path, value.clone());
        }
    }
    for child in &inst.children {
        collect_lookup(child, out);
    }
}

/// Check a configuration text against a key table. The result is the typed keys, or the
/// list of errors of stage 1 in file order (6.19). It reads each `_env` and `_file` value.
pub fn check_text(text: &str, schema: &Schema, env: &CapturedEnvironment) -> Result<Checked, Vec<ConfigError>> {
    check_text_with(text, schema, env, ReadMode::All)
}

/// Like `check_text`, with a read mode (6.18). A key whose value the mode does not read has
/// the value `Value::NotRead`, and its checks of the value do not run.
pub fn check_text_with(
    text: &str,
    schema: &Schema,
    env: &CapturedEnvironment,
    mode: ReadMode<'_>,
) -> Result<Checked, Vec<ConfigError>> {
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(vec![ConfigError::file(format!(
            "the configuration file has {} bytes; the maximum is {MAX_CONFIG_BYTES} bytes",
            text.len()
        ))]);
    }
    if text.starts_with('\u{feff}') {
        return Err(vec![ConfigError::file("the configuration file starts with a byte order mark")]);
    }
    // A syntax error and a duplicate attribute stop the check at once (6.3).
    let body = syntax::parse(text).map_err(|e| vec![e])?;

    static ROOT: BlockDesc = BlockDesc::new("", "", "");
    let root = build_instance(
        schema,
        &ROOT,
        Labels { label: None, sub_label: None },
        String::new(),
        0,
        &body,
        Reads { env, mode },
    );

    let mut lookup = HashMap::new();
    for child in root.children.iter().filter(|c| !c.desc.labelled) {
        collect_lookup(child, &mut lookup);
    }
    let mut fin = Finalizer { schema, lookup, errors: Vec::new(), keys: Vec::new(), blocks: Vec::new() };
    let mut root = root;
    // The pseudo block at the top has no keys: finalize its errors and its blocks.
    fin.errors.append(&mut root.errors);
    let mut children = root.children;
    for desc in schema.child_blocks("") {
        let (mine, rest): (Vec<Inst>, Vec<Inst>) =
            std::mem::take(&mut children).into_iter().partition(|c| c.desc.path == desc.path);
        children = rest;
        let rank = schema.keys.iter().position(|k| Schema::root_of(k.path) == desc.path).unwrap_or(usize::MAX);
        let before = fin.blocks.len();
        fin.finalize_children(desc, mine, desc.name(), None, Place { rank, instance: 0, sub: 0 });
        debug_assert!(fin.blocks.len() >= before);
    }

    if !fin.errors.is_empty() {
        let mut errors = fin.errors;
        errors.sort_by_key(|e| e.line.unwrap_or(0));
        return Err(limit_errors(errors));
    }
    let mut keys = fin.keys;
    keys.sort_by_key(|(_, order)| *order);
    Ok(Checked { keys: keys.into_iter().map(|(k, _)| k).collect(), blocks: fin.blocks })
}

/// Read a configuration file and check it. The file is a regular file of at most 1 MiB, in
/// UTF-8, with no byte order mark (6.25).
pub fn check_file(path: &Path, schema: &Schema, env: &CapturedEnvironment) -> Result<Checked, Vec<ConfigError>> {
    check_file_with(path, schema, env, ReadMode::All)
}

/// Like `check_file`, with a read mode (6.18).
pub fn check_file_with(
    path: &Path,
    schema: &Schema,
    env: &CapturedEnvironment,
    mode: ReadMode<'_>,
) -> Result<Checked, Vec<ConfigError>> {
    let bytes = match read_regular_file(path, MAX_CONFIG_BYTES) {
        Ok(b) => b,
        Err(ReadError::TooLarge(n)) => {
            return Err(vec![ConfigError::file(format!(
                "the configuration file has {n} bytes; the maximum is {MAX_CONFIG_BYTES} bytes"
            ))]);
        }
        Err(e) => {
            return Err(vec![ConfigError::file(format!("cannot read the configuration file {}: {e}", path.display()))]);
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Err(vec![ConfigError::file("the configuration file is not UTF-8")]);
    };
    check_text_with(&text, schema, env, mode)
}
