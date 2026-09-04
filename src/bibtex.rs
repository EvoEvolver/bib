use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_bibtex::de::Deserializer;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Record {
    pub entry_type: String,
    pub entry_key: String,
    pub fields: BTreeMap<String, String>,
}

impl Record {
    pub fn normalized(mut self) -> Self {
        self.entry_type.make_ascii_lowercase();
        self.fields = self
            .fields
            .into_iter()
            .map(|(key, value)| (key.to_ascii_lowercase(), value))
            .collect();
        self
    }
}

pub fn parse(source: &str) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for (index, record) in Deserializer::from_str(source)
        .into_iter_regular_entry::<Record>()
        .enumerate()
    {
        records.push(
            record
                .with_context(|| format!("could not parse BibTeX entry {}", index + 1))?
                .normalized(),
        );
    }

    let mut seen = BTreeSet::new();
    for record in &records {
        if !seen.insert(&record.entry_key) {
            bail!("duplicate citation key: {}", record.entry_key);
        }
    }
    Ok(records)
}

pub fn render(records: &[Record]) -> Result<String> {
    serde_bibtex::to_string(records).context("could not serialize BibTeX")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_macros_and_normalizes_names() {
        let source = r#"
            @string{venue = {A Conference}}
            @Article{Key, Title = {A {Great} Paper}, JOURNAL = venue}
        "#;
        let entries = parse(source).unwrap();
        assert_eq!(entries[0].entry_type, "article");
        assert_eq!(entries[0].fields["title"], "A {Great} Paper");
        assert_eq!(entries[0].fields["journal"], "A Conference");
    }
}
