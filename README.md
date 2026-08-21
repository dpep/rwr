# rwr

Ruby structural search and rewrites — `rg`/`sed` for Ruby *programs* rather than
Ruby *text*.

Find code by structure, rewrite only what matches, preserve everything else, and
refuse when it can't be sure.

```bash
rwr 'return nil'                       # find, whole repo
rwr 'return nil' app/models            # find, scoped
rwr '$R.select { |$P| $B }.first'      # metavariables
rwr check rule.yml app/                # CI and git hooks
rwr rewrite rule.yml app/              # apply
```

## What makes it different

**It knows what a receiver is.** Renaming `Account#display_name` leaves
`Company#display_name` and `Account.display_name` alone — they are different
methods. No other Ruby structural tool does this: `node_pattern` has no notion
of a receiver, ast-grep's FAQ disclaims type analysis, and Ruby LSP matches
methods by bare name.

```yaml
method: Account#display_name
rename: full_name
```

That reaches the definition, an override in a subclass, explicit-receiver calls,
and implicit-self calls — and nothing else.

**It tells you what it missed.** Ruby dispatches through symbols, and a rename
that only rewrites call sites silently breaks `attr_accessor :display_name`. rwr
reports every occurrence it could not account for, classified:

```
rewrote 3 site(s)

2 occurrence(s) this rule could not account for (1 symbol, 0 string, 1 call, 0 definition):
  app/models/account.rb:4:17: Symbol:   attr_accessor :display_name
  app/jobs/sync.rb:22:5:     Call:      thing.display_name
```

**Its diffs are minimal.** Only what changed moves. Layout, block spelling and
heredocs survive, because an unchanged subtree is never spliced.

**It refuses rather than guesses.** Ambiguity produces a diagnostic and zero
edits — a comment that cannot be unambiguously attached to a reordered element
declines with the source untouched.

## Install

```bash
cargo install rwr
```

## Rules

A rule is Ruby source with `$METAVARS`, plus constraints source syntax cannot
express:

```yaml
match: $R.$SEL { |$P| $B }.first
where:
  $SEL: { name: [select, find_all] }     # one rule, both synonyms
rewrite: $R.detect { |$P| $B }
```

|            | one node | zero or more |
|------------|----------|--------------|
| anonymous  | `_`      | `*_`         |
| captured   | `$NAME`  | `*$NAME`     |

All four are valid Ruby, so the language's own grammar validates where a
sequence may appear.

## Exit codes

Agents and hooks branch on these before parsing any output.

| | `find` / `rewrite` | `check` |
|---|---|---|
| 0 | matched | clean |
| 1 | no match | work to do |
| 2 | error | error |
| 3 | rule did not parse | rule did not parse |
| 4 | retryable — rerun makes progress | — |
| 5 | refused — needs judgement | refused |

`check` inverts polarity deliberately: a clean tree is success, so a pre-commit
hook does not block a commit where a rule correctly matches nothing.

## Scope

rwr does not own style. Indentation and trailing commas are presentation —
a trailing comma is invisible to rwr's own equality — so they belong to a
formatter. rwr repairs what it disturbs and shells out for the rest.

Also out of reach: non-anchored insertions, coordinated multi-site edits,
sub-identifier name transforms (`find_by_*`), and non-Ruby templates (ERB,
Haml). See [DESIGN.md](DESIGN.md).

rwr is not a RuboCop replacement. RuboCop owns the standing community rule
corpus; rwr is for one-off migrations that do not deserve a cop class, and for
rules RuboCop cannot express because its patterns are purely syntactic.

## Performance

Cost tracks how many files mention an identifier, not repository size: a literal
prefilter skips any file that cannot contribute. Across 18,535 files rwr
discovers, reads, searches, parses the survivors and matches structurally in
~270 ms — faster than `rg -l` doing only the search.

`--profile` reports where the time went. See [docs/scaling.md](docs/scaling.md).

## Documentation

- [DESIGN.md](DESIGN.md) — what it is and how it works
- [docs/decisions.md](docs/decisions.md) — every decision, and what would reverse it
- [docs/phase0-conclusion.md](docs/phase0-conclusion.md) — whether this should exist, and the evidence
- [docs/scaling.md](docs/scaling.md) — the cost model, measured
- [docs/prior-art.md](docs/prior-art.md) — ast-grep, Comby, Semgrep, RuboCop, Ruby LSP

## Gathering data from another machine

`rwr-phase0` emits JSON aggregates — counts, timings, receiver distributions —
with no source text or paths, so a codebase that cannot be shared can still be
measured. See [docs/data-collection.md](docs/data-collection.md).

## License

MIT
