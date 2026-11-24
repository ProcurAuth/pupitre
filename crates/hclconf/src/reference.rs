// SPDX-License-Identifier: MIT OR Apache-2.0
//! Generate the reference files from the key table (6.21): a complete configuration file
//! for a profile, and the reference document in Markdown.

use crate::schema::{Applies, BlockDesc, Forms, KeyDesc, Profile, Ref, Schema};
use crate::value::json_string;

fn ref_value_text(r: &Ref) -> Option<String> {
    match r {
        Ref::Absent => None,
        Ref::Str(s) => Some(json_string(s)),
        Ref::Int(n) => Some(n.to_string()),
        Ref::Bool(b) => Some(b.to_string()),
        Ref::StrList(items) => {
            let parts: Vec<String> = items.iter().map(|i| json_string(i)).collect();
            Some(format!("[{}]", parts.join(", ")))
        }
        Ref::File(_) => None,
    }
}

/// Give the attribute name and the value text that the reference file writes for a key.
/// Give `None` when the key is not in the file of this profile.
fn hcl_attr(key: &KeyDesc, profile: Profile) -> Option<(String, String)> {
    match key.refs[profile.index()] {
        Ref::Absent => None,
        Ref::File(path) => Some((format!("{}_file", key.name()), json_string(path))),
        ref r => ref_value_text(r).map(|v| (key.name().to_string(), v)),
    }
}

fn emit_block(schema: &Schema, desc: &BlockDesc, profile: Profile, depth: usize) -> Option<String> {
    let indent = "  ".repeat(depth);
    let attrs: Vec<(String, String)> = schema.keys_of(desc.path).filter_map(|k| hcl_attr(k, profile)).collect();
    let children: Vec<String> =
        schema.child_blocks(desc.path).filter_map(|c| emit_block(schema, c, profile, depth + 1)).collect();
    if attrs.is_empty() && children.is_empty() {
        return None;
    }
    let mut out = String::new();
    if desc.labelled {
        out.push_str(&format!("{indent}{} {} {{\n", desc.name(), json_string(desc.reference_label)));
    } else {
        out.push_str(&format!("{indent}{} {{\n", desc.name()));
    }
    let width = attrs.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (name, value) in &attrs {
        out.push_str(&format!("{indent}  {name:<width$} = {value}\n"));
    }
    for (i, child) in children.iter().enumerate() {
        if i > 0 || !attrs.is_empty() {
            out.push('\n');
        }
        out.push_str(child);
    }
    out.push_str(&format!("{indent}}}\n"));
    Some(out)
}

/// Write a complete configuration file for a profile. `header` holds the comment lines at
/// the top, without the `# `. The file has each key that has a reference value in the
/// profile, and no other key.
pub fn render_hcl(schema: &Schema, profile: Profile, header: &[&str]) -> String {
    let mut out = String::new();
    for line in header {
        out.push_str("# ");
        out.push_str(line);
        out.push('\n');
    }
    for desc in schema.child_blocks("") {
        if let Some(block) = emit_block(schema, desc, profile, 0) {
            out.push('\n');
            out.push_str(&block);
        }
    }
    out
}

/// The texts of the reference document that the program gives.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownSpec<'a> {
    /// The title of the document.
    pub title: &'a str,
    /// The paragraphs before the tables.
    pub intro: &'a [&'a str],
    /// The names of the two profiles, for the column of reference values.
    pub profile_names: [&'a str; 2],
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// Write a path of the key table with the label placeholders of its labelled blocks, for
/// example `command "<id>".param "<name>".type` (6.12).
fn labelled_path(schema: &Schema, path: &str) -> String {
    let mut out = String::new();
    let mut prefix = String::new();
    for (depth, part) in path.split('.').enumerate() {
        if depth > 0 {
            out.push('.');
            prefix.push('.');
        }
        out.push_str(part);
        prefix.push_str(part);
        if schema.block(&prefix).is_some_and(|b| b.labelled) {
            out.push_str(if depth == 0 { " \"<id>\"" } else { " \"<name>\"" });
        }
    }
    out
}

fn key_display(schema: &Schema, key: &KeyDesc) -> String {
    labelled_path(schema, key.path)
}

fn reference_cell(key: &KeyDesc, names: &[&str; 2]) -> String {
    let show = |r: &Ref| -> String {
        match r {
            Ref::Absent => "absent".to_string(),
            Ref::File(path) => format!("`{}_file = {}`", key.name(), json_string(path)),
            other => format!("`{}`", ref_value_text(other).unwrap_or_default()),
        }
    };
    if key.refs[0] == Ref::Absent && key.refs[1] == Ref::Absent && key.doc_ref != Ref::Absent {
        show(&key.doc_ref)
    } else if key.refs[0] == key.refs[1] {
        show(&key.refs[0])
    } else {
        format!("{} ({}), {} ({})", show(&key.refs[0]), names[0], show(&key.refs[1]), names[1])
    }
}

fn forms_text(forms: Forms) -> &'static str {
    match forms {
        Forms::V => "V",
        Forms::Vef => "V, E, F",
        Forms::Ef => "E, F",
    }
}

/// The path of a block, with the labels of its labelled blocks, for example
/// `clickhouse "<id>".tls`.
fn block_path_display(schema: &Schema, desc: &BlockDesc) -> String {
    labelled_path(schema, desc.path)
}

fn block_heading(schema: &Schema, desc: &BlockDesc) -> String {
    format!("`{}`", block_path_display(schema, desc))
}

/// Write the reference document in Markdown: one table for each block, with one row for
/// each key. The program gives the texts of `spec`.
pub fn render_markdown(schema: &Schema, spec: &MarkdownSpec<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", spec.title));
    for paragraph in spec.intro {
        out.push_str(paragraph);
        out.push_str("\n\n");
    }
    out.push_str("## Blocks\n\n");
    out.push_str("| Block | Count | Necessary when | Forbidden when |\n|---|---|---|---|\n");
    for desc in schema.blocks {
        let count = if desc.labelled {
            format!("{} to {}", desc.min, desc.max)
        } else if matches!(desc.applies, Applies::Always) {
            "1".to_string()
        } else {
            "0 or 1".to_string()
        };
        let path = block_path_display(schema, desc);
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            cell(&path),
            count,
            cell(&schema.necessary_when(&desc.applies, desc.necessary_text)),
            cell(&schema.forbidden_when(&desc.applies, desc.forbidden_text)),
        ));
    }
    out.push('\n');
    for desc in schema.blocks {
        out.push_str(&format!("## Block {}\n\n", block_heading(schema, desc)));
        out.push_str(desc.description);
        out.push_str("\n\n");
        out.push_str(
            "| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |\n",
        );
        out.push_str("|---|---|---|---|---|---|---|---|\n");
        for key in schema.keys_of(desc.path) {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} | {} | {} |\n",
                cell(&key_display(schema, key)),
                cell(&key.kind.expected()),
                if key.secret { "yes" } else { "no" },
                forms_text(key.forms),
                cell(&schema.necessary_when(&key.applies, key.necessary_text)),
                cell(&schema.forbidden_when(&key.applies, key.forbidden_text)),
                cell(&reference_cell(key, &spec.profile_names)),
                cell(key.description),
            ));
        }
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}
