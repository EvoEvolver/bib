# biblock

`biblock` is a review-first CLI for BibTeX. It finds likely duplicate entries,
reconciles metadata with literature providers, and attaches source-bound
integrity markers. It is designed for agent-assisted bibliography maintenance:
`biblock` prepares evidence and candidates; a human or agent decides what to merge
or approve.

The command is deliberately non-destructive by default. It never silently picks
a metadata candidate, merges citation keys, or rewrites arbitrary BibTeX fields.
Crossref is the default metadata backend; DOI content negotiation is also built
in. Workflow state lives in a JSON `FILE.lock` sidecar, so the `.bib` remains
standard, portable BibTeX.

Provider results retain compact request, response-hash, and projection-hash
receipts; response bodies are never embedded in the bibliography. Agent-created
records are explicitly labeled as agent assertions instead of looking
provider-verified.

## Install

Linux and macOS users can install the latest prebuilt release without Rust:

```sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/EvoEvolver/biblock/main/install.sh | sh
```

The installer supports Linux x86_64/ARM64 and macOS Intel/Apple Silicon. It
detects the platform, downloads the GitHub Actions build, verifies its SHA-256
checksum, and installs only the `biblock` binary to `~/.local/bin`.

Choose another directory or a specific release with environment variables:

```sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/EvoEvolver/biblock/main/install.sh |
  BIBLOCK_INSTALL_DIR="$HOME/bin" BIBLOCK_VERSION=v0.9.0 sh
```

Export the variables first when that reads more clearly:

```sh
export BIBLOCK_INSTALL_DIR="$HOME/bin" BIBLOCK_VERSION=v0.9.0
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/EvoEvolver/biblock/main/install.sh | sh
```

Verify the installation:

```sh
biblock --version
```

`biblock` is self-contained after installation and does not require Rust or Python.
`jq` is optional: use it as a separate process when a pipeline needs JSON
selection or transformation.

## Quick start

### Find duplicates

Compare one or more BibTeX files and emit scored candidate pairs for review:

```sh
biblock dedupe references.bib other-references.bib
```

The result is JSON. Each pair includes the combined score, title and author
scores, input files, citation keys, and complete fields:

```sh
biblock dedupe references.bib --compact > duplicate-candidates.json
```

Exit status is `0` when no pair meets the threshold, `3` when candidates need
review, and `2` for invalid input. Adjust the threshold with `--min-score`
(default `0.75`). `biblock dedupe` never merges or removes entries.

### Reconcile metadata

Verify a complete bibliography against Crossref and DOI.org, first as a dry run:

```sh
biblock source verify references.bib --all
```

The JSON report distinguishes exact records that are ready, entries already
verified by a provider, ambiguous search candidates, and lookup failures. Exact
DOIs and explicit DOI or arXiv URLs are safe to apply automatically; title and
author search results are never selected automatically. Commit all successful
exact matches with one atomic replacement:

```sh
biblock source verify references.bib --all --in-place
```

The default fallback order is Crossref, then DOI content negotiation. Successful
entries receive provider provenance and integrity in the same transaction. Exit
status `3` means at least one entry still needs agent review; successful exact
matches can still be written when `--in-place` is present.

Plan metadata replacements for selected entries:

```sh
biblock source plan references.bib --key watson1953
```

Entries with a DOI receive an exact provider lookup. When an entry has a URL but
no DOI, `biblock` first looks for stable identifiers in the URL, redirects, publisher
metadata, JSON-LD, and supported identifier APIs. Try resolution independently:

```sh
biblock source resolve 'https://doi.org/10.1038/171737a0'
```

One unambiguous DOI can flow directly into the provider lookup. Otherwise `plan`
returns candidates for review. After choosing an exact provider record, apply it
without disturbing comments, string declarations, citation keys, or local-only
fields:

```sh
biblock source apply references.bib \
  --key watson1953 \
  --id 10.1038/171737a0 \
  --add-integrity \
  --in-place
```

The updated `.bib` contains only bibliographic fields. `references.bib.lock`
records the provider identity, request metadata, response hash, projection hash,
integrity approval, and history. Structured receipt values such as candidates,
signals, and projections are native JSON values rather than JSON-encoded strings,
so agents can query them directly with `jq`.

When a URL was resolved first, another source object records the input URL,
resolution method, matched identifier, signals, and any network-response hash.
The provider source links to it through `resolution`. Inspect the chain as JSON:

```sh
biblock source trace references.bib --key watson1953
```

Inspect the review status of every entry:

```sh
biblock integrity status references.bib
```

List just the citation keys that need attention:

```sh
biblock inspect references.bib --json |
  jq -r '.[] | select(.integrity.status != "verified") | .id'
```

When metadata was supplied by an agent rather than an API, say so explicitly:

```sh
biblock integrity add references.bib --key turing1936 \
  --source agent --agent claude-code --in-place
```

This records an attributed agent source in the lockfile. A later content change
makes integrity `stale`; missing, damaged, or inconsistent source evidence makes
it `invalid`.

## Dedupe

`biblock dedupe` is a candidate generator, not a merge engine. It compares every
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
biblock dedupe references.bib --min-score 0.8 |
  jq '.[] | {score, entries: [.entries[] | {file, id, title: .fields.title, doi: .fields.doi}]}'
```

Duplicate citation keys are rejected before scoring, including duplicates across
input files. This matches the fact that downstream LaTeX tooling cannot resolve
ambiguous keys.

## Inspect and pipe

`biblock inspect` emits one JSON array. `--json` may be included to make that contract
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
      "error": "missing provenance source reference"
    }
  }
}
```

`biblock` does not embed jq or evaluate filters. Pipe its stable JSON output to `jq`,
another JSON processor, or an agent harness:

```sh
# List entries that still need review.
biblock inspect references.bib |
  jq -r '.[] | select(.integrity.status != "verified") | .id'

# Give an agent a compact review packet.
biblock inspect references.bib --compact |
  jq -c '[.[] | select(.integrity.status != "verified") |
    {id, type, title: .fields.title, doi: .fields.doi}]'

# Select entries and feed their keys to a controlled write command.
biblock inspect references.bib |
  jq -r '.[] | select(.fields.year == "2026") | .id' |
  biblock integrity add references.bib --keys-from - \
    --source agent --agent claude-code --in-place

# Read BibTeX from stdin.
cat references.bib | biblock inspect - | jq -r '.[].fields.doi // empty'
```

Multiple files are combined into one array. Use `-` as a filename, or omit files,
to read BibTeX from stdin. `inspect` never modifies input. Duplicate citation
keys, including duplicates
across input files, are rejected instead of producing ambiguous entries.

There is intentionally no JSON-to-BibTeX conversion or arbitrary metadata editor.
Edit bibliography data with the appropriate editor or domain tool. `biblock` stores
trusted workflow state in the lockfile rather than inventing BibTeX fields.

## Command reference

| Command | Purpose |
| --- | --- |
| `biblock inspect [FILE ...]` | Emit bibliography entries and trust state as JSON |
| `biblock dedupe [FILE ...]` | Find title-and-author duplicate candidates for review |
| `biblock source providers` | List installed metadata providers |
| `biblock source verify FILE --all` | Batch exact verification with provider fallback; report ambiguous candidates |
| `biblock source web FILE --all` | Bind existing entry contents to fetched web-source receipts |
| `biblock source resolve URL` | Resolve a URL into auditable identifier candidates |
| `biblock source search QUERY` | Search a provider and return ranked common records |
| `biblock source plan FILE --key KEY` | Produce candidates and field-level diffs |
| `biblock source apply FILE --key KEY [--id ID]` | Apply one exact provider record and save a compact receipt |
| `biblock source apply FILE --key KEY --add-integrity` | Apply and add provider-backed integrity atomically |
| `biblock source trace FILE --key KEY` | Emit the provider and URL-resolution evidence chain |
| `biblock source strip-responses FILE` | Remove response bodies embedded by versions before 0.5 |
| `biblock integrity status FILE [--json]` | Report `verified`, `stale`, `unverified`, or `invalid` |
| `biblock integrity hash FILE KEY` | Print the expected SHA-256 value |
| `biblock integrity add FILE --key KEY --source SOURCE` | Add attributed or provider-backed integrity |
| `biblock integrity add FILE --keys-from - --source SOURCE` | Add integrity for newline-delimited keys from a pipeline |
| `biblock integrity remove FILE --key KEY [--in-place]` | Remove approval markers |
| `biblock lock FILE --frozen` | Fail when the JSON lockfile is missing or stale |
| `biblock lock FILE --sync --actor ACTOR` | Synchronize an external BibTeX edit without approving it |
| `biblock history status FILE [--json]` | Validate revision hashes, snapshots, and links |
| `biblock history log FILE --key KEY` | Emit one entry's revision chain as JSON |
| `biblock history show FILE --revision REV` | Print a stored BibTeX snapshot |
| `biblock history diff FILE --revision REV` | Compare a stored snapshot with the current entry |
| `biblock history restore FILE --revision REV --in-place` | Restore and record a prior snapshot |

Run `biblock --help`, `biblock source --help`, `biblock integrity --help`, `biblock lock --help`,
`biblock history --help`, or a specific subcommand's `--help` for the complete option
list.

## Literature providers

All backends implement the same operations over a common literature record:

- exact lookup by the provider's stable record identifier;
- ranked bibliographic search;
- mapping authorship, title, container, publication date, identifiers, and
  publication details into provider-neutral fields;
- projection of that record into standard BibTeX, with provider identity retained
  in the lockfile.

Crossref is selected by default. The `doi` backend performs exact lookup through
DOI content negotiation with `Accept: application/x-bibtex`; it supports any DOI
registration agency whose resolver supplies BibTeX. Use `--provider NAME` to
select a backend; `biblock source providers` lists the registry.

Set an email address for providers that support polite API identification:

```sh
export BIBLOCK_MAILTO=researcher@example.org
```

### Agent review workflow

For entries with an existing DOI, a stored provider identifier, or a resolvable
URL:

```sh
biblock source plan references.bib --key paper1
biblock source apply references.bib --key paper1 --add-integrity --in-place
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
biblock source apply references.bib \
  --key paper1 --id 10.1234/chosen-record \
  --selected-by codex --add-integrity --in-place
```

With `--selected-by`, `apply` reruns the bibliographic search and requires the
chosen ID to be present in its candidates. It records a compact search receipt
containing the query, candidates, request URL, and response hash; a selection
receipt records the chosen ID and actor; and the exact provider receipt links to
that selection. The final integrity remains provider-backed rather than becoming
an agent assertion.

For software, product pages, documentation, and other sources that do not have a
literature-provider record, preserve the existing metadata and bind it to the
source URL instead:

```sh
biblock source web references.bib --key product-page --in-place
```

The web receipt stores the input and final URLs, media type, response byte count,
response SHA-256, and a hash of the BibTeX content. It does not embed the page
body or claim that Crossref verified the entry.

`apply` makes provider-controlled fields exactly match the deterministic
projection produced during that provider request. Old provider fields absent
from the result are removed. Local fields such as `file`, `keywords`, `note`, and
annotations remain. Without `--add-integrity`, the receipt is still saved and
integrity remains a separate explicit step:

```sh
biblock integrity add references.bib --key paper1 --source provider --in-place
```

`source trace` exposes the compact evidence chain for an agent pipeline. It does
not fetch the network again and never outputs a stored response body:

```sh
biblock source trace references.bib --key paper1 --compact | jq .
```

To remove response bodies previously written by `biblock` 0.4 or older while keeping
their compact receipt metadata and citation references:

```sh
biblock source strip-responses references.bib --in-place
```

Use `source verify --all` for batch exact verification and a review report for
the complete bibliography. Use `source plan` and key-by-key `source apply` when
an agent has selected one of several search candidates explicitly.

## Review workflow

1. Find entries without a valid marker and create a review packet:

   ```sh
   biblock inspect references.bib |
     jq '[.[] | select(.integrity.status != "verified")]'
   ```

2. Let the agent fix the selected entries and show the changes to the human.

3. After explicit confirmation, mark only the approved citation keys and identify
   who supplied the assertion. Keys can be passed directly:

   ```sh
   biblock integrity add references.bib --key turing1936 --key shannon1948 \
     --source human --reviewer alice --in-place
   ```

   Or streamed one per line from a selection pipeline:

   ```sh
   biblock inspect references.bib |
     jq -r '.[] | select(.integrity.status == "unverified") | .id' |
     biblock integrity add references.bib --keys-from - \
       --source human --reviewer alice --in-place
   ```

4. Check the complete file:

   ```sh
   biblock integrity status references.bib
   ```

Use `--all` only when every entry has been reviewed:

```sh
biblock integrity add references.bib --all \
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
biblock integrity status references.bib --json
biblock integrity hash references.bib turing1936
biblock integrity remove references.bib --key turing1936 --in-place
```

`integrity status` exits with `0` when every selected entry is verified, `3`
when any selected entry is stale, unverified, or invalid, and `2` for an
operational or input error. This makes it suitable for CI and agent loops.

## JSON lockfile and edit history

For `references.bib`, workflow state is stored in `references.bib.lock`. The
JSON document has stable, key-sorted sections:

- `lockfileVersion` and `toolVersion` describe the format and writer;
- `bibliography.contentHash` binds the complete current bibliography;
- `entries` binds each citation key to its canonical content, current source,
  integrity approval, provider identity, snapshot, and history head;
- `sources` stores content-addressed provider, web, resolution, search,
  selection, agent, and human evidence;
- `revisions` stores the content-addressed history DAG.

The `.bib` never contains `integrity`, `bibsource`, `bibprovider`,
`bibprevious`, `@bibsource`, or `@bibversion`. Deleting the lockfile leaves valid
BibTeX, but deliberately discards all provenance, integrity, and history.
Normally the lockfile should be committed with the project; omit it only from a
submission bundle that accepts BibTeX but does not need the audit trail.

```json
{
  "lockfileVersion": "1.0",
  "toolVersion": "0.9.0",
  "bibliography": { "contentHash": "..." },
  "entries": {
    "turing1936": {
      "contentHash": "...",
      "source": "bibsource:provider:...",
      "integrity": { "contentHash": "...", "source": "bibsource:provider:..." },
      "head": "rev:8f31c9d0"
    }
  },
  "sources": {},
  "revisions": {}
}
```

Check it in CI or synchronize a change made by an external editor:

```sh
biblock lock references.bib --frozen
biblock lock references.bib --sync --actor alice
jq '.entries.turing1936, .sources, .revisions' references.bib.lock
```

An out-of-sync lockfile blocks mutating commands until `biblock lock --sync` records
the external edit. Synchronization does not approve the new content or silently
inherit integrity. Revision IDs use an eight-hex-digit prefix and retain the full
SHA-256 in the revision object; collisions extend the new ID.

Files written by versions before 0.9 are migrated on `biblock lock FILE --sync` or
the next successful in-place edit: embedded workflow fields and `@bibsource`
entries move into JSON and are removed from the `.bib`.

```sh
biblock history status references.bib
biblock history log references.bib --key paper1
biblock history show references.bib --revision 8f31c9d0
biblock history diff references.bib --revision 8f31c9d0
biblock history restore references.bib --revision 8f31c9d0 --in-place
```

`restore` is itself an ordinary recorded edit, so it can be reversed. Dry runs,
stdout output, and no-op writes do not create revisions. During a write, the
bibliography is replaced first and the lockfile last as the commit marker. An
interrupted update is detected by the bibliography hash rather than being
mistaken for verified state.

## Integrity format

Integrity uses this deterministic format:

1. Parse the entry and resolve BibTeX string macros.
2. Lowercase the entry type and field names.
3. Exclude the citation key and all workflow metadata.
4. Serialize the remaining fields plus `ENTRYTYPE` as compact, key-sorted,
   UTF-8 JSON.
5. Store the lowercase SHA-256 digest under the entry's lockfile state, together
   with the source object that authorized it.

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
