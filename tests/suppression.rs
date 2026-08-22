//! Ground truth for `# rwr:ignore`, scored against the shipped pack.
//!
//! `testbed/suppression/` is written from the *failure* side: every case is one
//! where a plausible implementation is silently wrong rather than visibly
//! broken. `GT:flagged` carries most of the weight -- it asks what happens
//! *after* a covered statement ends, which is the question the first node-scoped
//! implementation passed every happy-path case without answering, while
//! swallowing a whole file.

use std::path::Path;
use std::process::Command;

/// Every `GT:` marker, as (line, kind).
fn ground_truth(source: &str) -> Vec<(usize, String)> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            line.find("GT:").map(|at| {
                let kind: String = line[at + 3..]
                    .chars()
                    .take_while(char::is_ascii_alphabetic)
                    .collect();
                (index + 1, kind)
            })
        })
        .collect()
}

fn report() -> (Vec<(usize, String)>, serde_json::Value) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("testbed/suppression/directives.rb");
    let source = std::fs::read_to_string(&file).expect("readable");

    let out = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args([
            "check",
            "style/return-nil",
            file.to_str().expect("utf8"),
            "-j",
        ])
        .output()
        .expect("binary runs");
    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");

    let truth = ground_truth(&source);
    assert!(!truth.is_empty(), "the testbed carries no GT: markers");
    (truth, doc)
}

fn lines(rows: &serde_json::Value) -> Vec<usize> {
    rows.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["line"].as_u64().map(|l| l as usize))
                .collect()
        })
        .unwrap_or_default()
}

/// Each marker gets what it claims, and nothing else does.
#[test]
fn every_directive_accepts_exactly_what_it_covers() {
    let (truth, doc) = report();
    let want = |kind: &str| -> Vec<usize> {
        truth
            .iter()
            .filter(|(_, k)| k == kind)
            .map(|(line, _)| *line)
            .collect()
    };

    let mut accepted = lines(&doc["suppressed"]);
    accepted.sort_unstable();
    assert_eq!(accepted, want("accepted"), "suppressed sites");

    let mut stale = lines(&doc["stale_suppressions"]);
    stale.sort_unstable();
    assert_eq!(stale, want("stale"), "stale directives");

    let mut malformed = lines(&doc["malformed_directives"]);
    malformed.sort_unstable();
    assert_eq!(malformed, want("malformed"), "malformed directives");
}

/// The other half, and the one that catches over-reach: a violation with no
/// directive of its own must survive, however near a directive it sits.
#[test]
fn a_directive_never_reaches_past_what_it_covers() {
    let (truth, doc) = report();
    let flagged: Vec<usize> = truth
        .iter()
        .filter(|(_, k)| k == "flagged" || k == "malformed")
        .map(|(line, _)| *line)
        .collect();

    let suppressed = lines(&doc["suppressed"]);
    for line in &flagged {
        assert!(
            !suppressed.contains(line),
            "line {line} must not be suppressed: {doc}"
        );
    }
    // And they are all still counted as work to do.
    let sites: usize = doc["changed"]
        .as_array()
        .map(|rows| rows.iter().filter_map(|r| r["sites"].as_u64()).sum::<u64>() as usize)
        .unwrap_or_default();
    assert_eq!(sites, flagged.len(), "every unsuppressed violation counted");
}
