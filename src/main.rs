use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use bib_cli::bibtex::{Record, parse};
use bib_cli::catalog::{
    BibliographicQuery, Candidate, FieldChange, LiteratureIdentifier, LiteratureRecord,
    PROVIDER_FIELD, PROVIDER_ID_FIELD, changes,
};
use bib_cli::integrity::{Status, atomic_write, hash, status, update_entry_fields, update_source};
use bib_cli::providers::{self, DEFAULT_PROVIDER};
use bib_cli::query;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;

const LONG_ABOUT: &str = "Reconcile BibTeX with pluggable literature metadata providers, query and transform entries with jq-compatible filters, then add integrity markers to records that have been explicitly reviewed. Crossref is the default provider. Query input is an array of entry objects; query results go to stdout and never overwrite input files.";

const AFTER_HELP: &str = r#"INPUT OBJECT
  {
    "id": "paper1",
    "type": "article",
    "fields": {"title": "A Paper", "year": "2026"},
    "integrity": {"status": "unverified", "expected": "...", "stored": null}
  }

QUERY EXAMPLES
  List citation keys that need review:
    bib -r '.[] | select(.integrity.status != "verified") | .id' refs.bib

  Build a compact review packet:
    bib -c '[.[] | {id, title: .fields.title, status: .integrity.status}]' refs.bib

  Read stdin or combine multiple files into one input array:
    bib -r '.[].id' -
    bib -r '.[].id' first.bib second.bib

LITERATURE SOURCES
  Providers map their native metadata into one common literature record. Crossref is
  the default backend; commands remain provider-neutral:
    bib source plan refs.bib --key paper1
    bib source apply refs.bib --key paper1 --in-place

  An entry with a DOI is looked up exactly. Without a DOI, plan returns ranked
  candidates and exits 3; inspect them and pass the chosen record id explicitly:
    bib source apply refs.bib --key paper1 --id 10.1234/example --in-place

  Apply writes bibprovider and bibproviderid fields and preserves citation keys, comments,
  string declarations, local fields, and fields absent from provider metadata. It
  never adds integrity; review the diff and approve the entry separately.

EDITING
  Filters can change entry objects. Use --bibtex to serialize them back to BibTeX:
    bib --bibtex 'map(if .id == "paper1" then .fields.year = "2026" else . end)' refs.bib > updated.bib

  Query mode writes only to stdout. After replacing a file, changed entries with an
  existing marker report "stale". Review the result, then approve explicit keys:
    bib integrity status updated.bib
    bib integrity add updated.bib --key paper1 --in-place

INTEGRITY
  verified    Stored integrity matches the current covered fields.
  stale       A marker exists, but the covered fields have changed.
  unverified  No integrity marker exists.

  Run 'bib integrity --help' for status, hash, add, and remove commands. Adding
  integrity always requires one or more --key options or an explicit --all.

EXIT STATUS
  0  Success (or every selected entry is verified for 'integrity status')
  2  Invalid input, filter, or operational error
  3  Review or selection is needed, or integrity is stale/unverified
  With -e, query mode also follows jq result statuses: 1 for false/null, 4 for no result."#;

const SOURCE_AFTER_HELP: &str = r#"WORKFLOW
  1. Plan replacements and inspect exact matches or ranked candidates:
       bib source plan refs.bib --key paper1
  2. For a DOI-backed exact match, apply it directly. For search results, pass the
     chosen candidate id explicitly with --id:
       bib source apply refs.bib --key paper1 --id 10.1234/example --in-place
  3. Review the resulting entry, then record human approval separately:
       bib integrity add refs.bib --key paper1 --in-place

PROVENANCE
  Applied entries receive bibprovider = {name} and bibproviderid = {id}. These
  record where normalized metadata came from and are separate from the integrity
  marker. Crossref is the default provider. Set BIB_MAILTO or pass --mailto for
  polite API identification."#;

#[derive(Parser)]
#[command(
    name = "bib",
    version,
    about = "Reconcile, query, and verify BibTeX",
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
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
    /// Find and apply records from a literature metadata provider.
    Source(SourceArgs),
    /// Inspect, add, or remove integrity markers.
    Integrity(IntegrityArgs),
}

#[derive(Args)]
#[command(after_help = SOURCE_AFTER_HELP)]
struct SourceArgs {
    #[command(subcommand)]
    command: SourceCommand,
}

#[derive(Subcommand)]
enum SourceCommand {
    /// List installed literature metadata providers.
    Providers,
    /// Search a provider and return ranked records as JSON.
    Search {
        query: String,
        /// Metadata provider name.
        #[arg(long, default_value = DEFAULT_PROVIDER)]
        provider: String,
        /// Maximum candidate count.
        #[arg(long, default_value_t = 5, value_parser = parse_limit)]
        limit: usize,
        /// Emit compact JSON.
        #[arg(short, long)]
        compact: bool,
        /// Email sent to providers that support polite API identification.
        #[arg(long, env = "BIB_MAILTO")]
        mailto: Option<String>,
    },
    /// Plan provider replacements for explicitly selected BibTeX entries.
    Plan {
        file: PathBuf,
        /// Citation key to inspect. Repeat for multiple entries.
        #[arg(short, long)]
        key: Vec<String>,
        /// Inspect every entry.
        #[arg(long, conflicts_with = "key")]
        all: bool,
        /// Metadata provider name. Defaults to stored provenance, then Crossref.
        #[arg(long)]
        provider: Option<String>,
        /// Maximum candidates for entries without a provider identifier.
        #[arg(long, default_value_t = 5, value_parser = parse_limit)]
        limit: usize,
        /// Emit compact JSON.
        #[arg(short, long)]
        compact: bool,
        /// Email sent to providers that support polite API identification.
        #[arg(long, env = "BIB_MAILTO")]
        mailto: Option<String>,
    },
    /// Apply one exact provider record to one BibTeX entry.
    Apply {
        file: PathBuf,
        /// Citation key to update.
        #[arg(short, long)]
        key: String,
        /// Exact provider record identifier. Uses stored provenance or DOI when omitted.
        #[arg(long)]
        id: Option<String>,
        /// Metadata provider name. Defaults to stored provenance, then Crossref.
        #[arg(long)]
        provider: Option<String>,
        /// Atomically update FILE instead of writing the result to stdout.
        #[arg(short, long)]
        in_place: bool,
        /// Email sent to providers that support polite API identification.
        #[arg(long, env = "BIB_MAILTO")]
        mailto: Option<String>,
    },
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

#[derive(Serialize)]
struct PlannedCandidate {
    score: Option<f64>,
    record: LiteratureRecord,
    changes: Vec<FieldChange>,
}

#[derive(Serialize)]
struct PlanRow {
    id: String,
    provider: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    candidates: Vec<PlannedCandidate>,
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
            Command::Source(args) => run_source(args.command),
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

fn run_source(command: SourceCommand) -> Result<u8> {
    match command {
        SourceCommand::Providers => {
            for name in providers::names() {
                println!(
                    "{name}{}",
                    if *name == DEFAULT_PROVIDER {
                        "\tdefault"
                    } else {
                        ""
                    }
                );
            }
            Ok(0)
        }
        SourceCommand::Search {
            query,
            provider,
            limit,
            compact,
            mailto,
        } => {
            let backend = providers::open(&provider, mailto.as_deref())?;
            let candidates = backend.search(&BibliographicQuery { citation: query }, limit)?;
            print_serializable(&candidates, compact)?;
            Ok(if candidates.is_empty() { 3 } else { 0 })
        }
        SourceCommand::Plan {
            file,
            key,
            all,
            provider,
            limit,
            compact,
            mailto,
        } => {
            let source = read_file(&file)?;
            let records = parse(&source)?;
            let selected = select_records(&records, key, all)?;
            let mut rows = Vec::new();
            let mut needs_review = false;
            let mut backends = BTreeMap::new();
            for record in selected {
                let (provider_name, stored_id) = source_identity(record, provider.as_deref());
                if !backends.contains_key(&provider_name) {
                    match providers::open(&provider_name, mailto.as_deref()) {
                        Ok(backend) => {
                            backends.insert(provider_name.clone(), backend);
                        }
                        Err(error) => {
                            needs_review = true;
                            rows.push(PlanRow {
                                id: record.entry_key.clone(),
                                provider: provider_name.clone(),
                                status: "error",
                                query: None,
                                error: Some(format!("{error:#}")),
                                candidates: vec![],
                            });
                            continue;
                        }
                    }
                }
                let Some(backend) = backends.get(&provider_name) else {
                    unreachable!("provider was inserted above")
                };
                let identifier = stored_id.map(LiteratureIdentifier::ProviderId).or_else(|| {
                    record
                        .fields
                        .get("doi")
                        .cloned()
                        .map(LiteratureIdentifier::Doi)
                });
                if let Some(identifier) = identifier {
                    match backend.lookup(&identifier) {
                        Ok(candidate) => rows.push(PlanRow {
                            id: record.entry_key.clone(),
                            provider: provider_name.clone(),
                            status: "exact",
                            query: None,
                            error: None,
                            candidates: vec![planned_candidate(
                                record,
                                Candidate {
                                    score: None,
                                    record: candidate,
                                },
                            )],
                        }),
                        Err(error) => {
                            needs_review = true;
                            rows.push(PlanRow {
                                id: record.entry_key.clone(),
                                provider: provider_name.clone(),
                                status: "error",
                                query: None,
                                error: Some(format!("{error:#}")),
                                candidates: vec![],
                            });
                        }
                    }
                } else {
                    needs_review = true;
                    let query = BibliographicQuery::from_record(record);
                    match backend.search(&query, limit) {
                        Ok(candidates) => rows.push(PlanRow {
                            id: record.entry_key.clone(),
                            provider: provider_name.clone(),
                            status: "needs-selection",
                            query: Some(query.citation),
                            error: None,
                            candidates: candidates
                                .into_iter()
                                .map(|candidate| planned_candidate(record, candidate))
                                .collect(),
                        }),
                        Err(error) => rows.push(PlanRow {
                            id: record.entry_key.clone(),
                            provider: provider_name.clone(),
                            status: "error",
                            query: Some(query.citation),
                            error: Some(format!("{error:#}")),
                            candidates: vec![],
                        }),
                    }
                }
            }
            print_serializable(&rows, compact)?;
            Ok(if needs_review { 3 } else { 0 })
        }
        SourceCommand::Apply {
            file,
            key,
            id,
            provider,
            in_place,
            mailto,
        } => {
            let source = read_file(&file)?;
            let records = parse(&source)?;
            let record = records
                .iter()
                .find(|record| record.entry_key == key)
                .with_context(|| format!("citation key not found: {key}"))?;
            let (provider_name, stored_id) = source_identity(record, provider.as_deref());
            let identifier = id
                .map(LiteratureIdentifier::ProviderId)
                .or_else(|| stored_id.map(LiteratureIdentifier::ProviderId))
                .or_else(|| {
                    record
                        .fields
                        .get("doi")
                        .cloned()
                        .map(LiteratureIdentifier::Doi)
                })
                .context("no exact provider id; pass --id after selecting a search candidate")?;
            let backend = providers::open(&provider_name, mailto.as_deref())?;
            let provider_record = backend.lookup(&identifier)?;
            let fields = provider_record.bibtex_fields();
            let output =
                update_entry_fields(&source, &key, provider_record.bibtex_type(), &fields)?;
            parse(&output)
                .context("provider update produced invalid BibTeX; file was not changed")?;
            if in_place {
                atomic_write(&file, &output)?;
                eprintln!(
                    "updated {key} from {provider_name}:{} in {}; review it before adding integrity",
                    provider_record.id,
                    file.display()
                );
            } else {
                print!("{output}");
            }
            Ok(0)
        }
    }
}

fn planned_candidate(record: &Record, candidate: Candidate) -> PlannedCandidate {
    PlannedCandidate {
        score: candidate.score,
        changes: changes(record, &candidate.record),
        record: candidate.record,
    }
}

fn source_identity(record: &Record, requested: Option<&str>) -> (String, Option<String>) {
    let stored = record
        .fields
        .get(PROVIDER_FIELD)
        .zip(record.fields.get(PROVIDER_ID_FIELD))
        .map(|(provider, id)| (provider.as_str(), id.as_str()));
    match requested {
        Some(provider) => (
            provider.to_owned(),
            stored
                .and_then(|(stored_provider, id)| (stored_provider == provider).then_some(id))
                .map(str::to_owned),
        ),
        None => stored.map_or_else(
            || (DEFAULT_PROVIDER.to_owned(), None),
            |(provider, id)| (provider.to_owned(), Some(id.to_owned())),
        ),
    }
}

fn select_records(records: &[Record], keys: Vec<String>, all: bool) -> Result<Vec<&Record>> {
    let selected: BTreeSet<_> = keys.into_iter().collect();
    if !all && selected.is_empty() {
        bail!("no entries selected; pass --key KEY or --all");
    }
    ensure_keys_exist(records, &selected)?;
    Ok(records
        .iter()
        .filter(|record| all || selected.contains(&record.entry_key))
        .collect())
}

fn print_serializable(value: &impl Serialize, compact: bool) -> Result<()> {
    if compact {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
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

fn parse_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be an integer from 1 to 20".to_owned())?;
    (1..=20)
        .contains(&limit)
        .then_some(limit)
        .ok_or_else(|| "limit must be an integer from 1 to 20".to_owned())
}
