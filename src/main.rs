use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use jsonl_peek::lines::LineReader;
use jsonl_peek::rng::Reservoir;

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
