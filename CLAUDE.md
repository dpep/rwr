# rwr development conventions

`rwr` is a **Ruby structural search-and-rewrite engine** — `rg`/`sed` for Ruby
*programs* rather than Ruby *text*. Read [DESIGN.md](DESIGN.md) for the design,
[docs/decisions.md](docs/decisions.md) for why each piece is shaped as it is
(and what would reverse it), and [docs/open-questions.md](docs/open-questions.md)
for what is still unresolved.

> **Pre-Phase-0.** The design docs are ahead of the code. They are the contract —
> keep them in sync, changing them in the same commit as the code that changes
> the design.

## First principles (do not drift from these)

- **Refuse rather than guess.** Ambiguity produces a diagnostic and zero edits.
  A wrong rewrite invalidates the entire product thesis; a refusal merely costs
  the caller a round trip.
- **Never silently drop an edit.** ast-grep and Synvert both do; it is the
  failure that makes a rewriting tool untrustworthy. If an edit doesn't apply,
  say so and set the exit code.
- **Report what you couldn't see — unconditionally.** Residue and skip reporting
  never hide behind `--verbose` (Semgrep's mistake). The account of blind spots
  is the product, not a diagnostic.
- **Make unsafe operations unrepresentable.** The capture API does not expose
  raw node locations, because splicing from one silently detaches heredoc bodies
  and the result still parses. `effective_range()` is the only splice-able
  range. RuboCop has had the right data for a decade and still ships heredoc
  bugs — documenting the hazard is not enough.
- **Minimal diffs.** Never rewrite code that doesn't need rewriting. Formatting
  is a separate concern and rwr does not own style.
- **Syntax works without semantics.** Structural matching must not require a
  type checker; the semantic layer enhances matching, never gates it.
- **Every command is agent/script-friendly.** Anything that prints honors
  `-j/--json` and `-J/--ndjson`. Field names stay stable across commands. Exit
  codes stay meaningful, and `Retryable` (2) stays distinct from `Refused` (3).
  When you add a command, add its structured output and an e2e assertion in the
  same change. See [docs/cli-conventions.md](docs/cli-conventions.md).

## Language and toolchain

Rust, single static binary. Prism (`ruby-prism`) for parsing — decision D1; the
argument is Ruby fidelity for *valid* source, not error recovery.

This machine's Rust came via Homebrew's keg-only `rustup`, so `cargo` may not be
on `PATH`. Either add it once —

```sh
echo 'export PATH="/opt/homebrew/opt/rustup/bin:$PATH"' >> ~/.bash_profile
```

— or invoke directly, or pass `CARGO=` to any `make` target.

## The Claude skill

`claude/rwr-skill.md` is the source; `claude/INSTALL.md` explains both halves of
an install. The skill teaches an agent to drive rwr, so it must describe the
binary that actually shipped — a skill a release forgets misinforms every agent
that reads it, silently, for a whole cycle.

**It ships through the private marketplace, which is not where `release` looks.**
`rq` and `gqls` live in `code@dpep`; rwr sits in `rwr@myclaude` until it has real
mileage. The release script defaults `PLUGINS_REPO` to `~/code/lib/claude` and
derives the destination as `plugins/code/skills/<name>/SKILL.md`, so a release
needs:

```sh
PLUGINS_REPO=~/code/lib/myclaude \
SKILL_DST=~/code/lib/myclaude/plugins/rwr/skills/rwr/SKILL.md \
release <version>
```

`PLUGIN_MANIFEST` is *not* overridable — it is derived as
`$PLUGINS_REPO/plugins/code/...`, which does not exist in myclaude — so the
plugin's own version bump is manual until the skill moves. Bump it: `claude
plugin update` compares versions rather than content, so a skill change that
does not move the version reaches nobody.

## Repo layout

Single crate; modules mirror the design. Keep it a single crate until there is a
concrete reason to split. Simpler wins.

```text
rwr/
  src/
    main.rs      ← CLI entry
    cli/         ← arg parsing, structured output, exit codes (public contract)
    source/      ← walking, scoping, Prism parsing
    pattern/     ← pattern parsing + structural matching
    rewrite/     ← action tree, effective_range, splicing
    residue/     ← name-scoped residue reporting
  rules/         ← the shipped rule pack, loaded as a directory (D54)
  claude/        ← the Claude skill and its install doc
  testbed/       ← a Ruby app with marked ground truth, for Q1's recall
  docs/          ← design, decisions, open questions, research
  tests/         ← e2e over the built binary
```

## Building, testing, linting

```sh
make build      # dev build → target/debug/rwr
make test       # cargo test
make check      # pre-push gate: fmt + clippy (-D warnings) + tests
make lint       # fmt --check + clippy
```

Before committing: `make check` — or better, `script/commit.sh -m "..."`, which
runs the gate and commits only if it passes.

**Never chain the gate to an action in one shell command.** `check.sh; git commit`
ignores the gate, and `check.sh | grep … && git commit` masks it behind grep's
exit status. Both look correct and neither is; two commits went out red before
`script/commit.sh` existed.

**Edit, then format — not the reverse.** `cargo fmt` reflows long lines, so a
text replacement written against pre-format source stops matching after it runs.
Batch related edits, then format once, then build.

## Testing conventions

- Write tests for new code, focused on quality not quantity — edge cases and
  error handling over restating the happy path.
- **Verify through `cargo test`, not by hand-running the binary.** CLI behavior
  belongs in `tests/cli_e2e.rs`, driving the built binary (`CARGO_BIN_EXE_rwr`)
  against a temp repo — reproducible, CI-checked, no permission prompts.
- **Pin design premises as executable facts.** When a decision rests on a claim
  about Prism or Ruby, write the test that would fail if the claim were wrong
  (see `source::tests::heredoc_location_excludes_its_body`, which pins D14).
- **Identity-rewrite property test**: a match with no template change must
  produce byte-identical output. Cheap, and it catches the range-arithmetic bug
  class at dev time rather than in a user's repo.
- **Use generic, non-identifying test data** — `Widget`, `Foo`, `Account` over
  real class names. This is a public repo, and the Phase 0 corpus is drawn from
  a private monolith: scrub fixtures and assertions as they are written, never
  at release time.
- Spec descriptions stay simple and resilient ("refuses and exits 3", not a
  brittle exact-string assertion).
