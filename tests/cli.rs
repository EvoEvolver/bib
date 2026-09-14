use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use assert_cmd::Command;
use predicates::prelude::*;

const SAMPLE: &str = r#"% retained comment
@string{conf = {Great Conf}}

@article{alpha,
  title = {Alpha},
  journal = conf,
  year = {2026},
}

@book{beta,
  title = {Beta},
  year = {2025},
}
"#;

#[test]
fn top_level_help_documents_inspect_pipe_and_review_workflow() {
    Command::cargo_bin("bib")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("INSPECT AND PIPE")
                .and(predicate::str::contains("LITERATURE SOURCES"))
                .and(predicate::str::contains("SCOPE"))
                .and(predicate::str::contains("INTEGRITY"))
                .and(predicate::str::contains("EXIT STATUS"))
                .and(predicate::str::contains(
                    "bib integrity add refs.bib --keys-from - --source agent --agent MODEL --in-place",
                ))
                .and(predicate::str::contains(
                    "does not provide arbitrary metadata editing",
                )),
        );
}

#[test]
fn inspect_help_defines_external_pipeline_contract() {
    Command::cargo_bin("bib")
        .unwrap()
        .args(["inspect", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("one JSON array")
                .and(predicate::str::contains("bib inspect refs.bib | jq"))
                .and(predicate::str::contains("does not evaluate filters")),
        );
}

#[test]
fn source_help_explains_provider_and_review_workflow() {
    Command::cargo_bin("bib")
        .unwrap()
        .args(["source", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("bib source plan refs.bib")
                .and(predicate::str::contains("@bibsource"))
                .and(predicate::str::contains("never embeds response"))
                .and(predicate::str::contains("bib source trace"))
                .and(predicate::str::contains("bib integrity add refs.bib")),
        );
}

#[test]
fn provider_apply_preserves_local_content_and_requires_later_review() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(
        &path,
        r#"% retain me
@string{venue = {Old Journal}}
@article{paper1,
  title = {Old title},
  journal = venue,
  file = {/local/paper.pdf},
}
"#,
    )
    .unwrap();
    let base_url = mock_crossref(
        r#"{"message":{"DOI":"10.1234/example","type":"journal-article","title":["Provider title"],"author":[{"given":"Jane","family":"Doe"}],"container-title":["Provider Journal"],"issued":{"date-parts":[[2026]]}}}"#,
    );

    Command::cargo_bin("bib")
        .unwrap()
        .env("BIB_CROSSREF_API_BASE", base_url)
        .args(["source", "apply"])
        .arg(&path)
        .args(["--key", "paper1", "--id", "10.1234/example", "--in-place"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "review it before adding integrity",
        ));

    let output = fs::read_to_string(&path).unwrap();
    assert!(output.starts_with("% retain me\n@string{venue"));
    assert!(output.contains("title = {Provider title}"));
    assert!(output.contains("journal = {Provider Journal}"));
    assert!(output.contains("file = {/local/paper.pdf}"));
    assert!(output.contains("bibprovider = {crossref}"));
    assert!(output.contains("bibproviderid = {10.1234/example}"));
    assert!(!output.contains("integrity ="));
}

#[test]
fn provider_apply_does_not_write_invalid_bibtex() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    let original = "@article{paper1, title={Original}}\n";
    fs::write(&path, original).unwrap();
    let base_url = mock_crossref(
        r#"{"message":{"DOI":"10.1234/broken","type":"journal-article","title":["Unbalanced } title"]}}"#,
    );

    Command::cargo_bin("bib")
        .unwrap()
        .env("BIB_CROSSREF_API_BASE", base_url)
        .args(["source", "apply"])
        .arg(&path)
        .args(["--key", "paper1", "--id", "10.1234/broken", "--in-place"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "provider update produced invalid BibTeX",
        ));

    assert_eq!(fs::read_to_string(path).unwrap(), original);
}

#[test]
fn inspect_emits_pipeline_ready_json_from_stdin() {
    let output = Command::cargo_bin("bib")
        .unwrap()
        .args(["inspect", "-", "--json", "--compact"])
        .write_stdin(SAMPLE)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let entries: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 2);
    assert_eq!(entries[0]["id"], "alpha");
    assert_eq!(entries[0]["type"], "article");
    assert_eq!(entries[0]["integrity"]["status"], "unverified");
}

#[test]
fn integrity_lifecycle_has_scriptable_exit_codes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(&path, SAMPLE).unwrap();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--key", "alpha"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("unverified\talpha"));

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "add"])
        .arg(&path)
        .args([
            "--all",
            "--source",
            "agent",
            "--agent",
            "test-agent",
            "--in-place",
        ])
        .assert()
        .success();

    let sealed = fs::read_to_string(&path).unwrap();
    assert!(sealed.starts_with("% retained comment\n@string"));

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--key", "alpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains("verified\talpha"));

    fs::write(
        &path,
        sealed.replace("title = {Alpha}", "title = {Changed}"),
    )
    .unwrap();
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--key", "alpha"])
        .assert()
        .code(3)
        .stdout("stale\talpha\tagent\n");
}

#[test]
fn add_requires_an_explicit_review_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(&path, SAMPLE).unwrap();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "add"])
        .arg(&path)
        .args(["--source", "agent", "--agent", "test-agent"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "pass --key KEY or --all after review",
        ));
}

#[test]
fn integrity_add_requires_explicit_provenance_kind() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(&path, SAMPLE).unwrap();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "add"])
        .arg(&path)
        .args(["--key", "alpha"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--source <SOURCE>"));
}

#[test]
fn agent_integrity_creates_attributed_source_entry() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(&path, SAMPLE).unwrap();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "add"])
        .arg(&path)
        .args([
            "--key",
            "alpha",
            "--source",
            "agent",
            "--agent",
            "claude-code/test",
            "--in-place",
        ])
        .assert()
        .success();

    let output = fs::read_to_string(&path).unwrap();
    assert!(output.contains("bibsource = {bibsource:agent:"));
    assert!(output.contains("@bibsource"));
    assert!(output.contains("kind = {agent}"));
    assert!(output.contains("actor = {claude-code/test}"));
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--key", "alpha"])
        .assert()
        .success()
        .stdout("verified\talpha\tagent\n");

    let inspected = Command::cargo_bin("bib")
        .unwrap()
        .args(["inspect"])
        .arg(&path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let entries: serde_json::Value = serde_json::from_slice(&inspected).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 2);
    assert_eq!(entries[0]["id"], "alpha");
    assert_eq!(entries[1]["id"], "beta");

    fs::write(
        &path,
        output.replace("actor = {claude-code/test}", "actor = {other-agent}"),
    )
    .unwrap();
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--key", "alpha"])
        .assert()
        .code(3)
        .stdout("invalid\talpha\tagent\n");
}

#[test]
fn integrity_add_reads_agent_selected_keys_from_stdin() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(&path, SAMPLE).unwrap();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "add"])
        .arg(&path)
        .args([
            "--keys-from",
            "-",
            "--source",
            "agent",
            "--agent",
            "pipeline-agent",
            "--in-place",
        ])
        .write_stdin("alpha\n\n")
        .assert()
        .success();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--keys-from", "-"])
        .write_stdin("alpha\n")
        .assert()
        .success()
        .stdout("verified\talpha\tagent\n");

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--key", "beta"])
        .assert()
        .code(3)
        .stdout("unverified\tbeta\n");

    let keys_path = directory.path().join("approved.keys");
    fs::write(&keys_path, "alpha\n").unwrap();
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "remove"])
        .arg(&path)
        .arg("--keys-from")
        .arg(&keys_path)
        .arg("--in-place")
        .assert()
        .success();
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .args(["--key", "alpha"])
        .assert()
        .code(3)
        .stdout("unverified\talpha\tagent\n");
}

#[test]
fn explicit_empty_keys_pipeline_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(&path, SAMPLE).unwrap();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "add"])
        .arg(&path)
        .args([
            "--keys-from",
            "-",
            "--source",
            "agent",
            "--agent",
            "pipeline-agent",
        ])
        .write_stdin("\n")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no citation keys found in -"));
}

#[test]
fn crossref_pipeline_records_compact_receipt_and_adds_valid_integrity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(
        &path,
        "@article{paper1, doi={10.1234/example}, volume={stale}, pages={1--2}, note={local}}\n",
    )
    .unwrap();
    let body = r#"{"message":{"DOI":"10.1234/example","type":"journal-article","title":["Provider title"],"author":[{"given":"Jane","family":"Doe"}],"issued":{"date-parts":[[2026]]}}}"#;
    let base_url = mock_crossref(body);

    Command::cargo_bin("bib")
        .unwrap()
        .env("BIB_CROSSREF_API_BASE", base_url)
        .args(["source", "apply"])
        .arg(&path)
        .args(["--key", "paper1", "--add-integrity", "--in-place"])
        .assert()
        .success();

    let output = fs::read_to_string(&path).unwrap();
    assert!(output.contains("kind = {provider}"));
    assert!(output.contains("provider = {crossref}"));
    assert!(output.contains("mediatype = {application/vnd.crossref-api-message+json}"));
    assert!(output.contains("responsesha256 = {"));
    assert!(output.contains("projectionsha256 = {"));
    assert!(!output.contains("responseencoding ="));
    assert!(!output.contains("response ="));
    assert!(!output.contains("volume = {stale}"));
    assert!(!output.contains("pages = {1--2}"));
    let parsed = bib_cli::bibtex::parse(&output).unwrap();
    let paper = parsed
        .iter()
        .find(|record| record.entry_key == "paper1")
        .unwrap();
    assert_eq!(paper.fields.get("note").map(String::as_str), Some("local"));
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .assert()
        .success()
        .stdout("verified\tpaper1\tprovider\n");

    let trace = Command::cargo_bin("bib")
        .unwrap()
        .args(["source", "trace"])
        .arg(&path)
        .args(["--key", "paper1", "--compact"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
    assert_eq!(trace["target"], "paper1");
    assert_eq!(trace["provider"]["fields"]["provider"], "crossref");
    assert!(trace["provider"]["fields"].get("response").is_none());

    fs::write(
        &path,
        output.replace("projectionsha256 = {", "projectionsha256 = {0"),
    )
    .unwrap();
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
        .assert()
        .code(3)
        .stdout("invalid\tpaper1\tprovider\n");
}

#[test]
fn resolves_doi_url_without_network_access() {
    let output = Command::cargo_bin("bib")
        .unwrap()
        .args([
            "source",
            "resolve",
            "https://publisher.example/article/10.1234/Example",
            "--compact",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["status"], "resolved");
    assert_eq!(report["candidates"][0]["kind"], "doi");
    assert_eq!(report["candidates"][0]["value"], "10.1234/example");
    assert_eq!(report["candidates"][0]["evidence"]["method"], "url-doi");
}

#[test]
fn strip_responses_migrates_legacy_evidence_without_reformatting() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(
        &path,
        "% keep\n@bibsource{receipt, kind={provider}, responseencoding={base64}, response={WA==}, responsesha256={abc}}\n",
    )
    .unwrap();

    Command::cargo_bin("bib")
        .unwrap()
        .args(["source", "strip-responses"])
        .arg(&path)
        .arg("--in-place")
        .assert()
        .success()
        .stderr(predicate::str::contains("removed 2 legacy response field"));

    let output = fs::read_to_string(path).unwrap();
    assert!(output.starts_with("% keep\n@bibsource"));
    assert!(!output.contains("responseencoding"));
    assert!(!output.contains("response={"));
    assert!(output.contains("responsesha256={abc}"));
}

#[test]
fn apply_resolves_entry_url_and_records_linked_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(
        &path,
        "@article{paper1, url={https://doi.org/10.1234/Example}, note={local}}\n",
    )
    .unwrap();
    let body = r#"{"message":{"DOI":"10.1234/example","type":"journal-article","title":["Resolved title"],"issued":{"date-parts":[[2026]]}}}"#;
    let base_url = mock_crossref(body);

    Command::cargo_bin("bib")
        .unwrap()
        .env("BIB_CROSSREF_API_BASE", base_url)
        .args(["source", "apply"])
        .arg(&path)
        .args(["--key", "paper1", "--add-integrity", "--in-place"])
        .assert()
        .success();

    let output = fs::read_to_string(&path).unwrap();
    assert!(output.contains("kind = {resolution}"));
    assert!(output.contains("method = {url-doi}"));
    assert!(output.contains("identifier = {10.1234/example}"));
    assert!(output.contains("resolution = {bibsource:resolution:"));
    assert!(output.contains("projectionsha256 = {"));
    assert!(!output.contains("responseencoding ="));
    assert!(!output.contains("response ="));

    let trace = Command::cargo_bin("bib")
        .unwrap()
        .args(["source", "trace"])
        .arg(&path)
        .args(["--key", "paper1", "--compact"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
    assert_eq!(trace["resolution"]["fields"]["method"], "url-doi");
    assert_eq!(
        trace["resolution"]["fields"]["identifier"],
        "10.1234/example"
    );

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(path)
        .assert()
        .success()
        .stdout("verified\tpaper1\tprovider\n");
}

#[test]
fn plan_resolves_entry_url_before_exact_provider_lookup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(
        &path,
        "@article{paper1, url={https://doi.org/10.1234/Example}}\n",
    )
    .unwrap();
    let base_url = mock_crossref(
        r#"{"message":{"DOI":"10.1234/example","type":"journal-article","title":["Resolved title"]}}"#,
    );

    let output = Command::cargo_bin("bib")
        .unwrap()
        .env("BIB_CROSSREF_API_BASE", base_url)
        .args(["source", "plan"])
        .arg(&path)
        .args(["--key", "paper1", "--compact"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(rows[0]["status"], "resolved-exact");
    assert_eq!(
        rows[0]["resolution"]["candidates"][0]["value"],
        "10.1234/example"
    );
    assert_eq!(rows[0]["candidates"][0]["record"]["id"], "10.1234/example");
}

#[test]
fn apply_rejects_provider_record_that_disagrees_with_resolved_doi() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    let original = "@article{paper1, url={https://doi.org/10.1234/Expected}}\n";
    fs::write(&path, original).unwrap();
    let base_url = mock_crossref(
        r#"{"message":{"DOI":"10.1234/different","type":"journal-article","title":["Wrong record"]}}"#,
    );

    Command::cargo_bin("bib")
        .unwrap()
        .env("BIB_CROSSREF_API_BASE", base_url)
        .args(["source", "apply"])
        .arg(&path)
        .args(["--key", "paper1", "--add-integrity", "--in-place"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "does not match provider record id",
        ));

    assert_eq!(fs::read_to_string(path).unwrap(), original);
}

#[test]
fn doi_content_negotiation_pipeline_is_provider_backed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(&path, "@article{paper1, doi={10.1234/example}}\n").unwrap();
    let base_url = mock_response(
        "application/x-bibtex",
        "@article{remote, title={DOI title}, author={Doe, Jane}, year={2026}, doi={10.1234/example}}",
    );

    Command::cargo_bin("bib")
        .unwrap()
        .env("BIB_DOI_API_BASE", base_url)
        .args(["source", "apply"])
        .arg(&path)
        .args([
            "--key",
            "paper1",
            "--provider",
            "doi",
            "--add-integrity",
            "--in-place",
        ])
        .assert()
        .success();

    let output = fs::read_to_string(&path).unwrap();
    assert!(output.contains("title = {DOI title}"));
    assert!(output.contains("provider = {doi}"));
    assert!(output.contains("mediatype = {application/x-bibtex}"));
    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(path)
        .assert()
        .success()
        .stdout("verified\tpaper1\tprovider\n");
}

fn mock_crossref(body: &'static str) -> String {
    mock_response("application/json", body)
}

fn mock_response(content_type: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    format!("http://{address}/")
}
