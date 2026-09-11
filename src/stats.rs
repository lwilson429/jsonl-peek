//! Single-pass health check for a JSONL file: record counts, the top-level
//! key table, line-length percentiles and optional per-field value
//! distributions. `Stats::from_reader` is the whole pass; the `stats`
//! command in `main.rs` only formats what comes out of it.

use std::collections::HashMap;
use std::io::{self, BufRead};

use crate::hist::Histogram;
use crate::json::{self, Value};
use crate::lines::LineReader;
use crate::path::{FieldPath, Segment};

/// How many distinct top-level keys are tracked before the table stops
/// growing. A file with more distinct keys than this is almost certainly not
/// uniform JSONL, and the report says so via [`Stats::keys_truncated`].
const MAX_KEYS: usize = 512;

/// How many distinct values a single `--field` tracks before it stops
/// counting new ones (existing ones keep incrementing).
const MAX_FIELD_VALUES: usize = 10_000;

pub struct StatsOptions {
    pub fields: Vec<FieldPath>,
    pub top: usize,
    pub max_errors: usize,
}

impl Default for StatsOptions {
    fn default() -> Self {
        StatsOptions {
            fields: Vec::new(),
            top: 10,
            max_errors: 10,
        }
    }
}

/// One rejected line, with enough detail to point a caller straight at it.
pub struct ParseIssue {
    pub line: usize,
    pub column: usize,
    pub reason: String,
}

/// A count per JSON type name, kept in first-seen order internally and
/// handed out sorted by descending count for reporting.
#[derive(Default, Clone)]
pub struct TypeTally {
    counts: Vec<(&'static str, usize)>,
}

impl TypeTally {
    fn record(&mut self, name: &'static str) {
        match self.counts.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 += 1,
            None => self.counts.push((name, 1)),
        }
    }

    /// `(type name, count)` pairs, most common first; ties keep the order in
    /// which the type was first seen.
    pub fn sorted(&self) -> Vec<(&'static str, usize)> {
        let mut v = self.counts.clone();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }
}

/// How often, and as what types, one top-level key showed up.
pub struct KeyStat {
    pub key: String,
    pub count: usize,
    pub types: TypeTally,
}

/// The value distribution collected for one `--field` path.
pub struct FieldStat {
    pub path: FieldPath,
    /// Records that contained at least one value at this path.
    pub records_present: usize,
    /// Total values seen at this path, fanned out over `[]` segments.
    pub values: usize,
    pub types: TypeTally,
    value_counts: HashMap<String, usize>,
    pub values_truncated: bool,
}

impl FieldStat {
    fn new(path: FieldPath) -> Self {
        FieldStat {
            path,
            records_present: 0,
            values: 0,
            types: TypeTally::default(),
            value_counts: HashMap::new(),
            values_truncated: false,
        }
    }

    fn record(&mut self, value: &Value) {
        self.values += 1;
        self.types.record(type_name(value));

        let rendered = render_value(value);
        if let Some(count) = self.value_counts.get_mut(&rendered) {
            *count += 1;
        } else if self.value_counts.len() < MAX_FIELD_VALUES {
            self.value_counts.insert(rendered, 1);
        } else {
            self.values_truncated = true;
        }
    }

    /// The `n` most common values, most common first; ties break on the
    /// rendered value itself so the order is deterministic.
    pub fn top(&self, n: usize) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> =
            self.value_counts.iter().map(|(k, c)| (k.clone(), *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    pub fn distinct_values(&self) -> usize {
        self.value_counts.len()
    }
}

pub struct Stats {
    pub lines: usize,
    pub blank: usize,
    pub valid: usize,
    pub invalid: usize,
    pub bytes: u64,
    pub top_level: TypeTally,
    pub line_length: Histogram,
    pub keys: Vec<KeyStat>,
    pub keys_truncated: bool,
    pub fields: Vec<FieldStat>,
    pub issues: Vec<ParseIssue>,
    max_errors: usize,
}

impl Stats {
    fn new(options: &StatsOptions) -> Self {
        Stats {
            lines: 0,
            blank: 0,
            valid: 0,
            invalid: 0,
            bytes: 0,
            top_level: TypeTally::default(),
            line_length: Histogram::new(),
            keys: Vec::new(),
            keys_truncated: false,
            fields: options
                .fields
                .iter()
                .cloned()
                .map(FieldStat::new)
                .collect(),
            issues: Vec::new(),
            max_errors: options.max_errors,
        }
    }

    /// Runs the single pass: reads `reader` line by line, parsing each
    /// non-blank line as JSON and folding it into the running counts.
    pub fn from_reader<R: BufRead>(reader: R, options: StatsOptions) -> io::Result<Stats> {
        let mut stats = Stats::new(&options);
        let mut line_reader = LineReader::new(reader);

        while let Some(line) = line_reader.next_line()? {
            stats.lines += 1;
            stats.bytes += line.len() as u64;

            if line.is_empty() {
                stats.blank += 1;
                continue;
            }
            stats.line_length.add(line.len() as u64);

            let text = match std::str::from_utf8(line) {
                Ok(text) => text,
                Err(e) => {
                    let line_number = stats.lines;
                    stats.record_issue(line_number, e.valid_up_to() + 1, "invalid utf-8".to_string());
                    continue;
                }
            };

            match json::parse(text) {
                Ok(value) => {
                    stats.valid += 1;
                    stats.record_value(&value);
                }
                Err(e) => {
                    let line_number = stats.lines;
                    stats.record_issue(line_number, e.column, e.message);
                }
            }
        }

        Ok(stats)
    }

    fn record_issue(&mut self, line: usize, column: usize, reason: String) {
        self.invalid += 1;
        if self.issues.len() < self.max_errors {
            self.issues.push(ParseIssue { line, column, reason });
        }
    }

    fn record_value(&mut self, value: &Value) {
        self.top_level.record(type_name(value));

        if let Value::Object(entries) = value {
            let mut seen_this_record: Vec<&str> = Vec::new();
            for (key, member) in entries {
                if let Some(existing) = self.keys.iter_mut().find(|k| k.key == *key) {
                    existing.types.record(type_name(member));
                    if !seen_this_record.contains(&key.as_str()) {
                        existing.count += 1;
                        seen_this_record.push(key);
                    }
                } else if self.keys.len() < MAX_KEYS {
                    let mut types = TypeTally::default();
                    types.record(type_name(member));
                    self.keys.push(KeyStat {
                        key: key.clone(),
                        count: 1,
                        types,
                    });
                    seen_this_record.push(key);
                } else {
                    self.keys_truncated = true;
                }
            }
        }

        for field in &mut self.fields {
            let matches = walk(value, field.path.segments());
            if !matches.is_empty() {
                field.records_present += 1;
            }
            for matched in matches {
                field.record(matched);
            }
        }
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A display form used to tell distinct field values apart. Strings render
/// quoted so `"1"` and `1` don't collide; this is for reporting, not for
/// round-tripping back into JSON.
fn render_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Array(items) => format!("[array of {}]", items.len()),
        Value::Object(entries) => format!("{{object of {}}}", entries.len()),
    }
}

/// Walks `value` through `segments`, fanning out over every `[]` and
/// stopping at any segment that doesn't match the value's shape (a `.key` on
/// an array, an out-of-range index, and so on just yield no matches).
fn walk<'v>(value: &'v Value, segments: &[Segment]) -> Vec<&'v Value> {
    let Some((segment, rest)) = segments.split_first() else {
        return vec![value];
    };

    match segment {
        Segment::Key(key) => match value {
            Value::Object(entries) => entries
                .iter()
                .filter(|(k, _)| k == key)
                .flat_map(|(_, v)| walk(v, rest))
                .collect(),
            _ => Vec::new(),
        },
        Segment::Index(i) => match value {
            Value::Array(items) => {
                let index = if *i < 0 { *i + items.len() as i64 } else { *i };
                if index >= 0 && (index as usize) < items.len() {
                    walk(&items[index as usize], rest)
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        },
        Segment::Every => match value {
            Value::Array(items) => items.iter().flat_map(|v| walk(v, rest)).collect(),
            _ => Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(input: &str, options: StatsOptions) -> Stats {
        Stats::from_reader(Cursor::new(input.as_bytes()), options).unwrap()
    }

    #[test]
    fn counts_blank_valid_and_invalid_lines() {
        let stats = run(
            "{\"a\":1}\n\n{\"a\":2,}\n{\"a\":3}\n",
            StatsOptions::default(),
        );
        assert_eq!(stats.lines, 4);
        assert_eq!(stats.blank, 1);
        assert_eq!(stats.valid, 2);
        assert_eq!(stats.invalid, 1);
    }

    #[test]
    fn records_top_level_types() {
        let stats = run("1\n\"a\"\n[1,2]\n{}\n", StatsOptions::default());
        let types: Vec<_> = stats.top_level.sorted();
        assert_eq!(types.len(), 4);
        assert!(types.contains(&("int", 1)));
        assert!(types.contains(&("string", 1)));
        assert!(types.contains(&("array", 1)));
        assert!(types.contains(&("object", 1)));
    }

    #[test]
    fn builds_the_top_level_key_table() {
        let stats = run(
            "{\"id\":1,\"tags\":[1]}\n{\"id\":2}\n",
            StatsOptions::default(),
        );
        let id = stats.keys.iter().find(|k| k.key == "id").unwrap();
        assert_eq!(id.count, 2);
        assert_eq!(id.types.sorted(), vec![("int", 2)]);

        let tags = stats.keys.iter().find(|k| k.key == "tags").unwrap();
        assert_eq!(tags.count, 1);
    }

    #[test]
    fn a_duplicate_key_in_one_record_counts_presence_once() {
        let stats = run("{\"a\":1,\"a\":2}\n", StatsOptions::default());
        let a = stats.keys.iter().find(|k| k.key == "a").unwrap();
        assert_eq!(a.count, 1);
        assert_eq!(a.types.sorted(), vec![("int", 2)]);
    }

    #[test]
    fn line_length_histogram_ignores_blank_lines() {
        let stats = run("{\"a\":1}\n\n", StatsOptions::default());
        assert_eq!(stats.line_length.count(), 1);
    }

    #[test]
    fn profiles_a_simple_field() {
        let path = FieldPath::parse("meta.source").unwrap();
        let options = StatsOptions {
            fields: vec![path],
            ..StatsOptions::default()
        };
        let stats = run(
            "{\"meta\":{\"source\":\"web\"}}\n{\"meta\":{\"source\":\"web\"}}\n{\"meta\":{\"source\":\"forum\"}}\n{\"id\":1}\n",
            options,
        );
        let field = &stats.fields[0];
        assert_eq!(field.records_present, 3);
        assert_eq!(field.values, 3);
        assert_eq!(field.distinct_values(), 2);
        let top = field.top(1);
        assert_eq!(top[0], ("\"web\"".to_string(), 2));
    }

    #[test]
    fn profiles_a_fanned_out_field() {
        let path = FieldPath::parse("messages[].role").unwrap();
        let options = StatsOptions {
            fields: vec![path],
            ..StatsOptions::default()
        };
        let stats = run(
            "{\"messages\":[{\"role\":\"user\"},{\"role\":\"assistant\"}]}\n",
            options,
        );
        let field = &stats.fields[0];
        assert_eq!(field.records_present, 1);
        assert_eq!(field.values, 2);
    }

    #[test]
    fn issues_are_capped_but_the_invalid_total_is_exact() {
        let mut input = String::new();
        for _ in 0..5 {
            input.push_str("{bad}\n");
        }
        let options = StatsOptions {
            max_errors: 2,
            ..StatsOptions::default()
        };
        let stats = run(&input, options);
        assert_eq!(stats.invalid, 5);
        assert_eq!(stats.issues.len(), 2);
    }

    #[test]
    fn invalid_utf8_is_counted_as_invalid() {
        let mut input = vec![0xFF, 0xFE, b'\n'];
        input.extend_from_slice(b"{}\n");
        let stats = Stats::from_reader(Cursor::new(input), StatsOptions::default()).unwrap();
        assert_eq!(stats.invalid, 1);
        assert_eq!(stats.valid, 1);
    }
}
