//! End-to-end tests driving the built binary.
//!
//! Per CLAUDE.md, CLI behavior is verified here rather than by hand-running
//! `rwr` — reproducible, CI-checked, and immune to a stale `target/debug/rwr`.

use std::process::{Command, Output};

fn rwr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(args)
        .output()
        .expect("binary runs")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A temp directory holding one Ruby file.
fn fixture(source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("fixture.rb"), source).expect("write fixture");
    dir
}

/// The shorthand really searches: one argument routes to `find` and reports
/// structural matches, not a stub.
#[test]
fn bare_pattern_is_shorthand_for_find() {
    let dir = fixture("def a\n  return nil if x\n  # return nil\n  s = \"return nil\"\nend\n");
    let out = rwr(&["return nil", dir.path().to_str().expect("utf8")]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        text.lines().count(),
        1,
        "comment and string literal are not code: {text}"
    );
}

/// Exit 1 means "no match" and is a clean result, not an error.
#[test]
fn no_match_exits_one() {
    let dir = fixture("def a\n  1\nend\n");
    let out = rwr(&["return nil", dir.path().to_str().expect("utf8")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

/// A pattern that is not valid Ruby gets its own code, distinct from an I/O or
/// internal failure -- the caller must fix the rule, not the invocation.
#[test]
fn unparseable_pattern_exits_three() {
    let out = rwr(&["def foo("]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
}

/// Trailing positionals are paths, rg-style, and must not be mistaken for a
/// replacement -- which is why the replacement is a flag (D31). Scoping to a
/// directory without matches must find nothing.
#[test]
fn trailing_positionals_scope_the_search() {
    let has = fixture("def a\n  return nil\nend\n");
    let hasnt = fixture("def a\n  1\nend\n");

    let hit = rwr(&["return nil", has.path().to_str().expect("utf8")]);
    assert_eq!(hit.status.code(), Some(0), "{}", stderr(&hit));

    let miss = rwr(&["return nil", hasnt.path().to_str().expect("utf8")]);
    assert_eq!(miss.status.code(), Some(1), "{}", stderr(&miss));
}

/// The safety property behind D30, asserted on the filesystem rather than on a
/// message: the shorthand is read-only *by construction*. A pattern plus a
/// replacement previews and must never write, because a terse two-argument
/// command that silently mutated a repo is exactly the foot-gun D29 removed the
/// mode flags to avoid.
#[test]
fn shorthand_with_replacement_cannot_reach_rewrite() {
    let dir = fixture("def a\n  return nil\nend\n");
    let file = dir.path().join("fixture.rb");
    let before = std::fs::read(&file).expect("read");

    let out = rwr(&[
        "return nil",
        "-r",
        "return",
        dir.path().to_str().expect("utf8"),
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected preview: {}",
        stderr(&out)
    );

    let after = std::fs::read(&file).expect("read");
    assert_eq!(before, after, "the shorthand wrote to disk");
}

/// And the verb does write, so the invariant above is about the shorthand
/// rather than about rwr being unable to rewrite at all.
#[test]
fn the_rewrite_verb_writes() {
    let dir = fixture("def a\n  return nil\nend\n");
    let file = dir.path().join("fixture.rb");

    let out = rwr(&[
        "rewrite",
        "return nil",
        "-r",
        "return",
        dir.path().to_str().expect("utf8"),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let after = String::from_utf8(std::fs::read(&file).expect("read")).expect("utf8");
    assert_eq!(after, "def a\n  return\nend\n");
}

#[test]
fn writing_requires_the_rewrite_verb() {
    assert!(stderr(&rwr(&["rewrite", "rule.yml"])).contains("rewrite"));
}

/// No arguments is a usage error, not a silent no-op.
#[test]
fn bare_invocation_explains_itself() {
    let out = rwr(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--help"), "{}", stderr(&out));
}
