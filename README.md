# bib

`bib` is a review-first CLI for BibTeX. It finds likely duplicate entries,
reconciles metadata with literature providers, and attaches source-bound
integrity markers. It is designed for agent-assisted bibliography maintenance:
`bib` prepares evidence and candidates; a human or agent decides what to merge
or approve.

The command is deliberately non-destructive by default. It never silently picks
a metadata candidate, merges citation keys, or rewrites arbitrary BibTeX fields.
Every integrity marker references a separate `@bibsource` provenance entry.
Crossref is the default metadata backend; DOI content negotiation is also built
in.

Provider results retain compact request, response-hash, and projection-hash
receipts; response bodies are never embedded in the bibliography. Agent-created
records are explicitly labeled as agent assertions instead of looking
provider-verified.

## Install

Linux and macOS users can install the latest prebuilt release without Rust:

```sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/EvoEvolver/bib/main/install.sh | sh
```

The installer supports Linux x86_64/ARM64 and macOS Intel/Apple Silicon. It
detects the platform, downloads the GitHub Actions build, verifies its SHA-256
checksum, and installs only the `bib` binary to `~/.local/bin`.

Choose another directory or a specific release with environment variables:

```sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/EvoEvolver/bib/main/install.sh |
  BIB_INSTALL_DIR="$HOME/bin" BIB_VERSION=v0.6.0 sh
```

Export the variables first when that reads more clearly:

```sh
export BIB_INSTALL_DIR="$HOME/bin" BIB_VERSION=v0.6.0
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/EvoEvolver/bib/main/install.sh | sh
```

Verify the installation:

```sh
bib --version
```

`bib` is self-contained after installation and does not require Rust or Python.
`jq` is optional: use it as a separate process when a pipeline needs JSON
selection or transformation.

## Quick start

### Find duplicates

Compare one or more BibTeX files and emit scored candidate pairs for review:

```sh
bib dedupe references.bib other-references.bib
```

The result is JSON. Each pair includes the combined score, title and author
scores, input files, citation keys, and complete fields:

```sh
bib dedupe references.bib --compact > duplicate-candidates.json
```

Exit status is `0` when no pair meets the threshold, `3` when candidates need
review, and `2` for invalid input. Adjust the threshold with `--min-score`
(default `0.75`). `bib dedupe` never merges or removes entries.

### Reconcile metadata

Plan metadata replacements for selected entries:

```sh
bib source plan references.bib --key watson1953
```

Entries with a DOI receive an exact provider lookup. When an entry has a URL but
no DOI, `bib` first looks for stable identifiers in the URL, redirects, publisher
metadata, JSON-LD, and supported identifier APIs. Try resolution independently:

```sh
bib source resolve 'https://doi.org/10.1038/171737a0'
```

One unambiguous DOI can flow directly into the provider lookup. Otherwise `plan`
returns candidates for review. After choosing an exact provider record, apply it
without disturbing comments, string declarations, citation keys, or local-only
fields:

```sh
bib source apply references.bib \
  --key watson1953 \
  --id 10.1038/171737a0 \
  --add-integrity \
  --in-place
```

The literature entry receives a source reference:

```bibtex
bibprovider = {crossref},
bibproviderid = {10.1038/171737a0},
bibsource = {bibsource:provider:...},
integrity = {...}
```

The same file receives an independent compact receipt. It records where the
metadata came from and binds the provider-controlled BibTeX fields to a stable
projection hash without storing the HTTP response body:

```bibtex
@bibsource{bibsource:provider:...,
  kind = {provider},
  provider = {crossref},
  providerid = {10.1038/171737a0},
  requesturl = {https://api.crossref.org/works/10.1038%2F171737a0},
  mediatype = {application/vnd.crossref-api-message+json},
  responsesha256 = {...},
  projection = {literature-record-v1},
  projectionsha256 = {...},
}
```

When a URL was resolved first, another `@bibsource` receipt records the input URL,
resolution method, matched identifier, signals, and any network-response hash.
The provider receipt links to it through `resolution`. Inspect the chain as JSON:

```sh
bib source trace references.bib --key watson1953
```

Inspect the review status of every entry:

```sh
bib integrity status references.bib
```

List just the citation keys that need attention:

```sh
bib inspect references.bib --json |
  jq -r '.[] | select(.integrity.status != "verified") | .id'
```

When metadata was supplied by an agent rather than an API, say so explicitly:

```sh
bib integrity add references.bib --key turing1936 \
  --source agent --agent claude-code --in-place
```

This creates `@bibsource{..., kind={agent}, actor={claude-code}, ...}`. A later
content change makes integrity `stale`; missing, damaged, or inconsistent source
evidence makes it `invalid`.

## Dedupe

`bib dedupe` is a candidate generator, not a merge engine. It compares every
pair of bibliography entries that has both `title` and `author` fields. TeX
commands, braces, punctuation, case, and whitespace are normalized before
comparison. The combined score is:

```text
score = 0.7 * title_score + 0.3 * author_score
```

Author similarity emphasizes family-name overlap and tolerates common BibTeX
format differences such as `Smith, John` and `John Smith`. Results are sorted by
score and preserve the complete input fields so an agent can inspect conflicts:

```sh
bib dedupe references.bib --min-score 0.8 |
  jq '.[] | {score, entries: [.entries[] | {file, id, title: .fields.title, doi: .fields.doi}]}'
```

Duplicate citation keys are rejected before scoring, including duplicates across
input files. This matches the fact that downstream LaTeX tooling cannot resolve
ambiguous keys.

## Inspect and pipe

`bib inspect` emits one JSON array. `--json` may be included to make that contract
explicit in agent scripts. Each entry has this shape:

```json
{
  "id": "turing1936",
  "type": "article",
  "fields": {
    "author": "Turing, Alan M.",
    "title": "On Computable Numbers",
    "year": "1936"
  },
  "integrity": {
    "status": "unverified",
    "expected": "...",
    "stored": null,
    "source": {
      "key": null,
      "kind": null,
      "valid": false,
      "error": "missing bibsource provenance reference"
    }
  }
}
```

`bib` does not embed jq or evaluate filters. Pipe its stable JSON output to `jq`,
another JSON processor, or an agent harness:

```sh
# List entries that still need review.
bib inspect references.bib |
  jq -r '.[] | select(.integrity.status != "verified") | .id'

# Give an agent a compact review packet.
bib inspect references.bib --compact |
  jq -c '[.[] | select(.integrity.status != "verified") |
    {id, type, title: .fields.title, doi: .fields.doi}]'

# Select entries and feed their keys to a controlled write command.
bib inspect references.bib |
  jq -r '.[] | select(.fields.year == "2026") | .id' |
  bib integrity add references.bib --keys-from - \
    --source agent --agent claude-code --in-place

# Read BibTeX from stdin.
cat references.bib | bib inspect - | jq -r '.[].fields.doi // empty'
```

Multiple files are combined into one array. Use `-` as a filename, or omit files,
to read BibTeX from stdin. `inspect` never modifies input and omits `@bibsource`
evidence entries from the array. Duplicate citation keys, including duplicates
across input files, are rejected instead of producing ambiguous entries.

There is intentionally no JSON-to-BibTeX conversion or arbitrary metadata editor.
Edit bibliography data with the appropriate editor or domain tool. Only
`bib source apply` and `bib integrity` write trusted workflow fields.

## Command reference

| Command | Purpose |
| --- | --- |
| `bib inspect [FILE ...]` | Emit bibliography entries and trust state as JSON |
| `bib dedupe [FILE ...]` | Find title-and-author duplicate candidates for review |
| `bib source providers` | List installed metadata providers |
| `bib source resolve URL` | Resolve a URL into auditable identifier candidates |
| `bib source search QUERY` | Search a provider and return ranked common records |
| `bib source plan FILE --key KEY` | Produce candidates and field-level diffs |
| `bib source apply FILE --key KEY [--id ID]` | Apply one exact provider record and save a compact receipt |
| `bib source apply FILE --key KEY --add-integrity` | Apply and add provider-backed integrity atomically |
| `bib source trace FILE --key KEY` | Emit the provider and URL-resolution evidence chain |
| `bib source strip-responses FILE` | Remove response bodies embedded by versions before 0.5 |
| `bib integrity status FILE [--json]` | Report `verified`, `stale`, `unverified`, or `invalid` |
| `bib integrity hash FILE KEY` | Print the expected SHA-256 value |
| `bib integrity add FILE --key KEY --source SOURCE` | Add attributed or provider-backed integrity |
| `bib integrity add FILE --keys-from - --source SOURCE` | Add integrity for newline-delimited keys from a pipeline |
| `bib integrity remove FILE --key KEY [--in-place]` | Remove approval markers |

Run `bib --help`, `bib source --help`, `bib integrity --help`, or a specific
subcommand's `--help` for the complete option list.

## Literature providers

All backends implement the same operations over a common literature record:

- exact lookup by the provider's stable record identifier;
- ranked bibliographic search;
- mapping authorship, title, container, publication date, identifiers, and
  publication details into provider-neutral fields;
- projection of that record into BibTeX plus `bibprovider` and `bibproviderid`.

Crossref is selected by default. The `doi` backend performs exact lookup through
DOI content negotiation with `Accept: application/x-bibtex`; it supports any DOI
registration agency whose resolver supplies BibTeX. Use `--provider NAME` to
select a backend; `bib source providers` lists the registry.

Set an email address for providers that support polite API identification:

```sh
export BIB_MAILTO=researcher@example.org
```

### Agent review workflow

For entries with an existing DOI, a stored provider identifier, or a resolvable
URL:

```sh
bib source plan references.bib --key paper1
bib source apply references.bib --key paper1 --add-integrity --in-place
```

URL resolution is conservative. A unique DOI from an explicit URL or supported
metadata signal is eligible for exact lookup. Conflicting identifiers make
`resolve` and `plan` exit with status `3`; an agent must inspect them rather than
silently choosing one.

Network URL resolution follows at most five redirects, limits responses to 2 MiB,
and rejects credentials, non-default ports, localhost, and non-public IP
addresses. This keeps bibliography URLs from becoming an unrestricted network
fetch primitive inside an agent workflow.

For entries without an exact identifier after URL resolution, `plan` returns up
to five ranked provider candidates and exits with status `3`. An agent should
compare title, authors, year, venue, record type, and the proposed field diff. It
must not silently choose the highest provider score. Apply only an explicitly
selected candidate:

```sh
bib source apply references.bib \
  --key paper1 --id 10.1234/chosen-record --in-place
```

`apply` makes provider-controlled fields exactly match the deterministic
projection produced during that provider request. Old provider fields absent
from the result are removed. Local fields such as `file`, `keywords`, `note`, and
annotations remain. Without `--add-integrity`, the receipt is still saved and
integrity remains a separate explicit step:

```sh
bib integrity add references.bib --key paper1 --source provider --in-place
```

`source trace` exposes the compact evidence chain for an agent pipeline. It does
not fetch the network again and never outputs a stored response body:

```sh
bib source trace references.bib --key paper1 --compact | jq .
```

To remove response bodies previously written by `bib` 0.4 or older while keeping
their compact receipt metadata and citation references:

```sh
bib source strip-responses references.bib --in-place
```

Use `--all` with `source plan` to generate a review packet for the complete
bibliography. Applying records remains intentionally key-by-key so candidate
selection stays explicit.

## Review workflow

1. Find entries without a valid marker and create a review packet:

   ```sh
   bib inspect references.bib |
     jq '[.[] | select(.integrity.status != "verified")]'
   ```

2. Let the agent fix the selected entries and show the changes to the human.

3. After explicit confirmation, mark only the approved citation keys and identify
   who supplied the assertion. Keys can be passed directly:

   ```sh
   bib integrity add references.bib --key turing1936 --key shannon1948 \
     --source human --reviewer alice --in-place
   ```

   Or streamed one per line from a selection pipeline:

   ```sh
   bib inspect references.bib |
     jq -r '.[] | select(.integrity.status == "unverified") | .id' |
     bib integrity add references.bib --keys-from - \
       --source human --reviewer alice --in-place
   ```

4. Check the complete file:

   ```sh
   bib integrity status references.bib
   ```

Use `--all` only when every entry has been reviewed:

```sh
bib integrity add references.bib --all \
  --source agent --agent claude-code --in-place
```

Without `--in-place`, `add` and `remove` write the updated BibTeX to stdout.
Writes with `--in-place` use an atomic replacement and retain comments,
`@string` declarations, and unrelated formatting.

Neither command chooses entries implicitly: pass one or more `--key` options,
`--keys-from FILE`, or the explicit `--all` option. `--keys-from -` reads
newline-delimited citation keys from stdin and rejects an empty selection. This
keeps an agent from marking unrelated entries during a partial review.

Other integrity commands:

```sh
bib integrity status references.bib --json
bib integrity hash references.bib turing1936
bib integrity remove references.bib --key turing1936 --in-place
```

`integrity status` exits with `0` when every selected entry is verified, `3`
when any selected entry is stale, unverified, or invalid, and `2` for an
operational or input error. This makes it suitable for CI and agent loops.

## Integrity format

The marker uses this deterministic format:

1. Parse the entry and resolve BibTeX string macros.
2. Lowercase the entry type and field names.
3. Exclude the citation key and `integrity` field. Include the `bibsource`
   reference, binding content integrity to one provenance entry.
4. Serialize the remaining fields plus `ENTRYTYPE` as compact, key-sorted,
   UTF-8 JSON.
5. Store the lowercase SHA-256 digest in the `integrity` field.

```bibtex
@article{turing1936,
  author = {Turing, Alan M.},
  title = {On Computable Numbers},
  year = {1936},
  bibsource = {bibsource:agent:...},
  integrity = {8f...},
}
```

Verification also validates the referenced source. Provider sources bind the
request metadata and response hash to a projection hash, then compare that hash
with every current provider-controlled BibTeX field. URL-derived records also
validate the linked resolution receipt and DOI/provider identity. Agent and human
sources bind an actor label and content snapshot.

These markers are tamper-evident workflow records, not digital signatures or
archival proofs of an HTTP exchange. Actor labels and provider receipts are
assertions about what the tool processed; the response hash cannot prove by
itself that a remote API served those bytes.

Licensed under MIT.
