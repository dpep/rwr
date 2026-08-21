//! The aggregate reporter's contract: it never reports a clean nothing.
//!
//! Every failure here used to produce a valid-looking report with `"repos": []`
//! and no diagnostic -- an unrecognised flag was taken as a path, and a path
//! that was not a directory was filtered away in silence. A report that
//! measured nothing is indistinguishable from a corpus with no Ruby in it.

use std::process::{Command, Output};

fn phase0(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rwr-phase0"))
        .args(args)
        .output()
        .expect("binary runs")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn an_unknown_option_is_not_a_path() {
    let out = phase0(&["--labe", "typo", "."]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--labe"), "{}", stderr(&out));
    assert!(
        out.stdout.is_empty(),
        "no report when the arguments are wrong"
    );
}

/// The commonest real case: a quoted `~` the shell never expanded.
#[test]
fn a_path_that_is_not_a_directory_refuses() {
    let out = phase0(&["--label", "x", "~/no/such/place"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("not a directory"), "{}", stderr(&out));
    assert!(out.stdout.is_empty());
}

#[test]
fn label_without_a_value_refuses() {
    let out = phase0(&["--label"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

/// A real run reports how much it walked, how much it read, and what the
/// `hot_names` cap left out -- so a truncated list cannot read as a whole one.
#[test]
fn a_report_accounts_for_everything_it_walked() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("a.rb"),
        "class A\n  def go; x.y; end\nend\n",
    )
    .expect("write");

    let out = phase0(&["--label", "t", dir.path().to_str().expect("utf8")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");
    let repo = &report["repos"][0];
    assert_eq!(repo["files"], 1);
    assert_eq!(repo["files_measured"], 1);
    assert_eq!(repo["files_unreadable"], 0);
    assert!(repo["hot_names_omitted"].is_number(), "{repo}");
    // Named for the corpus, not for the fallback.
    assert_ne!(repo["name"], "corpus");
}
