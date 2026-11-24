// SPDX-License-Identifier: MIT OR Apache-2.0
//! The key table: descriptors of the blocks and the keys (6.7 to 6.12).
//!
//! The table is data. The engine checks a file against it, and it generates the reference
//! files (6.21) from it, so the code and the documents agree.

use crate::value::{format_duration, json_string};

/// A rule for the characters of a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRule {
    /// No rule on the characters.
    Any,
    /// No control character.
    NoControl,
    /// Printable ASCII only (bytes 0x20 to 0x7E).
    PrintableAscii,
    /// No space and no control character.
    NoControlNoSpace,
    /// A DNS name.
    DnsName,
    /// A DNS name or an IP address.
    HostOrIp,
}

/// The type of a key, with its range.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// A text with a length in bytes and a rule.
    Text {
        /// The shortest length in bytes.
        min: usize,
        /// The longest length in bytes.
        max: usize,
        /// The rule on the characters.
        rule: TextRule,
    },
    /// A list of texts. The list has a count range. Each item has a length in bytes and a rule.
    /// A list accepts the form V only.
    TextList {
        /// The fewest items.
        min_items: usize,
        /// The most items.
        max_items: usize,
        /// The shortest length of an item in bytes.
        item_min: usize,
        /// The longest length of an item in bytes.
        item_max: usize,
        /// The rule on the characters of an item.
        rule: TextRule,
    },
    /// One text of a closed list.
    Enum(&'static [&'static str]),
    /// A whole number with a range.
    Integer {
        /// The lowest value.
        min: u64,
        /// The highest value.
        max: u64,
    },
    /// A size in bytes, a whole number with a range.
    Size {
        /// The lowest value.
        min: u64,
        /// The highest value.
        max: u64,
    },
    /// A duration with a range in milliseconds.
    Duration {
        /// The lowest value in milliseconds.
        min_ms: u64,
        /// The highest value in milliseconds.
        max_ms: u64,
    },
    /// A boolean.
    Bool,
    /// An absolute path of 1 to 4096 bytes with no NUL byte (6.6).
    Path,
    /// An IP address and a port in canonical form.
    SocketAddr,
}

impl Kind {
    /// The text "type and range" of the kind, as an error and the reference show it.
    pub fn expected(&self) -> String {
        match self {
            Kind::Text { min, max, rule } => match rule {
                TextRule::Any => format!("a text of {min} to {max} bytes"),
                TextRule::NoControl => format!("a text of {min} to {max} bytes with no control character"),
                TextRule::PrintableAscii => format!("a text of {min} to {max} bytes of printable ASCII"),
                TextRule::NoControlNoSpace => {
                    format!("a text of {min} to {max} bytes with no space and no control character")
                }
                TextRule::DnsName => format!("a DNS name of {min} to {max} bytes"),
                TextRule::HostOrIp => format!("a DNS name of {min} to {max} bytes, or an IP address"),
            },
            Kind::TextList { min_items, max_items, item_min, item_max, rule } => {
                let item = match rule {
                    TextRule::Any => format!("{item_min} to {item_max} bytes"),
                    TextRule::NoControl => format!("{item_min} to {item_max} bytes with no control character"),
                    TextRule::PrintableAscii => format!("{item_min} to {item_max} bytes of printable ASCII"),
                    TextRule::NoControlNoSpace => {
                        format!("{item_min} to {item_max} bytes with no space and no control character")
                    }
                    TextRule::DnsName => format!("{item_min} to {item_max} bytes of a DNS name"),
                    TextRule::HostOrIp => format!("{item_min} to {item_max} bytes of a DNS name or an IP address"),
                };
                format!("a list of {min_items} to {max_items} texts, each of {item}")
            }
            Kind::Enum(values) => {
                let quoted: Vec<String> = values.iter().map(|v| json_string(v)).collect();
                match quoted.split_last() {
                    Some((last, rest)) if !rest.is_empty() => format!("{} or {}", rest.join(", "), last),
                    Some((last, _)) => last.clone(),
                    None => String::new(),
                }
            }
            Kind::Integer { min, max } => format!("an integer from {min} to {max}"),
            Kind::Size { min, max } => format!("a size in bytes from {min} to {max}"),
            Kind::Duration { min_ms, max_ms } => {
                format!("a duration from {} to {}", format_duration(*min_ms), format_duration(*max_ms))
            }
            Kind::Bool => "a boolean".to_string(),
            Kind::Path => "an absolute path of 1 to 4096 bytes".to_string(),
            Kind::SocketAddr => "an IP address and a port in canonical form".to_string(),
        }
    }

    /// True when the kind is a text kind: the only kinds that accept `_env` and `_file`.
    pub fn is_text_like(&self) -> bool {
        matches!(self, Kind::Text { .. } | Kind::Path)
    }
}

/// The forms that a key accepts (6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forms {
    /// The value in the file only.
    V,
    /// The value, `_env` or `_file`.
    Vef,
    /// `_env` or `_file` only. A secret has these forms.
    Ef,
}

/// One condition of a mode: the key `key` has the value `equals`. For a boolean key,
/// `equals` is `true` or `false`.
#[derive(Debug, Clone, Copy)]
pub struct Term {
    /// The path of the mode key.
    pub key: &'static str,
    /// The value that the mode key must have.
    pub equals: &'static str,
}

/// When a key or a block applies. A key that applies is necessary, and a key that does not
/// apply is forbidden (6.7): there is no default value.
#[derive(Debug, Clone, Copy)]
pub enum Applies {
    /// Always.
    Always,
    /// When each term holds. The key or block is then necessary.
    When(&'static [Term]),
    /// The key or block is forbidden when a term fails. When each term holds, it is
    /// optional for the engine: the code of the program decides if it is necessary.
    WhenOptional(&'static [Term]),
    /// The engine does not decide. The code of the program checks the rule after it reads
    /// the values (a contradiction check). The key is optional for the engine.
    Manual,
}

/// The value that the reference writes for a key in one profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ref {
    /// The key is not in the file of this profile.
    Absent,
    /// A text value, written in quotes (a duration is a text).
    Str(&'static str),
    /// A whole number.
    Int(u64),
    /// A boolean.
    Bool(bool),
    /// A list of texts, written in brackets.
    StrList(&'static [&'static str]),
    /// The form `<name>_file = "<path>"`.
    File(&'static str),
}

/// The two profiles of the reference files (6.16, 6.17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The volume backend, the mode `open`.
    Local,
    /// The PostgreSQL backend, the mode `authzen`.
    Distributed,
}

impl Profile {
    /// The index of the profile in the arrays of reference values.
    pub fn index(self) -> usize {
        match self {
            Profile::Local => 0,
            Profile::Distributed => 1,
        }
    }
}

/// A descriptor of a key.
#[derive(Debug, Clone, Copy)]
pub struct KeyDesc {
    /// The path, for example `server.listen` or `clickhouse.tls.ca_file`. The last part is
    /// the name of the attribute. The rest is the path of its block.
    pub path: &'static str,
    /// The type and the range.
    pub kind: Kind,
    /// True for a secret.
    pub secret: bool,
    /// The forms.
    pub forms: Forms,
    /// When the key applies.
    pub applies: Applies,
    /// A text for the column "Necessary when". Empty: the engine writes it from `applies`.
    pub necessary_text: &'static str,
    /// A text for the column "Forbidden when". Empty: the engine writes it from `applies`.
    pub forbidden_text: &'static str,
    /// The reference values: local, distributed.
    pub refs: [Ref; 2],
    /// A value that the reference document shows when `refs` has none (a key that no
    /// reference file sets).
    pub doc_ref: Ref,
    /// One line of description, in the reference document.
    pub description: &'static str,
}

impl KeyDesc {
    /// Start a descriptor: the form V, always necessary, absent from both references.
    pub const fn new(path: &'static str, kind: Kind, description: &'static str) -> Self {
        KeyDesc {
            path,
            kind,
            secret: false,
            forms: Forms::V,
            applies: Applies::Always,
            necessary_text: "",
            forbidden_text: "",
            refs: [Ref::Absent, Ref::Absent],
            doc_ref: Ref::Absent,
            description,
        }
    }

    /// Mark the key as a secret. A secret accepts only `_env` and `_file`.
    pub const fn secret(mut self) -> Self {
        self.secret = true;
        self.forms = Forms::Ef;
        self
    }

    /// Set the accepted forms.
    pub const fn forms(mut self, forms: Forms) -> Self {
        self.forms = forms;
        self
    }

    /// Set the terms of the mode: the key applies when each term holds.
    pub const fn when(mut self, terms: &'static [Term]) -> Self {
        self.applies = Applies::When(terms);
        self
    }

    /// Set the terms of a mode that only forbids: the key is optional when each term
    /// holds. Give the texts of the reference.
    pub const fn when_optional(mut self, terms: &'static [Term], necessary: &'static str) -> Self {
        self.applies = Applies::WhenOptional(terms);
        self.necessary_text = necessary;
        self
    }

    /// Mark the rule as checked by the code of the program, with the two texts of the
    /// reference.
    pub const fn manual(mut self, necessary: &'static str, forbidden: &'static str) -> Self {
        self.applies = Applies::Manual;
        self.necessary_text = necessary;
        self.forbidden_text = forbidden;
        self
    }

    /// Override the generated text of the column "Necessary when".
    pub const fn necessary(mut self, text: &'static str) -> Self {
        self.necessary_text = text;
        self
    }

    /// Override the generated text of the column "Forbidden when".
    pub const fn forbidden(mut self, text: &'static str) -> Self {
        self.forbidden_text = text;
        self
    }

    /// Set the reference values of the two profiles.
    pub const fn refs(mut self, local: Ref, distributed: Ref) -> Self {
        self.refs = [local, distributed];
        self
    }

    /// Set the value that the reference document shows for a key that no reference file sets.
    pub const fn doc_ref(mut self, value: Ref) -> Self {
        self.doc_ref = value;
        self
    }

    /// Set the same reference value for the two profiles.
    pub const fn both(self, value: Ref) -> Self {
        self.refs(value, value)
    }

    /// The name of the attribute: the last part of the path.
    pub fn name(&self) -> &'static str {
        self.path.rsplit_once('.').map_or(self.path, |(_, n)| n)
    }

    /// The path of the block of the key.
    pub fn block_path(&self) -> &'static str {
        self.path.rsplit_once('.').map_or("", |(b, _)| b)
    }
}

/// A descriptor of a block.
#[derive(Debug, Clone, Copy)]
pub struct BlockDesc {
    /// The path, for example `storage.postgres.tls`.
    pub path: &'static str,
    /// True when the block has one label (`clickhouse "prod" { }`).
    pub labelled: bool,
    /// The fewest blocks (a labelled block can come several times).
    pub min: usize,
    /// The most blocks.
    pub max: usize,
    /// When the block applies.
    pub applies: Applies,
    /// A text for the column "Necessary when". Empty: generated.
    pub necessary_text: &'static str,
    /// A text for the column "Forbidden when". Empty: generated.
    pub forbidden_text: &'static str,
    /// The label that the reference file writes for a labelled block.
    pub reference_label: &'static str,
    /// The title of the section of the reference document.
    pub title: &'static str,
    /// One line of description, in the reference document.
    pub description: &'static str,
}

impl BlockDesc {
    /// Start a descriptor: one block, always necessary.
    pub const fn new(path: &'static str, title: &'static str, description: &'static str) -> Self {
        BlockDesc {
            path,
            labelled: false,
            min: 1,
            max: 1,
            applies: Applies::Always,
            necessary_text: "",
            forbidden_text: "",
            reference_label: "",
            title,
            description,
        }
    }

    /// Make the block labelled, with a count range and the label of the reference.
    pub const fn labelled(mut self, min: usize, max: usize, reference_label: &'static str) -> Self {
        self.labelled = true;
        self.min = min;
        self.max = max;
        self.reference_label = reference_label;
        self
    }

    /// Set the terms of the mode.
    pub const fn when(mut self, terms: &'static [Term]) -> Self {
        self.applies = Applies::When(terms);
        self
    }

    /// Mark the rule as checked by the code of the program, with the two texts.
    pub const fn manual(mut self, necessary: &'static str, forbidden: &'static str) -> Self {
        self.applies = Applies::Manual;
        self.necessary_text = necessary;
        self.forbidden_text = forbidden;
        self
    }

    /// The path of the parent block, or an empty text for a block at the top.
    pub fn parent_path(&self) -> &'static str {
        self.path.rsplit_once('.').map_or("", |(p, _)| p)
    }

    /// The name of the block: the last part of the path.
    pub fn name(&self) -> &'static str {
        self.path.rsplit_once('.').map_or(self.path, |(_, n)| n)
    }
}

/// The key table of a program.
#[derive(Debug, Clone, Copy)]
pub struct Schema {
    /// The blocks, in table order. A parent comes before its children.
    pub blocks: &'static [BlockDesc],
    /// The keys, in table order.
    pub keys: &'static [KeyDesc],
}

impl Schema {
    /// Find a key by its path.
    pub fn key(&self, path: &str) -> Option<&'static KeyDesc> {
        self.keys.iter().find(|k| k.path == path)
    }

    /// Find a block by its path.
    pub fn block(&self, path: &str) -> Option<&'static BlockDesc> {
        self.blocks.iter().find(|b| b.path == path)
    }

    /// The index of a key in the table.
    pub fn key_index(&self, path: &str) -> Option<usize> {
        self.keys.iter().position(|k| k.path == path)
    }

    /// The blocks whose parent is `parent` (an empty text for the top), in table order.
    pub fn child_blocks(&self, parent: &str) -> impl Iterator<Item = &'static BlockDesc> + '_ {
        let parent = parent.to_string();
        self.blocks.iter().filter(move |b| b.parent_path() == parent)
    }

    /// The keys that sit directly in the block `block_path`, in table order.
    pub fn keys_of(&self, block_path: &str) -> impl Iterator<Item = &'static KeyDesc> + '_ {
        let block_path = block_path.to_string();
        self.keys.iter().filter(move |k| k.block_path() == block_path)
    }

    /// The name of the top block of a path.
    pub fn root_of(path: &str) -> &str {
        path.split('.').next().unwrap_or(path)
    }

    /// Write the term as the reference shows it: `` `access.mode = "authzen"` ``.
    pub fn term_text(&self, term: &Term) -> String {
        let bare = matches!(self.key(term.key).map(|k| k.kind), Some(Kind::Bool));
        if bare {
            format!("`{} = {}`", term.key, term.equals)
        } else {
            format!("`{} = {}`", term.key, json_string(term.equals))
        }
    }

    fn negated_term_text(&self, term: &Term) -> String {
        match self.key(term.key).map(|k| k.kind) {
            Some(Kind::Bool) => {
                let other = if term.equals == "true" { "false" } else { "true" };
                format!("`{} = {}`", term.key, other)
            }
            Some(Kind::Enum(values)) => {
                let others: Vec<String> =
                    values.iter().filter(|v| **v != term.equals).map(|v| json_string(v)).collect();
                format!("`{} = {}`", term.key, others.join(" or "))
            }
            _ => format!("not {}", self.term_text(term)),
        }
    }

    /// The text of the column "Necessary when" for an `applies` rule.
    pub fn necessary_when(&self, applies: &Applies, override_text: &str) -> String {
        if !override_text.is_empty() {
            return override_text.to_string();
        }
        match applies {
            Applies::Always => "always".to_string(),
            Applies::When(terms) | Applies::WhenOptional(terms) => {
                terms.iter().map(|t| self.term_text(t)).collect::<Vec<_>>().join(" and ")
            }
            Applies::Manual => String::new(),
        }
    }

    /// The text of the column "Forbidden when" for an `applies` rule.
    pub fn forbidden_when(&self, applies: &Applies, override_text: &str) -> String {
        if !override_text.is_empty() {
            return override_text.to_string();
        }
        match applies {
            Applies::Always => "never".to_string(),
            Applies::When(terms) | Applies::WhenOptional(terms) => {
                terms.iter().map(|t| self.negated_term_text(t)).collect::<Vec<_>>().join(", or ")
            }
            Applies::Manual => String::new(),
        }
    }

    /// Check that the table is consistent. Return the first problem. A test calls it.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for key in self.keys {
            if !seen.insert(key.path) {
                return Err(format!("duplicate key {}", key.path));
            }
            if key.block_path().is_empty() {
                return Err(format!("key {} has no block", key.path));
            }
            if self.block(key.block_path()).is_none() {
                return Err(format!("key {} has no block descriptor", key.path));
            }
            if key.secret && key.forms != Forms::Ef {
                return Err(format!("secret {} must have the forms E and F only", key.path));
            }
            if key.forms != Forms::V && !key.kind.is_text_like() {
                return Err(format!("key {} accepts _env or _file but is not a text", key.path));
            }
            self.validate_applies(key.path, &key.applies, key.necessary_text, key.forbidden_text)?;
            for (i, r) in key.refs.iter().enumerate() {
                if matches!(r, Ref::File(_)) && key.forms == Forms::V {
                    return Err(format!("key {} has a file reference but does not accept _file", key.path));
                }
                if matches!(r, Ref::Str(_) | Ref::Int(_) | Ref::Bool(_) | Ref::StrList(_)) && key.forms == Forms::Ef {
                    return Err(format!("key {} has a clear reference value (profile {i})", key.path));
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for block in self.blocks {
            if !seen.insert(block.path) {
                return Err(format!("duplicate block {}", block.path));
            }
            if !block.parent_path().is_empty() && self.block(block.parent_path()).is_none() {
                return Err(format!("block {} has no parent", block.path));
            }
            // A labelled block is at the top, or directly inside a labelled block at the top (6.12).
            if block.labelled && !block.parent_path().is_empty() {
                let parent = block.parent_path();
                let parent_is_top_labelled = !parent.contains('.') && self.block(parent).is_some_and(|p| p.labelled);
                if !parent_is_top_labelled {
                    return Err(format!(
                        "labelled block {} must be at the top or directly inside a labelled block at the top",
                        block.path
                    ));
                }
            }
            self.validate_applies(block.path, &block.applies, block.necessary_text, block.forbidden_text)?;
        }
        Ok(())
    }

    fn validate_applies(&self, path: &str, applies: &Applies, nec: &str, forb: &str) -> Result<(), String> {
        match applies {
            Applies::When(terms) | Applies::WhenOptional(terms) => {
                for t in *terms {
                    let Some(k) = self.key(t.key) else {
                        return Err(format!("{path}: the term key {} does not exist", t.key));
                    };
                    let ok = match k.kind {
                        Kind::Bool => t.equals == "true" || t.equals == "false",
                        Kind::Enum(values) => values.contains(&t.equals),
                        Kind::Text { .. } | Kind::Path => true,
                        _ => false,
                    };
                    if !ok {
                        return Err(format!("{path}: the term on {} has a value that cannot match", t.key));
                    }
                    if self.block_has_labels(k.block_path()) {
                        return Err(format!("{path}: the term key {} is in a labelled block", t.key));
                    }
                }
            }
            Applies::Manual if nec.is_empty() || forb.is_empty() => {
                return Err(format!("{path}: a manual rule needs both texts"));
            }
            _ => {}
        }
        Ok(())
    }

    fn block_has_labels(&self, block_path: &str) -> bool {
        self.block(Self::root_of(block_path)).is_some_and(|b| b.labelled)
    }
}
