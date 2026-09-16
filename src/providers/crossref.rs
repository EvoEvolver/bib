use std::time::Duration;
use std::{thread, time};

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::Url;
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::catalog::{
    BibliographicQuery, Candidate, Contributor, LiteratureIdentifier, LiteratureRecord,
    PublicationDate,
};

use super::{FetchedRecord, LiteratureProvider};

const DEFAULT_BASE_URL: &str = "https://api.crossref.org/";

pub struct CrossrefProvider {
    client: Client,
    base_url: Url,
    mailto: Option<String>,
}

impl CrossrefProvider {
    pub fn new(mailto: Option<&str>) -> Result<Self> {
        let base_url =
            std::env::var("BIB_CROSSREF_API_BASE").unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned());
        let base_url = Url::parse(&base_url).context("invalid Crossref API base URL")?;
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(format!(
                "bib/{} (https://github.com/EvoEvolver/bib{})",
                env!("CARGO_PKG_VERSION"),
                mailto
                    .map(|value| format!("; mailto:{value}"))
                    .unwrap_or_default()
            ))
            .build()
            .context("could not initialize HTTP client")?;
        Ok(Self {
            client,
            base_url,
            mailto: mailto.map(str::to_owned),
        })
    }

    fn works_url(&self, id: Option<&str>) -> Result<Url> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Crossref API base URL cannot be a base"))?;
            segments.pop_if_empty();
            segments.push("works");
            if let Some(id) = id {
                segments.push(normalize_doi(id));
            }
        }
        Ok(url)
    }

    fn get(&self, mut url: Url) -> Result<(String, Vec<u8>)> {
        if let Some(mailto) = &self.mailto {
            url.query_pairs_mut().append_pair("mailto", mailto);
        }
        for attempt in 0..3 {
            let response = self
                .client
                .get(url.clone())
                .send()
                .with_context(|| format!("could not contact Crossref at {url}"))?;
            let status = response.status();
            if status.is_success() {
                let final_url = response.url().clone();
                let bytes = response
                    .bytes()
                    .with_context(|| format!("could not read Crossref response from {url}"))?;
                return Ok((sanitized_url(final_url), bytes.to_vec()));
            }
            if (status.as_u16() == 429 || status.is_server_error()) && attempt < 2 {
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(1 << attempt)
                    .clamp(1, 30);
                thread::sleep(time::Duration::from_secs(retry_after));
                continue;
            }
            bail!("Crossref returned HTTP {status} for {url}");
        }
        unreachable!("retry loop always returns or fails")
    }
}

impl LiteratureProvider for CrossrefProvider {
    fn name(&self) -> &'static str {
        "crossref"
    }

    fn lookup(&self, identifier: &LiteratureIdentifier) -> Result<FetchedRecord> {
        let (request_url, response) = self.get(self.works_url(Some(identifier.value()))?)?;
        let record = record_from_response(&response)?;
        Ok(FetchedRecord {
            record,
            request_url,
            media_type: "application/vnd.crossref-api-message+json".to_owned(),
            response,
        })
    }

    fn search(&self, query: &BibliographicQuery, limit: usize) -> Result<Vec<Candidate>> {
        if query.citation.trim().is_empty() {
            bail!("cannot search without bibliographic metadata");
        }
        let mut url = self.works_url(None)?;
        url.query_pairs_mut()
            .append_pair("query.bibliographic", &query.citation)
            .append_pair("rows", &limit.clamp(1, 20).to_string());
        let (_, response) = self.get(url)?;
        let response: SearchResponse =
            serde_json::from_slice(&response).context("invalid Crossref search response")?;
        Ok(response
            .message
            .items
            .into_iter()
            .map(|item| Candidate {
                score: item.score,
                record: item.work.into_record(self.name()),
            })
            .collect())
    }
}

pub(crate) fn record_from_response(response: &[u8]) -> Result<LiteratureRecord> {
    let response: SingletonResponse =
        serde_json::from_slice(response).context("invalid Crossref singleton response")?;
    Ok(response.message.into_record("crossref"))
}

fn sanitized_url(mut url: Url) -> String {
    if url.query().is_some() {
        let retained = url
            .query_pairs()
            .filter(|(key, _)| key != "mailto")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        if !retained.is_empty() {
            url.query_pairs_mut().extend_pairs(retained);
        }
    }
    url.to_string()
}

fn normalize_doi(value: &str) -> &str {
    let value = value.trim();
    let value = value
        .strip_prefix("https://doi.org/")
        .or_else(|| value.strip_prefix("http://doi.org/"))
        .or_else(|| value.strip_prefix("doi:"))
        .unwrap_or(value);
    value.trim()
}

#[derive(Deserialize)]
struct SingletonResponse {
    message: CrossrefWork,
}

#[derive(Deserialize)]
struct SearchResponse {
    message: SearchMessage,
}

#[derive(Deserialize)]
struct SearchMessage {
    #[serde(default)]
    items: Vec<ScoredWork>,
}

#[derive(Deserialize)]
struct ScoredWork {
    #[serde(default)]
    score: Option<f64>,
    #[serde(flatten)]
    work: CrossrefWork,
}

#[derive(Default, Deserialize)]
struct CrossrefWork {
    #[serde(rename = "DOI")]
    doi: String,
    #[serde(rename = "type", default)]
    record_type: String,
    #[serde(default)]
    title: Vec<String>,
    #[serde(default)]
    author: Vec<CrossrefContributor>,
    #[serde(default)]
    editor: Vec<CrossrefContributor>,
    #[serde(rename = "container-title", default)]
    container_title: Vec<String>,
    publisher: Option<String>,
    issued: Option<CrossrefDate>,
    #[serde(rename = "published-print")]
    published_print: Option<CrossrefDate>,
    #[serde(rename = "published-online")]
    published_online: Option<CrossrefDate>,
    volume: Option<String>,
    issue: Option<String>,
    page: Option<String>,
    #[serde(rename = "article-number")]
    article_number: Option<String>,
    #[serde(rename = "URL")]
    url: Option<String>,
    #[serde(rename = "ISBN", default)]
    isbn: Vec<String>,
    #[serde(rename = "ISSN", default)]
    issn: Vec<String>,
}

impl CrossrefWork {
    fn into_record(self, provider: &str) -> LiteratureRecord {
        let issued = self
            .published_print
            .or(self.published_online)
            .or(self.issued)
            .and_then(CrossrefDate::into_date);
        LiteratureRecord {
            provider: provider.to_owned(),
            id: normalize_doi(&self.doi).to_owned(),
            record_type: self.record_type,
            title: self.title.first().map(|value| clean_markup(value)),
            authors: self
                .author
                .into_iter()
                .map(CrossrefContributor::into_contributor)
                .collect(),
            editors: self
                .editor
                .into_iter()
                .map(CrossrefContributor::into_contributor)
                .collect(),
            container_title: self
                .container_title
                .first()
                .map(|value| clean_markup(value)),
            publisher: self.publisher.map(|value| clean_markup(&value)),
            issued,
            volume: self.volume,
            issue: self.issue,
            pages: self.page,
            article_number: self.article_number,
            doi: Some(normalize_doi(&self.doi).to_owned()),
            url: self.url,
            isbn: self.isbn,
            issn: self.issn,
        }
    }
}

#[derive(Default, Deserialize)]
struct CrossrefContributor {
    #[serde(default)]
    family: String,
    given: Option<String>,
    #[serde(rename = "ORCID")]
    orcid: Option<String>,
    name: Option<String>,
}

impl CrossrefContributor {
    fn into_contributor(self) -> Contributor {
        Contributor {
            family: if self.family.is_empty() {
                self.name.unwrap_or_default()
            } else {
                clean_markup(&self.family)
            },
            given: self.given.map(|value| clean_markup(&value)),
            orcid: self
                .orcid
                .map(|value| value.trim_start_matches("https://orcid.org/").to_owned()),
        }
    }
}

#[derive(Default, Deserialize)]
struct CrossrefDate {
    #[serde(rename = "date-parts", default)]
    date_parts: Vec<Vec<Option<i32>>>,
}

impl CrossrefDate {
    fn into_date(self) -> Option<PublicationDate> {
        let parts = self.date_parts.first()?;
        Some(PublicationDate {
            year: parts.first().copied().flatten()?,
            month: parts
                .get(1)
                .copied()
                .flatten()
                .and_then(|value| u8::try_from(value).ok()),
            day: parts
                .get(2)
                .copied()
                .flatten()
                .and_then(|value| u8::try_from(value).ok()),
        })
    }
}

fn clean_markup(value: &str) -> String {
    let wrapped = format!("<root>{value}</root>");
    let mut reader = Reader::from_str(&wrapped);
    reader.config_mut().trim_text(false);
    let mut output = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => match text.decode() {
                Ok(text) => output.push_str(&text),
                Err(_) => return value.to_owned(),
            },
            Ok(Event::CData(text)) => match text.decode() {
                Ok(text) => output.push_str(&text),
                Err(_) => return value.to_owned(),
            },
            Ok(Event::Eof) => break,
            Err(_) => return value.to_owned(),
            _ => {}
        }
    }
    output.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_crossref_json_to_common_record() {
        let work: CrossrefWork = serde_json::from_str(
            r#"{
                "DOI":"10.1000/Test",
                "type":"journal-article",
                "title":["A <i>useful</i> result"],
                "author":[{"given":"Jane","family":"Doe","ORCID":"https://orcid.org/0000-0001"}],
                "container-title":["Example Journal"],
                "published-print":{"date-parts":[[2025,4,2]]},
                "volume":"7", "issue":"2", "page":"10-20",
                "URL":"https://doi.org/10.1000/Test"
            }"#,
        )
        .unwrap();
        let record = work.into_record("crossref");
        assert_eq!(record.id, "10.1000/Test");
        assert_eq!(record.title.as_deref(), Some("A useful result"));
        assert_eq!(record.issued.as_ref().map(|date| date.year), Some(2025));
        assert_eq!(record.bibtex_fields()["author"], "Doe, Jane");
    }

    #[test]
    fn normalizes_common_doi_forms() {
        assert_eq!(normalize_doi("https://doi.org/10.1/ABC"), "10.1/ABC");
        assert_eq!(normalize_doi("doi:10.1/ABC"), "10.1/ABC");
    }

    #[test]
    fn ignores_crossref_dates_with_null_years() {
        let work: CrossrefWork =
            serde_json::from_str(r#"{"DOI":"10.1000/test","issued":{"date-parts":[[null]]}}"#)
                .unwrap();
        assert!(work.into_record("crossref").issued.is_none());
    }
}
