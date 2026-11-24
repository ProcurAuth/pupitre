// SPDX-License-Identifier: MIT OR Apache-2.0
//! The text outputs of the configuration: `print-config` (6.20) and `print-reference` (6.21).

use hclconf::{MarkdownSpec, Profile};

use super::LoadedConfig;
use super::keys::SCHEMA;

/// The format of `print-reference`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceFormat {
    /// A complete configuration file of a profile.
    Hcl,
    /// The reference document `docs/configuration-reference.md`.
    Markdown,
}

/// The effective configuration: one line `<key> = <value>  # <source>` for each key, in the
/// order of the table of keys (6.20).
pub fn render_print_config(loaded: &LoadedConfig) -> String {
    let mut out = String::new();
    for entry in &loaded.effective {
        out.push_str(&format!("{} = {}  # {}\n", entry.key, entry.value, entry.source));
    }
    out
}

const INTRO: [&str; 6] = [
    "This document is the reference of the configuration keys of Pupitre. It is part of section 6 of the specification. The command `pupitre print-reference --format markdown` generates it from the table of keys in the code. Do not edit it by hand.",
    "Pupitre reads one HCL file. The file is the only source of the configuration. A key whose description gives a default can be absent: the default then applies, and `print-config` shows the source `default`. Each other key that applies is necessary, and each key that does not apply is forbidden.",
    "The column Forms uses three letters. V is a value in the file. E is `<name>_env`, the name of an environment variable. F is `<name>_file`, the absolute path of a file. Give exactly one form for each key. A secret accepts only E and F.",
    "A duration is a text with one whole number and one unit: `ms`, `s`, `m` or `h`. A size is a whole number of bytes. An integer is a whole number with no sign and no zero at the start. A text is in double quotes. A list is texts in brackets with commas.",
    "The reference value is the value that the reference files use. It is not a default. Run `pupitre print-reference --format hcl --profile local` or `--profile distributed` to get a complete file. The two files hold no `command` block. The reference value of a key of a `command` block is a value for the example of 4.32.",
    "Section 6 of the specification gives the checks after the table of keys, the contradictions and the warnings.",
];

/// Write a reference. `Hcl` writes the complete file of a profile (6.16, 6.17), and `Markdown`
/// writes the reference document.
pub fn render_reference(format: ReferenceFormat, profile: Profile) -> String {
    match format {
        ReferenceFormat::Hcl => {
            let first = match profile {
                Profile::Local => {
                    "Pupitre configuration, profile local: the backend files, the access mode open, no telemetry."
                }
                Profile::Distributed => {
                    "Pupitre configuration, profile distributed: the backend postgres, the access mode authzen, no telemetry."
                }
            };
            hclconf::render_hcl(
                &SCHEMA,
                profile,
                &[
                    first,
                    "Each necessary key that applies is in the file. A key with a default is absent: its default applies.",
                ],
            )
        }
        ReferenceFormat::Markdown => {
            let spec = MarkdownSpec {
                title: "Pupitre configuration reference",
                intro: &INTRO,
                profile_names: ["local", "distributed"],
            };
            let mut out = hclconf::render_markdown(&SCHEMA, &spec);
            out.push('\n');
            out
        }
    }
}
