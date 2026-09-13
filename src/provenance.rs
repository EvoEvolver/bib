use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bibtex::{Record, render};
use crate::integrity::content_hash;
use crate::providers::{self, FetchedRecord};

pub const SOURCE_FIELD: &str = "bibsource";
pub const SOURCE_TYPE: &str = "bibsource";

pub const CONTROLLED_FIELDS: &[&str] = &[
    "title",
    "author",
    "editor",
    "journal",
    "booktitle",
    "publisher",
    "year",
    "month",
    "volume",
    "number",
    "pages",
    "eid",
    "doi",
    "url",
    "isbn",
    "issn",
    "bibprovider",
    "bibproviderid",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    Provider,
    Agent,
    Human,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceSummary {
    pub key: Option<String>,
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Agent => "agent",
            Self::Human => "human",
        }
    }
}

pub fn provider_source(fetched: &FetchedRecord) -> Record {
    let response_sha256 = sha256(&fetched.response);
    let projection = "literature-record-v1";
    let tool_version = env!("CARGO_PKG_VERSION");
    let identity = provider_identity(
        &fetched.record.provider,
        &fetched.record.id,
        &fetched.request_url,
        &fetched.media_type,
        projection,
        tool_version,
        &response_sha256,
    );
    let key = format!("bibsource:provider:{identity}");
    Record {
        entry_type: SOURCE_TYPE.to_owned(),
        entry_key: key,
        fields: BTreeMap::from([
            ("kind".to_owned(), "provider".to_owned()),
            ("provider".to_owned(), fetched.record.provider.clone()),
            ("providerid".to_owned(), fetched.record.id.clone()),
            ("requesturl".to_owned(), fetched.request_url.clone()),
            ("mediatype".to_owned(), fetched.media_type.clone()),
            ("projection".to_owned(), projection.to_owned()),
            ("responseencoding".to_owned(), "base64".to_owned()),
            ("response".to_owned(), STANDARD.encode(&fetched.response)),
            ("responsesha256".to_owned(), response_sha256),
            ("toolversion".to_owned(), tool_version.to_owned()),
        ]),
    }
}

pub fn actor_source(kind: SourceKind, actor: &str, target: &Record) -> Result<Record> {
    if !matches!(kind, SourceKind::Agent | SourceKind::Human) {
        bail!("actor provenance must be agent or human");
    }
    let actor = actor.trim();
    if actor.is_empty() {
        bail!("source actor cannot be empty");
    }
    let snapshot = content_hash(target)?;
    let identity = sha256(
        format!(
            "{}\0{actor}\0{}\0{snapshot}",
            kind.as_str(),
            target.entry_key
        )
        .as_bytes(),
    );
    Ok(Record {
        entry_type: SOURCE_TYPE.to_owned(),
        entry_key: format!("bibsource:{}:{identity}", kind.as_str()),
        fields: BTreeMap::from([
            ("kind".to_owned(), kind.as_str().to_owned()),
            ("actor".to_owned(), actor.to_owned()),
            ("target".to_owned(), target.entry_key.clone()),
            ("contenthash".to_owned(), snapshot),
        ]),
    })
}

pub fn append_source(source: &str, record: &Record, records: &[Record]) -> Result<String> {
    if let Some(existing) = records
        .iter()
        .find(|item| item.entry_key == record.entry_key)
    {
        if existing == record {
            return Ok(source.to_owned());
        }
        bail!("provenance key collision: {}", record.entry_key);
    }
    let separator = if source.ends_with('\n') { "\n" } else { "\n\n" };
    Ok(format!(
        "{source}{separator}{}",
        render(std::slice::from_ref(record))?
    ))
}

pub fn validate(record: &Record, records: &[Record]) -> Result<()> {
    let source_key = record
        .fields
        .get(SOURCE_FIELD)
        .filter(|value| !value.trim().is_empty())
        .context("missing bibsource provenance reference")?;
    let source = records
        .iter()
        .find(|candidate| candidate.is_provenance() && candidate.entry_key == *source_key)
        .with_context(|| format!("referenced provenance entry not found: {source_key}"))?;
    match required(source, "kind")? {
        "provider" => validate_provider(record, source),
        "agent" => validate_actor(record, source, "agent"),
        "human" => validate_actor(record, source, "human"),
        other => bail!("unknown provenance kind {other:?}"),
    }
}

pub fn summary(record: &Record, records: &[Record]) -> SourceSummary {
    let key = record.fields.get(SOURCE_FIELD).cloned();
    let source = key.as_deref().and_then(|key| {
        records
            .iter()
            .find(|candidate| candidate.is_provenance() && candidate.entry_key == key)
    });
    let validation = validate(record, records);
    SourceSummary {
        key,
        kind: source.and_then(|source| source.fields.get("kind").cloned()),
        actor: source.and_then(|source| source.fields.get("actor").cloned()),
        provider: source.and_then(|source| source.fields.get("provider").cloned()),
        provider_id: source.and_then(|source| source.fields.get("providerid").cloned()),
        valid: validation.is_ok(),
        error: validation.err().map(|error| format!("{error:#}")),
    }
}

pub fn raw_response(record: &Record, records: &[Record]) -> Result<Vec<u8>> {
    validate(record, records)?;
    let source_key = record
        .fields
        .get(SOURCE_FIELD)
        .context("missing bibsource provenance reference")?;
    let source = records
        .iter()
        .find(|candidate| candidate.is_provenance() && candidate.entry_key == *source_key)
        .with_context(|| format!("referenced provenance entry not found: {source_key}"))?;
    if required(source, "kind")? != "provider" {
        bail!("source raw is only available for provider provenance");
    }
    STANDARD
        .decode(required(source, "response")?)
        .context("invalid base64 provider response")
}

fn validate_provider(record: &Record, source: &Record) -> Result<()> {
    if required(source, "responseencoding")? != "base64" {
        bail!("unsupported provenance response encoding");
    }
    let response = STANDARD
        .decode(required(source, "response")?)
        .context("invalid base64 provider response")?;
    if sha256(&response) != required(source, "responsesha256")? {
        bail!("provider response SHA-256 mismatch");
    }
    let provider = required(source, "provider")?;
    let provider_id = required(source, "providerid")?;
    let expected_key = format!(
        "bibsource:provider:{}",
        provider_identity(
            provider,
            provider_id,
            required(source, "requesturl")?,
            required(source, "mediatype")?,
            required(source, "projection")?,
            required(source, "toolversion")?,
            required(source, "responsesha256")?,
        )
    );
    if source.entry_key != expected_key {
        bail!("provider provenance key does not match its metadata");
    }
    let projected = providers::record_from_evidence(provider, provider_id, &response)?;
    if projected.provider != provider || projected.id != provider_id {
        bail!("provider response identity does not match provenance entry");
    }
    if !record
        .entry_type
        .eq_ignore_ascii_case(projected.bibtex_type())
    {
        bail!("BibTeX entry type no longer matches provider response");
    }
    let expected = projected.bibtex_fields();
    for field in CONTROLLED_FIELDS {
        if record.fields.get(*field) != expected.get(*field) {
            bail!("BibTeX field {field} no longer matches provider response");
        }
    }
    Ok(())
}

fn validate_actor(record: &Record, source: &Record, kind: &str) -> Result<()> {
    let actor = required(source, "actor")?;
    if required(source, "target")? != record.entry_key {
        bail!("{kind} provenance target does not match citation key");
    }
    if required(source, "contenthash")? != content_hash(record)? {
        bail!("{kind} provenance content snapshot no longer matches entry");
    }
    let expected_identity = sha256(
        format!(
            "{kind}\0{actor}\0{}\0{}",
            required(source, "target")?,
            required(source, "contenthash")?
        )
        .as_bytes(),
    );
    if source.entry_key != format!("bibsource:{kind}:{expected_identity}") {
        bail!("{kind} provenance key does not match its metadata");
    }
    Ok(())
}

fn provider_identity(
    provider: &str,
    provider_id: &str,
    request_url: &str,
    media_type: &str,
    projection: &str,
    tool_version: &str,
    response_sha256: &str,
) -> String {
    sha256(
        format!(
            "provider\0{provider}\0{provider_id}\0{request_url}\0{media_type}\0{projection}\0{tool_version}\0{response_sha256}"
        )
        .as_bytes(),
    )
}

fn required<'a>(record: &'a Record, field: &str) -> Result<&'a str> {
    record
        .fields
        .get(field)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("provenance entry {} is missing {field}", record.entry_key))
}

pub fn provenance_keys(records: &[Record]) -> BTreeSet<&str> {
    records
        .iter()
        .filter(|record| record.is_provenance())
        .map(|record| record.entry_key.as_str())
        .collect()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
