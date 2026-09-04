use std::fs;

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
fn top_level_help_documents_query_edit_and_review_workflow() {
    Command::cargo_bin("bib")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("INPUT OBJECT")
                .and(predicate::str::contains("QUERY EXAMPLES"))
                .and(predicate::str::contains("EDITING"))
                .and(predicate::str::contains("INTEGRITY"))
                .and(predicate::str::contains("EXIT STATUS"))
                .and(predicate::str::contains(".fields.year = \"2026\""))
                .and(predicate::str::contains(
                    "bib integrity add updated.bib --key paper1 --in-place",
                )),
        );
}

#[test]
fn query_supports_jq_filters_and_raw_output() {
    Command::cargo_bin("bib")
        .unwrap()
        .args(["-r", ".[] | select(.type == \"article\") | .id", "-"])
        .write_stdin(SAMPLE)
        .assert()
        .success()
        .stdout("alpha\n");
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
        .assert()
        .code(3)
        .stdout(predicate::str::contains("unverified\talpha"));

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "add"])
        .arg(&path)
        .args(["--all", "--in-place"])
        .assert()
        .success();

    let sealed = fs::read_to_string(&path).unwrap();
    assert!(sealed.starts_with("% retained comment\n@string"));

    Command::cargo_bin("bib")
        .unwrap()
        .args(["integrity", "status"])
        .arg(&path)
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
        .stdout("stale\talpha\n");
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
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "pass --key KEY or --all after review",
        ));
}
