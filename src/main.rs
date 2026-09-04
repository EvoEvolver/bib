use std::collections::BTreeSet;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use bib_cli::bibtex::{Record, parse};
use bib_cli::integrity::{Status, atomic_write, hash, status, update_source};
use bib_cli::query;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;

#[derive(Parser)]
#[command(
    name = "bib",
    version,
    about = "Query BibTeX like jq and mark human-reviewed entries",
    long_about = None,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Emit compact JSON.
    #[arg(short, long)]
    compact_output: bool,

    /// Emit strings without JSON quotes.
    #[arg(short, long)]
    raw_output: bool,

    /// Set the exit status from the last filter result, like jq -e.
    #[arg(short = 'e', long)]
    exit_status: bool,

    /// Render filtered entry objects as BibTeX instead of JSON.
    #[arg(long)]
    bibtex: bool,

    #[command(subcommand)]
    command: Option<Command>,

    /// A jq-compatible filter.
    #[arg(default_value = ".")]
    filter: String,

    /// BibTeX files. Reads stdin when omitted or when FILE is `-`.
    files: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect, add, or remove integrity markers.
    Integrity(IntegrityArgs),
}

#[derive(Args)]
struct IntegrityArgs {
    #[command(subcommand)]
    command: IntegrityCommand,
}

#[derive(Subcommand)]
enum IntegrityCommand {
    /// Show whether entry contents match their stored integrity markers.
    Status {
        file: PathBuf,
        /// Return a JSON array suitable for agents and scripts.
        #[arg(long)]
        json: bool,
        /// Limit the report to these citation keys.
        #[arg(short, long)]
        key: Vec<String>,
    },
    /// Print the expected integrity hash for one entry.
    Hash { file: PathBuf, key: String },
    /// Add or refresh integrity for explicitly reviewed entries.
    Add {
        file: PathBuf,
        /// Citation key to approve. Repeat for multiple entries.
        #[arg(short, long)]
        key: Vec<String>,
        /// Approve every entry. This must be explicit.
        #[arg(long, conflicts_with = "key")]
        all: bool,
        /// Atomically update FILE instead of writing the result to stdout.
        #[arg(short, long)]
        in_place: bool,
    },
    /// Remove integrity from selected entries.
    Remove {
        file: PathBuf,
        /// Citation key to unapprove. Repeat for multiple entries.
        #[arg(short, long)]
        key: Vec<String>,
        /// Remove integrity from every entry.
        #[arg(long, conflicts_with = "key")]
        all: bool,
        /// Atomically update FILE instead of writing the result to stdout.
        #[arg(short, long)]
        in_place: bool,
    },
}

#[derive(Serialize)]
struct StatusRow<'a> {
    id: &'a str,
    status: Status,
    expected: String,
    stored: Option<&'a str>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("bib: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    if let Some(command) = cli.command {
        return match command {
            Command::Integrity(args) => run_integrity(args.command),
        };
    }

    let records = read_query_inputs(&cli.files)?;
    let values = query::execute(&cli.filter, query::input(&records)?)?;
    if cli.bibtex {
        print!("{}", query::render_bibtex(&values)?);
    } else {
        print_json_values(&values, cli.compact_output, cli.raw_output)?;
    }

    if cli.exit_status {
        Ok(jq_exit_status(values.last()))
    } else {
        Ok(0)
    }
}

fn run_integrity(command: IntegrityCommand) -> Result<u8> {
    match command {
        IntegrityCommand::Status { file, json, key } => {
            let source = read_file(&file)?;
            let records = parse(&source)?;
            let selected: BTreeSet<_> = key.into_iter().collect();
            ensure_keys_exist(&records, &selected)?;
            let records: Vec<_> = records
                .iter()
                .filter(|record| selected.is_empty() || selected.contains(&record.entry_key))
                .collect();
            let rows = records
                .iter()
                .map(|record| {
                    Ok(StatusRow {
                        id: &record.entry_key,
                        status: status(record)?,
                        expected: hash(record)?,
                        stored: record.fields.get("integrity").map(String::as_str),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for row in &rows {
                    println!("{}\t{}", row.status, row.id);
                }
            }
            Ok(if rows.iter().all(|row| row.status == Status::Verified) {
                0
            } else {
                3
            })
        }
        IntegrityCommand::Hash { file, key } => {
            let source = read_file(&file)?;
            let records = parse(&source)?;
            let record = records
                .iter()
                .find(|record| record.entry_key == key)
                .with_context(|| format!("citation key not found: {key}"))?;
            println!("{}", hash(record)?);
            Ok(0)
        }
        IntegrityCommand::Add {
            file,
            key,
            all,
            in_place,
        } => change_integrity(&file, key, all, in_place, false),
        IntegrityCommand::Remove {
            file,
            key,
            all,
            in_place,
        } => change_integrity(&file, key, all, in_place, true),
    }
}

fn change_integrity(
    file: &Path,
    keys: Vec<String>,
    all: bool,
    in_place: bool,
    remove: bool,
) -> Result<u8> {
    let source = read_file(file)?;
    let records = parse(&source)?;
    let selected = if all {
        records
            .iter()
            .map(|record| record.entry_key.clone())
            .collect()
    } else {
        let selected: BTreeSet<_> = keys.into_iter().collect();
        if selected.is_empty() {
            bail!("no entries selected; pass --key KEY or --all after review");
        }
        selected
    };
    ensure_keys_exist(&records, &selected)?;
    let output = update_source(&source, &records, &selected, remove)?;
    if in_place {
        atomic_write(file, &output)?;
        eprintln!(
            "{} integrity for {} entr{} in {}",
            if remove { "removed" } else { "updated" },
            selected.len(),
            if selected.len() == 1 { "y" } else { "ies" },
            file.display()
        );
    } else {
        print!("{output}");
    }
    Ok(0)
}

fn ensure_keys_exist(records: &[Record], selected: &BTreeSet<String>) -> Result<()> {
    let existing: BTreeSet<_> = records.iter().map(|record| &record.entry_key).collect();
    for key in selected {
        if !existing.contains(key) {
            bail!("citation key not found: {key}");
        }
    }
    Ok(())
}

fn read_query_inputs(files: &[PathBuf]) -> Result<Vec<Record>> {
    if files.is_empty() {
        return parse(&read_stdin()?);
    }
    let mut records = Vec::new();
    for file in files {
        let source = if file == Path::new("-") {
            read_stdin()?
        } else {
            read_file(file)?
        };
        records.extend(parse(&source)?);
    }
    Ok(records)
}

fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))
}

fn read_stdin() -> Result<String> {
    if io::stdin().is_terminal() {
        bail!("no input: pass a .bib file or pipe BibTeX on stdin");
    }
    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .context("could not read stdin")?;
    Ok(source)
}

fn print_json_values(values: &[Value], compact: bool, raw: bool) -> Result<()> {
    for value in values {
        if raw {
            match value {
                Value::String(value) => println!("{value}"),
                _ if compact => println!("{}", serde_json::to_string(value)?),
                _ => println!("{}", serde_json::to_string_pretty(value)?),
            }
        } else if compact {
            println!("{}", serde_json::to_string(value)?);
        } else {
            println!("{}", serde_json::to_string_pretty(value)?);
        }
    }
    Ok(())
}

fn jq_exit_status(last: Option<&Value>) -> u8 {
    match last {
        Some(Value::Bool(false) | Value::Null) => 1,
        Some(_) => 0,
        None => 4,
    }
}
