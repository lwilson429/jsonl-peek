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

    print_stats(file_arg.unwrap_or("-"), &stats, top);
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
