use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use bib_cli::bibtex::{Record, parse};
use bib_cli::catalog::{
    BibliographicQuery, Candidate, FieldChange, LiteratureIdentifier, LiteratureRecord,
    PROVIDER_FIELD, PROVIDER_ID_FIELD, changes,
};
use bib_cli::inspect;
use bib_cli::integrity::{
    Status, atomic_write, hash, status, update_entry_fields, update_entry_fields_exact,
    update_source,
};
use bib_cli::provenance::{self, CONTROLLED_FIELDS, SOURCE_FIELD, SourceKind};
use bib_cli::providers::{self, DEFAULT_PROVIDER};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

const LONG_ABOUT: &str = "Reconcile BibTeX with pluggable literature metadata providers, preserve raw API evidence, inspect entries as JSON, and add source-bound integrity markers. Every integrity marker must reference a separate @bibsource entry. Crossref is the default provider; DOI content negotiation is also built in.";

const AFTER_HELP: &str = r#"INSPECT AND PIPE
  Emit bibliography entries and integrity state as one JSON array:
    bib inspect refs.bib

  Use an external JSON processor when selection or transformation is useful:
    bib inspect refs.bib | jq -r '.[] | select(.integrity.status != "verified") | .id'

  Feed selected citation keys back to a controlled write command:
    bib inspect refs.bib | jq -r '.[].id' |
      bib integrity add refs.bib --keys-from - --source agent --agent MODEL --in-place

LITERATURE SOURCES
  Providers map their native metadata into one common literature record. Crossref is
  the default backend; commands remain provider-neutral:
    bib source plan refs.bib --key paper1
    bib source apply refs.bib --key paper1 --in-place

  An entry with a DOI is looked up exactly. Without a DOI, plan returns ranked
  candidates and exits 3; inspect them and pass the chosen record id explicitly:
    bib source apply refs.bib --key paper1 --id 10.1234/example --in-place

  Apply writes a separate @bibsource entry containing the exact API response, its
  SHA-256, request URL, media type, provider, and record id. To reconcile and seal
  an exact provider projection in one atomic operation:
    bib source apply refs.bib --key paper1 --add-integrity --in-place

  Use --provider doi for DOI content negotiation (raw application/x-bibtex) or
  --provider crossref for the Crossref works API (raw JSON).

SCOPE
  bib deliberately does not provide arbitrary metadata editing or an embedded jq
  implementation. Use normal editors, domain tools, and shell pipelines for data
  processing. Only source and integrity commands write trusted workflow fields.

INTEGRITY
  verified    Stored integrity matches the current covered fields.
  stale       A marker exists, but the covered fields have changed.
  unverified  No integrity marker exists.
  invalid     The marker matches, but provenance is missing, damaged, or inconsistent.

  Adding integrity always requires --source provider, --source agent --agent ID,
  or --source human --reviewer ID, plus --key, --keys-from, or --all.

EXIT STATUS
  0  Success (or every selected entry is verified for 'integrity status')
  2  Invalid input or operational error
  3  Review or selection is needed, or integrity is stale/unverified."#;

const INSPECT_AFTER_HELP: &str = r#"OUTPUT
  inspect writes one JSON array containing only bibliography entries. @bibsource
  evidence entries are omitted, but each bibliography entry includes a summary of
  its integrity and provenance state.

PIPELINE EXAMPLES
  bib inspect refs.bib | jq -r '.[].id'
  bib inspect refs.bib --compact |
    jq -r '.[] | select(.integrity.status != "verified") | .id'
  cat refs.bib | bib inspect -

jq is optional and external. bib itself does not evaluate filters or turn edited
JSON back into BibTeX."#;

const SOURCE_AFTER_HELP: &str = r#"WORKFLOW
  1. Plan replacements and inspect exact matches or ranked candidates:
       bib source plan refs.bib --key paper1
     Multiple keys may come from a newline-delimited pipeline:
       bib inspect refs.bib | jq -r '.[].id' |
         bib source plan refs.bib --keys-from -
  2. For a DOI-backed exact match, apply it directly. For search results, pass the
     chosen candidate id explicitly with --id:
       bib source apply refs.bib --key paper1 --id 10.1234/example --in-place
  3. Either add provider-backed integrity atomically with apply:
       bib source apply refs.bib --key paper1 --add-integrity --in-place
     or record an attributed review separately:
       bib integrity add refs.bib --key paper1 --source agent --agent MODEL --in-place

PROVENANCE
  Apply creates @bibsource evidence with the exact base64-encoded response and
  SHA-256. The literature entry references it through bibsource. Verification
  replays the provider projection offline and rejects changed raw data or metadata.
  Crossref is the default; doi is exact-lookup-only. Set BIB_MAILTO or pass
  --mailto for polite Crossref API identification. Recover verified response
  bytes with: bib source raw refs.bib --key paper1"#;

const INTEGRITY_AFTER_HELP: &str = r#"SOURCE MODES
  provider  Reuse @bibsource evidence created by `bib source apply`. Raw response,
            response hash, provider identity, and projected fields are validated.
  agent     Create @bibsource kind={agent}; requires --agent MODEL_OR_AGENT_ID.
  human     Create @bibsource kind={human}; requires --reviewer REVIEWER_ID.

EXAMPLES
  bib integrity add refs.bib --key paper1 --source provider --in-place
  bib integrity add refs.bib --key draft1 --source agent --agent claude-code --in-place
  bib integrity add refs.bib --key paper1 --source human --reviewer alice --in-place
  jq -r '.[].id' review.json | bib integrity add refs.bib --keys-from - \
    --source human --reviewer alice --in-place

Agent and human modes are attributed assertions, not cryptographic identities.
Provider mode proves deterministic agreement with the stored response bytes; it
does not prove that a manually forged response was genuinely served by the API."#;

#[derive(Parser)]
#[command(
    name = "bib",
    version,
    about = "Reconcile, inspect, and verify BibTeX",
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Emit bibliography entries and their trust state as JSON.
    Inspect(InspectArgs),
    /// Find and apply records from a literature metadata provider.
    Source(SourceArgs),
    /// Inspect, add, or remove integrity markers.
    Integrity(IntegrityArgs),
}

#[derive(Args)]
#[command(after_help = INSPECT_AFTER_HELP)]
struct InspectArgs {
    /// BibTeX files. Reads stdin when omitted or when FILE is `-`.
    files: Vec<PathBuf>,
    /// Explicitly request JSON output (already the default).
    #[arg(long)]
    json: bool,
    /// Emit the JSON array on one line.
    #[arg(short, long)]
    compact: bool,
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
        /// Read citation keys, one per line. Use `-` for stdin.
        #[arg(long, value_name = "FILE")]
        keys_from: Option<PathBuf>,
        /// Inspect every entry.
        #[arg(long, conflicts_with_all = ["key", "keys_from"])]
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
        /// Add provider-backed integrity after storing and validating the raw response.
        #[arg(long)]
        add_integrity: bool,
        /// Email sent to providers that support polite API identification.
        #[arg(long, env = "BIB_MAILTO")]
        mailto: Option<String>,
    },
    /// Write the validated, exact provider response bytes to stdout.
    Raw {
        file: PathBuf,
        /// Citation key whose provider evidence should be emitted.
        #[arg(short, long)]
        key: String,
    },
}

#[derive(Args)]
#[command(after_help = INTEGRITY_AFTER_HELP)]
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
        /// Read citation keys, one per line. Use `-` for stdin.
        #[arg(long, value_name = "FILE")]
        keys_from: Option<PathBuf>,
    },
    /// Print the expected integrity hash for one entry.
    Hash { file: PathBuf, key: String },
    /// Add or refresh integrity for explicitly reviewed entries.
    Add {
        file: PathBuf,
        /// Citation key to approve. Repeat for multiple entries.
        #[arg(short, long)]
        key: Vec<String>,
        /// Read citation keys, one per line. Use `-` for stdin.
        #[arg(long, value_name = "FILE")]
        keys_from: Option<PathBuf>,
        /// Approve every entry. This must be explicit.
        #[arg(long, conflicts_with_all = ["key", "keys_from"])]
        all: bool,
        /// Atomically update FILE instead of writing the result to stdout.
        #[arg(short, long)]
        in_place: bool,
        /// Required provenance kind.
        #[arg(long, value_enum)]
        source: IntegritySourceArg,
        /// Agent/model identifier, required with `--source agent`.
        #[arg(long)]
        agent: Option<String>,
        /// Human reviewer identifier, required with `--source human`.
        #[arg(long)]
        reviewer: Option<String>,
    },
    /// Remove integrity from selected entries.
    Remove {
        file: PathBuf,
        /// Citation key to unapprove. Repeat for multiple entries.
        #[arg(short, long)]
        key: Vec<String>,
        /// Read citation keys, one per line. Use `-` for stdin.
        #[arg(long, value_name = "FILE")]
        keys_from: Option<PathBuf>,
        /// Remove integrity from every entry.
        #[arg(long, conflicts_with_all = ["key", "keys_from"])]
        all: bool,
        /// Atomically update FILE instead of writing the result to stdout.
        #[arg(short, long)]
        in_place: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum IntegritySourceArg {
    Provider,
    Agent,
    Human,
}

#[derive(Serialize)]
struct StatusRow<'a> {
    id: &'a str,
    status: Status,
    expected: String,
    stored: Option<&'a str>,
    source: provenance::SourceSummary,
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
    match cli.command {
        Command::Inspect(args) => {
            let InspectArgs {
                files,
                json: _,
                compact,
            } = args;
            let records = read_bib_inputs(&files)?;
            print_serializable(&inspect::document(&records)?, compact)?;
            Ok(0)
        }
        Command::Source(args) => run_source(args.command),
        Command::Integrity(args) => run_integrity(args.command),
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
            keys_from,
            all,
            provider,
            limit,
            compact,
            mailto,
        } => {
            let source = read_file(&file)?;
            let records = parse(&source)?;
            let selected = select_records(&records, merge_keys(key, keys_from)?, all)?;
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
                        Ok(fetched) => rows.push(PlanRow {
                            id: record.entry_key.clone(),
                            provider: provider_name.clone(),
                            status: "exact",
                            query: None,
                            error: None,
                            candidates: vec![planned_candidate(
                                record,
                                Candidate {
                                    score: None,
                                    record: fetched.record,
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
            add_integrity,
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
            let fetched = backend.lookup(&identifier)?;
            let provider_source = provenance::provider_source(&fetched);
            let mut fields = fetched.record.bibtex_fields();
            fields.insert(SOURCE_FIELD.to_owned(), provider_source.entry_key.clone());
            let output = update_entry_fields_exact(
                &source,
                &key,
                fetched.record.bibtex_type(),
                &fields,
                CONTROLLED_FIELDS,
            )?;
            let records_after_fields = parse(&output)
                .context("provider update produced invalid BibTeX; file was not changed")?;
            let mut output =
                provenance::append_source(&output, &provider_source, &records_after_fields)?;
            if add_integrity {
                let records = parse(&output)?;
                output = update_source(&output, &records, &BTreeSet::from([key.clone()]), false)?;
            }
            if in_place {
                atomic_write(&file, &output)?;
                eprintln!(
                    "updated {key} from {provider_name}:{} in {}; raw response recorded as {}{}",
                    fetched.record.id,
                    file.display(),
                    provider_source.entry_key,
                    if add_integrity {
                        " and provider integrity added"
                    } else {
                        "; review it before adding integrity"
                    }
                );
            } else {
                print!("{output}");
            }
            Ok(0)
        }
        SourceCommand::Raw { file, key } => {
            let source = read_file(&file)?;
            let records = parse(&source)?;
            let record = records
                .iter()
                .find(|record| !record.is_provenance() && record.entry_key == key)
                .with_context(|| format!("citation key not found: {key}"))?;
            let response = provenance::raw_response(record, &records)
                .with_context(|| format!("provider evidence for {key} is not valid"))?;
            io::stdout()
                .write_all(&response)
                .context("could not write provider response to stdout")?;
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
        .filter(|record| !record.is_provenance() && (all || selected.contains(&record.entry_key)))
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
        IntegrityCommand::Status {
            file,
            json,
            key,
            keys_from,
        } => {
            let source = read_file(&file)?;
            let records = parse(&source)?;
            let selected: BTreeSet<_> = merge_keys(key, keys_from)?.into_iter().collect();
            ensure_keys_exist(&records, &selected)?;
            let bibliography: Vec<_> = records
                .iter()
                .filter(|record| {
                    !record.is_provenance()
                        && (selected.is_empty() || selected.contains(&record.entry_key))
                })
                .collect();
            let rows = bibliography
                .iter()
                .map(|record| {
                    Ok(StatusRow {
                        id: &record.entry_key,
                        status: status(record, &records)?,
                        expected: hash(record)?,
                        stored: record.fields.get("integrity").map(String::as_str),
                        source: provenance::summary(record, &records),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for row in &rows {
                    let origin = row
                        .source
                        .kind
                        .as_deref()
                        .map(|kind| format!("\t{kind}"))
                        .unwrap_or_default();
                    println!("{}\t{}{}", row.status, row.id, origin);
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
            keys_from,
            all,
            in_place,
            source,
            agent,
            reviewer,
        } => add_integrity(
            &file,
            merge_keys(key, keys_from)?,
            all,
            in_place,
            source,
            agent,
            reviewer,
        ),
        IntegrityCommand::Remove {
            file,
            key,
            keys_from,
            all,
            in_place,
        } => remove_integrity(&file, merge_keys(key, keys_from)?, all, in_place),
    }
}

fn add_integrity(
    file: &Path,
    keys: Vec<String>,
    all: bool,
    in_place: bool,
    source_kind: IntegritySourceArg,
    agent: Option<String>,
    reviewer: Option<String>,
) -> Result<u8> {
    let actor = match source_kind {
        IntegritySourceArg::Provider => {
            if agent.is_some() || reviewer.is_some() {
                bail!("--source provider does not accept --agent or --reviewer");
            }
            None
        }
        IntegritySourceArg::Agent => {
            if reviewer.is_some() {
                bail!("--source agent does not accept --reviewer");
            }
            Some((
                SourceKind::Agent,
                agent.context("--source agent requires --agent ID")?,
            ))
        }
        IntegritySourceArg::Human => {
            if agent.is_some() {
                bail!("--source human does not accept --agent");
            }
            Some((
                SourceKind::Human,
                reviewer.context("--source human requires --reviewer ID")?,
            ))
        }
    };
    let mut source = read_file(file)?;
    let mut records = parse(&source)?;
    let selected = selected_keys(&records, keys, all)?;
    if let Some((kind, actor)) = actor {
        for key in &selected {
            let target = records
                .iter()
                .find(|record| record.entry_key == *key)
                .cloned()
                .with_context(|| format!("citation key not found: {key}"))?;
            let evidence = provenance::actor_source(kind, &actor, &target)?;
            source = provenance::append_source(&source, &evidence, &records)?;
            source = update_entry_fields(
                &source,
                key,
                &target.entry_type,
                &BTreeMap::from([(SOURCE_FIELD.to_owned(), evidence.entry_key)]),
            )?;
            records = parse(&source)?;
        }
    }
    let output = update_source(&source, &records, &selected, false)?;
    write_changed(file, &output, &selected, in_place, "updated")
}

fn remove_integrity(file: &Path, keys: Vec<String>, all: bool, in_place: bool) -> Result<u8> {
    let source = read_file(file)?;
    let records = parse(&source)?;
    let selected = selected_keys(&records, keys, all)?;
    let output = update_source(&source, &records, &selected, true)?;
    write_changed(file, &output, &selected, in_place, "removed")
}

fn selected_keys(records: &[Record], keys: Vec<String>, all: bool) -> Result<BTreeSet<String>> {
    let selected = if all {
        records
            .iter()
            .filter(|record| !record.is_provenance())
            .map(|record| record.entry_key.clone())
            .collect()
    } else {
        let selected: BTreeSet<_> = keys.into_iter().collect();
        if selected.is_empty() {
            bail!("no entries selected; pass --key KEY or --all after review");
        }
        selected
    };
    ensure_keys_exist(records, &selected)?;
    Ok(selected)
}

fn write_changed(
    file: &Path,
    output: &str,
    selected: &BTreeSet<String>,
    in_place: bool,
    action: &str,
) -> Result<u8> {
    if in_place {
        atomic_write(file, output)?;
        eprintln!(
            "{action} integrity for {} entr{} in {}",
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
    let existing: BTreeSet<_> = records
        .iter()
        .filter(|record| !record.is_provenance())
        .map(|record| &record.entry_key)
        .collect();
    for key in selected {
        if !existing.contains(key) {
            bail!("citation key not found: {key}");
        }
    }
    Ok(())
}

fn merge_keys(mut keys: Vec<String>, keys_from: Option<PathBuf>) -> Result<Vec<String>> {
    let Some(path) = keys_from else {
        return Ok(keys);
    };
    let source = if path == Path::new("-") {
        read_stdin("citation keys")?
    } else {
        fs::read_to_string(&path)
            .with_context(|| format!("could not read citation keys from {}", path.display()))?
    };
    keys.extend(
        source
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned),
    );
    if keys.is_empty() {
        bail!("no citation keys found in {}", path.display());
    }
    Ok(keys)
}

fn read_bib_inputs(files: &[PathBuf]) -> Result<Vec<Record>> {
    if files.is_empty() {
        return parse(&read_stdin("BibTeX")?);
    }
    let mut records = Vec::new();
    for file in files {
        let source = if file == Path::new("-") {
            read_stdin("BibTeX")?
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

fn read_stdin(description: &str) -> Result<String> {
    if io::stdin().is_terminal() {
        bail!("no {description} on stdin");
    }
    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .context("could not read stdin")?;
    Ok(source)
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
