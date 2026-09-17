use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Url;
use reqwest::blocking::Client;

use crate::bibtex;
use crate::catalog::{BibliographicQuery, LiteratureIdentifier, LiteratureRecord};

use super::{FetchedRecord, FetchedSearch, LiteratureProvider};

const DEFAULT_BASE_URL: &str = "https://doi.org/";
const MEDIA_TYPE: &str = "application/x-bibtex";

pub struct DoiProvider {
    client: Client,
    base_url: Url,
}

impl DoiProvider {
    pub fn new() -> Result<Self> {
        let base_url =
            std::env::var("BIBLOCK_DOI_API_BASE").unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned());
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(format!(
                    "biblock/{} (https://github.com/EvoEvolver/biblock)",
                    env!("CARGO_PKG_VERSION")
                ))
                .build()
                .context("could not initialize HTTP client")?,
            base_url: Url::parse(&base_url).context("invalid DOI API base URL")?,
        })
    }

    fn url(&self, identifier: &LiteratureIdentifier) -> Result<Url> {
        let doi = normalize_doi(identifier.value());
        if doi.is_empty() {
            bail!("DOI identifier cannot be empty");
        }
        self.base_url
            .join(doi)
            .context("could not construct DOI content-negotiation URL")
    }
}

impl LiteratureProvider for DoiProvider {
    fn name(&self) -> &'static str {
        "doi"
    }

    fn lookup(&self, identifier: &LiteratureIdentifier) -> Result<FetchedRecord> {
        let url = self.url(identifier)?;
        let response = self
            .client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, MEDIA_TYPE)
            .send()
            .with_context(|| format!("could not resolve DOI at {url}"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("DOI resolver returned HTTP {status} for {url}");
        }
        let media_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or(MEDIA_TYPE)
            .to_owned();
        let bytes = response
            .bytes()
            .context("could not read DOI response")?
            .to_vec();
        let record = record_from_response(identifier.value(), &bytes)?;
        Ok(FetchedRecord {
            record,
            request_url: url.to_string(),
            media_type,
            response: bytes,
        })
    }

    fn search(&self, _query: &BibliographicQuery, _limit: usize) -> Result<FetchedSearch> {
        bail!("the doi provider supports exact DOI lookup only; use crossref for search")
    }
}

pub(crate) fn record_from_response(id: &str, response: &[u8]) -> Result<LiteratureRecord> {
    let source = std::str::from_utf8(response).context("DOI BibTeX response is not UTF-8")?;
    let mut records = bibtex::parse(source).context("invalid BibTeX from DOI resolver")?;
    if records.len() != 1 {
        bail!(
            "DOI resolver returned {} BibTeX entries; expected one",
            records.len()
        );
    }
    Ok(LiteratureRecord::from_bibtex_record(
        "doi",
        normalize_doi(id),
        &records.remove(0),
    ))
}

fn normalize_doi(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix("https://doi.org/")
        .or_else(|| value.strip_prefix("http://doi.org/"))
        .or_else(|| value.strip_prefix("doi:"))
        .unwrap_or(value)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_common_record_from_raw_bibtex() {
        let record = record_from_response(
            "10.1/example",
            b"@article{x, title={Raw title}, author={Doe, Jane}, year={2026}, doi={10.1/example}}",
        )
        .unwrap();
        assert_eq!(record.provider, "doi");
        assert_eq!(record.title.as_deref(), Some("Raw title"));
        assert_eq!(record.bibtex_fields()["author"], "Doe, Jane");
    }
}
