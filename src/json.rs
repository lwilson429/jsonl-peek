//! A strict JSON reader for a single value: no trailing commas, no single
//! quotes, no bare `NaN`/`Infinity`, no unescaped control characters and no
//! lone UTF-16 surrogates. Every rejection carries the line and column of
//! the offending character so a caller can point at the exact spot in a
//! multi-gigabyte file that broke.
//!
//! This module only knows how to parse one value out of a string; splitting
//! a file into lines is `jsonl_peek::lines`'s job.

use std::fmt;

/// A parsed JSON value.
///
/// Integers and floats are kept distinct: `1` parses to `Int(1)`, `1.0` and
/// `1e0` parse to `Float(1.0)`. An integer literal that does not fit in an
/// `i64` falls back to `Float`, same as an oversized literal would lose
/// precision in any other JSON reader.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    /// Object members in source order. Duplicate keys are kept as separate
    /// entries rather than merged, so a caller that cares can see the
    /// duplication.
    Object(Vec<(String, Value)>),
}

/// A parse failure, with the 1-based line and column of the character that
/// caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {} col {}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parses `input` as a single JSON value. Leading and trailing whitespace is
/// allowed; anything else left over after the value is a trailing-characters
/// error rather than being silently ignored.
pub fn parse(input: &str) -> Result<Value, ParseError> {
    let mut scanner = Scanner::new(input);
    let value = scanner.parse_value()?;
    scanner.skip_whitespace();
    if scanner.peek().is_some() {
        return Err(scanner.error_here("trailing characters after value"));
    }
    Ok(value)
}

struct Scanner<'a> {
    input: &'a str,
    iter: std::iter::Peekable<std::str::CharIndices<'a>>,
    line: usize,
    col: usize,
}

impl<'a> Scanner<'a> {
    fn new(input: &'a str) -> Self {
        Scanner {
            input,
            iter: input.char_indices().peekable(),
            line: 1,
            col: 1,
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.iter.peek().map(|&(_, c)| c)
    }

    fn peek_offset(&mut self) -> Option<usize> {
        self.iter.peek().map(|&(i, _)| i)
    }

    fn bump(&mut self) -> Option<char> {
        let (_, c) = self.iter.next()?;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn error_here(&mut self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            column: self.col,
            message: message.into(),
        }
    }

    fn error_at(&self, line: usize, column: usize, message: impl Into<String>) -> ParseError {
        ParseError {
            line,
            column,
            message: message.into(),
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t') | Some('\n') | Some('\r')) {
            self.bump();
        }
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some('"') => self.parse_string().map(Value::String),
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('t') => self.parse_literal("true", Value::Bool(true)),
            Some('f') => self.parse_literal("false", Value::Bool(false)),
            Some('n') => self.parse_literal("null", Value::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(self.error_here(format!("unexpected character '{c}'"))),
            None => Err(self.error_here("unexpected end of input")),
        }
    }

    fn parse_literal(&mut self, text: &str, value: Value) -> Result<Value, ParseError> {
        let start_line = self.line;
        let start_col = self.col;
        for expected in text.chars() {
            if self.peek() != Some(expected) {
                return Err(self.error_at(start_line, start_col, format!("expected '{text}'")));
            }
            self.bump();
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.peek_offset().expect("caller confirmed a number starts here");
        let start_line = self.line;
        let start_col = self.col;

        if self.peek() == Some('-') {
            self.bump();
        }

        match self.peek() {
            Some('0') => {
                self.bump();
            }
            Some(c) if c.is_ascii_digit() => {
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.bump();
                }
            }
            _ => return Err(self.error_at(start_line, start_col, "invalid number")),
        }

        let mut is_float = false;

        if self.peek() == Some('.') {
            is_float = true;
            self.bump();
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                        self.bump();
                    }
                }
                _ => return Err(self.error_here("expected digit after decimal point")),
            }
        }

        if matches!(self.peek(), Some('e') | Some('E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.bump();
            }
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                        self.bump();
                    }
                }
                _ => return Err(self.error_here("expected digit in exponent")),
            }
        }

        let end = self.peek_offset().unwrap_or(self.input.len());
        let text = &self.input[start..end];

        if is_float {
            text.parse::<f64>()
                .map(Value::Float)
                .map_err(|_| self.error_at(start_line, start_col, "invalid number"))
        } else {
            match text.parse::<i64>() {
                Ok(n) => Ok(Value::Int(n)),
                Err(_) => text
                    .parse::<f64>()
                    .map(Value::Float)
                    .map_err(|_| self.error_at(start_line, start_col, "invalid number")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        let start_line = self.line;
        let start_col = self.col;
        self.bump(); // opening quote

        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error_at(start_line, start_col, "unterminated string")),
                Some('"') => {
                    self.bump();
                    break;
                }
                Some('\\') => {
                    self.bump();
                    match self.peek() {
                        Some('"') => {
                            s.push('"');
                            self.bump();
                        }
                        Some('\\') => {
                            s.push('\\');
                            self.bump();
                        }
                        Some('/') => {
                            s.push('/');
                            self.bump();
                        }
                        Some('b') => {
                            s.push('\u{0008}');
                            self.bump();
                        }
                        Some('f') => {
                            s.push('\u{000C}');
                            self.bump();
                        }
                        Some('n') => {
                            s.push('\n');
                            self.bump();
                        }
                        Some('r') => {
                            s.push('\r');
                            self.bump();
                        }
                        Some('t') => {
                            s.push('\t');
                            self.bump();
                        }
                        Some('u') => {
                            self.bump();
                            s.push(self.read_unicode_escape()?);
                        }
                        Some(_) => return Err(self.error_here("invalid escape sequence")),
                        None => return Err(self.error_here("unterminated escape sequence")),
                    }
                }
                Some(c) if (c as u32) < 0x20 => {
                    return Err(self.error_here("control character must be escaped"));
                }
                Some(c) => {
                    s.push(c);
                    self.bump();
                }
            }
        }
        Ok(s)
    }

    fn read_hex4(&mut self) -> Result<u16, ParseError> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            match self.peek() {
                Some(c) if c.is_ascii_hexdigit() => {
                    value = value * 16 + c.to_digit(16).expect("just checked it's a hex digit") as u16;
                    self.bump();
                }
                _ => return Err(self.error_here("invalid unicode escape")),
            }
        }
        Ok(value)
    }

    /// Reads the four hex digits after a `\u` that has already been
    /// consumed, combining a surrogate pair into one scalar value and
    /// rejecting any surrogate that is not part of a valid pair.
    fn read_unicode_escape(&mut self) -> Result<char, ParseError> {
        let err_line = self.line;
        let err_col = self.col;
        let hi = self.read_hex4()?;

        if (0xD800..=0xDBFF).contains(&hi) {
            if self.peek() != Some('\\') {
                return Err(self.error_at(err_line, err_col, "lone UTF-16 surrogate"));
            }
            self.bump();
            if self.peek() != Some('u') {
                return Err(self.error_at(err_line, err_col, "lone UTF-16 surrogate"));
            }
            self.bump();
            let lo = self.read_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&lo) {
                return Err(self.error_at(err_line, err_col, "lone UTF-16 surrogate"));
            }
            let combined = 0x10000u32 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
            Ok(char::from_u32(combined).expect("a valid surrogate pair decodes to a valid scalar"))
        } else if (0xDC00..=0xDFFF).contains(&hi) {
            Err(self.error_at(err_line, err_col, "lone UTF-16 surrogate"))
        } else {
            Ok(char::from_u32(hi as u32).expect("a non-surrogate u16 is always a valid char"))
        }
    }

    fn parse_array(&mut self) -> Result<Value, ParseError> {
        self.bump(); // '['
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.parse_value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error_here("expected ',' or ']'")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Value, ParseError> {
        self.bump(); // '{'
        let mut entries = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Value::Object(entries));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some('"') {
                return Err(self.error_here("expected string key"));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.peek() != Some(':') {
                return Err(self.error_here("expected ':' after object key"));
            }
            self.bump();
            self.skip_whitespace();
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some('}') => {
                    self.bump();
                    return Ok(Value::Object(entries));
                }
                _ => return Err(self.error_here("expected ',' or '}'")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_parses(input: &str) -> Value {
        parse(input).unwrap_or_else(|e| panic!("expected {input:?} to parse, got error: {e}"))
    }

    fn assert_rejected(input: &str) {
        assert!(parse(input).is_err(), "expected {input:?} to be rejected");
    }

    #[test]
    fn accepts_literals() {
        assert_eq!(assert_parses("null"), Value::Null);
        assert_eq!(assert_parses("true"), Value::Bool(true));
        assert_eq!(assert_parses("false"), Value::Bool(false));
    }

    #[test]
    fn accepts_numbers() {
        assert_eq!(assert_parses("0"), Value::Int(0));
        assert_eq!(assert_parses("-0"), Value::Int(0));
        assert_eq!(assert_parses("123"), Value::Int(123));
        assert_eq!(assert_parses("-45"), Value::Int(-45));
        assert_eq!(assert_parses("0.5"), Value::Float(0.5));
        assert_eq!(assert_parses("1e10"), Value::Float(1e10));
        assert_eq!(assert_parses("1.5E-3"), Value::Float(1.5e-3));
        match assert_parses("99999999999999999999") {
            Value::Float(_) => {}
            other => panic!("expected an overflowing integer literal to become a float, got {other:?}"),
        }
    }

    #[test]
    fn accepts_strings_with_escapes() {
        assert_eq!(assert_parses(r#""hi""#), Value::String("hi".into()));
        assert_eq!(assert_parses(r#""a\nb""#), Value::String("a\nb".into()));
        assert_eq!(assert_parses("\"\\u0041\""), Value::String("A".into()));
        assert_eq!(
            assert_parses(r#""😀""#),
            Value::String("\u{1F600}".into())
        );
        let surrogate_pair_escape = "\"\\ud83d\\ude00\"";
        assert_eq!(
            assert_parses(surrogate_pair_escape),
            Value::String("\u{1F600}".into())
        );
    }

    #[test]
    fn accepts_arrays_and_objects() {
        assert_eq!(assert_parses("[]"), Value::Array(vec![]));
        assert_eq!(
            assert_parses("[1, 2, 3]"),
            Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
        assert_eq!(assert_parses("{}"), Value::Object(vec![]));
        assert_eq!(
            assert_parses(r#" { "a" : 1 } "#),
            Value::Object(vec![("a".into(), Value::Int(1))])
        );
    }

    #[test]
    fn rejects_near_json() {
        assert_rejected("");
        assert_rejected(r#"{'a': 1}"#);
        assert_rejected("[1, 2,]");
        assert_rejected(r#"{"a": 1,}"#);
        assert_rejected("1.");
        assert_rejected("1e");
        assert_rejected("NaN");
        assert_rejected("Infinity");
        assert_rejected(r#""unterminated"#);
        assert_rejected(r#""bad escape \x""#);
        assert_rejected("{1: 2}");
        assert_rejected("[1 2]");
        assert_rejected(r#"{"a":1 "b":2}"#);
        assert_rejected("123 456");
        assert_rejected("truee");
    }

    #[test]
    fn rejects_lone_surrogates() {
        assert_rejected(r#""\ud800""#);
        assert_rejected(r#""\udc00""#);
        assert_rejected(r#""\ud800A""#);
    }

    #[test]
    fn rejects_raw_control_characters() {
        assert_rejected("\"a\u{9}b\"");
        assert_rejected("\"a\u{0}b\"");
    }

    #[test]
    fn reports_line_and_column_of_the_failure() {
        let err = parse("{\n  \"a\": 1,\n  \"b\": ,\n}").unwrap_err();
        assert_eq!(err.line, 3);
        assert_eq!(err.column, 8);
    }
}
