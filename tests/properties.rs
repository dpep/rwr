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
        assert!(
            out.status.code() != Some(2),
            "{pattern} errored: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
    assert_ne!(there.status.code(), Some(2), "forward rename errored");
    let back = rwr(&["rewrite", "$R.rwr_tmp_size", "-r", "$R.size", path]);
    assert_ne!(back.status.code(), Some(2), "reverse rename errored");

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
    assert_ne!(out.status.code(), Some(2), "rename errored");

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
