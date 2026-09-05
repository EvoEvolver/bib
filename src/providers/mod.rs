mod crossref;

use anyhow::{Result, bail};

use crate::catalog::{BibliographicQuery, Candidate, LiteratureIdentifier, LiteratureRecord};

pub const DEFAULT_PROVIDER: &str = "crossref";

pub trait LiteratureProvider {
    fn name(&self) -> &'static str;
    fn lookup(&self, identifier: &LiteratureIdentifier) -> Result<LiteratureRecord>;
    fn search(&self, query: &BibliographicQuery, limit: usize) -> Result<Vec<Candidate>>;
}

pub fn names() -> &'static [&'static str] {
    &[DEFAULT_PROVIDER]
}

pub fn open(name: &str, mailto: Option<&str>) -> Result<Box<dyn LiteratureProvider>> {
    match name.to_ascii_lowercase().as_str() {
        DEFAULT_PROVIDER => Ok(Box::new(crossref::CrossrefProvider::new(mailto)?)),
        _ => bail!(
            "unknown literature provider: {name}; available providers: {}",
            names().join(", ")
        ),
    }
}
