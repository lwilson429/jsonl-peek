use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use jsonl_peek::lines::LineReader;

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

fn open_input(file_arg: Option<&str>) -> Result<Box<dyn Read>, Error> {
    match file_arg {
        None | Some("-") => Ok(Box::new(io::stdin())),
        Some(path) => File::open(path)
            .map(|f| Box::new(f) as Box<dyn Read>)
            .map_err(|e| Error::Runtime(format!("cannot open '{path}': {e}"))),
    }
}
