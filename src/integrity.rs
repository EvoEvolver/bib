use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::bibtex::Record;

pub const FIELD: &str = "integrity";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Verified,
    Stale,
    Unverified,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verified => f.write_str("verified"),
            Self::Stale => f.write_str("stale"),
            Self::Unverified => f.write_str("unverified"),
        }
    }
}

pub fn canonical_json(record: &Record) -> Result<String> {
    let mut payload = BTreeMap::new();
    payload.insert("ENTRYTYPE".to_owned(), record.entry_type.clone());
    payload.extend(
        record
            .fields
            .iter()
            .filter(|(key, _)| !key.eq_ignore_ascii_case(FIELD))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.clone())),
    );
    serde_json::to_string(&payload).context("could not serialize integrity payload")
}

pub fn hash(record: &Record) -> Result<String> {
    let digest = Sha256::digest(canonical_json(record)?.as_bytes());
    Ok(format!("{digest:x}"))
}

pub fn status(record: &Record) -> Result<Status> {
    match record.fields.get(FIELD).map(|value| value.trim()) {
        None | Some("") => Ok(Status::Unverified),
        Some(stored) if stored == hash(record)? => Ok(Status::Verified),
        Some(_) => Ok(Status::Stale),
    }
}

pub fn update_source(
    source: &str,
    records: &[Record],
    selected: &BTreeSet<String>,
    remove: bool,
) -> Result<String> {
    let spans = scan_entries(source)?;
    let by_key: BTreeMap<_, _> = records
        .iter()
        .map(|record| (record.entry_key.as_str(), record))
        .collect();
    let present: BTreeSet<_> = spans.iter().map(|entry| entry.key.as_str()).collect();

    for key in selected {
        if !present.contains(key.as_str()) {
            bail!("citation key not found: {key}");
        }
    }

    let mut edits = Vec::new();
    for entry in spans.iter().filter(|entry| selected.contains(&entry.key)) {
        let record = by_key
            .get(entry.key.as_str())
            .with_context(|| format!("citation key was not parsed: {}", entry.key))?;
        let integrity_fields: Vec<_> = entry
            .fields
            .iter()
            .filter(|field| field.name.eq_ignore_ascii_case(FIELD))
            .collect();
        if integrity_fields.len() > 1 {
            bail!("entry {} has multiple integrity fields", entry.key);
        }

        if remove {
            if let Some(field) = integrity_fields.first() {
                edits.push(Edit {
                    start: field.segment_start,
                    end: field.remove_end,
                    replacement: String::new(),
                });
            }
        } else {
            let value = format!("integrity = {{{}}}", hash(record)?);
            if let Some(field) = integrity_fields.first() {
                edits.push(Edit {
                    start: field.trimmed_start,
                    end: field.trimmed_end,
                    replacement: value,
                });
            } else {
                edits.push(insertion_edit(source, entry, value));
            }
        }
    }

    edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
    let mut output = source.to_owned();
    for edit in edits {
        output.replace_range(edit.start..edit.end, &edit.replacement);
    }
    Ok(output)
}

pub fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let permissions = fs::metadata(path)
        .with_context(|| format!("could not inspect {}", path.display()))?
        .permissions();
    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create temporary file in {}", parent.display()))?;
    temp.as_file_mut()
        .set_permissions(permissions)
        .context("could not preserve file permissions")?;
    temp.write_all(contents.as_bytes())
        .context("could not write temporary file")?;
    temp.as_file_mut()
        .sync_all()
        .context("could not sync temporary file")?;
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

#[derive(Debug)]
struct EntrySpan {
    key: String,
    open: usize,
    close: usize,
    fields: Vec<FieldSpan>,
}

#[derive(Debug)]
struct FieldSpan {
    name: String,
    segment_start: usize,
    remove_end: usize,
    trimmed_start: usize,
    trimmed_end: usize,
}

struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

fn insertion_edit(source: &str, entry: &EntrySpan, value: String) -> Edit {
    let bytes = source.as_bytes();
    let mut body_end = entry.close;
    while body_end > entry.open + 1 && bytes[body_end - 1].is_ascii_whitespace() {
        body_end -= 1;
    }
    let has_trailing_comma = body_end > entry.open + 1 && bytes[body_end - 1] == b',';
    let indent = entry
        .fields
        .first()
        .and_then(|field| line_indent(source, field.trimmed_start))
        .unwrap_or("  ");
    let prefix = if has_trailing_comma { "" } else { "," };
    Edit {
        start: body_end,
        end: body_end,
        replacement: format!("{prefix}\n{indent}{value},"),
    }
}

fn line_indent(source: &str, position: usize) -> Option<&str> {
    let line_start = source[..position].rfind('\n').map_or(0, |index| index + 1);
    let indent = &source[line_start..position];
    indent.chars().all(char::is_whitespace).then_some(indent)
}

fn scan_entries(source: &str) -> Result<Vec<EntrySpan>> {
    let bytes = source.as_bytes();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            index = skip_line(bytes, index);
            continue;
        }
        if bytes[index] != b'@' {
            index += 1;
            continue;
        }

        let mut cursor = index + 1;
        skip_space(bytes, &mut cursor);
        let type_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        let entry_type = source[type_start..cursor].to_ascii_lowercase();
        skip_space(bytes, &mut cursor);
        if cursor >= bytes.len() || !matches!(bytes[cursor], b'{' | b'(') {
            index += 1;
            continue;
        }
        let open = cursor;
        let close = find_closing(bytes, open)
            .with_context(|| format!("unclosed @{entry_type} entry at byte {index}"))?;
        if matches!(entry_type.as_str(), "string" | "preamble" | "comment") {
            index = close + 1;
            continue;
        }

        let comma = find_top_level(bytes, open + 1, close, b',')
            .with_context(|| format!("entry at byte {index} has no citation-key comma"))?;
        let key = source[open + 1..comma].trim().to_owned();
        if key.is_empty() {
            bail!("entry at byte {index} has an empty citation key");
        }
        let fields = scan_fields(source, comma + 1, close)?;
        entries.push(EntrySpan {
            key,
            open,
            close,
            fields,
        });
        index = close + 1;
    }
    Ok(entries)
}

fn scan_fields(source: &str, start: usize, end: usize) -> Result<Vec<FieldSpan>> {
    let bytes = source.as_bytes();
    let mut fields = Vec::new();
    let mut segment_start = start;
    loop {
        let comma = find_top_level(bytes, segment_start, end, b',');
        let segment_end = comma.unwrap_or(end);
        let mut trimmed_start = segment_start;
        let mut trimmed_end = segment_end;
        while trimmed_start < trimmed_end && bytes[trimmed_start].is_ascii_whitespace() {
            trimmed_start += 1;
        }
        while trimmed_end > trimmed_start && bytes[trimmed_end - 1].is_ascii_whitespace() {
            trimmed_end -= 1;
        }
        if trimmed_start < trimmed_end {
            let equals = find_top_level(bytes, trimmed_start, trimmed_end, b'=')
                .with_context(|| format!("malformed field at byte {trimmed_start}"))?;
            let name = source[trimmed_start..equals].trim().to_ascii_lowercase();
            fields.push(FieldSpan {
                name,
                segment_start,
                remove_end: comma.map_or(segment_end, |position| position + 1),
                trimmed_start,
                trimmed_end,
            });
        }
        match comma {
            Some(position) => segment_start = position + 1,
            None => break,
        }
    }
    Ok(fields)
}

fn find_closing(bytes: &[u8], open: usize) -> Option<usize> {
    let opening = bytes[open];
    let closing = if opening == b'{' { b'}' } else { b')' };
    let mut depth = 1usize;
    let mut brace_depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut index = open + 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if opening == b'(' && !quoted && byte == b'{' {
            brace_depth += 1;
        } else if opening == b'(' && !quoted && byte == b'}' {
            brace_depth = brace_depth.saturating_sub(1);
        } else if byte == b'"' && brace_depth == 0 {
            quoted = !quoted;
        } else if !quoted && brace_depth == 0 && byte == opening {
            depth += 1;
        } else if !quoted && brace_depth == 0 && byte == closing {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn find_top_level(bytes: &[u8], start: usize, end: usize, needle: u8) -> Option<usize> {
    let mut brace_depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut index = start;
    while index < end {
        let byte = bytes[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' && brace_depth == 0 {
            quoted = !quoted;
        } else if !quoted && byte == b'{' {
            brace_depth += 1;
        } else if !quoted && byte == b'}' {
            brace_depth = brace_depth.saturating_sub(1);
        } else if !quoted && brace_depth == 0 && byte == needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn skip_space(bytes: &[u8], index: &mut usize) {
    while *index < bytes.len() && bytes[*index].is_ascii_whitespace() {
        *index += 1;
    }
}

fn skip_line(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::bibtex::parse;

    use super::*;

    const SAMPLE: &str = r#"% keep this comment
@string{conf = {Great Conf}}

@Article{One,
  title = {A {Nested, Great} Paper},
  journal = conf,
  year = 2026,
}

@misc(Two, title = "Quoted, title")
"#;

    #[test]
    fn adding_and_removing_preserves_unrelated_source() {
        let records = parse(SAMPLE).unwrap();
        let selected = BTreeSet::from(["One".to_owned()]);
        let added = update_source(SAMPLE, &records, &selected, false).unwrap();
        assert!(added.starts_with("% keep this comment\n@string"));
        assert!(added.contains("integrity = {"));
        assert!(matches!(
            status(&parse(&added).unwrap()[0]),
            Ok(Status::Verified)
        ));

        let removed = update_source(&added, &parse(&added).unwrap(), &selected, true).unwrap();
        assert_eq!(removed, SAMPLE);
    }

    #[test]
    fn changed_content_makes_integrity_stale() {
        let records = parse(SAMPLE).unwrap();
        let selected = BTreeSet::from(["Two".to_owned()]);
        let added = update_source(SAMPLE, &records, &selected, false).unwrap();
        let changed = added.replace("Quoted, title", "Different title");
        assert_eq!(status(&parse(&changed).unwrap()[1]).unwrap(), Status::Stale);
    }

    #[test]
    fn hash_matches_url2bibtex_python_implementation() {
        let record = parse(
            "@Article{Key, Title={Hello {World}}, AUTHOR={Doe, Jane and Smith, John}, year=2025}",
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            hash(&record).unwrap(),
            "b46c23357cd2a600fc9ab6607e90b77e7d48d88fd3d68666c532be50777a8a77"
        );
    }

    #[test]
    fn scans_parenthesized_entry_with_closing_paren_in_braces() {
        let source = "@misc(Key, title={A title (revised)}, note={contains ) safely})\n";
        let records = parse(source).unwrap();
        let selected = BTreeSet::from(["Key".to_owned()]);
        let added = update_source(source, &records, &selected, false).unwrap();
        assert_eq!(
            status(&parse(&added).unwrap()[0]).unwrap(),
            Status::Verified
        );
    }
}
