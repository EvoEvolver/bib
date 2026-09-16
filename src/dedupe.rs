use std::collections::BTreeSet;

use serde::Serialize;

use crate::bibtex::Record;

#[derive(Clone, Debug)]
pub struct LocatedRecord {
    pub file: String,
    pub record: Record,
}

#[derive(Debug, Serialize)]
pub struct DedupeEntry<'a> {
    pub file: &'a str,
    pub id: &'a str,
    #[serde(rename = "type")]
    pub entry_type: &'a str,
    pub fields: &'a std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct DedupeCandidate<'a> {
    pub score: f64,
    pub title_score: f64,
    pub author_score: f64,
    pub entries: [DedupeEntry<'a>; 2],
}

pub fn candidates(records: &[LocatedRecord], min_score: f64) -> Vec<DedupeCandidate<'_>> {
    let bibliography: Vec<_> = records
        .iter()
        .filter(|item| !item.record.is_provenance())
        .filter_map(|item| {
            let title = item.record.fields.get("title")?;
            let author = item.record.fields.get("author")?;
            Some((item, title.as_str(), author.as_str()))
        })
        .collect();
    let mut matches = Vec::new();

    for (index, (left, left_title, left_author)) in bibliography.iter().enumerate() {
        for (right, right_title, right_author) in &bibliography[index + 1..] {
            let title_score = token_dice(&tokens(left_title), &tokens(right_title));
            let author_score = author_similarity(left_author, right_author);
            let score = 0.7 * title_score + 0.3 * author_score;
            if score >= min_score {
                matches.push(DedupeCandidate {
                    score: rounded(score),
                    title_score: rounded(title_score),
                    author_score: rounded(author_score),
                    entries: [entry(left), entry(right)],
                });
            }
        }
    }

    matches.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.entries[0].file.cmp(right.entries[0].file))
            .then_with(|| left.entries[0].id.cmp(right.entries[0].id))
            .then_with(|| left.entries[1].file.cmp(right.entries[1].file))
            .then_with(|| left.entries[1].id.cmp(right.entries[1].id))
    });
    matches
}

fn entry(item: &LocatedRecord) -> DedupeEntry<'_> {
    DedupeEntry {
        file: &item.file,
        id: &item.record.entry_key,
        entry_type: &item.record.entry_type,
        fields: &item.record.fields,
    }
}

fn author_similarity(left: &str, right: &str) -> f64 {
    let surname_score = token_dice(&author_surnames(left), &author_surnames(right));
    let full_score = token_dice(&tokens(left), &tokens(right));
    0.75 * surname_score + 0.25 * full_score
}

fn author_surnames(value: &str) -> BTreeSet<String> {
    value
        .split(" and ")
        .filter_map(|author| {
            let family = author
                .split_once(',')
                .map(|(family, _)| family)
                .unwrap_or(author);
            let words = normalized_words(family);
            if author.contains(',') {
                Some(words.join(" "))
            } else {
                words.into_iter().next_back()
            }
        })
        .filter(|family| !family.is_empty())
        .collect()
}

fn tokens(value: &str) -> BTreeSet<String> {
    normalized_words(value).into_iter().collect()
}

fn normalized_words(value: &str) -> Vec<String> {
    let mut normalized = String::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\\' {
            if chars.peek().is_some_and(|next| next.is_alphabetic()) {
                while chars.peek().is_some_and(|next| next.is_alphabetic()) {
                    chars.next();
                }
            } else {
                chars.next();
            }
            normalized.push(' ');
        } else if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().map(str::to_owned).collect()
}

fn token_dice(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    2.0 * left.intersection(right).count() as f64 / (left.len() + right.len()) as f64
}

fn rounded(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use crate::bibtex::parse;

    use super::*;

    fn located(source: &str) -> LocatedRecord {
        LocatedRecord {
            file: "refs.bib".into(),
            record: parse(source).unwrap().remove(0),
        }
    }

    #[test]
    fn scores_equivalent_title_and_author_forms() {
        let records = vec![
            located("@article{one, title={A Great Paper}, author={Smith, John and Doe, Jane}}"),
            located("@article{two, title={A {Great} Paper}, author={John Smith and Jane Doe}}"),
        ];
        let matches = candidates(&records, 0.75);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].title_score, 1.0);
        assert!(matches[0].author_score >= 0.75);
    }

    #[test]
    fn ignores_records_without_both_title_and_author() {
        let records = vec![
            located("@article{one, title={Same}}"),
            located("@article{two, title={Same}}"),
        ];
        assert!(candidates(&records, 0.0).is_empty());
    }
}
