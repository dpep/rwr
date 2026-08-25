//! Property tests over real Ruby.
//!
//! The corpus checks specific transformations against specific expectations.
//! These check invariants that must hold for *any* input, which is what catches
//! range-arithmetic bugs -- a class where a wrong offset produces output that
//! still parses and is therefore invisible to reparse-verify.
//!
//! Run against the local Ruby checkouts when present; skipped otherwise, so CI
//! stays green without them.

use std::path::{Path, PathBuf};
use std::process::Command;

fn corpus_root() -> PathBuf {
    std::env::var("RWR_CORPUS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("code/lib/ruby")
        })
}

/// A scratch copy of up to `limit` Ruby files from a repository.
fn scratch_copy(from: &Path, limit: usize) -> Option<(tempfile::TempDir, Vec<PathBuf>)> {
    if !from.is_dir() {
        return None;
    }
    let dir = tempfile::tempdir().ok()?;
    let mut copied = Vec::new();
    for entry in ignore::WalkBuilder::new(from)
        .build()
        .filter_map(Result::ok)
    {
        if copied.len() >= limit {
            break;
        }
        let path = entry.into_path();
        if path.extension().is_some_and(|x| x == "rb") {
            let target = dir.path().join(format!("f{}.rb", copied.len()));
            if std::fs::copy(&path, &target).is_ok() {
                copied.push(target);
            }
        }
    }
    (!copied.is_empty()).then_some((dir, copied))
}

fn rwr(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(args)
        .output()
        .expect("rwr runs")
}

/// A refused run is not a passed run.
///
/// Every property test here asserted `!= Some(2)` -- *errored* -- and exit 5 is
/// **Refused**, which is a different outcome entirely. A refusal discards the
/// whole transformation, so the files come back byte-identical to the original
/// and an assertion that they match passes with flying colours. Three tests
/// were blind to it, which is how a verification layer that refused correct
/// work reached a release: the suite could not tell "did nothing because there
/// was nothing to do" from "did nothing because it gave up".
fn assert_not_refused(out: &std::process::Output, what: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(2), "{what} errored: {stderr}");
    assert_ne!(out.status.code(), Some(5), "{what} was refused: {stderr}");
    // The exit code is not enough on its own: a run over many files can refuse
    // some and still finish with a different code, and the round-trip test read
    // clean through exactly that. The message is per file, so look for it.
    assert!(
        !stderr.contains("refused"),
        "{what} refused at least one file: {stderr}"
    );
}

/// Rewriting a pattern to itself must change nothing, byte for byte.
///
/// A wrong offset here produces output that still parses -- so reparse-verify
/// cannot catch it, and only comparing against the input can.
/// The same property over ERB, where it guards something harder.
///
/// A rewrite through a template computes edits against a *stitched* Ruby
/// buffer and maps them back through a fragment map. An off-by-one anywhere in
/// that map corrupts the template while still producing valid output, and
/// nothing else in the suite would notice. An identity rewrite has to come back
/// byte-identical.
#[test]
fn an_identity_rewrite_through_erb_changes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let cases: &[&str] = &[
        "<h1><%= account.display_name %></h1>\n",
        // Two tags on one line: the map has to keep them apart.
        "<p><%= a.display_name %> and <%= b.display_name %></p>\n",
        // Control flow, which only parses because the tags are stitched.
        "<% accounts.each do |account| %>\n  <li><%= account.display_name %></li>\n<% end %>\n",
        // Trim markers and sigils are not part of the Ruby.
        "<%- if x -%>\n<%== account.display_name %>\n<%- end -%>\n",
        // A comment tag holds prose, and `<%%` is an escaped literal.
        "<%# display_name is mentioned here %>\n<%%= not_a_tag %>\n<%= account.display_name %>\n",
        // No Ruby at all.
        "<h1>just html</h1>\n",
    ];

    for (index, source) in cases.iter().enumerate() {
        let view = dir.path().join(format!("v{index}.html.erb"));
        std::fs::write(&view, source).expect("write");
    }

    for pattern in ["$R.display_name", "$R.each { |$P| $B }", "account.$M"] {
        let out = rwr(&[
            "rewrite",
            pattern,
            "-r",
            pattern,
            dir.path().to_str().expect("utf8"),
        ]);
        assert!(
            out.status.code() != Some(2),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        for (index, source) in cases.iter().enumerate() {
            let view = dir.path().join(format!("v{index}.html.erb"));
            let after = std::fs::read_to_string(&view).expect("read");
            assert_eq!(
                after, *source,
                "pattern {pattern:?} changed case {index} under an identity rewrite"
            );
        }
    }
}

#[test]
fn an_identity_rewrite_changes_nothing() {
    let Some((dir, files)) = scratch_copy(&corpus_root().join("rails"), 400) else {
        eprintln!("skipping: no rails corpus");
        return;
    };
    let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();

    for pattern in [
        "return nil",
        "$R.each { |$P| $B }",
        "$R.map { |$P| $B }",
        "foo($A, *$REST)",
        "{$K: $V}",
    ] {
        let out = rwr(&[
            "rewrite",
            pattern,
            "-r",
            pattern,
            dir.path().to_str().expect("utf8"),
        ]);
        assert_not_refused(&out, &format!("identity rewrite of {pattern:?}"));
        for (file, original) in files.iter().zip(&before) {
            let now = std::fs::read(file).expect("read");
            assert_eq!(
                &now,
                original,
                "identity rewrite of {pattern:?} changed {}",
                file.display()
            );
        }
    }
}

/// Renaming a method and renaming it back must restore the original exactly.
///
/// Round-tripping exercises the splice in both directions, so an off-by-one
/// that happens to be self-consistent in one direction still shows up.
#[test]
fn a_rename_round_trips() {
    let Some((dir, files)) = scratch_copy(&corpus_root().join("rails"), 400) else {
        eprintln!("skipping: no rails corpus");
        return;
    };
    let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    let path = dir.path().to_str().expect("utf8");

    let there = rwr(&["rewrite", "$R.size", "-r", "$R.rwr_tmp_size", path]);
    assert_not_refused(&there, "forward rename");
    let back = rwr(&["rewrite", "$R.rwr_tmp_size", "-r", "$R.size", path]);
    assert_not_refused(&back, "reverse rename");

    for (file, original) in files.iter().zip(&before) {
        let now = std::fs::read(file).expect("read");
        assert_eq!(&now, original, "round trip changed {}", file.display());
    }
}

/// Whatever rwr writes must parse. Reparse-verify enforces this per file, and
/// this checks it end to end over real source rather than fixtures.
#[test]
fn rewritten_output_always_parses() {
    let Some((dir, files)) = scratch_copy(&corpus_root().join("rails"), 400) else {
        eprintln!("skipping: no rails corpus");
        return;
    };
    let out = rwr(&[
        "rewrite",
        "$R.size",
        "-r",
        "$R.length",
        dir.path().to_str().expect("utf8"),
    ]);
    assert_not_refused(&out, "rename");

    for file in &files {
        let src = std::fs::read(file).expect("read");
        assert_eq!(
            ruby_prism::parse(&src).errors().count(),
            0,
            "{} no longer parses",
            file.display()
        );
    }
}

/// Rewrites that actually change real code, over patterns that nest.
///
/// The identity property cannot exercise the result checks at all: an identity
/// rewrite emits zero edits, so there is no changed site to check. Only a
/// rewrite that *moves* something reaches them, and only real code contains the
/// shapes that break them -- a rule matching inside its own capture refused a
/// correct rewrite, and nothing in the pack, the testbed or the fixtures
/// matches inside its own capture, so every one of them was silent.
///
/// Measured before being written: with the nested-capture guard removed, this
/// refuses on `$R.size` and `$R.freeze` over rails. It is the test that would
/// have caught it.
#[test]
fn a_changing_rewrite_over_real_code_is_never_refused() {
    for (pattern, template) in [
        // Chains where the receiver of a match is itself a match.
        ("$R.size", "$R.rwr_len"),
        ("$R.freeze", "$R.rwr_frozen"),
        ("$R.to_s", "$R.rwr_str"),
        ("$R.map { |$P| $B }", "$R.rwr_collect { |$P| $B }"),
    ] {
        let Some((dir, files)) = scratch_copy(&corpus_root().join("rails"), 400) else {
            eprintln!("skipping: no rails corpus");
            return;
        };
        let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
        let out = rwr([
            "rewrite",
            pattern,
            "-r",
            template,
            dir.path().to_str().expect("utf8"),
        ]
        .as_ref());
        assert_not_refused(&out, &format!("{pattern:?} -> {template:?}"));

        // And it has to have done something, or the assertion above is vacuous:
        // a run that quietly matched nothing would sail through it.
        let touched = files
            .iter()
            .zip(&before)
            .filter(|(f, original)| {
                std::fs::read(f).expect("read").as_slice() != original.as_slice()
            })
            .count();
        assert!(
            touched > 0,
            "{pattern:?} changed nothing, so this proves nothing"
        );

        // Whatever it wrote must still parse.
        for file in &files {
            let src = std::fs::read(file).expect("read");
            assert_eq!(
                ruby_prism::parse(&src).errors().count(),
                0,
                "{} no longer parses after {pattern:?}",
                file.display()
            );
        }
    }
}
