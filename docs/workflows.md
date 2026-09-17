# Workflows and command guide

This guide covers the operational details intentionally omitted from the main
README. Run any command with `--help` for its complete option reference.

## Inspect and compose

`biblock inspect` emits one JSON array. Multiple files are combined into the same
array; use `-` as a filename, or omit filenames, to read BibTeX from stdin.

```sh
# List entries that need review.
biblock inspect references.bib |
  jq -r '.[] | select(.integrity.status != "verified") | .id'

# Build a compact agent review packet.
biblock inspect references.bib --compact |
  jq -c '[.[] | select(.integrity.status != "verified") |
    {id, type, title: .fields.title, doi: .fields.doi}]'

# Inspect several files together.
biblock inspect references.bib imported.bib

# Read from stdin.
cat references.bib | biblock inspect -
```

Duplicate citation keys, including duplicates across input files, are rejected.
`inspect` never modifies its input.

## Find duplicate candidates

`dedupe` compares entries that have both a title and an author. It normalizes TeX
commands, braces, punctuation, case, whitespace, and common author-name forms.
The score is:

```text
score = 0.7 * title_score + 0.3 * author_score
```

```sh
biblock dedupe references.bib imported.bib --min-score 0.8 |
  jq '.[] | {score, entries: [.entries[] | {file, id, fields}]}'
```

The output is a candidate packet, not a merge instruction. `biblock` never
deletes or merges entries automatically.

## Verify exact identifiers

Crossref is the default provider. The `doi` provider uses DOI content negotiation
with `Accept: application/x-bibtex`. List the installed providers with:

```sh
biblock source providers
```

Batch verification first tries exact identifiers such as an existing DOI, a DOI
URL, or a supported repository URL. Entries without an exact identifier return
ranked candidates but are never selected automatically.

```sh
# Report only.
biblock source verify references.bib --all

# Write all successful exact matches atomically.
biblock source verify references.bib --all --in-place

# Change provider order.
biblock source verify references.bib --all --providers doi,crossref
```

Set an email address for providers that support polite API identification:

```sh
export BIBLOCK_MAILTO=researcher@example.org
```

## Review ambiguous records

Plan a replacement before writing it:

```sh
biblock source plan references.bib --key paper1
```

The plan contains provider candidates and field-level changes. Compare title,
authors, publication year, venue, record type, and identifiers. Apply only the
chosen stable provider ID:

```sh
biblock source apply references.bib \
  --key paper1 \
  --id 10.1234/chosen-record \
  --selected-by alice \
  --add-integrity \
  --in-place
```

With `--selected-by`, `apply` reruns the search and requires the chosen ID to be
present among its candidates. The lockfile records the query, candidate set,
selection actor, exact lookup, and resulting projection.

Provider-controlled fields are replaced by the deterministic provider
projection. Local fields such as `file`, `keywords`, `note`, and annotations are
preserved.

## Resolve URLs

URL resolution extracts stable identifiers from the input URL, redirects,
publisher metadata, JSON-LD, and supported repository APIs:

```sh
biblock source resolve 'https://doi.org/10.1038/171737a0'
```

Conflicting identifiers require review and produce exit status `3`. Resolution
follows at most five redirects, limits ordinary responses to 2 MiB, and rejects
credentials, non-default ports, localhost, and non-public IP addresses.

For software, documentation, product pages, and other sources without a
literature-provider record, preserve the existing fields and bind them to the web
source:

```sh
biblock source web references.bib --key product-page --in-place
```

The receipt stores URLs, media type, byte count, response hash, and a content
hash. It does not embed the page body or claim provider verification.

## Inspect provenance

```sh
biblock source trace references.bib --key paper1 --compact | jq .
```

`trace` validates and emits the stored evidence chain without making another
network request.

## Add attributed integrity

Provider-backed integrity reuses an exact provider receipt:

```sh
biblock integrity add references.bib --key paper1 \
  --source provider --in-place
```

Human and agent review can be recorded explicitly:

```sh
biblock integrity add references.bib --key paper1 \
  --source human --reviewer alice --in-place

biblock integrity add references.bib --key draft1 \
  --source agent --agent codex --in-place
```

Selection is always explicit: pass one or more `--key` arguments,
`--keys-from FILE`, or `--all`. An empty `--keys-from -` selection is rejected.

```sh
biblock integrity status references.bib --json
biblock integrity hash references.bib paper1
biblock integrity remove references.bib --key paper1 --in-place
```

## CI

Commit the bibliography and lockfile, then fail CI if they diverge or any entry
lacks an exact provider API source or explicit human approval:

```sh
biblock lock references.bib --frozen
biblock integrity status references.bib
```

`integrity status` exits `0` when every selected entry is verified, `3` when any
entry is valid but not verified, stale, or invalid, and `2` for invalid input or an
operational error. Review-producing commands also use exit status `3` when human
or agent judgment is required.

`verified` requires valid integrity whose direct source is `provider` or `human`.
An internally consistent `agent` or `web` source is `valid`, but does not satisfy
the release gate. Missing integrity or provenance is `invalid`; a later content
change is `stale`.

## Command index

| Command | Purpose |
| --- | --- |
| `inspect [FILE ...]` | Emit entries and trust state as JSON |
| `dedupe [FILE ...]` | Find title-and-author duplicate candidates |
| `source providers` | List metadata providers |
| `source verify FILE --all` | Batch exact verification with provider fallback |
| `source web FILE --all` | Bind current entries to web-source receipts |
| `source resolve URL` | Resolve a URL into identifier candidates |
| `source search QUERY` | Search a provider |
| `source plan FILE --key KEY` | Produce candidates and field-level changes |
| `source apply FILE --key KEY` | Apply one exact provider record |
| `source trace FILE --key KEY` | Emit a stored evidence chain |
| `integrity status FILE` | Inspect integrity state |
| `integrity add FILE --key KEY` | Add provider, human, or agent approval |
| `integrity remove FILE --key KEY` | Remove approval state |
| `lock FILE --frozen` | Require an up-to-date lockfile |
| `lock FILE --sync` | Record an external BibTeX edit |
| `history status FILE` | Validate revision objects and links |
| `history log FILE --key KEY` | Show an entry's revision chain |
| `history show FILE --revision REV` | Print a stored snapshot |
| `history diff FILE --revision REV` | Compare a snapshot with the current entry |
| `history restore FILE --revision REV` | Restore a snapshot as a new edit |
