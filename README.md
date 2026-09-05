# bib

`bib` is a Rust CLI for reconciling BibTeX with literature metadata providers,
querying entries with jq filters, and attaching reviewable integrity markers to
approved records. It uses a provider-neutral literature model; Crossref is the
default backend.

It is designed for a workflow where an agent edits or researches bibliography
entries, a human reviews the result, and the agent marks only the confirmed
citation keys. A later content change makes that marker `stale`.

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
  BIB_INSTALL_DIR="$HOME/bin" BIB_VERSION=v0.2.0 sh
```

Export the variables first when that reads more clearly:

```sh
export BIB_INSTALL_DIR="$HOME/bin" BIB_VERSION=v0.2.0
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/EvoEvolver/bib/main/install.sh | sh
```

Verify the installation:

```sh
bib --version
```

`bib` is self-contained after installation. It does not require Rust, Python,
or `jq`.

### Build from source

Developers with a current stable Rust toolchain can install from a checkout:

```sh
git clone https://github.com/EvoEvolver/bib.git
cd bib
cargo install --locked --path .
```

To try the CLI without installing it:

```sh
git clone https://github.com/EvoEvolver/bib.git
cd bib
cargo run -- -r '.[].id' references.bib
```

## Quick start

Plan metadata replacements for selected entries:

```sh
bib source plan references.bib --key watson1953
```

Entries with a DOI receive an exact provider lookup. Entries without one return
ranked candidates for review. After choosing an exact provider record, apply it
without disturbing comments, string declarations, citation keys, or local-only
fields:

```sh
bib source apply references.bib \
  --key watson1953 \
  --id 10.1038/171737a0 \
  --in-place
```

The updated entry receives provider-neutral provenance fields:

```bibtex
bibprovider = {crossref},
bibproviderid = {10.1038/171737a0}
```

Inspect the changes before recording human approval. Provider provenance and
human verification are deliberately separate states.

Inspect the review status of every entry:

```sh
bib integrity status references.bib
```

List just the citation keys that need attention:

```sh
bib -r '.[] | select(.integrity.status != "verified") | .id' references.bib
```

After reviewing one entry, attach its integrity marker atomically:

```sh
bib integrity add references.bib --key turing1936 --in-place
```

The status will become `stale` if any covered field is later changed.

## Query like jq

The query input is an array. Each entry has this shape:

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
    "stored": null
  }
}
```

Use normal jq syntax. `bib` embeds the Rust `jaq` engine, so a separate `jq`
installation is not required.

```sh
# List entries that still need review.
bib -r '.[] | select(.integrity.status != "verified") | .id' references.bib

# Give an agent a compact review packet.
bib -c '[.[] | select(.integrity.status != "verified") |
  {id, type, title: .fields.title, doi: .fields.doi}]' references.bib

# Select entries and emit BibTeX again.
bib --bibtex 'map(select(.fields.year == "2026"))' references.bib

# Read BibTeX from stdin.
cat references.bib | bib -r '.[].fields.doi // empty'
```

The familiar jq flags `-c` (compact), `-r` (raw strings), and `-e` (result-based
exit status) are supported. Multiple files are combined into one input array.
Use `-` as a filename to read that input from stdin. Querying never modifies an
input file.

## Command reference

| Command | Purpose |
| --- | --- |
| `bib FILTER [FILE ...]` | Run a jq filter and emit JSON |
| `bib --bibtex FILTER [FILE ...]` | Emit filtered entry objects as BibTeX |
| `bib source providers` | List installed metadata providers |
| `bib source search QUERY` | Search a provider and return ranked common records |
| `bib source plan FILE --key KEY` | Produce candidates and field-level diffs |
| `bib source apply FILE --key KEY [--id ID]` | Apply one exact provider record |
| `bib integrity status FILE [--json]` | Report `verified`, `stale`, or `unverified` |
| `bib integrity hash FILE KEY` | Print the expected SHA-256 value |
| `bib integrity add FILE --key KEY [--in-place]` | Add or refresh approved entries |
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

Crossref is currently installed and selected by default. Use `--provider NAME`
to select another installed backend; `bib source providers` lists the registry.
The command structure and saved provenance do not depend on Crossref-specific
field names.

Set an email address for providers that support polite API identification:

```sh
export BIB_MAILTO=researcher@example.org
```

### Agent review workflow

For entries with an existing DOI or stored provider identifier:

```sh
bib source plan references.bib --key paper1
bib source apply references.bib --key paper1 --in-place
```

For entries without an exact identifier, `plan` returns up to five ranked
candidates and exits with status `3`. An agent should compare title, authors,
year, venue, record type, and the proposed field diff with the cited work. It
must not silently choose the highest provider score. Apply only an explicitly
selected candidate:

```sh
bib source apply references.bib \
  --key paper1 --id 10.1234/chosen-record --in-place
```

`apply` only updates fields supplied by the provider. It retains fields absent
from the provider record, including local fields such as `file`, `keywords`,
and annotations. It does not add or refresh integrity. After human review:

```sh
bib integrity add references.bib --key paper1 --in-place
```

Use `--all` with `source plan` to generate a review packet for the complete
bibliography. Applying records remains intentionally key-by-key so candidate
selection stays explicit.

## Review workflow

1. Find entries without a valid marker:

   ```sh
   bib -c '[.[] | select(.integrity.status != "verified")]' references.bib
   ```

2. Let the agent fix the selected entries and show the changes to the human.

3. After explicit confirmation, mark only the approved citation keys:

   ```sh
   bib integrity add references.bib --key turing1936 --key shannon1948 --in-place
   ```

4. Check the complete file:

   ```sh
   bib integrity status references.bib
   ```

Use `--all` only when every entry has been reviewed:

```sh
bib integrity add references.bib --all --in-place
```

Without `--in-place`, `add` and `remove` write the updated BibTeX to stdout.
Writes with `--in-place` use an atomic replacement and retain comments,
`@string` declarations, and unrelated formatting.

Neither command chooses entries implicitly: pass one or more `--key` options or
the explicit `--all` option. This keeps an agent from marking unrelated entries
during a partial review.

Other integrity commands:

```sh
bib integrity status references.bib --json
bib integrity hash references.bib turing1936
bib integrity remove references.bib --key turing1936 --in-place
```

`integrity status` exits with `0` when every selected entry is verified, `3`
when any selected entry is stale or unverified, and `2` for an operational or
input error. This makes it suitable for CI and agent loops.

## Integrity format

The marker uses this deterministic format:

1. Parse the entry and resolve BibTeX string macros.
2. Lowercase the entry type and field names.
3. Exclude the citation key and `integrity` field.
4. Serialize the remaining fields plus `ENTRYTYPE` as compact, key-sorted,
   UTF-8 JSON.
5. Store the lowercase SHA-256 digest in the `integrity` field.

```bibtex
@article{turing1936,
  author = {Turing, Alan M.},
  title = {On Computable Numbers},
  year = {1936},
  integrity = {8f...},
}
```

The citation key is deliberately excluded, so a key rename does not invalidate
the marker. The marker is a tamper-evident record of reviewed content. It is not
a digital signature, does not identify the reviewer, and does not by itself
prove that external metadata is true.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Licensed under MIT.
