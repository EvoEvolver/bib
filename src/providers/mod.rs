mod crossref;
mod doi;
mod openreview;

use anyhow::{Result, bail};

use crate::catalog::{BibliographicQuery, Candidate, LiteratureIdentifier, LiteratureRecord};

pub const DEFAULT_PROVIDER: &str = "crossref";
pub const DOI_PROVIDER: &str = "doi";
pub const OPENREVIEW_PROVIDER: &str = "openreview";

#[derive(Clone, Debug)]
pub struct FetchedRecord {
    pub record: LiteratureRecord,
    pub request_url: String,
    pub media_type: String,
    pub response: Vec<u8>,
    pub request_method: String,
    pub request_body_sha256: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FetchedSearch {
    pub candidates: Vec<Candidate>,
    pub request_url: String,
    pub media_type: String,
    pub response: Vec<u8>,
}

pub trait LiteratureProvider {
    fn name(&self) -> &'static str;
    fn lookup(&self, identifier: &LiteratureIdentifier) -> Result<FetchedRecord>;
    fn search(&self, query: &BibliographicQuery, limit: usize) -> Result<FetchedSearch>;
}

pub fn names() -> &'static [&'static str] {
    &[DEFAULT_PROVIDER, DOI_PROVIDER, OPENREVIEW_PROVIDER]
}

pub fn open(name: &str, mailto: Option<&str>) -> Result<Box<dyn LiteratureProvider>> {
    match name.to_ascii_lowercase().as_str() {
        DEFAULT_PROVIDER => Ok(Box::new(crossref::CrossrefProvider::new(mailto)?)),
        DOI_PROVIDER => Ok(Box::new(doi::DoiProvider::new()?)),
        OPENREVIEW_PROVIDER => Ok(Box::new(openreview::OpenReviewProvider::new()?)),
        _ => bail!(
            "unknown literature provider: {name}; available providers: {}",
            names().join(", ")
        ),
    }
}

pub fn record_from_evidence(
    provider: &str,
    provider_id: &str,
    response: &[u8],
) -> Result<LiteratureRecord> {
    match provider.to_ascii_lowercase().as_str() {
        DEFAULT_PROVIDER => crossref::record_from_response(response),
        DOI_PROVIDER => doi::record_from_response(provider_id, response),
        OPENREVIEW_PROVIDER => openreview::record_from_response(provider_id, response),
        _ => bail!("cannot validate evidence from unknown provider: {provider}"),
    }
}
