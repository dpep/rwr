# Changelog

## Unreleased

## 0.1.0 - 2026-08-21

First working release. `find`, `check` and `rewrite` all do real work.

**Structural matching.** Patterns are Ruby source with `$METAVARS`; comments,
strings and heredoc bodies are not code. On rails, `rwr 'return nil'` finds 22
sites where ripgrep reports 40.

**Receiver narrowing.** `method: Account#display_name` renames the definition, a
subclass override, explicit-receiver calls and implicit-self calls -- and leaves
`Company#display_name` and `Account.display_name` alone, because those are
different methods.

**Residue reporting.** Every occurrence a rule could not account for is
reported and classified, so a rename that would silently break
`attr_accessor :display_name` says so.

**Minimal diffs.** Only what changed moves; layout, block spelling and heredocs
survive.

**Refusal.** Ambiguity produces a diagnostic and zero edits. Exit codes
distinguish retryable from terminal, and `check` inverts polarity so a clean
tree does not block a commit.

`--profile` reports where the time went. `rwr-phase0` emits shareable JSON
aggregates for codebases that cannot leave their machine.

## 0.0.1 - 2026-08-20

Namespace placeholder. No functionality.
