// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ids of 2.1. Each type checks its rule at parse and never changes a text to make it fit.

use std::fmt;

/// An id that breaks its rule (2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdError {
    /// The name of the id, for example `an environment id`.
    pub what: &'static str,
    /// The rule, for example `^[a-z0-9][a-z0-9-]{0,62}$`.
    pub rule: &'static str,
    /// The text that the caller gave, as a JSON string.
    pub found: String,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {} that matches {}, found {}", self.what, self.rule, self.found)
    }
}

impl std::error::Error for IdError {}

fn error(what: &'static str, rule: &'static str, text: &str) -> IdError {
    IdError { what, rule, found: crate::json::string_form(text) }
}

/// The rule `^[a-z0-9][a-z0-9-]{0,62}$`.
pub const SLUG_RULE: &str = "^[a-z0-9][a-z0-9-]{0,62}$";

fn is_slug(text: &str) -> bool {
    let b = text.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// True for `^[a-z][a-z0-9_]{0,max-1}$`.
fn is_ident(text: &str, max: usize) -> bool {
    let b = text.as_bytes();
    !b.is_empty()
        && b.len() <= max
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

macro_rules! text_id {
    ($(#[$doc:meta])* $name:ident, $what:expr, $rule:expr, $check:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Check the text against the rule of 2.1.
            pub fn parse(text: &str) -> Result<Self, IdError> {
                let check: fn(&str) -> bool = $check;
                if check(text) { Ok(Self(text.to_string())) } else { Err(error($what, $rule, text)) }
            }

            /// The id as text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

text_id!(
    /// The id of an environment.
    EnvId, "an environment id", SLUG_RULE, is_slug
);
text_id!(
    /// The id of a unit.
    UnitId, "a unit id", SLUG_RULE, is_slug
);
/// The names that Ansible reserves in an inventory: two implicit groups and the implicit
/// host. A host id or a host label is never one of them (2.1, 4.11).
pub const ANSIBLE_RESERVED: [&str; 3] = ["all", "ungrouped", "localhost"];

text_id!(
    /// The id of a host. It is a host name of the inventory (4.11).
    HostId,
    "a host id",
    "^[a-z0-9][a-z0-9-]{0,62}$, not all, ungrouped or localhost",
    |t| is_slug(t) && !ANSIBLE_RESERVED.contains(&t)
);
text_id!(
    /// The id of a block `schema` of the configuration.
    SchemaId, "a schema id", SLUG_RULE, is_slug
);
text_id!(
    /// The id of a block `playbook` of the configuration.
    PlaybookId, "a playbook id", SLUG_RULE, is_slug
);
text_id!(
    /// The id of a block `command` of the configuration.
    CommandId, "a command id", SLUG_RULE, is_slug
);
text_id!(
    /// The name of a parameter of a command.
    ParamName, "a parameter name", "^[a-z][a-z0-9_]{0,31}$", |t| is_ident(t, 32)
);
text_id!(
    /// A host label. It is also an inventory group (4.11).
    HostLabel,
    "a host label",
    "^[a-z][a-z0-9_]{0,62}$, not all, ungrouped, localhost, ansible_* or pupitre_*",
    |t| is_ident(t, 63)
        && !ANSIBLE_RESERVED.contains(&t)
        && !t.starts_with("ansible_")
        && !t.starts_with("pupitre_")
);
/// The names of variables that Ansible reserves. A schema never names its variable with one of
/// them, because the values of the unit would hide the variable of Ansible (2.11).
pub const RESERVED_VARIABLES: [&str; 17] = [
    "environment",
    "group_names",
    "groups",
    "hostvars",
    "inventory_dir",
    "inventory_file",
    "inventory_hostname",
    "inventory_hostname_short",
    "item",
    "omit",
    "play_hosts",
    "playbook_dir",
    "role_name",
    "role_names",
    "role_path",
    "ungrouped",
    "vars",
];

text_id!(
    /// The Ansible variable of a schema (2.11).
    AnsibleVariable,
    "an Ansible variable",
    "^[a-z][a-z0-9_]{0,62}$, not ansible_*, pupitre_* or a name that Ansible reserves (hostvars, groups, vars, ...)",
    |t| is_ident(t, 63)
        && !t.starts_with("ansible_")
        && !t.starts_with("pupitre_")
        && !RESERVED_VARIABLES.contains(&t)
);
text_id!(
    /// The inventory group of a schema (2.11).
    AnsibleGroup,
    "an Ansible group",
    "^[a-z][a-z0-9_]{0,62}$, not all, ungrouped, ansible_* or pupitre_*",
    |t| is_ident(t, 63)
        && t != "all"
        && t != "ungrouped"
        && !t.starts_with("ansible_")
        && !t.starts_with("pupitre_")
);
text_id!(
    /// A key path: 1 to 8 segments of `^[a-z][a-z0-9_]{0,62}$` joined by `.`, 255 bytes at most.
    KeyPath,
    "a key path",
    "1 to 8 segments of ^[a-z][a-z0-9_]{0,62}$ joined by '.', 255 bytes at most",
    |t| t.len() <= 255 && t.split('.').count() <= 8 && t.split('.').all(|s| is_ident(s, 63))
);

impl KeyPath {
    /// The segments of the path.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('.')
    }

    /// True when `self` is `other` or starts `other` at a segment boundary: `a.b` starts
    /// `a.b.c` (2.11).
    pub fn is_prefix_of(&self, other: &KeyPath) -> bool {
        other.0 == self.0 || (other.0.starts_with(&self.0) && other.0.as_bytes().get(self.0.len()) == Some(&b'.'))
    }
}

/// A run id: a UUID version 7 in the form `8-4-4-4-12`, lower-case hexadecimal (2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId([u8; 16]);

/// The rule of a run id.
pub const RUN_ID_RULE: &str = "a UUID version 7 in the form 8-4-4-4-12, lower-case hexadecimal";

impl RunId {
    /// Make a run id from a time in milliseconds since the Unix epoch and 10 random bytes.
    /// The caller gives the random bytes: this crate has no source of randomness.
    pub fn from_parts(unix_ms: u64, random: [u8; 10]) -> RunId {
        let mut b = [0u8; 16];
        let ms = unix_ms.to_be_bytes();
        b[..6].copy_from_slice(&ms[2..]);
        b[6..].copy_from_slice(&random);
        b[6] = 0x70 | (b[6] & 0x0f);
        b[8] = 0x80 | (b[8] & 0x3f);
        RunId(b)
    }

    /// Read the text form of 2.1.
    pub fn parse(text: &str) -> Result<RunId, IdError> {
        let bad = || error("a run id", RUN_ID_RULE, text);
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return Err(bad());
        }
        let mut out = [0u8; 16];
        let mut n = 0;
        let mut i = 0;
        while i < 36 {
            if matches!(i, 8 | 13 | 18 | 23) {
                if bytes[i] != b'-' {
                    return Err(bad());
                }
                i += 1;
                continue;
            }
            let hex = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            let (Some(hi), Some(lo)) = (hex(bytes[i]), hex(bytes[i + 1])) else {
                return Err(bad());
            };
            out[n] = (hi << 4) | lo;
            n += 1;
            i += 2;
        }
        if out[6] >> 4 != 7 || out[8] >> 6 != 0b10 {
            return Err(bad());
        }
        Ok(RunId(out))
    }

    /// The time of the id, in milliseconds since the Unix epoch.
    pub fn unix_ms(&self) -> u64 {
        let mut ms = [0u8; 8];
        ms[2..].copy_from_slice(&self.0[..6]);
        u64::from_be_bytes(ms)
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The subject of a request or of a write (2.18): 1 to 256 characters with no control
/// character and no format character (2.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subject(String);

impl Subject {
    /// The subject of the mode `open` (3.2).
    pub const ANONYMOUS: &'static str = "-";
    /// The subject of a change outside Pupitre (2.33).
    pub const EXTERNAL: &'static str = "external";
    /// The subject of the command `import` (6.24).
    pub const IMPORT: &'static str = "import";

    /// Check the subject.
    pub fn parse(text: &str) -> Result<Subject, IdError> {
        let n = text.chars().count();
        if n == 0 || n > 256 || crate::text::has_hidden(text, &[]) {
            return Err(error(
                "a subject",
                "1 to 256 characters with no control character and no format character",
                text,
            ));
        }
        Ok(Subject(text.to_string()))
    }

    /// The subject `-`.
    pub fn anonymous() -> Subject {
        Subject(Self::ANONYMOUS.to_string())
    }

    /// The subject as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        for good in ["production", "a", "0", "host-a", &"a".repeat(63)] {
            assert!(EnvId::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-a", "A", "a_b", "a.b", "a/b", &"a".repeat(64), "é"] {
            assert!(HostId::parse(bad).is_err(), "{bad}");
        }
        let e = UnitId::parse("Relay").unwrap_err();
        assert_eq!(e.to_string(), "expected a unit id that matches ^[a-z0-9][a-z0-9-]{0,62}$, found \"Relay\"");
    }

    #[test]
    fn key_paths() {
        assert!(KeyPath::parse("limits.max_clients").is_ok());
        assert!(KeyPath::parse("a.b.c.d.e.f.g.h").is_ok());
        assert!(KeyPath::parse("a.b.c.d.e.f.g.h.i").is_err());
        for bad in ["", ".", "a.", ".a", "a..b", "A.b", "1a", "a-b"] {
            assert!(KeyPath::parse(bad).is_err(), "{bad}");
        }
        let a = KeyPath::parse("a.b").unwrap();
        assert!(a.is_prefix_of(&KeyPath::parse("a.b.c").unwrap()));
        assert!(a.is_prefix_of(&KeyPath::parse("a.b").unwrap()));
        assert!(!a.is_prefix_of(&KeyPath::parse("a.bc").unwrap()));
    }

    #[test]
    fn ansible_names() {
        assert!(AnsibleVariable::parse("relay_config").is_ok());
        assert!(AnsibleVariable::parse("pupitre_x").is_err());
        assert!(AnsibleVariable::parse("ansible_host").is_err());
        // The names that Ansible reserves (2.11).
        for reserved in super::RESERVED_VARIABLES {
            assert!(AnsibleVariable::parse(reserved).is_err(), "{reserved}");
        }
        assert!(AnsibleVariable::parse("hostvars_config").is_ok() && AnsibleVariable::parse("vars2").is_ok());
        assert!(AnsibleGroup::parse("all").is_err());
        assert!(AnsibleGroup::parse("ungrouped").is_err());
        assert!(AnsibleGroup::parse("relay_hosts").is_ok());
    }

    /// 2.1, 4.11: a host id or a host label is never a name that Ansible reserves, and a label
    /// never takes the prefix of the groups of Pupitre.
    #[test]
    fn the_names_that_ansible_reserves_are_refused() {
        for reserved in ["all", "ungrouped", "localhost"] {
            assert!(HostId::parse(reserved).is_err(), "{reserved}");
            assert!(HostLabel::parse(reserved).is_err(), "{reserved}");
        }
        for bad in ["pupitre_run", "ansible_host"] {
            assert!(HostLabel::parse(bad).is_err(), "{bad}");
        }
        assert!(HostId::parse("all-hosts").is_ok() && HostId::parse("localhost2").is_ok());
        assert!(HostLabel::parse("canary").is_ok() && HostLabel::parse("all_eu").is_ok());
        assert_eq!(
            HostId::parse("all").unwrap_err().to_string(),
            "expected a host id that matches ^[a-z0-9][a-z0-9-]{0,62}$, not all, ungrouped or localhost, found \"all\""
        );
    }

    #[test]
    fn run_ids_round_trip_and_keep_the_time() {
        let id = RunId::from_parts(1_795_514_400_000, [0xff; 10]);
        let text = id.to_string();
        assert_eq!(text.len(), 36);
        assert_eq!(&text[14..15], "7");
        assert_eq!(RunId::parse(&text).unwrap(), id);
        assert_eq!(id.unix_ms(), 1_795_514_400_000);
        let later = RunId::from_parts(1_795_514_400_001, [0; 10]);
        assert!(later > id);
        assert!(RunId::parse(&text.to_uppercase()).is_err());
        assert!(RunId::parse("0193a1f2-7c4e-4d10-8a3b-5c2e9f0d1a42").is_err());
    }

    #[test]
    fn subjects() {
        assert!(Subject::parse("alice").is_ok());
        assert!(Subject::parse("key:9b1c04e2aa71f0c3").is_ok());
        assert!(Subject::parse("").is_err());
        assert!(Subject::parse("a\nb").is_err());
        assert_eq!(Subject::anonymous().as_str(), "-");
        // 2.1, 2.18: a bidirectional control or another format character is refused.
        for hidden in ["\u{202E}", "\u{2066}", "\u{200F}", "\u{061C}", "\u{200B}", "\u{E0041}"] {
            let e = Subject::parse(&format!("alice{hidden}")).unwrap_err();
            assert!(e.to_string().contains("no format character"), "{e}");
        }
        assert!(Subject::parse("אליס").is_ok());
    }
}
