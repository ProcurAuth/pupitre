// SPDX-License-Identifier: MIT OR Apache-2.0
//! The scrubber of 4.21: it replaces each copy of each secret value of a run with `[secret]`.

use pupitre_core::json::string_form;

/// The text that replaces a value.
pub const REPLACEMENT: &[u8] = b"[secret]";

/// The values of one run, in the forms of 4.21. The first form is the raw bytes. The second form
/// is the JSON string form without its quotes, for the event lines. The next forms are the texts
/// that an event can hold when a value goes through two layers of escapes: the JSON form of the
/// JSON form (a `diff` that the plugin made with `json.dumps`), and the Python `repr` form of
/// a text that Python makes with `str()` of a collection, with the JSON form of it.
#[derive(Clone, Default)]
pub struct Scrubber {
    patterns: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Scrubber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Scrubber({} patterns)", self.patterns.len())
    }
}

/// The JSON string form of a text, without its quotes.
fn json_inner(text: &str) -> String {
    let quoted = string_form(text);
    quoted[1..quoted.len() - 1].to_string()
}

/// The inner text of the Python `repr` of a string. `quote` is true when Python uses single
/// quotes and so escapes `'`. Python uses them when the text holds both kinds of quote, or
/// none. When only `'` occurs, Python writes the text with double quotes and escapes nothing.
fn python_inner(text: &str, quote: bool) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' if quote => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || c == '\u{a0}' || c == '\u{ad}' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The forms of a text value that can be in an output or an event line, other than the raw
/// form: the JSON form, the JSON form of the JSON form, the two `repr` forms of Python and the
/// JSON form of each of them.
fn forms(text: &str) -> Vec<String> {
    let json = json_inner(text);
    let mut out = vec![json_inner(&json), json];
    for quote in [false, true] {
        let repr = python_inner(text, quote);
        out.push(json_inner(&repr));
        out.push(repr);
    }
    out.retain(|form| form != text);
    out.sort();
    out.dedup();
    out
}

impl Scrubber {
    /// Make a scrubber from the secret values of a run. A value of the run has 8 bytes or more
    /// (2.25).
    pub fn new<'a, I: IntoIterator<Item = &'a [u8]>>(values: I) -> Scrubber {
        let mut patterns: Vec<Vec<u8>> = Vec::new();
        for value in values {
            if value.is_empty() {
                continue;
            }
            patterns.push(value.to_vec());
            if let Ok(text) = std::str::from_utf8(value) {
                for form in forms(text) {
                    patterns.push(form.into_bytes());
                }
            }
        }
        patterns.retain(|p| !p.is_empty());
        // The longest first, so that a value inside another value never leaves a part.
        patterns.sort_by_key(|p| std::cmp::Reverse(p.len()));
        patterns.dedup();
        Scrubber { patterns }
    }

    /// The length of the longest pattern. A stream keeps this many bytes minus one between two
    /// chunks (4.21).
    pub fn longest(&self) -> usize {
        self.patterns.first().map_or(0, Vec::len)
    }

    /// Replace each copy of each pattern in a complete buffer.
    pub fn scrub(&self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        let mut covered = 0;
        for i in 0..input.len() {
            self.step(input, i, &mut covered, &mut out);
        }
        out
    }

    /// The length of the longest pattern that starts at `at`, or 0.
    fn match_at(&self, buf: &[u8], at: usize) -> usize {
        // The patterns are sorted by length, the longest first.
        self.patterns.iter().find(|p| buf[at..].starts_with(p)).map_or(0, Vec::len)
    }

    /// Decide the byte at `at`. `covered` is the count of bytes from `at` that a copy of a
    /// value covers. Copies that overlap give one `[secret]`, so that no part of a value stays.
    /// Copies that touch with no overlap give one `[secret]` each.
    fn step(&self, buf: &[u8], at: usize, covered: &mut usize, out: &mut Vec<u8>) {
        let found = self.match_at(buf, at);
        if found > 0 {
            if *covered == 0 {
                out.extend_from_slice(REPLACEMENT);
            }
            *covered = (*covered).max(found);
        }
        if *covered > 0 {
            *covered -= 1;
        } else {
            out.push(buf[at]);
        }
    }
}

/// A scrubber for a stream of chunks (4.21). It keeps the last `longest() - 1` bytes between
/// two chunks, so that a value across two chunks is also replaced. The output never depends
/// on the cut of the chunks.
#[derive(Debug, Clone)]
pub struct StreamScrubber {
    scrubber: Scrubber,
    held: Vec<u8>,
    total_in: u64,
    /// The count of bytes from the first held byte that a copy of a value covers.
    covered: usize,
}

impl StreamScrubber {
    /// Start a stream with no byte held.
    pub fn new(scrubber: Scrubber) -> StreamScrubber {
        StreamScrubber { scrubber, held: Vec::new(), total_in: 0, covered: 0 }
    }

    /// Add a chunk. The answer holds the bytes that no value can still change, scrubbed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.total_in += chunk.len() as u64;
        let longest = self.scrubber.longest();
        if longest == 0 {
            return chunk.to_vec();
        }
        let mut buf = std::mem::take(&mut self.held);
        buf.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(buf.len());
        let mut i = 0;
        // A position is decided when the longest pattern would fit after it.
        while buf.len() - i >= longest {
            self.scrubber.step(&buf, i, &mut self.covered, &mut out);
            i += 1;
        }
        self.held = buf[i..].to_vec();
        out
    }

    /// End the stream: scrub and give the bytes that the stream still holds.
    pub fn finish(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.held);
        let mut out = Vec::with_capacity(rest.len());
        for i in 0..rest.len() {
            self.scrubber.step(&rest, i, &mut self.covered, &mut out);
        }
        out
    }

    /// The count of input bytes that the output already covers. The bytes after this count
    /// are held, or not read yet.
    pub fn decided(&self) -> u64 {
        self.total_in - self.held.len() as u64
    }

    /// The count of bytes that the stream holds.
    pub fn held(&self) -> usize {
        self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_raw_and_the_json_forms() {
        let s = Scrubber::new([b"pa\"ss word".as_slice(), b"hunter22".as_slice()]);
        assert_eq!(s.scrub(b"x pa\"ss word y"), b"x [secret] y".to_vec());
        assert_eq!(s.scrub(br#"{"msg":"pa\"ss word"}"#), br#"{"msg":"[secret]"}"#.to_vec());
        assert_eq!(s.scrub(b"hunter22hunter22"), b"[secret][secret]".to_vec());
        assert_eq!(s.scrub(b"nothing here"), b"nothing here".to_vec());
        // The longest pattern is the JSON form of the JSON form: `pa\\\"ss word`.
        assert_eq!(s.longest(), 13);
        // The value `hunter22` has one pattern. The value `pa"ss word` has its raw form and its other forms.
        assert!(format!("{s:?}").starts_with("Scrubber("));
    }

    fn stream(s: &Scrubber, input: &[u8], cuts: &[usize]) -> Vec<u8> {
        let mut st = StreamScrubber::new(s.clone());
        let mut out = Vec::new();
        let mut last = 0;
        for &c in cuts {
            out.extend(st.push(&input[last..c]));
            assert!(st.held() < s.longest().max(1), "the stream holds at most longest - 1 bytes");
            last = c;
        }
        out.extend(st.push(&input[last..]));
        out.extend(st.finish());
        out
    }

    #[test]
    fn a_value_across_two_chunks_is_replaced() {
        let s = Scrubber::new([b"hunter22".as_slice()]);
        let input = b"before hunter22 after";
        for cut in 0..=input.len() {
            assert_eq!(stream(&s, input, &[cut]), b"before [secret] after".to_vec(), "cut at {cut}");
        }
        // One byte at a time.
        let all: Vec<usize> = (1..input.len()).collect();
        assert_eq!(stream(&s, input, &all), b"before [secret] after".to_vec());
    }

    #[test]
    fn the_output_does_not_depend_on_the_cut() {
        let s = Scrubber::new([b"pa\"ss word".as_slice(), b"hunter22".as_slice(), b"hunter2".as_slice()]);
        let input = b"x hunter22hunter22 pa\"ss word hunter2 hunter hunter22".to_vec();
        let whole = s.scrub(&input);
        // A cheap generator of cuts: each step of 1 to 5 bytes.
        let mut seed = 7u32;
        for _ in 0..200 {
            let mut cuts = Vec::new();
            let mut at = 0usize;
            while at < input.len() {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                at += 1 + (seed >> 16) as usize % 5;
                if at < input.len() {
                    cuts.push(at);
                }
            }
            assert_eq!(stream(&s, &input, &cuts), whole);
        }
    }

    /// 4.21: two values that overlap in the output leave no part of either value. The result
    /// does not depend on the cut of the chunks.
    #[test]
    fn overlapping_values_leave_no_part() {
        let s = Scrubber::new([b"abcdefgh".as_slice(), b"efghijkl".as_slice()]);
        let input = b"x abcdefghijkl y efghijkl abcdefgh";
        let expected = b"x [secret] y [secret] [secret]".to_vec();
        assert_eq!(s.scrub(input), expected);
        for cut in 0..=input.len() {
            assert_eq!(stream(&s, input, &[cut]), expected, "cut at {cut}");
        }
        let all: Vec<usize> = (1..input.len()).collect();
        assert_eq!(stream(&s, input, &all), expected);
        // A value that overlaps itself, and a chain of three values.
        let s = Scrubber::new([b"aaaaaaaa".as_slice()]);
        assert_eq!(s.scrub(b"-aaaaaaaaaaa-"), b"-[secret]-".to_vec());
        let s = Scrubber::new([b"11112222".as_slice(), b"22223333".as_slice(), b"33334444".as_slice()]);
        assert_eq!(s.scrub(b"<1111222233334444>"), b"<[secret]>".to_vec());
        assert_eq!(stream(&s, b"<1111222233334444>", &[3, 9, 14]), b"<[secret]>".to_vec());
    }

    #[test]
    fn decided_counts_the_bytes_that_the_output_covers() {
        let s = Scrubber::new([b"hunter22".as_slice()]);
        let mut st = StreamScrubber::new(s);
        let out = st.push(b"abcdefghij");
        assert_eq!(out, b"abc".to_vec());
        assert_eq!(st.decided(), 3);
        assert_eq!(st.held(), 7);
        let out = st.finish();
        assert_eq!(out, b"defghij".to_vec());
    }

    /// A secret with a quote and a backslash, in each text that Ansible and the plugin can make
    /// from it (4.21): the value itself, a diff that the plugin made with `json.dumps` and that the
    /// event line escapes again, and a message that Python made with `str()` of a dictionary. The
    /// output does not depend on the cut of the chunks.
    #[test]
    fn a_value_with_a_quote_and_a_backslash_is_found_after_two_escapes() {
        // `pa"ss\word'x1`
        let secret = b"pa\"ss\\word'x1".as_slice();
        let s = Scrubber::new([secret]);
        let lines: [(&[u8], &[u8]); 4] = [
            // The line of the plugin with the diff of the fallback: json.dumps twice.
            (br#"{"diff":"{\"after\": \"pa\\\"ss\\\\word'x1\"}"}"#, br#"{"diff":"{\"after\": \"[secret]\"}"}"#),
            // The line with a message that str() made from a dictionary: the repr, then json.dumps.
            (br#"{"message":"{'after': 'pa\"ss\\\\word\\'x1'}"}"#, br#"{"message":"{'after': '[secret]'}"}"#),
            // The line with a value of one escape.
            (br#"{"message":"pa\"ss\\word'x1"}"#, br#"{"message":"[secret]"}"#),
            // The raw text.
            (b"x pa\"ss\\word'x1 y", b"x [secret] y"),
        ];
        check_all_cuts(&s, &lines);

        // A value with a single quote and no double quote: Python writes it between double quotes.
        let s = Scrubber::new([b"x\\y'z1234".as_slice()]);
        let lines: [(&[u8], &[u8]); 1] =
            [(br#"{"message":"{'a': \"x\\\\y'z1234\"}"}"#, br#"{"message":"{'a': \"[secret]\"}"}"#)];
        check_all_cuts(&s, &lines);
    }

    /// Each line gives its expected text for the whole buffer, for each cut in two chunks, and byte by byte.
    fn check_all_cuts(s: &Scrubber, lines: &[(&[u8], &[u8])]) {
        for (input, expected) in lines {
            let shown = String::from_utf8_lossy(input);
            assert_eq!(s.scrub(input), expected.to_vec(), "{shown}");
            for cut in 0..=input.len() {
                assert_eq!(stream(s, input, &[cut]), expected.to_vec(), "cut at {cut}: {shown}");
            }
            let all: Vec<usize> = (1..input.len()).collect();
            assert_eq!(stream(s, input, &all), expected.to_vec(), "{shown}");
        }
    }

    #[test]
    fn a_stream_with_no_value_passes_each_byte() {
        let mut st = StreamScrubber::new(Scrubber::default());
        assert_eq!(st.push(b"abc"), b"abc".to_vec());
        assert_eq!(st.finish(), Vec::<u8>::new());
        assert_eq!(st.decided(), 3);
    }
}
