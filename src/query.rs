use anyhow::{Context, Result, anyhow};
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, data, unwrap_valr};
use jaq_json::Val;
use serde::Serialize;
use serde_json::Value;

use crate::bibtex::{Record, render};
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

pub fn input(records: &[Record]) -> Result<Value> {
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
    serde_json::to_value(entries).context("could not build query input")
}

pub fn execute(program: &str, input: Value) -> Result<Vec<Value>> {
    let arena = Arena::default();
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let loader = Loader::new(defs);
    let modules = loader
        .load(
            &arena,
            File {
                path: (),
                code: program,
            },
        )
        .map_err(|errors| anyhow!("invalid filter: {errors:?}"))?;
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());
    let filter = Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errors| anyhow!("could not compile filter: {errors:?}"))?;
    let input: Val = serde_json::from_value(input).context("could not convert query input")?;
    let context = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));

    filter
        .id
        .run((context, input))
        .map(unwrap_valr)
        .map(|value| {
            let value = value.map_err(|error| anyhow!("filter failed: {error:?}"))?;
            serde_json::from_str(&value.to_string())
                .context("filter produced a jaq extension value that cannot be represented as JSON")
        })
        .collect()
}

pub fn render_bibtex(values: &[Value]) -> Result<String> {
    let mut records = Vec::new();
    for value in values {
        if let Value::Array(items) = value {
            for item in items {
                records.push(value_to_record(item)?);
            }
        } else {
            records.push(value_to_record(value)?);
        }
    }
    render(&records)
}

fn value_to_record(value: &Value) -> Result<Record> {
    #[derive(serde::Deserialize)]
    struct FilteredEntry {
        id: String,
        #[serde(rename = "type")]
        entry_type: String,
        fields: std::collections::BTreeMap<String, String>,
    }

    let mut entry: FilteredEntry = serde_json::from_value(value.clone()).context(
        "--bibtex requires each filter result to be an entry object or an array of entry objects",
    )?;
    let reserved = ["integrity", "bibsource", "bibprovider", "bibproviderid"];
    entry.fields.retain(|field, _| {
        !reserved
            .iter()
            .any(|reserved| field.eq_ignore_ascii_case(reserved))
    });
    Ok(Record {
        entry_type: entry.entry_type,
        entry_key: entry.id,
        fields: entry.fields,
    }
    .normalized())
}

#[cfg(test)]
mod tests {
    use crate::bibtex::parse;

    use super::*;

    #[test]
    fn jq_filter_selects_pending_entries() {
        let records = parse("@article{one, title={One}}\n@book{two, title={Two}}").unwrap();
        let output = execute(
            r#".[] | select(.integrity.status == "unverified") | .id"#,
            input(&records).unwrap(),
        )
        .unwrap();
        assert_eq!(
            output,
            vec![Value::String("one".into()), Value::String("two".into())]
        );
    }

    #[test]
    fn filtered_entries_can_render_as_bibtex() {
        let records = parse("@article{one, title={One}}\n@book{two, title={Two}}").unwrap();
        let output = execute(r#"map(select(.type == "book"))"#, input(&records).unwrap()).unwrap();
        let bibtex = render_bibtex(&output).unwrap();
        assert!(bibtex.contains("@book{two,"));
        assert!(!bibtex.contains("@article"));
    }

    #[test]
    fn bibtex_output_strips_reserved_trust_fields() {
        let records = parse("@article{one, title={One}}").unwrap();
        let output = execute(
            r#"map(.fields.Integrity = "forged" | .fields.bibsource = "fake" | .fields.bibprovider = "crossref" | .fields.bibproviderid = "10.1/fake")"#,
            input(&records).unwrap(),
        )
        .unwrap();
        let bibtex = render_bibtex(&output).unwrap();
        assert!(bibtex.contains("title = {One}"));
        assert!(!bibtex.contains("integrity"));
        assert!(!bibtex.contains("bibsource"));
        assert!(!bibtex.contains("bibprovider"));
    }
}
