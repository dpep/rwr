# rwr

Ruby structural search and rewrites — `rg`/`sed` for Ruby *programs* rather than
Ruby *text*.

Find code by structure, rewrite only what matches, preserve everything else, and
refuse when it can't be sure.

```bash
rwr 'foo($A, $B)'                      # find, whole repo
rwr 'foo($A, $B)' app/models           # find, scoped
rwr 'foo($A)' -r 'bar($A)'             # preview the diff
rwr rewrite add-context.yml app/       # apply it
```

The shorthand never writes: mutation always requires typing `rewrite`.

Renames use Ruby's own method notation, which carries the distinction that
matters:

```yaml
method: Account#display_name    # the instance method
rename: full_name
```

`Account#display_name` and `Account.display_name` are different methods, and rwr
treats them that way — renaming one never touches the other. One line expands to
the rule set a complete rename needs: the definition, the explicit-receiver
calls, and the implicit-self calls.

## Status

**Pre-implementation.** The design is written and the scaffold builds; no
functionality yet. Phase 0 — which decides whether this should exist at all —
has not run.

- [DESIGN.md](DESIGN.md) — what it is, how it works, the phases
- [docs/decisions.md](docs/decisions.md) — what was decided, and what reverses it
- [docs/open-questions.md](docs/open-questions.md) — unresolved risk
- [docs/cli-conventions.md](docs/cli-conventions.md) — CLI, JSON, and exit-code contract
- [docs/prior-art.md](docs/prior-art.md) — survey of comparable tools
- [docs/review-staff-eng.md](docs/review-staff-eng.md) — independent design review

## Why not an existing tool

[ast-grep](https://ast-grep.github.io/), RuboCop's `node_pattern`, Semgrep, and
[Comby](https://comby.dev/) all do structural search and rewrite, and rwr is not
novel at the syntax layer. The intended differences are Ruby fidelity via
[Prism](https://github.com/ruby/prism), an explicit account of what the tool
*could not* see, and semantic receiver-narrowing. Whether those are worth a new
tool is exactly what Phase 0 measures — and the honest outcome may be to
contribute upstream instead.

## Scope

Not supported: non-anchored insertions (adding an `include` or `require`), coordinated
multi-site edits, sub-identifier name transforms (`find_by_*`), semantic guards, and non-Ruby
templates (ERB, Haml). See [DESIGN.md](DESIGN.md) for the full boundary.

rwr is not a RuboCop replacement. RuboCop owns the standing community rule corpus; rwr is for
one-off migrations that do not deserve a cop class, and for rules RuboCop cannot express
because its patterns are purely syntactic.

## Gathering data from another machine

`rwr-phase0` emits a JSON report of aggregates -- counts, timings, receiver distributions --
with no source text or paths, so Phase 0 can be measured on codebases that cannot be shared.
See [docs/data-collection.md](docs/data-collection.md).

```sh
cargo install --git https://github.com/dpep/rwr
rwr-phase0 --label monolith ~/path/to/monolith > phase0-monolith.json
```

## License

MIT
