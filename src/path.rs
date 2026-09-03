//! Parsing for the `--field PATH` syntax used by `stats` to pick a value out
//! of each record. See the README's "Field path syntax" table for the
//! surface this accepts; this module only builds the parsed representation,
//! walking a `Value` with it is `jsonl_peek::stats`'s job.

use std::fmt;

/// One step of a [`FieldPath`]: either an object member or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// `key` or `.key`: selects an object member.
    Key(String),
    /// `[N]`: selects one array element. Negative counts from the end, so
    /// `-1` is the last element.
    Index(i64),
    /// `[]`: selects every array element, fanning the rest of the path out
    /// over each one.
    Every,
}

/// A parsed `--field` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPath {
    raw: String,
    segments: Vec<Segment>,
}

/// Why a path string failed to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError {
    pub message: String,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for PathError {}

impl FieldPath {
    /// Parses a path like `meta.source`, `messages[].role` or `[0].id`.
    pub fn parse(input: &str) -> Result<FieldPath, PathError> {
        if input.is_empty() {
            return Err(PathError {
                message: "field path is empty".into(),
            });
        }

        let bytes = input.as_bytes();
        let mut segments = Vec::new();
        let mut pos = 0usize;
        // Set once a key or index has been read; cleared by a '.'. A bare
        // key can never follow another bare key without a separator, but a
        // '[' needs no separator of its own (`messages[0]`, not `messages.[0]`).
        let mut expect_separator = false;

        while pos < bytes.len() {
            match bytes[pos] {
                b'.' => {
                    if !expect_separator {
                        return Err(PathError {
                            message: format!("unexpected '.' at position {pos} in '{input}'"),
                        });
                    }
                    pos += 1;
                    if pos >= bytes.len() {
                        return Err(PathError {
                            message: format!("trailing '.' in '{input}'"),
                        });
                    }
                    expect_separator = false;
                }
                b'[' => {
                    let close = input[pos..]
                        .find(']')
                        .map(|i| i + pos)
                        .ok_or_else(|| PathError {
                            message: format!("unterminated '[' in '{input}'"),
                        })?;
                    let inside = &input[pos + 1..close];
                    let segment = if inside.is_empty() {
                        Segment::Every
                    } else {
                        let n: i64 = inside.parse().map_err(|_| PathError {
                            message: format!("invalid array index '{inside}' in '{input}'"),
                        })?;
                        Segment::Index(n)
                    };
                    segments.push(segment);
                    pos = close + 1;
                    expect_separator = true;
                }
                b']' => {
                    return Err(PathError {
                        message: format!("unexpected ']' at position {pos} in '{input}'"),
                    });
                }
                _ => {
                    if expect_separator {
                        return Err(PathError {
                            message: format!("expected '.' or '[' at position {pos} in '{input}'"),
                        });
                    }
                    let start = pos;
                    while pos < bytes.len() && bytes[pos] != b'.' && bytes[pos] != b'[' && bytes[pos] != b']' {
                        pos += 1;
                    }
                    segments.push(Segment::Key(input[start..pos].to_string()));
                    expect_separator = true;
                }
            }
        }

        if segments.is_empty() {
            return Err(PathError {
                message: format!("no field selected in '{input}'"),
            });
        }

        Ok(FieldPath {
            raw: input.to_string(),
            segments,
        })
    }

    /// The original, unparsed path string.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_bare_key() {
        let path = FieldPath::parse("role").unwrap();
        assert_eq!(path.segments(), &[Segment::Key("role".into())]);
    }

    #[test]
    fn parses_nested_keys() {
        let path = FieldPath::parse("meta.source").unwrap();
        assert_eq!(
            path.segments(),
            &[Segment::Key("meta".into()), Segment::Key("source".into())]
        );
    }

    #[test]
    fn parses_a_positive_index() {
        let path = FieldPath::parse("messages[0].content").unwrap();
        assert_eq!(
            path.segments(),
            &[
                Segment::Key("messages".into()),
                Segment::Index(0),
                Segment::Key("content".into()),
            ]
        );
    }

    #[test]
    fn parses_a_negative_index() {
        let path = FieldPath::parse("messages[-1].content").unwrap();
        assert_eq!(
            path.segments(),
            &[
                Segment::Key("messages".into()),
                Segment::Index(-1),
                Segment::Key("content".into()),
            ]
        );
    }

    #[test]
    fn parses_a_wildcard_index() {
        let path = FieldPath::parse("messages[].role").unwrap();
        assert_eq!(
            path.segments(),
            &[
                Segment::Key("messages".into()),
                Segment::Every,
                Segment::Key("role".into()),
            ]
        );
    }

    #[test]
    fn parses_a_leading_index() {
        let path = FieldPath::parse("[0].id").unwrap();
        assert_eq!(
            path.segments(),
            &[Segment::Index(0), Segment::Key("id".into())]
        );
    }

    #[test]
    fn round_trips_through_display() {
        let path = FieldPath::parse("messages[].role").unwrap();
        assert_eq!(path.to_string(), "messages[].role");
        assert_eq!(path.as_str(), "messages[].role");
    }

    #[test]
    fn rejects_empty_path() {
        assert!(FieldPath::parse("").is_err());
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(FieldPath::parse(".a").is_err());
    }

    #[test]
    fn rejects_trailing_dot() {
        assert!(FieldPath::parse("a.").is_err());
    }

    #[test]
    fn rejects_double_dot() {
        assert!(FieldPath::parse("a..b").is_err());
    }

    #[test]
    fn rejects_unterminated_bracket() {
        assert!(FieldPath::parse("a[0").is_err());
    }

    #[test]
    fn rejects_stray_close_bracket() {
        assert!(FieldPath::parse("a]0").is_err());
    }

    #[test]
    fn rejects_non_numeric_index() {
        assert!(FieldPath::parse("a[x]").is_err());
    }

    #[test]
    fn rejects_two_keys_with_no_separator() {
        assert!(FieldPath::parse("a[0]b").is_err());
    }
}
