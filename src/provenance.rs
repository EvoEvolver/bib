use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bibtex::{Record, render};
use crate::integrity::content_hash;
use crate::providers::FetchedRecord;
use crate::resolver::{self, ResolutionCandidate};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ResolutionSummary>,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolutionSummary {
    pub key: String,
    pub input_url: Option<String>,
    pub method: Option<String>,
    pub identifier_kind: Option<String>,
    pub identifier: Option<String>,
    pub confidence: Option<String>,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceTrace {
    pub target: String,
    pub provider: EvidenceNode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<EvidenceNode>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceNode {
    pub key: String,
    pub fields: BTreeMap<String, String>,
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
    provider_source_with_resolution(fetched, None)
}

pub fn provider_source_with_resolution(
    fetched: &FetchedRecord,
    resolution_key: Option<&str>,
) -> Record {
    let response_sha256 = sha256(&fetched.response);
    let projection = "literature-record-v1";
    let projection_sha256 = projection_hash(
        fetched.record.bibtex_type(),
        &fetched.record.bibtex_fields(),
    );
    let tool_version = env!("CARGO_PKG_VERSION");
    let identity = provider_identity(ProviderIdentity {
        provider: &fetched.record.provider,
        provider_id: &fetched.record.id,
        request_url: &fetched.request_url,
        media_type: &fetched.media_type,
        projection,
        tool_version,
        response_sha256: &response_sha256,
        projection_sha256: Some(&projection_sha256),
        resolution_key,
    });
    let key = format!("bibsource:provider:{identity}");
    let mut fields = BTreeMap::from([
        ("kind".to_owned(), "provider".to_owned()),
        ("provider".to_owned(), fetched.record.provider.clone()),
        ("providerid".to_owned(), fetched.record.id.clone()),
        ("requesturl".to_owned(), fetched.request_url.clone()),
        ("mediatype".to_owned(), fetched.media_type.clone()),
        ("projection".to_owned(), projection.to_owned()),
        ("projectionsha256".to_owned(), projection_sha256),
        ("responsesha256".to_owned(), response_sha256),
        ("toolversion".to_owned(), tool_version.to_owned()),
    ]);
    if let Some(resolution_key) = resolution_key {
        fields.insert("resolution".to_owned(), resolution_key.to_owned());
    }
    Record {
        entry_type: SOURCE_TYPE.to_owned(),
        entry_key: key,
        fields,
    }
}

pub fn resolution_source(candidate: &ResolutionCandidate) -> Result<Record> {
    let evidence = &candidate.evidence;
    let mut fields = BTreeMap::from([
        ("kind".to_owned(), "resolution".to_owned()),
        ("method".to_owned(), evidence.method.clone()),
        ("inputurl".to_owned(), evidence.input_url.clone()),
        ("finalurl".to_owned(), evidence.final_url.clone()),
        ("identifierkind".to_owned(), candidate.kind.clone()),
        ("identifier".to_owned(), candidate.value.clone()),
        (
            "confidence".to_owned(),
            candidate.confidence.as_str().to_owned(),
        ),
        (
            "signals".to_owned(),
            serde_json::to_string(&candidate.signals)?,
        ),
        (
            "toolversion".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
        ),
    ]);
    if let Some(request_url) = &evidence.request_url {
        fields.insert("requesturl".to_owned(), request_url.clone());
    }
    if let Some(media_type) = &evidence.media_type {
        fields.insert("mediatype".to_owned(), media_type.clone());
    }
    if let Some(response_sha256) = &evidence.response_sha256 {
        if evidence.response_bytes == 0 {
            bail!("resolution response hash has no response byte count");
        }
        fields.insert("responsesha256".to_owned(), response_sha256.clone());
        fields.insert(
            "responsebytes".to_owned(),
            evidence.response_bytes.to_string(),
        );
    }
    let identity = resolution_identity(&fields);
    Ok(Record {
        entry_type: SOURCE_TYPE.to_owned(),
        entry_key: format!("bibsource:resolution:{identity}"),
        fields,
    })
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
        "provider" => validate_provider(record, source, records),
        "agent" => validate_actor(record, source, "agent"),
        "human" => validate_actor(record, source, "human"),
        "resolution" => bail!("resolution evidence cannot directly verify a BibTeX entry"),
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
    let resolution = source
        .and_then(|source| source.fields.get("resolution"))
        .map(|key| resolution_summary(key, records));
    SourceSummary {
        key,
        kind: source.and_then(|source| source.fields.get("kind").cloned()),
        actor: source.and_then(|source| source.fields.get("actor").cloned()),
        provider: source.and_then(|source| source.fields.get("provider").cloned()),
        provider_id: source.and_then(|source| source.fields.get("providerid").cloned()),
        resolution,
        valid: validation.is_ok(),
        error: validation.err().map(|error| format!("{error:#}")),
    }
}

pub fn trace(record: &Record, records: &[Record]) -> Result<EvidenceTrace> {
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
        bail!("source trace is only available for provider provenance");
    }
    let resolution = source
        .fields
        .get("resolution")
        .map(|key| {
            let record = records
                .iter()
                .find(|candidate| candidate.is_provenance() && candidate.entry_key == *key)
                .with_context(|| format!("resolution evidence not found: {key}"))?;
            Ok::<EvidenceNode, anyhow::Error>(EvidenceNode {
                key: record.entry_key.clone(),
                fields: public_evidence_fields(record),
            })
        })
        .transpose()?;
    Ok(EvidenceTrace {
        target: record.entry_key.clone(),
        provider: EvidenceNode {
            key: source.entry_key.clone(),
            fields: public_evidence_fields(source),
        },
        resolution,
    })
}

fn validate_provider(record: &Record, source: &Record, records: &[Record]) -> Result<()> {
    let provider = required(source, "provider")?;
    let provider_id = required(source, "providerid")?;
    if required(source, "projection")? != "literature-record-v1" {
        bail!("unsupported provider projection");
    }
    let expected_key = format!(
        "bibsource:provider:{}",
        provider_identity(ProviderIdentity {
            provider,
            provider_id,
            request_url: required(source, "requesturl")?,
            media_type: required(source, "mediatype")?,
            projection: required(source, "projection")?,
            tool_version: required(source, "toolversion")?,
            response_sha256: required_sha256(source, "responsesha256")?,
            projection_sha256: source.fields.get("projectionsha256").map(String::as_str),
            resolution_key: source.fields.get("resolution").map(String::as_str),
        })
    );
    if source.entry_key != expected_key {
        bail!("provider provenance key does not match its metadata");
    }
    if let Some(resolution_key) = source.fields.get("resolution") {
        let resolution = records
            .iter()
            .find(|candidate| candidate.is_provenance() && candidate.entry_key == *resolution_key)
            .with_context(|| format!("resolution evidence not found: {resolution_key}"))?;
        validate_resolution(resolution)?;
        if required(resolution, "identifierkind")? == "doi"
            && !required(resolution, "identifier")?.eq_ignore_ascii_case(provider_id)
        {
            bail!("resolved DOI does not match provider record id");
        }
    }
    if record.fields.get("bibprovider").map(String::as_str) != Some(provider)
        || record.fields.get("bibproviderid").map(String::as_str) != Some(provider_id)
    {
        bail!("BibTeX provider identity no longer matches provider receipt");
    }
    if source.fields.contains_key("projectionsha256") {
        let expected = required_sha256(source, "projectionsha256")?;
        let fields = CONTROLLED_FIELDS
            .iter()
            .filter_map(|field| {
                record
                    .fields
                    .get(*field)
                    .map(|value| ((*field).to_owned(), value.clone()))
            })
            .collect();
        if projection_hash(&record.entry_type, &fields) != *expected {
            bail!("BibTeX fields no longer match the recorded provider projection");
        }
    }
    Ok(())
}

fn resolution_summary(key: &str, records: &[Record]) -> ResolutionSummary {
    let source = records
        .iter()
        .find(|candidate| candidate.is_provenance() && candidate.entry_key == key);
    let validation = source
        .context("resolution evidence entry not found")
        .and_then(validate_resolution);
    ResolutionSummary {
        key: key.to_owned(),
        input_url: source.and_then(|source| source.fields.get("inputurl").cloned()),
        method: source.and_then(|source| source.fields.get("method").cloned()),
        identifier_kind: source.and_then(|source| source.fields.get("identifierkind").cloned()),
        identifier: source.and_then(|source| source.fields.get("identifier").cloned()),
        confidence: source.and_then(|source| source.fields.get("confidence").cloned()),
        valid: validation.is_ok(),
        error: validation.err().map(|error| format!("{error:#}")),
    }
}

fn validate_resolution(source: &Record) -> Result<()> {
    if required(source, "kind")? != "resolution" {
        bail!("referenced resolution evidence has the wrong kind");
    }
    if source.entry_key
        != format!(
            "bibsource:resolution:{}",
            resolution_identity(&source.fields)
        )
    {
        bail!("resolution provenance key does not match its metadata");
    }
    if !matches!(required(source, "confidence")?, "exact" | "strong") {
        bail!("resolution evidence has an invalid confidence");
    }
    let expected = (
        required(source, "identifierkind")?.to_owned(),
        required(source, "identifier")?.to_ascii_lowercase(),
    );
    match required(source, "method")? {
        "url-doi" => {
            if !resolver::identifiers_in_url(required(source, "inputurl")?).contains(&expected) {
                bail!("resolution URL does not contain the recorded identifier");
            }
        }
        "arxiv-url" => {
            if required(source, "identifierkind")? != "arxiv"
                || resolver::arxiv_id_in_url(required(source, "inputurl")?).as_deref()
                    != Some(required(source, "identifier")?)
            {
                bail!("resolution URL does not contain the recorded arXiv identifier");
            }
        }
        "html-metadata" | "arxiv-atom" => {
            required(source, "requesturl")?;
            required(source, "finalurl")?;
            required(source, "mediatype")?;
            required_sha256(source, "responsesha256")?;
            if required(source, "responsebytes")?
                .parse::<usize>()
                .unwrap_or(0)
                == 0
            {
                bail!("resolution receipt has an invalid response byte count");
            }
        }
        other => bail!("unsupported resolution method: {other}"),
    }
    let signals: Vec<resolver::MatchSignal> = serde_json::from_str(required(source, "signals")?)
        .context("resolution signals are not valid JSON")?;
    if signals.is_empty()
        || signals.iter().any(|signal| {
            signal.value.to_ascii_lowercase() != expected.1 || signal.kind.trim().is_empty()
        })
    {
        bail!("resolution signals do not match the recorded identifier");
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

struct ProviderIdentity<'a> {
    provider: &'a str,
    provider_id: &'a str,
    request_url: &'a str,
    media_type: &'a str,
    projection: &'a str,
    tool_version: &'a str,
    response_sha256: &'a str,
    projection_sha256: Option<&'a str>,
    resolution_key: Option<&'a str>,
}

fn provider_identity(value: ProviderIdentity<'_>) -> String {
    let ProviderIdentity {
        provider,
        provider_id,
        request_url,
        media_type,
        projection,
        tool_version,
        response_sha256,
        projection_sha256,
        resolution_key,
    } = value;
    let mut identity = format!(
        "provider\0{provider}\0{provider_id}\0{request_url}\0{media_type}\0{projection}\0{tool_version}\0{response_sha256}"
    );
    if let Some(projection_sha256) = projection_sha256 {
        identity.push_str("\0projectionsha256\0");
        identity.push_str(projection_sha256);
    }
    if let Some(resolution_key) = resolution_key {
        identity.push_str("\0resolution\0");
        identity.push_str(resolution_key);
    }
    sha256(identity.as_bytes())
}

fn resolution_identity(fields: &BTreeMap<String, String>) -> String {
    let mut input = Vec::new();
    for (field, value) in fields {
        if matches!(field.as_str(), "response" | "responseencoding") {
            continue;
        }
        input.extend_from_slice(field.len().to_string().as_bytes());
        input.push(0);
        input.extend_from_slice(field.as_bytes());
        input.extend_from_slice(value.len().to_string().as_bytes());
        input.push(0);
        input.extend_from_slice(value.as_bytes());
    }
    sha256(&input)
}

fn projection_hash(entry_type: &str, fields: &BTreeMap<String, String>) -> String {
    let payload = (entry_type.to_ascii_lowercase(), fields);
    sha256(&serde_json::to_vec(&payload).expect("string projection is always serializable"))
}

fn public_evidence_fields(record: &Record) -> BTreeMap<String, String> {
    record
        .fields
        .iter()
        .filter(|(field, _)| !matches!(field.as_str(), "response" | "responseencoding"))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect()
}

fn required<'a>(record: &'a Record, field: &str) -> Result<&'a str> {
    record
        .fields
        .get(field)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("provenance entry {} is missing {field}", record.entry_key))
}

fn required_sha256<'a>(record: &'a Record, field: &str) -> Result<&'a str> {
    let value = required(record, field)?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("provenance entry {} has invalid {field}", record.entry_key);
    }
    Ok(value)
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
