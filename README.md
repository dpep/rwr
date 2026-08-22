# rwr

Ruby structural search and rewrites — `rg`/`sed` for Ruby *programs* rather than
Ruby *text*.

Find code by structure, rewrite only what matches, preserve everything else, and
refuse when it can't be sure.

```bash
rwr 'return nil'                       # find, whole repo
rwr 'return nil' app/models            # find, scoped
rwr '$R.select { |$P| $B }.first'      # metavariables
rwr check all app/                     # every built-in rule, read-only
rwr check all --diff main              # only the lines this branch touched
rwr rewrite rule.yml app/              # apply
rwr rewrite 'def legacy($A); $B; end' -d   # delete, doc comment and all
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
and implicit-self calls — and nothing else. Where a repository has Sorbet
signatures, `sig { returns(X) }` is read as a return type, so a chain like
`parser.document.name` resolves too — no RBI parser, no new file format, just
Ruby already in the tree. On one real monolith 76% of methods carry a signature.

**It tells you what it missed.** Ruby dispatches through symbols, and a rename
that only rewrites call sites silently breaks `attr_accessor :display_name`. rwr
reports every occurrence it could not account for, classified:

```
rewrote 3 site(s)

3 occurrence(s) this rule could not account for (1 symbol, 1 call, 1 comment):
  app/models/account.rb:4:17: Symbol:   attr_accessor :display_name
  app/models/account.rb:9:3:  Comment:  # Returns the display_name
  app/jobs/sync.rb:22:5:      Call:     thing.display_name
```

Comments are reported and never rewritten: `# See also #display_name on Company`
is about a different class, and nothing in the prose says so.

**Its diffs are minimal.** Only what changed moves. Layout, block spelling and
heredocs survive, because an unchanged subtree is never spliced.

**It refuses rather than guesses.** Ambiguity produces a diagnostic and zero
edits. A comment that cannot be unambiguously attached to a reordered element
declines with the source untouched; a deletion whose match does not occupy whole
lines is refused, since removing `a.name` from `x = a.name` leaves `x = `, which
swallows the line below and still parses.

## Install

```bash
cargo install rwr
```

## The built-in pack

A set of rules ships compiled into the binary, so it runs from any directory:

```bash
rwr check all app/                 # every safe rule
rwr check performance app/         # one family
rwr check style/return-nil app/    # one rule
```

`style/` covers things like `return nil` and hash shorthand; `performance/`
covers `detect`, `filter_map`, `sum`, `gsub` → `tr`, and the ActiveRecord set —
`where(...).count > 0` → `exists?`, `find_by`, `pluck`.

Rules that can change behaviour are **held back**, and the run says which and
why — `inject(:+)` returns nil for an empty collection where `sum` returns 0;
`select` on an ActiveRecord relation names columns rather than filtering rows.
`--unsafe` includes them and prints each caveat next to the diff.

Rules also declare the Ruby version their output needs, and are held back on an
older codebase — `{foo:}` is a syntax error before 3.1, and no amount of
verification catches that, because Prism parses the output happily. The version
comes from `.ruby-version`, a Gemfile `ruby` line or a gemspec; `--ruby X.Y`
overrides. An undetected version holds the rules back rather than assuming the
newest.

There are no per-rule options. A cop needs configuring because it is opaque
code; an rwr rule is four lines of YAML, so the rule *is* the option — to get
the other direction, copy the file and swap `match` with `rewrite`. See
[rules/README.md](rules/README.md).

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

`where:` also carries `type:`/`kind:`/`subclasses:` (receiver narrowing),
`same_name_as:` (two captures naming one identifier across node kinds), and
`is:`/`length:` (a capture's node kind and a literal's content) — the pair that
makes `gsub` → `tr` safe rather than plausible, since `tr` maps character by
character.

`contains:` holds a whole sub-pattern, and shared metavariables have to refer to
the same thing:

```yaml
match: $R.each { |$X| $B }
where:
  $B: { contains: $X.$ASSOC.$FIELD }
```

That is `performance/possible-n-plus-one`, which narrows discourse's 637
`each`/`map` blocks to 51 candidates.

**A rule with no `rewrite:` is a finding.** It reports its matches with its
`description` and proposes nothing, for shapes where the right answer depends on
something rwr cannot see — `.size` on a relation is `count` unloaded and `length`
loaded, and only the caller knows which was meant. Findings make `check` exit 1
like edits do; a lint that exits 0 gates nothing.

**Deletion** is `-d`, or an empty `rewrite:`; `-r ''` means the same. Removing a
definition takes the doc comment above it and one of the blank lines that
separated it, so the survivors keep their spacing.

A broken rule is refused before a file is read, naming the rule and the reason —
an unknown field, a constraint on a capture the pattern never binds, a template
metavariable that was never captured, a version string that isn't one.

## Templates

ERB is parsed, matched and rewritten. Tag bodies are stitched into a single Ruby
program — 95% of real templates parse that way — so a rename reaches inside a
view and leaves every byte of HTML where it was. An edit spanning two tags is
refused, since the bytes between them are not Ruby.

Haml is not parsed. Templates rwr cannot parse are text-searched at
whole-identifier boundaries and reported as their own class: grep-grade
evidence, labelled as weaker than anything parsed, because a call site missing
from the account is the dangerous direction.

## For agents, hooks and CI

Everything that prints honors `-j`/`--json`, which emits one document —
`{schema, rwr_version, changed, residue, findings, template_residue,
templates_skipped}`. `-J`/`--ndjson` streams instead: `find` writes a row per
match, and `check`/`rewrite` write the report on a single line.

`--diff` scopes a run to the lines a change touched, which is what makes `check`
adoptable on a codebase that has never run it: three new sites fail, two
thousand pre-existing ones do not. Bare `--diff` is the uncommitted work;
`--diff main` is what this branch introduces.

Exit codes, which a caller can branch on before parsing any output:

| | `find` / `rewrite` | `check` |
|---|---|---|
| 0 | matched | clean |
| 1 | no match | work to do |
| 2 | error | error |
| 3 | the rule is wrong | the rule is wrong |
| 4 | retryable — rerun makes progress | — |
| 5 | refused — needs judgement | refused |

`check` inverts polarity deliberately: a clean tree is success, so a pre-commit
hook does not block a commit where a rule correctly matches nothing.

## Scope

rwr does not own style. Indentation and trailing commas are presentation —
a trailing comma is invisible to rwr's own equality — so they belong to a
formatter. rwr repairs what it disturbs and shells out for the rest.

Also out of reach: non-anchored insertions, coordinated multi-site edits, and
sub-identifier name transforms (`find_by_*`). See [DESIGN.md](DESIGN.md).

rwr is not a RuboCop replacement. RuboCop owns the standing community rule
corpus; rwr is for one-off migrations that do not deserve a cop class, and for
rules RuboCop cannot express because its patterns are purely syntactic.

## Performance

Cost tracks how many files mention an identifier, not repository size: a literal
prefilter skips any file that cannot contribute. Over discourse's 11,006 files,
five warm runs — `find` on a single pattern **175 ms**, a rename **292 ms**, the
pack's safe rules **478 ms**, each discovering, reading, searching, parsing the
survivors and matching structurally. `--unsafe`, which runs every rule in the
pack rather than only those that need no caveat, takes about **1.4 s**.

`--profile` reports where the time went. See [docs/scaling.md](docs/scaling.md).

## Shell completions

```bash
rwr --completions            # the shell you are in
rwr --completions zsh        # or name one: bash, zsh, fish, elvish, powershell
```

Bare `--completions` reads `$SHELL`, since naming your own shell to a tool
already running inside it is friction with no purpose.

## Documentation

- [claude/INSTALL.md](claude/INSTALL.md) — installing the Claude skill, which
  teaches an agent to drive rwr
- [rules/README.md](rules/README.md) — the shipped pack, safety, writing rules
- [DESIGN.md](DESIGN.md) — what it is and how it works
- [docs/decisions.md](docs/decisions.md) — every decision, and what would reverse it
- [docs/cli-conventions.md](docs/cli-conventions.md) — the output and exit-code contract
- [docs/phase0-conclusion.md](docs/phase0-conclusion.md) — whether this should exist, and the evidence
- [docs/scaling.md](docs/scaling.md) — the cost model, measured
- [docs/prior-art.md](docs/prior-art.md) — ast-grep, Comby, Semgrep, RuboCop, Ruby LSP

## Gathering data from another machine

`rwr-phase0` emits JSON aggregates — counts, timings, receiver distributions —
with no source text or paths, so a codebase that cannot be shared can still be
measured. See [docs/data-collection.md](docs/data-collection.md).

## License

MIT
