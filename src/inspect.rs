use anyhow::Result;
use serde::Serialize;

use crate::bibtex::Record;
use crate::integrity::{Status, hash, status};
use crate::provenance::{self, SourceSummary};

#[derive(Serialize)]
struct IntegrityView<'a> {
    status: Status,
    expected: String,
    stored: Option<&'a str>,
    source: SourceSummary,
}

#[derive(Serialize)]
struct EntryView<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    entry_type: &'a str,
    fields: &'a std::collections::BTreeMap<String, String>,
    integrity: IntegrityView<'a>,
}

pub fn document(records: &[Record]) -> Result<serde_json::Value> {
    let entries = records
        .iter()
        .filter(|record| !record.is_provenance())
        .map(|record| {
            Ok(EntryView {
                id: &record.entry_key,
                entry_type: &record.entry_type,
                fields: &record.fields,
                integrity: IntegrityView {
                    status: status(record, records)?,
                    expected: hash(record)?,
                    stored: record.fields.get("integrity").map(String::as_str),
                    source: provenance::summary(record, records),
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_value(entries)?)
}

#[cfg(test)]
mod tests {
    use crate::bibtex::parse;

    use super::*;

    #[test]
    fn document_excludes_provenance_entries() {
        let records =
            parse("@article{one, title={One}}\n@bibsource{source, kind={agent}, actor={test}}")
                .unwrap();
        let value = document(&records).unwrap();
        let entries = value.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["id"], "one");
        assert_eq!(entries[0]["integrity"]["status"], "unverified");
    }
}
