//! Structural profile of a JSONL file: every distinct path up to a depth
//! limit, how often each path shows up, and what JSON types were seen there.
//! Where `stats` counts the fields you already know to look for, `schema`
//! finds every field there is.

use std::collections::HashMap;
use std::io::{self, BufRead};

use crate::json::{self, Value};
use crate::lines::LineReader;

/// How many distinct paths are tracked before the table stops growing.
const MAX_PATHS: usize = 2_000;

pub struct SchemaOptions {
    pub depth: usize,
    pub min_rate: f64,
}

impl Default for SchemaOptions {
    fn default() -> Self {
        SchemaOptions {
            depth: 3,
            min_rate: 0.0,
        }
    }
}

/// A count per JSON type name, handed out sorted by descending count.
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

/// One distinct path found in the file: how many records contained it at
/// least once, how many values were seen there in total (more than one per
/// record if the path fans out through a `[]`), and what types those values
/// were.
pub struct PathStat {
    pub path: String,
    pub records_present: usize,
    pub values: usize,
    pub types: TypeTally,
}

impl PathStat {
    /// Share of `records` that contained this path at least once.
    pub fn rate(&self, records: usize) -> f64 {
        if records == 0 {
            0.0
        } else {
            self.records_present as f64 / records as f64
        }
    }
}

pub struct Schema {
    pub records: usize,
    pub depth: usize,
    pub paths: Vec<PathStat>,
    pub paths_truncated: bool,
    pub unparseable: usize,
}

impl Schema {
    /// Runs the single pass: reads `reader` line by line, parsing each
    /// non-blank line as JSON and walking it up to `options.depth` levels
    /// deep, then filters out anything below `options.min_rate` and sorts
    /// the result by path.
    pub fn from_reader<R: BufRead>(reader: R, options: SchemaOptions) -> io::Result<Schema> {
        let mut records = 0usize;
        let mut unparseable = 0usize;
        let mut paths: Vec<PathStat> = Vec::new();
        let mut paths_truncated = false;
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut line_reader = LineReader::new(reader);

        while let Some(line) = line_reader.next_line()? {
            if line.is_empty() {
                continue;
            }
            let text = match std::str::from_utf8(line) {
                Ok(text) => text,
                Err(_) => {
                    unparseable += 1;
                    continue;
                }
            };
            match json::parse(text) {
                Ok(value) => {
                    records += 1;
                    let mut seen_this_record: Vec<String> = Vec::new();
                    walk(
                        &value,
                        String::new(),
                        0,
                        options.depth,
                        &mut paths,
                        &mut paths_truncated,
                        &mut index,
                        &mut seen_this_record,
                    );
                }
                Err(_) => unparseable += 1,
            }
        }

        paths.retain(|p| {
            let rate = if records == 0 {
                0.0
            } else {
                p.records_present as f64 / records as f64
            };
            rate >= options.min_rate
        });
        paths.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(Schema {
            records,
            depth: options.depth,
            paths,
            paths_truncated,
            unparseable,
        })
    }
}

/// Walks `value`, recording every object member and array element as a path
/// (object members joined with `.`, array elements marked `[]`) until
/// `depth` reaches `max_depth`. Scalars end the walk with nothing further to
/// record.
#[allow(clippy::too_many_arguments)]
fn walk(
    value: &Value,
    prefix: String,
    depth: usize,
    max_depth: usize,
    paths: &mut Vec<PathStat>,
    paths_truncated: &mut bool,
    index: &mut HashMap<String, usize>,
    seen_this_record: &mut Vec<String>,
) {
    if depth >= max_depth {
        return;
    }

    match value {
        Value::Object(entries) => {
            for (key, member) in entries {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                record(&path, member, paths, paths_truncated, index, seen_this_record);
                walk(
                    member,
                    path,
                    depth + 1,
                    max_depth,
                    paths,
                    paths_truncated,
                    index,
                    seen_this_record,
                );
            }
        }
        Value::Array(items) => {
            let path = format!("{prefix}[]");
            for item in items {
                record(&path, item, paths, paths_truncated, index, seen_this_record);
                walk(
                    item,
                    path.clone(),
                    depth + 1,
                    max_depth,
                    paths,
                    paths_truncated,
                    index,
                    seen_this_record,
                );
            }
        }
        _ => {}
    }
}

fn record(
    path: &str,
    value: &Value,
    paths: &mut Vec<PathStat>,
    paths_truncated: &mut bool,
    index: &mut HashMap<String, usize>,
    seen_this_record: &mut Vec<String>,
) {
    let idx = if let Some(&i) = index.get(path) {
        i
    } else if paths.len() < MAX_PATHS {
        let i = paths.len();
        paths.push(PathStat {
            path: path.to_string(),
            records_present: 0,
            values: 0,
            types: TypeTally::default(),
        });
        index.insert(path.to_string(), i);
        i
    } else {
        *paths_truncated = true;
        return;
    };

    let stat = &mut paths[idx];
    stat.values += 1;
    stat.types.record(type_name(value));
    if !seen_this_record.iter().any(|p| p == path) {
        stat.records_present += 1;
        seen_this_record.push(path.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(input: &str, options: SchemaOptions) -> Schema {
        Schema::from_reader(Cursor::new(input.as_bytes()), options).unwrap()
    }

    fn path<'a>(schema: &'a Schema, name: &str) -> &'a PathStat {
        schema
            .paths
            .iter()
            .find(|p| p.path == name)
            .unwrap_or_else(|| panic!("no path '{name}' in {:?}", schema.paths.iter().map(|p| &p.path).collect::<Vec<_>>()))
    }

    #[test]
    fn finds_top_level_and_nested_paths() {
        let schema = run(
            "{\"id\":1,\"meta\":{\"source\":\"web\"}}\n{\"id\":2,\"meta\":{\"source\":\"forum\"}}\n",
            SchemaOptions::default(),
        );
        assert_eq!(schema.records, 2);

        let id = path(&schema, "id");
        assert_eq!(id.records_present, 2);
        assert_eq!(id.types.sorted(), vec![("int", 2)]);

        let source = path(&schema, "meta.source");
        assert_eq!(source.records_present, 2);
        assert_eq!(source.types.sorted(), vec![("string", 2)]);
    }

    #[test]
    fn fans_out_over_array_elements_and_counts_presence_once() {
        let schema = run(
            "{\"messages\":[{\"role\":\"user\"},{\"role\":\"assistant\"}]}\n",
            SchemaOptions::default(),
        );
        let role = path(&schema, "messages[].role");
        assert_eq!(role.records_present, 1);
        assert_eq!(role.values, 2);
    }

    #[test]
    fn depth_limits_how_far_the_walk_descends() {
        let schema = run(
            "{\"a\":{\"b\":{\"c\":1}}}\n",
            SchemaOptions {
                depth: 2,
                ..SchemaOptions::default()
            },
        );
        assert!(schema.paths.iter().any(|p| p.path == "a"));
        assert!(schema.paths.iter().any(|p| p.path == "a.b"));
        assert!(!schema.paths.iter().any(|p| p.path == "a.b.c"));
    }

    #[test]
    fn min_rate_hides_rare_paths() {
        let schema = run(
            "{\"id\":1,\"tags\":[1]}\n{\"id\":2}\n{\"id\":3}\n{\"id\":4}\n",
            SchemaOptions {
                min_rate: 0.5,
                ..SchemaOptions::default()
            },
        );
        assert!(schema.paths.iter().any(|p| p.path == "id"));
        assert!(!schema.paths.iter().any(|p| p.path == "tags"));
    }

    #[test]
    fn paths_are_sorted() {
        let schema = run("{\"b\":1,\"a\":1}\n", SchemaOptions::default());
        let names: Vec<&str> = schema.paths.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn counts_unparseable_lines_without_touching_blank_ones() {
        let schema = run("{\"a\":1}\n\n{bad}\n", SchemaOptions::default());
        assert_eq!(schema.records, 1);
        assert_eq!(schema.unparseable, 1);
    }

    #[test]
    fn rate_is_relative_to_record_count() {
        let schema = run("{\"a\":1}\n{}\n", SchemaOptions::default());
        let a = path(&schema, "a");
        assert_eq!(a.rate(schema.records), 0.5);
    }
}
