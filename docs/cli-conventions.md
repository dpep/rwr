# CLI conventions

Inherited deliberately from `rq` (and `gqls`), so an agent that has learned one of these
tools has learned the others. Where rwr must differ, the difference is noted and justified
— rq is read-only navigation; rwr mutates source, which widens the exit-code space and
makes dry-run first-class.

This doc partially discharges two gaps the staff-engineer review raised: the unspecified
JSON/exit-code contract, and the total absence of scoping in `DESIGN.md` §8.

---

## Structured output

- `-j` / `--json` — a single object with named arrays, not a bare array.
- `-J` / `--ndjson` — a **tagged event stream**, one compact object per line.
- Mutually exclusive.
- **Every command that prints anything honors both**, not just `find`.

A homogeneous array cannot carry what rwr emits: matches, skipped files, residue
occurrences, conflicts, and a summary are heterogeneous records. So `--json` follows
semgrep's multi-array object shape, and `--ndjson` follows cargo's `--message-format json`:
every line carries a discriminator — `match`, `edit`, `skip`, `residue`, `conflict`,
`error`, `finished`.

**The `finished` terminator is load-bearing.** An agent reading a truncated NDJSON stream —
killed process, full disk — cannot otherwise distinguish "done, nothing more" from "died
halfway." ripgrep ships the same thing as its `summary` message.

Also adopt cargo's defensive-parsing guidance for consumers: only interpret a line as JSON
if it starts with `{`, since a subprocess may write to the same stdout.

- **Field names stay stable across commands.** A location is always
  `{file, line, col, byte_start, byte_end}`; a rule is always `rule`; captures are always
  `captures`.
- When you add a command, add its structured output **and** an e2e assertion in the same
  change.

Error messages name the fix, per rq:

```
rwr: --json can't frame a stream of rules — use --ndjson (-J)
```

## Exit codes

`2` is reserved for **error**, because grep, ripgrep, ruff, rubocop, biome, jq and semgrep
all agree it means "something went wrong." Handing it any other meaning misleads every agent
that has learned one of those tools — which D11 explicitly wants to be possible.

**Polarity is per verb, deliberately.** In search mode "no match" is a negative result; in
enforcement mode "no match" *is* success. pre-commit's contract is literally "the hook must
exit nonzero on failure," so a `check` that exited 1 on a clean tree would block every
commit where a rule correctly matches nothing. ast-grep splits `run` (grep polarity) and
`scan` (lint polarity) inside one binary for the same reason.

| Verb | 0 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|---|
| `find` | matched | no match | error | pattern/rule parse error | — | — |
| `rewrite` | matched / rewrote | no match | error | pattern/rule parse error | retryable | refused |
| `check` | clean | violations found | error | pattern/rule parse error | — | refused |

`3` separates a **pattern** parse failure from a **source file** parse failure — jq splits
compile-time from runtime errors the same way, and the two need different responses: fix the
rule vs skip the file.

`4` (retryable — matches skipped inside a rewritten range, rerunning makes progress) stays
distinct from `5` (refused — ambiguity needs judgement, rerunning changes nothing).
Collapsing them makes an agent either abandon recoverable work or spin on unrecoverable work.

**Error does not win over match.** ripgrep returns 2 if any error occurred even when it
matched; rwr does not. DESIGN.md §4 says unparseable files are reported and skipped, so 80
matches plus one unreadable vendored file is **exit 0 with the skip in the JSON**. This
differs from rg and must be documented, because callers will assume otherwise.

`--exit-zero-on-no-match` (plus `RWR_EXIT_ZERO_ON_NO_MATCH`) is available and baked into the
shipped hook definitions. Not the default.

Document every code in `rwr help exit-codes`, not only here — jq's manual documents four of
its five codes, and the undocumented one is a trap.

## Scoping

`DESIGN.md` §8 currently has none; rq already solved this and rwr should take it wholesale:

- Trailing positional `[PATH...]` on every verb — files or directories, rg-style (D31).
- `-p` / `--path DIR` — the explicit repeatable form, matching rq.
- The replacement template is `-r/--replace`, never a positional: with both, a trailing
  argument would be ambiguous between path and replacement, resolvable only by probing the
  filesystem — a guess, which principle 2 forbids.
- Respect `.gitignore` by default.
- Exclude generated and vendored code by default (`db/schema.rb`, `vendor/`, `node_modules/`),
  with a flag to include it. A rewrite that "succeeds" by editing `db/schema.rb` is a bug.

## Agent-friendliness

- **Positional shorthand** (D30): `rwr <pattern> [replacement]` needs no subcommand, matching
  rq's `rq <query>`. It desugars to `find` (one argument) or `check` (two) and **can never
  reach a writing verb** - mutation always requires typing `rewrite`.
- **The verb carries the mode.** `rwr check` shows what would happen; `rwr rewrite` does it.
  No `--write`, no `--dry-run` (D29). dprint's `fmt`/`check` split, which the UX research
  ranked above biome's `--write`.
- **Never block indefinitely.** rq's `--wait DUR` / `--no-wait` pattern: answer from what's
  committed, report a retryable status rather than hanging. Adopt this if rwr ever grows
  persistence — and note it's a reason to reconsider D5's "no persistence at all," since the
  pattern makes a warming index safe for agents rather than merely tolerable.
- **Suppress progress UI when not a TTY** or when `--json`/`--ndjson` is set. Piped callers
  get clean parseable output, never a spinner.
- Env-var overrides for anything an agent or CI would want to pin (`RWR_*`), following
  `RQ_DB` / `RQ_WAIT_BUDGET_MS` / `RQ_OPEN`.
- `value_hint` on every path-valued arg so shell completion is scoped correctly — and
  `ValueHint::Other` on pattern args, so shells don't offer filenames where a pattern goes.

## `--explain`

rq has `-e` / `--explain` to show the additive score breakdown behind a ranking, on the
principle that **ranking is explainable**.

rwr has no ranking, but the same principle transfers to the thing rwr must justify:
**why a match was skipped, refused, or flagged.** `--explain` should print, per match, which
constraint in the `where:` block rejected it, or which conflict suppressed it, or why a
residue occurrence was classified as it was. This is the debugging surface for the refusal
contract, and without it "rwr refused" is unactionable.

## Declarative flag conflicts

rq uses clap's `conflicts_with_all` to make illegal combinations unrepresentable at parse
time rather than checked in the body. Do the same — the conflict list also documents intent.

## Testing

Straight from rq's CLAUDE.md, and it matters more here because rwr writes files:

- **Verify through `cargo test`, not by hand-running the binary.** Drive the built binary
  (`CARGO_BIN_EXE_rwr`) from `tests/cli_e2e.rs` against a temp repo. Reproducible,
  CI-checked, and no permission prompts.
- Logic that would otherwise need a manual run gets factored into a pure function with its
  own unit test.
- **Generic, non-identifying fixtures** — `Widget`, `Foo`, `Account`, never real class names
  from a private monolith. rwr ships public; the Phase 0 corpus especially must be scrubbed,
  since it is drawn from proprietary code.
- Assert on behavior that survives refactoring ("refuses and exits 3"), not brittle strings.

## Project conventions

- Single crate until there is a concrete reason to split. Simpler wins.
- **Design docs are the contract** — when the design changes, the doc changes in the same
  commit. `DESIGN.md`, `decisions.md`, and `open-questions.md` are load-bearing, not notes.
- `CHANGELOG.md` entry under `## Unreleased` in the change that earns it.
