// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parser of the HCL subset of 6.3: blocks, labelled blocks, attributes and literal values.
//!
//! The parser keeps the line of each block and each attribute. It does not refuse an
//! expression at parse time: it keeps it as `Literal::Other`, so that the check of the key
//! gives the error "write a literal value" with the key path (6.3).

use crate::error::ConfigError;
use crate::value::{block_display, json_string};

/// The deepest nesting of brackets, braces and parentheses (6.25).
pub const MAX_NESTING: usize = 32;

/// A value as the file gives it.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// A text in double quotes, with its escapes resolved.
    Str(String),
    /// A number, as written. The check of the key decides whether it is a whole number.
    Number(String),
    /// `true` or `false`.
    Bool(bool),
    /// A list.
    List(Vec<Literal>),
    /// Something that is not a literal: an expression, a function call, a variable, a
    /// template, a heredoc, an object or `null`.
    Other {
        /// The kind, for the message: for example `function call`.
        what: &'static str,
        /// The source text, as far as the parser read it.
        text: String,
    },
}

impl Literal {
    /// The text that an error shows after `found`.
    pub fn describe(&self) -> String {
        match self {
            Literal::Str(s) => json_string(s),
            Literal::Number(n) => n.clone(),
            Literal::Bool(b) => b.to_string(),
            Literal::List(items) => {
                let parts: Vec<String> = items.iter().map(Literal::describe).collect();
                format!("[{}]", parts.join(", "))
            }
            Literal::Other { what, text } => format!("{what}: {text}"),
        }
    }
}

/// An attribute `name = value`.
#[derive(Debug, Clone)]
pub struct Attr {
    /// The name of the attribute.
    pub name: String,
    /// The value.
    pub value: Literal,
    /// The line of the name, from 1.
    pub line: u32,
}

/// A block, with its labels and its body.
#[derive(Debug, Clone)]
pub struct BlockNode {
    /// The type of the block, for example `clickhouse`.
    pub name: String,
    /// The labels of the block, in order.
    pub labels: Vec<String>,
    /// The line of the block type, from 1.
    pub line: u32,
    /// The attributes and the blocks inside.
    pub body: Body,
}

/// The content of a file or of a block.
#[derive(Debug, Clone, Default)]
pub struct Body {
    /// The attributes in file order.
    pub attrs: Vec<Attr>,
    /// The blocks in file order.
    pub blocks: Vec<BlockNode>,
}

/// Characters that a literal never needs outside quotes (6.3).
fn is_operator(c: char) -> bool {
    matches!(c, '?' | ':' | '+' | '-' | '*' | '/' | '%' | '!' | '<' | '>' | '&' | '|' | '=')
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
    /// Depth of the open blocks.
    block_depth: usize,
    /// Depth of the open lists.
    list_depth: usize,
}

type PResult<T> = Result<T, ConfigError>;

impl Parser {
    fn new(text: &str) -> Self {
        Parser { chars: text.chars().collect(), pos: 0, line: 1, col: 1, block_depth: 0, list_depth: 0 }
    }

    fn mark(&self) -> (usize, u32, u32) {
        (self.pos, self.line, self.col)
    }

    fn reset(&mut self, mark: (usize, u32, u32)) {
        self.pos = mark.0;
        self.line = mark.1;
        self.col = mark.2;
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn error(&self, detail: &str) -> ConfigError {
        ConfigError::file(format!(
            "the configuration file is not valid HCL at line {}, column {}: {}",
            self.line, self.col, detail
        ))
    }

    fn unexpected(&self) -> ConfigError {
        match self.peek() {
            Some(c) => self.error(&format!("unexpected character {c:?}")),
            None => self.error("unexpected end of file"),
        }
    }

    /// Skip spaces, tabs, carriage returns and comments. Skip line ends when `newlines`.
    fn skip_trivia(&mut self, newlines: bool) -> PResult<()> {
        loop {
            match self.peek() {
                Some(' ' | '\t' | '\r') => {
                    self.bump();
                }
                Some('\n') if newlines => {
                    self.bump();
                }
                Some('#') => self.skip_line_comment(),
                Some('/') if self.peek_at(1) == Some('/') => self.skip_line_comment(),
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.bump();
                    self.bump();
                    loop {
                        match self.peek() {
                            None => return Err(self.error("a comment that starts with /* has no end")),
                            Some('*') if self.peek_at(1) == Some('/') => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            Some(_) => {
                                self.bump();
                            }
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    fn read_ident(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if is_ident_char(c) {
                out.push(c);
                self.bump();
            } else {
                break;
            }
        }
        out
    }

    /// Parse the body of a file (`top`) or of a block. `stack` holds the display of the
    /// parent blocks, for the key path of a duplicate attribute.
    fn parse_body(&mut self, depth: usize, stack: &mut Vec<String>) -> PResult<Body> {
        let top = depth == 0;
        let mut body = Body::default();
        loop {
            self.skip_trivia(true)?;
            match self.peek() {
                None if top => return Ok(body),
                None => return Err(self.error("unexpected end of file; a block has no closing brace")),
                Some('}') if top => return Err(self.error("unexpected character '}'")),
                Some('}') => {
                    self.bump();
                    return Ok(body);
                }
                Some(c) if is_ident_start(c) => self.parse_item(depth, stack, &mut body)?,
                Some(_) => return Err(self.unexpected()),
            }
        }
    }

    fn parse_item(&mut self, depth: usize, stack: &mut Vec<String>, body: &mut Body) -> PResult<()> {
        let line = self.line;
        let name = self.read_ident();
        self.skip_trivia(false)?;
        match self.peek() {
            Some('=') if self.peek_at(1) != Some('=') => {
                self.bump();
                let value = self.parse_value(false)?;
                self.end_of_statement()?;
                if let Some(first) = body.attrs.iter().find(|a| a.name == name) {
                    let key = if stack.is_empty() { name.clone() } else { format!("{}.{}", stack.join("."), name) };
                    return Err(ConfigError::at(
                        key,
                        format!("duplicate attribute; the first one is at line {}", first.line),
                        line,
                    ));
                }
                body.attrs.push(Attr { name, value, line });
                Ok(())
            }
            Some('"' | '{') => {
                let mut labels = Vec::new();
                while self.peek() == Some('"') {
                    let (text, template) = self.parse_string()?;
                    if template {
                        return Err(self.error("a label is a plain text"));
                    }
                    labels.push(text);
                    self.skip_trivia(false)?;
                }
                if self.peek() != Some('{') {
                    return Err(self.unexpected());
                }
                if depth + 1 > MAX_NESTING {
                    return Err(self.error(&format!("nesting deeper than {MAX_NESTING}")));
                }
                self.bump();
                stack.push(block_display(&name, &labels));
                self.block_depth = depth + 1;
                let inner = self.parse_body(depth + 1, stack)?;
                self.block_depth = depth;
                stack.pop();
                self.end_of_statement()?;
                body.blocks.push(BlockNode { name, labels, line, body: inner });
                Ok(())
            }
            _ => Err(self.error("expected `=`, a label or `{` after a name")),
        }
    }

    /// After an attribute or a block: a line end, the end of the file or the end of the block.
    fn end_of_statement(&mut self) -> PResult<()> {
        self.skip_trivia(false)?;
        match self.peek() {
            None | Some('\n' | '}') => Ok(()),
            Some(_) => Err(self.error("expected the end of the line")),
        }
    }

    /// Parse a text in double quotes. Give the text and whether it holds `${` or `%{`.
    fn parse_string(&mut self) -> PResult<(String, bool)> {
        self.bump(); // the opening quote
        let mut out = String::new();
        let mut template = false;
        loop {
            match self.peek() {
                None | Some('\n') => return Err(self.error("a text ends on its line; the closing quote is missing")),
                Some('"') => {
                    self.bump();
                    return Ok((out, template));
                }
                Some('\\') => {
                    self.bump();
                    match self.bump() {
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('n') => out.push('\n'),
                        Some('r') => out.push('\r'),
                        Some('t') => out.push('\t'),
                        Some('u') => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                match self.peek().and_then(|c| c.to_digit(16)) {
                                    Some(d) => {
                                        code = code * 16 + d;
                                        self.bump();
                                    }
                                    None => return Err(self.error("the escape \\u needs four hexadecimal digits")),
                                }
                            }
                            match char::from_u32(code) {
                                Some(c) => out.push(c),
                                None => return Err(self.error("the escape \\u is not a valid character")),
                            }
                        }
                        _ => return Err(self.error("unknown escape in a text")),
                    }
                }
                Some(c) => {
                    if (c == '$' || c == '%') && self.peek_at(1) == Some('{') {
                        template = true;
                    }
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn text_between(&self, start: usize, end: usize) -> String {
        self.chars[start..end].iter().collect::<String>().trim().to_string()
    }

    /// Parse a value. `in_list` makes `,` and `]` end the value.
    fn parse_value(&mut self, in_list: bool) -> PResult<Literal> {
        self.skip_trivia(in_list)?;
        let start_mark = self.mark();
        let start = self.pos;
        let literal = match self.peek() {
            None | Some('\n' | '}') => return Err(self.error("expected a value")),
            Some('"') => {
                let (text, template) = self.parse_string()?;
                if template {
                    Literal::Other { what: "template", text: self.text_between(start, self.pos) }
                } else {
                    Literal::Str(text)
                }
            }
            Some('[') => return self.parse_list(),
            Some('{') => {
                let text = self.read_span(false, in_list)?;
                return Ok(Literal::Other { what: "object", text });
            }
            Some('<') if self.peek_at(1) == Some('<') => return self.parse_heredoc(),
            Some(c)
                if c.is_ascii_digit()
                    || ((c == '+' || c == '-') && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                self.parse_number()
            }
            Some(c) if is_ident_start(c) => {
                let word = self.read_ident();
                if (word == "true" || word == "false") && self.at_value_end(in_list) {
                    Literal::Bool(word == "true")
                } else {
                    self.reset(start_mark);
                    let text = self.read_span(true, in_list)?;
                    return Ok(classify_other(&text));
                }
            }
            Some(c) if is_operator(c) => {
                let text = self.read_span(false, in_list)?;
                return Ok(Literal::Other { what: "expression", text });
            }
            Some(_) => return Err(self.unexpected()),
        };
        // An operator after a literal makes an expression, for example `1 + 2`.
        self.skip_trivia(false)?;
        match self.peek() {
            Some(c) if is_operator(c) || c == '.' || c == '(' || c == '[' => {
                self.reset(start_mark);
                let text = self.read_span(false, in_list)?;
                Ok(Literal::Other { what: "expression", text })
            }
            _ => Ok(literal),
        }
    }

    fn at_value_end(&mut self, in_list: bool) -> bool {
        let mut i = 0;
        loop {
            match self.peek_at(i) {
                None | Some('\n' | '\r' | '}' | '#') => return true,
                Some(',' | ']') => return in_list,
                Some(' ' | '\t') => i += 1,
                Some('/') => return matches!(self.peek_at(i + 1), Some('/' | '*')),
                Some(_) => return false,
            }
        }
    }

    fn parse_number(&mut self) -> Literal {
        let mut text = String::new();
        if let Some(c @ ('+' | '-')) = self.peek() {
            text.push(c);
            self.bump();
        }
        while let Some(c) = self.peek() {
            let exponent_sign = (c == '+' || c == '-') && text.ends_with(['e', 'E']);
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || exponent_sign {
                text.push(c);
                self.bump();
            } else {
                break;
            }
        }
        Literal::Number(text)
    }

    fn parse_list(&mut self) -> PResult<Literal> {
        self.bump(); // [
        if self.block_depth + self.list_depth + 1 > MAX_NESTING {
            return Err(self.error(&format!("nesting deeper than {MAX_NESTING}")));
        }
        self.list_depth += 1;
        let result = self.parse_list_items();
        self.list_depth -= 1;
        result
    }

    fn parse_list_items(&mut self) -> PResult<Literal> {
        let mut items = Vec::new();
        loop {
            self.skip_trivia(true)?;
            match self.peek() {
                None => return Err(self.error("a list has no closing bracket")),
                Some(']') => {
                    self.bump();
                    return Ok(Literal::List(items));
                }
                Some(_) => {
                    items.push(self.parse_value(true)?);
                    self.skip_trivia(true)?;
                    match self.peek() {
                        Some(',') => {
                            self.bump();
                        }
                        Some(']') => {}
                        _ => return Err(self.error("expected `,` or `]` in a list")),
                    }
                }
            }
        }
    }

    fn parse_heredoc(&mut self) -> PResult<Literal> {
        self.bump();
        self.bump();
        if self.peek() == Some('-') {
            self.bump();
        }
        let delimiter = self.read_ident();
        if delimiter.is_empty() {
            return Err(self.error("a heredoc needs a delimiter"));
        }
        // Read to the end of the line, then to the closing delimiter.
        self.skip_line_comment_raw();
        loop {
            if self.peek().is_none() {
                return Err(self.error("a heredoc has no closing delimiter"));
            }
            self.bump(); // the line end
            let mut line = String::new();
            while let Some(c) = self.peek() {
                if c == '\n' {
                    break;
                }
                line.push(c);
                self.bump();
            }
            if line.trim() == delimiter {
                return Ok(Literal::Other { what: "heredoc", text: format!("<<{delimiter}") });
            }
        }
    }

    fn skip_line_comment_raw(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// Read the source text of an expression: to the end of the line, a comment, or the
    /// end of the block (or `,` and `]` in a list), outside nested brackets and quotes. When
    /// `word_start` is true, an operator outside brackets is a syntax error (6.3).
    fn read_span(&mut self, word_start: bool, in_list: bool) -> PResult<String> {
        let start = self.pos;
        let mut stack: Vec<char> = Vec::new();
        let mut prev_space = false;
        while let Some(c) = self.peek() {
            if stack.is_empty() {
                match c {
                    '\n' | '#' => break,
                    '}' => break,
                    ',' | ']' if in_list => break,
                    '/' if matches!(self.peek_at(1), Some('/' | '*')) => break,
                    _ => {}
                }
                if word_start && is_operator(c) && (c != '-' || prev_space) {
                    return Err(self.unexpected());
                }
            }
            match c {
                '(' | '[' | '{' => {
                    if stack.len() >= MAX_NESTING {
                        return Err(self.error(&format!("nesting deeper than {MAX_NESTING}")));
                    }
                    stack.push(c);
                    self.bump();
                }
                ')' | ']' | '}' => {
                    let open = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    if stack.pop() != Some(open) {
                        return Err(self.unexpected());
                    }
                    self.bump();
                }
                '"' => {
                    self.parse_string()?;
                }
                _ => {
                    prev_space = c == ' ' || c == '\t';
                    self.bump();
                    continue;
                }
            }
            prev_space = false;
        }
        if !stack.is_empty() {
            return Err(self.error("a bracket has no closing bracket"));
        }
        Ok(self.text_between(start, self.pos))
    }
}

/// Name the kind of a span that is not a literal.
fn classify_other(text: &str) -> Literal {
    let ident_end = text.find(|c: char| !(is_ident_char(c) || c == '.')).unwrap_or(text.len());
    let (head, rest) = text.split_at(ident_end);
    let what = if text == "null" {
        "null"
    } else if rest.trim_start().starts_with('(') {
        "function call"
    } else if rest.is_empty() && !head.is_empty() {
        "variable"
    } else {
        "expression"
    };
    Literal::Other { what, text: text.to_string() }
}

/// Parse a file. The error is a syntax error or a duplicate attribute: the check stops at
/// once with this one error (6.3).
pub fn parse(text: &str) -> Result<Body, ConfigError> {
    let mut parser = Parser::new(text);
    let mut stack = Vec::new();
    parser.parse_body(0, &mut stack)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr<'a>(body: &'a Body, name: &str) -> &'a Attr {
        body.attrs.iter().find(|a| a.name == name).unwrap()
    }

    fn block_value(src: &str) -> Literal {
        let body = parse(&format!("b {{\n  a = {src}\n}}\n")).unwrap();
        body.blocks[0].body.attrs[0].value.clone()
    }

    #[test]
    fn parses_blocks_labels_and_attributes_with_lines() {
        let src = "# c\nserver {\n  listen = \"x\"\n  n = 5\n  on = true\n}\n\nclickhouse \"prod\" {\n  label = \"P\"\n  tls {\n    a = 1\n  }\n}\n";
        let body = parse(src).unwrap();
        assert_eq!(body.blocks.len(), 2);
        let server = &body.blocks[0];
        assert_eq!((server.name.as_str(), server.line), ("server", 2));
        assert_eq!(attr(&server.body, "listen").line, 3);
        assert_eq!(attr(&server.body, "n").value, Literal::Number("5".into()));
        assert_eq!(attr(&server.body, "on").value, Literal::Bool(true));
        let ch = &body.blocks[1];
        assert_eq!(ch.labels, vec!["prod".to_string()]);
        assert_eq!(ch.line, 8);
        assert_eq!(ch.body.blocks[0].line, 10);
    }

    #[test]
    fn comments_of_three_kinds() {
        let src = "a { # one\n // two\n /* three\n lines */ x = 1 # end\n}\n";
        let body = parse(src).unwrap();
        assert_eq!(body.blocks[0].body.attrs[0].line, 4);
    }

    #[test]
    fn string_escapes() {
        let v = block_value(r#""a\"b\\c\n\r\t\u00e9""#);
        assert_eq!(v, Literal::Str("a\"b\\c\n\r\t\u{e9}".to_string()));
    }

    #[test]
    fn bad_escape_and_line_end_in_text_are_syntax_errors() {
        assert!(parse("a {\n x = \"\\q\"\n}\n").unwrap_err().message.contains("not valid HCL at line 2"));
        assert!(parse("a {\n x = \"abc\n\"\n}\n").unwrap_err().message.contains("ends on its line"));
        assert!(parse("a {\n x = \"\\u12\"\n}\n").is_err());
    }

    #[test]
    fn lists_parse() {
        assert_eq!(
            block_value("[\"a\", \"b\",\n 1, true,]"),
            Literal::List(vec![
                Literal::Str("a".into()),
                Literal::Str("b".into()),
                Literal::Number("1".into()),
                Literal::Bool(true)
            ])
        );
        assert_eq!(block_value("[]"), Literal::List(vec![]));
    }

    #[test]
    fn numbers_keep_their_text() {
        for n in ["16", "1.0", "1e3", "-1", "+16", "007", "0"] {
            assert_eq!(block_value(n), Literal::Number(n.to_string()), "{n}");
        }
    }

    #[test]
    fn non_literals_are_kept_with_their_kind() {
        let cases = [
            ("1 + 2", "expression"),
            ("\"a\" + \"b\"", "expression"),
            ("upper(\"x\")", "function call"),
            ("var.x", "variable"),
            ("yes", "variable"),
            ("null", "null"),
            ("{ a = 1 }", "object"),
            ("\"${x}\"", "template"),
            ("\"%{if x}a%{endif}\"", "template"),
            ("-x", "expression"),
            ("!true", "expression"),
            ("a.b[0]", "expression"),
        ];
        for (src, what) in cases {
            match block_value(src) {
                Literal::Other { what: w, .. } => assert_eq!(w, what, "{src}"),
                other => panic!("{src}: {other:?}"),
            }
        }
    }

    #[test]
    fn heredoc_is_kept() {
        let body = parse("b {\n a = <<EOT\nline\nEOT\n b = 1\n}\n").unwrap();
        let b = &body.blocks[0].body;
        assert!(matches!(&b.attrs[0].value, Literal::Other { what: "heredoc", .. }));
        assert_eq!(b.attrs[1].line, 5);
    }

    #[test]
    fn unquoted_word_with_operator_character_is_a_syntax_error() {
        let e = parse("b {\n a = http://x\n}\n").unwrap_err();
        assert!(e.message.starts_with("the configuration file is not valid HCL at line 2, column"), "{e}");
    }

    #[test]
    fn syntax_errors_have_line_and_column() {
        let e = parse("server {\n  listen = \n}\n").unwrap_err();
        assert!(e.message.contains("line 2"), "{e}");
        let e = parse("server {\n  x = 1\n").unwrap_err();
        assert!(e.message.contains("closing brace"), "{e}");
        let e = parse("}\n").unwrap_err();
        assert!(e.message.contains("line 1, column 1"), "{e}");
        let e = parse("a = 1 b = 2\n").unwrap_err();
        assert!(e.message.contains("end of the line"), "{e}");
        let e = parse("/* open\n").unwrap_err();
        assert!(e.message.contains("no end"), "{e}");
        assert!(parse("a { x = [1, 2 }\n").is_err());
        assert!(parse("a 5\n").is_err());
        assert!(parse("a \"x\" \"y\"\n").is_err());
    }

    #[test]
    fn duplicate_attribute_stops_with_a_key_path() {
        let e = parse("clickhouse \"p\" {\n  tls {\n    a = 1\n    a = 2\n  }\n}\n").unwrap_err();
        assert_eq!(e.to_string(), "clickhouse \"p\".tls.a: duplicate attribute; the first one is at line 3 (line 4)");
        let e = parse("a = 1\na = 2\n").unwrap_err();
        assert_eq!(e.key, "a");
    }

    #[test]
    fn nesting_limit_is_32() {
        let ok = format!("{}{}", "a {".repeat(32), "}".repeat(32));
        assert!(parse(&ok).is_ok());
        let bad = format!("{}{}", "a {".repeat(33), "}".repeat(33));
        assert!(parse(&bad).unwrap_err().message.contains("nesting deeper than 32"));
        let deep_list = format!("a {{ x = {}{} }}", "[".repeat(40), "]".repeat(40));
        assert!(parse(&deep_list).unwrap_err().message.contains("nesting"));
    }

    #[test]
    fn crlf_files_parse() {
        let body = parse("a {\r\n  x = 1\r\n}\r\n").unwrap();
        assert_eq!(body.blocks[0].body.attrs[0].line, 2);
    }

    #[test]
    fn one_line_block_parses() {
        let body = parse("a { x = 1 }\n").unwrap();
        assert_eq!(body.blocks[0].body.attrs.len(), 1);
    }

    #[test]
    fn describe_forms() {
        assert_eq!(Literal::Str("a".into()).describe(), "\"a\"");
        assert_eq!(Literal::Other { what: "function call", text: "f(1)".into() }.describe(), "function call: f(1)");
    }
}
