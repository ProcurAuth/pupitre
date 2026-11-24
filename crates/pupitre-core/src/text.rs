// SPDX-License-Identifier: MIT OR Apache-2.0
//! The character rules of the free texts (2.1) and the template markers of Ansible (2.12, 7.3).

/// The texts that start a Jinja expression, statement or comment. Ansible evaluates them when
/// a playbook reads a variable (7.3).
pub const TEMPLATE_MARKERS: [&str; 3] = ["{{", "{%", "{#"];

/// The rule of the template markers, for an error message.
pub const NO_TEMPLATE_MARKER: &str = r#"no template marker "{{", "{%" or "{#""#;

/// The first template marker of a text, or `None` (2.12).
pub fn template_marker(text: &str) -> Option<&'static str> {
    text.as_bytes().windows(2).find_map(|pair| match pair {
        b"{{" => Some(TEMPLATE_MARKERS[0]),
        b"{%" => Some(TEMPLATE_MARKERS[1]),
        b"{#" => Some(TEMPLATE_MARKERS[2]),
        _ => None,
    })
}

/// The ranges of the format characters: the Unicode general category Cf (Unicode 15.1), and
/// the whole block U+2060 to U+206F. The bidirectional controls U+061C, U+200E, U+200F,
/// U+202A to U+202E and U+2066 to U+2069 are in these ranges.
const FORMAT_RANGES: [(u32, u32); 20] = [
    (0x00AD, 0x00AD),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

/// True for a format character (2.1): the Unicode category Cf, for example a bidirectional
/// control or a zero-width space.
pub fn is_format(c: char) -> bool {
    let code = u32::from(c);
    FORMAT_RANGES.iter().any(|&(low, high)| (low..=high).contains(&code))
}

/// True when a free text of 2.1 holds a character that a person cannot see: a control
/// character other than the ones in `allowed`, or a format character.
pub fn has_hidden(text: &str, allowed: &[char]) -> bool {
    text.chars().any(|c| (c.is_control() && !allowed.contains(&c)) || is_format(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_each_template_marker() {
        assert_eq!(template_marker("{{ lookup('pipe', 'id') }}"), Some("{{"));
        assert_eq!(template_marker("a {% if x %}"), Some("{%"));
        assert_eq!(template_marker("{# note #}"), Some("{#"));
        assert_eq!(template_marker("x {% y {{"), Some("{%"));
        for clean in ["", "{", "}}", "{ {", "%}", "#}", "{x}", "${HOME}", "a{b}c"] {
            assert_eq!(template_marker(clean), None, "{clean}");
        }
    }

    #[test]
    fn the_bidirectional_controls_and_the_category_cf_are_format_characters() {
        let bidi = ['\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}'];
        let isolates = ['\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'];
        let others =
            ['\u{00AD}', '\u{200B}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{E0001}', '\u{E007F}', '\u{1D173}'];
        for c in bidi.iter().chain(&isolates).chain(&others) {
            assert!(is_format(*c), "U+{:04X}", u32::from(*c));
        }
        for c in ['a', 'é', ' ', '\u{00A0}', '\u{2028}', '日', '\u{1F600}', '\u{0590}', '\u{05D0}'] {
            assert!(!is_format(c), "U+{:04X}", u32::from(c));
        }
    }

    #[test]
    fn a_hidden_character_is_found() {
        assert!(!has_hidden("Production", &[]));
        assert!(!has_hidden("שלום עולם", &[]));
        assert!(has_hidden("admin\u{202E}txt.exe", &[]));
        assert!(has_hidden("a\nb", &[]));
        assert!(!has_hidden("a\nb", &['\n']));
        assert!(has_hidden("a\u{2066}b", &['\n']));
        assert!(has_hidden("a\u{7}b", &['\n']));
    }
}
