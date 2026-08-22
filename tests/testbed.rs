//! Q1's recall measurement, as an executable fact.
//!
//! `testbed/` is a small Ruby app written from the *Ruby* side: it enumerates
//! the ways a method name is reached dynamically, not the ways rwr currently
//! classifies them. Each site carries a `GT:` marker saying what must happen to
//! it, so the ground truth lives next to the code and cannot drift from a line
//! number.
//!
//! | marker | meaning |
//! |---|---|
//! | `GT:rewrite` | rwr must rewrite this site |
//! | `GT:residue` | this breaks and rwr cannot rewrite it, so it must be reported |
//! | `GT:blind`   | this breaks and rwr cannot see it; absence is expected and honest |
//! | `GT:ignore`  | this does not break; rewriting or reporting it is a false positive |
//! | `GT:notice`  | this does not break, and must be reported anyway -- a near-miss |
//!
//! Precision at scale is *not* measured here and cannot be -- a fixture the
//! author wrote proves nothing about noise on a million lines. That half of Q1
//! is measured against discourse and recorded in `docs/internal/phase0-results.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every `GT:` marker in the testbed, as (file, line, kind).
fn ground_truth(root: &Path) -> Vec<(PathBuf, usize, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, usize, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
                continue;
            }
            // The README documents the markers in a table, and scanning it
            // counted the documentation as ground truth.
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if let Some(at) = line.find("GT:") {
                    let kind: String = line[at + 3..]
                        .chars()
                        .take_while(char::is_ascii_alphabetic)
                        .collect();
                    out.push((path.clone(), index + 1, kind));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// Copy the testbed somewhere writable and point a rename at it.
fn run() -> (Vec<(PathBuf, usize, String)>, serde_json::Value) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("testbed");
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("testbed");
    copy_tree(&source, &root);

    let rule = dir.path().join("rename.yml");
    std::fs::write(&rule, "method: Account#display_name\nrename: full_name\n").expect("write");

    let out = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args([
            "check",
            rule.to_str().expect("utf8"),
            root.to_str().expect("utf8"),
            "-j",
        ])
        .output()
        .expect("binary runs");
    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");

    let truth = ground_truth(&root);
    assert!(!truth.is_empty(), "the testbed carries no GT: markers");
    (truth, report)
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable").flatten() {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_tree(&source, &target);
        } else {
            std::fs::copy(&source, &target).expect("copy");
        }
    }
}

/// A marker sits on its own comment line or on the site itself, so a report one
/// line below the marker is the same site.
fn reported(report: &serde_json::Value, file: &Path, line: usize) -> bool {
    let rows = [
        report["residue"].as_array(),
        report["template_residue"].as_array(),
    ];
    rows.into_iter().flatten().any(|rows| {
        rows.iter().any(|r| {
            r["file"].as_str() == file.to_str()
                && r["line"]
                    .as_u64()
                    .is_some_and(|l| l as usize == line || l as usize == line + 1)
        })
    })
}

/// The thesis: every dynamic reach a human enumerated is reported.
///
/// This found two real defects when it was first run. Residue was computed only
/// for files rwr had already *changed*, so a serializer full of `delegate` and
/// `validates` -- a file that is nothing but dynamic reaches, which is the
/// dangerous case exactly -- was never looked at. And the report was scoped to
/// the target class, which discards those same reaches, because a delegation
/// lives in a different class from the method it names. Recall was 2 of 7.
#[test]
fn every_dynamic_reach_is_reported() {
    let (truth, report) = run();
    let expected: Vec<_> = truth
        .iter()
        .filter(|(_, _, k)| k == "residue" || k == "notice")
        .collect();
    assert!(expected.len() >= 7, "the testbed lost coverage");

    let missed: Vec<String> = expected
        .iter()
        .filter(|(f, l, _)| !reported(&report, f, *l))
        .map(|(f, l, _)| {
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            format!("{name}:{l}")
        })
        .collect();

    // A ratchet, not a pass/fail. This corpus is written from Ruby semantics, so
    // it states what *should* happen and the tool does not meet all of it yet --
    // that is the corpus doing its job, and the original scored 2 of 7. What
    // must never happen is the number going up.
    //
    // The one outstanding miss is a namespaced class in *compact* form:
    // `class Account::Exporter` gives Prism the name `Exporter`, so its scope
    // stack is `["Exporter"]` and nothing connects it to Account. Written
    // nested, the same class scores. Both readings are wrong for the same
    // reason -- a namespace is not the class -- and the divergence shows it was
    // never decided (see ruby-situations.md E2).
    const KNOWN_MISSES: usize = 1;
    assert!(
        missed.len() <= KNOWN_MISSES,
        "recall regressed -- {} unreported, was {KNOWN_MISSES}: {missed:?}",
        missed.len()
    );
    assert_eq!(
        expected.len() - missed.len(),
        expected.len() - KNOWN_MISSES,
        "recall improved to {} of {} -- lower KNOWN_MISSES to lock it in. Outstanding: {missed:?}",
        expected.len() - missed.len(),
        expected.len()
    );
}

/// Every site that must change, changed -- and the count is exact, so a rewrite
/// leaking into a class it was not about would fail here too.
#[test]
fn every_site_that_must_change_changed() {
    let (truth, report) = run();
    let expected = truth.iter().filter(|(_, _, k)| k == "rewrite").count();
    let actual: u64 = report["changed"]
        .as_array()
        .expect("changed")
        .iter()
        .filter_map(|c| c["sites"].as_u64())
        .sum();
    assert_eq!(actual as usize, expected);
}

/// Noise has a budget, and it is small and named.
///
/// One false positive is expected and understood: a string literal equal to the
/// method name is indistinguishable from `send("display_name")` without running
/// the program, so reporting it is the conservative choice. Anything beyond
/// that is a regression -- notably `Company#display_name`, an unrelated class
/// whose own method shares the name.
#[test]
fn false_positives_stay_within_their_budget() {
    let (truth, report) = run();
    let wrong: Vec<String> = truth
        .iter()
        .filter(|(_, _, k)| k == "ignore")
        .filter(|(f, l, _)| reported(&report, f, *l))
        .map(|(f, l, _)| format!("{}:{l}", f.display()))
        .collect();
    // Also a ratchet. Each of these is understood:
    //
    // - a string literal equal to the method name, indistinguishable from
    //   `send("display_name")` without running the program;
    // - `Struct.new(:display_name, ...)`, which really does define the method --
    //   on a different class. It sits three lines from an identical-looking
    //   `Field.new(:display_name, ...)` that *is* a reach, and no list of
    //   caller names separates them. A precision ceiling, not a bug;
    // - a HAML line, found by text search and labelled `text` because HAML has
    //   no delimiters to stitch. Honest, weaker evidence, and still a miss-by-
    //   noise;
    // - a call inside `class << self`, correctly declined for rewriting and
    //   then reported. Arguably a `notice`.
    const KNOWN_NOISE: usize = 4;
    assert!(
        wrong.len() <= KNOWN_NOISE,
        "precision regressed -- {} false positives, was {KNOWN_NOISE}: {wrong:?}",
        wrong.len()
    );
}

/// The account has to reach a machine consumer. It was text-only, so `-j` --
/// the mode the skill tells agents to use -- returned edits with no account of
/// what they missed at all.
///
/// This asserted `templates_skipped >= 1` until structural ERB shipped and made
/// that false. It kept passing anyway, because the JSON was counting *parsed*
/// templates as skipped -- a stale premise propped up by a bug, each hiding the
/// other. The claim worth pinning is that the view's reach is reported at all.
#[test]
fn the_account_survives_json() {
    let (_, report) = run();
    assert!(report["residue"].is_array(), "{report}");
    let from_the_view = report["residue"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(report["template_residue"].as_array().into_iter().flatten())
        .any(|r| r["file"].as_str().is_some_and(|f| f.ends_with(".erb")));
    assert!(
        from_the_view,
        "the ERB view reaches the renamed name and the report must say so: {report}"
    );
    // Templates that cannot be stitched are counted, not hidden. HAML has no
    // delimiters to stitch, so the honest number here is nonzero -- this
    // asserted 0 until a HAML file arrived, which is the assertion drifting
    // from the corpus rather than the tool being wrong.
    let skipped = report["templates_skipped"].as_u64().expect("a count");
    let haml = report["template_residue"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|r| r["file"].as_str().is_some_and(|f| f.ends_with(".haml")));
    assert_eq!(
        skipped > 0,
        haml,
        "a text-searched template must be counted as skipped: {report}"
    );
}
