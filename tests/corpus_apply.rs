//! Running the Phase 0 corpus through rwr itself.
//!
//! The corpus is the regression suite, and until this existed it was only ever
//! checked by hand -- so a change could have broken every entry silently. It
//! drives the built binary rather than the library, because what needs
//! defending is the behaviour a user gets.
//!
//! Scoring is output equality (see `corpus/README.md`): a fixture with an
//! `out/` counterpart must be transformed into it byte for byte, and an
//! `in/refuses-*.rb` must be declined with the source untouched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

fn entries() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(corpus_dir())
        .expect("corpus/ exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.as_bytes().first().is_some_and(u8::is_ascii_digit))
        })
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "corpus has no entries");
    dirs
}

fn ruby_files(dir: &Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return vec![];
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .expect("readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rb"))
        .collect();
    files.sort();
    files
}

/// Apply an entry's rule to one fixture in a scratch directory.
fn apply(entry: &Path, fixture: &Path) -> (Option<i32>, String) {
    let scratch = tempfile::tempdir().expect("temp dir");
    let target = scratch.path().join(fixture.file_name().expect("named"));
    fs::copy(fixture, &target).expect("copy fixture");

    // Applied to a fixpoint, because a real caller is a loop. rwr deliberately
    // does not iterate internally -- `foo($A) -> foo(bar($A))` matches its own
    // output and diverges -- so convergence is the caller's job, and two things
    // make a second pass necessary: a match contained in a wider edit (exit 4),
    // and a node with several possible bindings, which `search` reports once.
    // Bounded, so a rule that never converges fails here rather than hanging.
    let mut code = None;
    let mut previous = fs::read_to_string(&target).expect("read");
    for pass in 0..8 {
        let run = Command::new(env!("CARGO_BIN_EXE_rwr"))
            .arg("rewrite")
            .arg(entry.join("rule.yml"))
            .arg(scratch.path())
            .output()
            .expect("rwr runs");
        code = run.status.code();
        let now = fs::read_to_string(&target).expect("read");
        if now == previous {
            break;
        }
        previous = now;
        assert!(pass < 7, "rule did not converge within 8 passes");
    }

    let produced = fs::read_to_string(&target).expect("read result");
    (code, produced)
}

/// Every fixture with an expected output must be transformed into it exactly.
#[test]
fn every_corpus_entry_produces_its_expected_output() {
    let mut checked = 0;
    for entry in entries() {
        for fixture in ruby_files(&entry.join("in")) {
            let name = fixture.file_name().expect("named");
            let expected_path = entry.join("out").join(name);
            if !expected_path.is_file() {
                continue;
            }

            let (code, produced) = apply(&entry, &fixture);
            let expected = fs::read_to_string(&expected_path).expect("read expected");
            assert_eq!(
                produced,
                expected,
                "{}/{} did not produce its expected output (exit {code:?})",
                entry.file_name().expect("named").to_string_lossy(),
                name.to_string_lossy(),
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no corpus fixtures were checked");
}

/// Refusing correctly is behaviour worth defending: an `in/refuses-*.rb` must
/// exit 5 and leave the source byte-identical. A rewrite that silently
/// succeeded here would be exactly the quiet wrongness the design exists to
/// prevent.
#[test]
fn refusal_fixtures_decline_and_leave_the_source_alone() {
    let mut checked = 0;
    for entry in entries() {
        for fixture in ruby_files(&entry.join("in")) {
            let name = fixture
                .file_name()
                .expect("named")
                .to_string_lossy()
                .into_owned();
            if !name.starts_with("refuses-") {
                continue;
            }

            let (code, produced) = apply(&entry, &fixture);
            let original = fs::read_to_string(&fixture).expect("read original");
            assert_eq!(code, Some(5), "{name} should refuse");
            assert_eq!(produced, original, "{name} was modified despite refusing");
            checked += 1;
        }
    }
    assert!(checked > 0, "no refusal fixtures were checked");
}
