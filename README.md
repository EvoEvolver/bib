# bib

`bib` is a Rust CLI for querying BibTeX with jq filters and attaching
reviewable integrity markers to approved entries.

It is designed for a workflow where an agent edits or researches bibliography
entries, a human reviews the result, and the agent marks only the confirmed
citation keys. A later content change makes that marker `stale`.

## Install

```sh
cargo install --git https://github.com/EvoEvolver/bib
```

For local development:

```sh
cargo install --path .
```

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
exit status) are supported. Querying never modifies the input file.

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

The marker is compatible with
[`doomspec/Url2Bibtex`](https://github.com/doomspec/Url2Bibtex):

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

The citation key is deliberately excluded for upstream compatibility, so a key
rename does not invalidate the marker. The marker is a tamper-evident record of
reviewed content. It is not a digital signature, does not identify the reviewer,
and does not by itself prove that external metadata is true.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Licensed under MIT.

