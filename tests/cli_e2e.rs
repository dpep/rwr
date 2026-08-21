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

#[test]
fn bare_pattern_is_shorthand_for_find() {
    let out = rwr(&["foo($A, $B)"]);
    assert!(stderr(&out).contains("find"), "{}", stderr(&out));
}

/// Trailing positionals are paths, rg-style, and must not be mistaken for a
/// replacement — which is why the replacement is a flag (D31). If this ever
/// routes to `check`, the argument grammar has become ambiguous.
#[test]
fn trailing_positionals_scope_the_search() {
    let err = stderr(&rwr(&["foo($A)", "app/models", "lib/tasks/thing.rb"]));
    assert!(err.contains("find"), "{err}");
    assert!(
        !err.contains("check"),
        "a path was read as a replacement: {err}"
    );
}

/// The safety property behind D30: the shorthand is read-only *by
/// construction*. A pattern plus a replacement previews — it must never reach
/// `rewrite`, because a terse two-argument command that silently mutated a repo
/// is exactly the foot-gun D29 removed the mode flags to avoid.
#[test]
fn shorthand_with_replacement_cannot_reach_rewrite() {
    let err = stderr(&rwr(&["foo($A)", "-r", "bar($A)"]));
    assert!(err.contains("check"), "{err}");
    assert!(
        !err.contains("rewrite"),
        "shorthand routed to a writing verb: {err}"
    );
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
