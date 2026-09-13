use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use jsonl_peek::lines::LineReader;
use jsonl_peek::path::FieldPath;
use jsonl_peek::rng::Reservoir;
use jsonl_peek::stats::{Stats, StatsOptions};

enum Error {
    Usage(String),
    Runtime(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(Error::Usage(msg)) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
        Err(Error::Runtime(msg)) => {
            eprintln!("{msg}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, Error> {
    let Some((cmd, rest)) = args.split_first() else {
        return Err(Error::Usage(
            "usage: jsonl-peek <head|sample|stats|schema> [options] [FILE]".into(),
        ));
    };
    match cmd.as_str() {
        "head" => run_head(rest),
        "sample" => run_sample(rest),
        "stats" => run_stats(rest),
        other => Err(Error::Usage(format!("unknown command '{other}'"))),
    }
}

fn run_head(args: &[String]) -> Result<ExitCode, Error> {
    let mut n: usize = 10;
    let mut file_arg: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-n" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| Error::Usage("-n requires a value".into()))?;
                n = value
                    .parse()
                    .map_err(|_| Error::Usage(format!("invalid value for -n: '{value}'")))?;
            }
            other if file_arg.is_none() => file_arg = Some(other),
            other => return Err(Error::Usage(format!("unexpected argument '{other}'"))),
        }
        i += 1;
    }

    let input = open_input(file_arg)?;
    let mut reader = LineReader::new(io::BufReader::new(input));
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let mut count = 0usize;
    while count < n {
        let line = reader
            .next_line()
            .map_err(|e| Error::Runtime(format!("read error: {e}")))?;
        let Some(line) = line else {
            break;
        };
        out.write_all(line)
            .and_then(|_| out.write_all(b"\n"))
            .map_err(|e| Error::Runtime(format!("write error: {e}")))?;
        count += 1;
    }

    Ok(ExitCode::SUCCESS)
}

fn run_sample(args: &[String]) -> Result<ExitCode, Error> {
    let mut n: usize = 10;
    let mut seed: Option<u64> = None;
    let mut file_arg: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-n" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| Error::Usage("-n requires a value".into()))?;
                n = value
                    .parse()
                    .map_err(|_| Error::Usage(format!("invalid value for -n: '{value}'")))?;
            }
            "--seed" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| Error::Usage("--seed requires a value".into()))?;
                seed = Some(value.parse().map_err(|_| {
                    Error::Usage(format!("invalid value for --seed: '{value}'"))
                })?);
            }
            other if file_arg.is_none() => file_arg = Some(other),
            other => return Err(Error::Usage(format!("unexpected argument '{other}'"))),
        }
        i += 1;
    }

    let input = open_input(file_arg)?;
    let mut reader = LineReader::new(io::BufReader::new(input));
    let mut reservoir = Reservoir::new(n, seed.unwrap_or_else(random_seed));

    while let Some(line) = reader
        .next_line()
        .map_err(|e| Error::Runtime(format!("read error: {e}")))?
    {
        if line.is_empty() {
            continue;
        }
        reservoir.consider(line.to_vec());
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in reservoir.into_sorted() {
        out.write_all(&line)
            .and_then(|_| out.write_all(b"\n"))
            .map_err(|e| Error::Runtime(format!("write error: {e}")))?;
    }

    Ok(ExitCode::SUCCESS)
}

fn run_stats(args: &[String]) -> Result<ExitCode, Error> {
    let mut fields: Vec<FieldPath> = Vec::new();
    let mut top: usize = 10;
    let mut max_errors: usize = 10;
    let mut json = false;
    let mut file_arg: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--field" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| Error::Usage("--field requires a value".into()))?;
                let path = FieldPath::parse(value)
                    .map_err(|e| Error::Usage(format!("invalid --field '{value}': {e}")))?;
                fields.push(path);
            }
            "--top" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| Error::Usage("--top requires a value".into()))?;
                top = value
                    .parse()
                    .map_err(|_| Error::Usage(format!("invalid value for --top: '{value}'")))?;
            }
            "--max-errors" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| Error::Usage("--max-errors requires a value".into()))?;
                max_errors = value.parse().map_err(|_| {
                    Error::Usage(format!("invalid value for --max-errors: '{value}'"))
                })?;
            }
            "--json" => json = true,
            other if file_arg.is_none() => file_arg = Some(other),
            other => return Err(Error::Usage(format!("unexpected argument '{other}'"))),
        }
        i += 1;
    }

    let input = open_input(file_arg)?;
    let options = StatsOptions {
        fields,
        top,
        max_errors,
    };
    let stats = Stats::from_reader(io::BufReader::new(input), options)
        .map_err(|e| Error::Runtime(format!("read error: {e}")))?;

    let file_label = file_arg.unwrap_or("-");
    if json {
        println!("{}", stats_to_json(file_label, &stats, top));
    } else {
        print_stats(file_label, &stats, top);
    }
    Ok(ExitCode::SUCCESS)
}

fn print_stats(file_label: &str, stats: &Stats, top: usize) {
    println!("file    {file_label}");
    println!(
        "lines   {}   blank {}   invalid {}   valid {}",
        format_count(stats.lines as u64),
        format_count(stats.blank as u64),
        format_count(stats.invalid as u64),
        format_count(stats.valid as u64),
    );
    println!(
        "bytes   {}   ({})",
        format_count(stats.bytes),
        format_bytes(stats.bytes)
    );

    let top_level: Vec<String> = stats
        .top_level
        .sorted()
        .into_iter()
        .map(|(name, count)| format!("{name}:{}", format_count(count as u64)))
        .collect();
    println!("top level  {}", top_level.join(", "));

    if stats.line_length.count() > 0 {
        println!();
        println!("line length in bytes");
        println!(
            "  min {}   p50 {}   p90 {}   p99 {}   max {}   mean {:.1}",
            stats.line_length.min(),
            stats.line_length.percentile(0.5),
            stats.line_length.percentile(0.9),
            stats.line_length.percentile(0.99),
            stats.line_length.max(),
            stats.line_length.mean(),
        );
    }

    if !stats.keys.is_empty() {
        println!();
        println!(
            "top level keys over {} objects",
            format_count(stats.valid as u64)
        );
        let mut keys: Vec<_> = stats.keys.iter().collect();
        keys.sort_by(|a, b| b.count.cmp(&a.count));
        for key in keys {
            let types: Vec<String> = key
                .types
                .sorted()
                .into_iter()
                .map(|(name, count)| format!("{name}:{}", format_count(count as u64)))
                .collect();
            println!(
                "  {:<28} {:>7}  {:>6}  {}",
                key.key,
                format_count(key.count as u64),
                percent(key.count, stats.valid),
                types.join(" ")
            );
        }
        if stats.keys_truncated {
            println!("  (key table limit reached, more keys were seen)");
        }
    }

    for field in &stats.fields {
        println!();
        println!("field {}", field.path);
        let types: Vec<String> = field
            .types
            .sorted()
            .into_iter()
            .map(|(name, count)| format!("{name}:{}", format_count(count as u64)))
            .collect();
        println!(
            "  present in {} of {} records ({}), {} values, types {}",
            format_count(field.records_present as u64),
            format_count(stats.valid as u64),
            percent(field.records_present, stats.valid),
            format_count(field.values as u64),
            types.join(" ")
        );
        println!(
            "  {} distinct values",
            format_count(field.distinct_values() as u64)
        );
        for (value, count) in field.top(top) {
            println!(
                "    {:>7}  {:>6}  {}",
                format_count(count as u64),
                percent(count, field.values),
                value
            );
        }
        if field.values_truncated {
            println!("  (distinct value limit reached, more values were seen)");
        }
    }

    if stats.invalid > 0 {
        println!();
        println!(
            "invalid lines ({} total, showing {})",
            format_count(stats.invalid as u64),
            stats.issues.len()
        );
        for issue in &stats.issues {
            println!("  line {} col {}: {}", issue.line, issue.column, issue.reason);
        }
    }
}

/// Builds the `--json` report for `stats`. Kept separate from `print_stats`
/// rather than folded into it: the two have almost no formatting in common
/// once you account for indentation and trailing commas, and a shared
/// function would just be a branch on `json` at every line.
fn stats_to_json(file_label: &str, stats: &Stats, top: usize) -> String {
    let mut fields = vec![
        ("file".to_string(), json_string(file_label)),
        ("lines".to_string(), stats.lines.to_string()),
        ("blank".to_string(), stats.blank.to_string()),
        ("invalid".to_string(), stats.invalid.to_string()),
        ("valid".to_string(), stats.valid.to_string()),
        ("bytes".to_string(), stats.bytes.to_string()),
        ("top_level".to_string(), json_type_tally(&stats.top_level)),
        (
            "line_length".to_string(),
            json_object(vec![
                ("min".to_string(), stats.line_length.min().to_string()),
                (
                    "p50".to_string(),
                    stats.line_length.percentile(0.5).to_string(),
                ),
                (
                    "p90".to_string(),
                    stats.line_length.percentile(0.9).to_string(),
                ),
                (
                    "p99".to_string(),
                    stats.line_length.percentile(0.99).to_string(),
                ),
                ("max".to_string(), stats.line_length.max().to_string()),
                ("mean".to_string(), json_number(stats.line_length.mean())),
            ]),
        ),
    ];

    let mut keys: Vec<_> = stats.keys.iter().collect();
    keys.sort_by(|a, b| b.count.cmp(&a.count));
    let key_items: Vec<String> = keys
        .iter()
        .map(|key| {
            json_object(vec![
                ("key".to_string(), json_string(&key.key)),
                ("count".to_string(), key.count.to_string()),
                (
                    "rate".to_string(),
                    json_number(rate(key.count, stats.valid)),
                ),
                ("types".to_string(), json_type_tally(&key.types)),
            ])
        })
        .collect();
    fields.push(("keys".to_string(), json_array(key_items)));
    fields.push((
        "keys_truncated".to_string(),
        stats.keys_truncated.to_string(),
    ));

    let field_items: Vec<String> = stats
        .fields
        .iter()
        .map(|field| {
            let top_items: Vec<String> = field
                .top(top)
                .into_iter()
                .map(|(value, count)| {
                    json_object(vec![
                        ("value".to_string(), json_string(&value)),
                        ("count".to_string(), count.to_string()),
                        ("rate".to_string(), json_number(rate(count, field.values))),
                    ])
                })
                .collect();
            json_object(vec![
                ("path".to_string(), json_string(&field.path.to_string())),
                (
                    "records_present".to_string(),
                    field.records_present.to_string(),
                ),
                (
                    "rate".to_string(),
                    json_number(rate(field.records_present, stats.valid)),
                ),
                ("values".to_string(), field.values.to_string()),
                ("types".to_string(), json_type_tally(&field.types)),
                (
                    "distinct_values".to_string(),
                    field.distinct_values().to_string(),
                ),
                (
                    "values_truncated".to_string(),
                    field.values_truncated.to_string(),
                ),
                ("top".to_string(), json_array(top_items)),
            ])
        })
        .collect();
    fields.push(("fields".to_string(), json_array(field_items)));

    let issue_items: Vec<String> = stats
        .issues
        .iter()
        .map(|issue| {
            json_object(vec![
                ("line".to_string(), issue.line.to_string()),
                ("column".to_string(), issue.column.to_string()),
                ("reason".to_string(), json_string(&issue.reason)),
            ])
        })
        .collect();
    fields.push(("issues".to_string(), json_array(issue_items)));
    fields.push((
        "issues_truncated".to_string(),
        (stats.invalid > stats.issues.len()).to_string(),
    ));

    json_object(fields)
}

fn rate(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        n as f64 / total as f64
    }
}

fn json_type_tally(tally: &jsonl_peek::stats::TypeTally) -> String {
    let entries: Vec<(String, String)> = tally
        .sorted()
        .into_iter()
        .map(|(name, count)| (name.to_string(), count.to_string()))
        .collect();
    json_object(entries)
}

/// Renders an ordered set of key/value pairs as a JSON object, with the
/// already-rendered `value` text indented to sit correctly under `key` no
/// matter how many lines it spans. Values are pre-rendered strings so
/// `stats_to_json` can build nested objects/arrays bottom-up rather than
/// needing a `Value`-like tree just for this one report.
fn json_object(fields: Vec<(String, String)>) -> String {
    if fields.is_empty() {
        return "{}".to_string();
    }
    let mut out = String::from("{\n");
    for (i, (key, value)) in fields.iter().enumerate() {
        out.push_str("  ");
        out.push_str(&json_string(key));
        out.push_str(": ");
        out.push_str(&indent_continuation(value));
        if i + 1 < fields.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push('}');
    out
}

fn json_array(items: Vec<String>) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let mut out = String::from("[\n");
    for (i, item) in items.iter().enumerate() {
        out.push_str("  ");
        out.push_str(&indent_continuation(item));
        if i + 1 < items.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push(']');
    out
}

/// Indents every line after the first by two spaces, so a multi-line nested
/// object or array lines up under the key or bracket that introduces it.
fn indent_continuation(text: &str) -> String {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let mut out = first.to_string();
    for line in lines {
        out.push('\n');
        out.push_str("  ");
        out.push_str(line);
    }
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Formats an `f64` for JSON output, trimmed to 4 decimal places with
/// trailing zeros (and a bare trailing `.`) removed so whole numbers read as
/// `5`, not `5.0000`.
fn json_number(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(bytes.len() + bytes.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = n as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn percent(n: usize, total: usize) -> String {
    if total == 0 {
        return "0.0%".to_string();
    }
    format!("{:.1}%", (n as f64 / total as f64) * 100.0)
}

/// A seed for `sample` runs where the user didn't pass `--seed`. Combines
/// wall-clock time with a stack address so that two processes started in the
/// same clock tick (a common case in scripts and tests) still get different
/// seeds; ASLR makes the address vary between runs.
fn random_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let marker = 0u8;
    let address = &marker as *const u8 as u64;
    nanos ^ address.rotate_left(17)
}

fn open_input(file_arg: Option<&str>) -> Result<Box<dyn Read>, Error> {
    match file_arg {
        None | Some("-") => Ok(Box::new(io::stdin())),
        Some(path) => File::open(path)
            .map(|f| Box::new(f) as Box<dyn Read>)
            .map_err(|e| Error::Runtime(format!("cannot open '{path}': {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn json_string_escapes_control_characters_and_quotes() {
        assert_eq!(json_string("hi"), "\"hi\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb\tc"), "\"a\\nb\\tc\"");
        assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn json_number_trims_trailing_zeros() {
        assert_eq!(json_number(0.0), "0");
        assert_eq!(json_number(5.0), "5");
        assert_eq!(json_number(0.5), "0.5");
        assert_eq!(json_number(466.9226), "466.9226");
        assert_eq!(json_number(1.0 / 3.0), "0.3333");
    }

    #[test]
    fn json_object_indents_nested_values() {
        let inner = json_object(vec![("b".to_string(), "1".to_string())]);
        let outer = json_object(vec![("a".to_string(), inner)]);
        assert_eq!(outer, "{\n  \"a\": {\n    \"b\": 1\n  }\n}");
    }

    #[test]
    fn json_array_indents_nested_objects() {
        let item = json_object(vec![("k".to_string(), "1".to_string())]);
        let arr = json_array(vec![item.clone(), item]);
        assert_eq!(
            arr,
            "[\n  {\n    \"k\": 1\n  },\n  {\n    \"k\": 1\n  }\n]"
        );
    }

    #[test]
    fn stats_to_json_produces_parseable_numbers_and_the_field_path() {
        let path = FieldPath::parse("meta.source").unwrap();
        let options = StatsOptions {
            fields: vec![path],
            ..StatsOptions::default()
        };
        let stats = Stats::from_reader(
            Cursor::new("{\"meta\":{\"source\":\"web\"}}\n{bad}\n".as_bytes()),
            options,
        )
        .unwrap();

        let json = stats_to_json("sample.jsonl", &stats, 10);
        assert!(json.contains("\"file\": \"sample.jsonl\""));
        assert!(json.contains("\"lines\": 2"));
        assert!(json.contains("\"invalid\": 1"));
        assert!(json.contains("\"path\": \"meta.source\""));
        assert!(json.contains("\"value\": \"\\\"web\\\"\""));
    }
}
