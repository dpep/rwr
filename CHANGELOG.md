# Changelog

## Unreleased

**The rule pack is compiled into the binary.** `rwr check all`, `rwr check performance`,
`rwr check style/return-nil` work from any directory — `cargo install` copies the binary and
nothing else, so a pack that lived only in the repo was not shipped. A real path still wins
over a built-in name.

**Four new rules**, plus `sorted-constant-array` and `string-replacement` (`gsub` → `tr`).
Two new `where:` predicates make them safe rather than plausible: `is:` constrains a
capture's node kind and `length:` its literal content in characters. `is: constant` also
picks the placeholder casing, which is what lets a pattern reach `FOO = [...]` at all —
before, `$C = [...]` silently meant a *local* assignment, since both casings parse.

**Rules that can change behaviour say so, and are held back by default.** A rule may carry
`unsafe: <reason>`; those need `--unsafe`, the run reports how many it skipped and why, and
a reason prints next to the diff when its rule fires. There are no per-rule options — the
rule is four lines of YAML, so the rule *is* the option (D57).

**A shipped rule pack.** `rwr check rules` runs every rule under a directory,
`rwr check rules/performance` runs one family. A run reports which rule
accounted for what, since a total across five rules is not reviewable. Rules
take their id from their path within the pack. See `rules/README.md` for what
was deliberately left out.

`--help` had promised "a rule file or directory of them" since 0.1.0; only a
file worked.

**Minimal diffs now survive sequence placeholders.** Any rule spelled with `*$REST` or
`**$REST` fell through to whole-node replacement, so hash shorthand returned multiline
hashes on a single line with their trailing commas removed. It now edits the one pair that
changed and leaves the layout alone (D56).

**Residue is reported for name-anchored rules only, as D7 always said.** A rule about a
*shape* — `select { }.first` -> `detect { }` — anchored on the chain's method names and
reported every `.first` in the repo. On Discourse that was 3,752 occurrences, which buried
the output. A rename still reports; the account of blind spots was never meant to be a
concordance of common method names.

**The `edits` field in JSON output is now `sites`, and counts differently.** It
had reported edits, and a rewrite that changes shape emits several edits for one
place a reader sees in the diff — `select { }.first` → `detect { }` counted as
two. It now counts matched sites.

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
