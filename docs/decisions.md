# Decisions

ADR-lite. Each entry records what was decided, why, and **what would reverse it** — so we
argue with evidence rather than relitigating from memory.

---

## D1 — Parser: Prism, not tree-sitter
**Decided.** Official [`ruby-prism`](https://crates.io/crates/ruby-prism) Rust bindings.

Prism is the Ruby parser and tracks CRuby by construction; the tree-sitter Ruby grammar is
a community reimplementation that lags new syntax. Decisive factor: tree-sitter's error
recovery silently produces a plausible-but-wrong tree, which is incompatible with a
"never silently guess" product. tree-sitter's incremental parsing is an editor
keystroke-latency feature this architecture never consumes.

*Reverses if:* Prism's Rust bindings prove unworkable for repeated whole-repo parsing
(allocation churn, FFI overhead), or Phase 0 shows Ruby-fidelity differences don't
actually affect precision on the corpus.

## D2 — Pattern syntax: Ruby source + `$METAVARS`, constraints in `where:`
**Decided.** Shape is copy-pasteable Ruby; anything source syntax can't express goes in a
separate constraint block. Rejected the node-constructor form as more expressive and much
worse to write.

*Reverses if:* the corpus turns up common transformations whose *shape* is inexpressible
as Ruby source — likely candidates are anything about arity, ordering, or absence.

## D3 — Distribution: OSS via the tap
**Decided** (user, this session). Consequences: pattern syntax is a public contract from
v0.1; the semantic layer can't hardcode one repo's conventions; "why not ast-grep?" must
be answered in the README on day one.

## D4 — Two Phase-0 corpora
**Decided.** Private monolith (>1M LOC) for real fidelity and perf; a public repo
(Discourse / Mastodon / GitLab) for reproducible benchmarks. One corpus cannot do both
jobs, and an OSS tool challenging an incumbent needs numbers a skeptic can rerun.

## D5 — Phase 1 ships a cache, not an index
**Decided.** Memoize parses keyed by file content hash. Same benefit as an index for the
repeated-invocation case, a fraction of the work, no coherence design problem.

*Reverses if:* Phase 0 shows cold parallel parse over 1M+ LOC is too slow to hide behind a
cache for realistic agent workloads.

## D6 — Symbol index pulled forward to Phase 2
**Decided.** Was Phase 4 in the original plan. At >1M LOC, bare `foo(...)` matching has a
false-positive rate high enough that the refusal contract fires constantly and trains
users to ignore it. Receiver-narrowing makes basic matching usable; it is not a later
enhancement.

## D7 — Two-part safety contract, known-unknowns first-class
**Decided.** "83/83 changed" is unachievable in Ruby and claiming it is a lie. Every
`find`/`rewrite` reports both completeness over static syntax *and* the dynamic-dispatch
sites that could reach the same name. See Q1 — tractability is the open risk.

## D8 — Original Phases 5–7 demoted to vision
**Decided.** A seven-phase plan reads as a seven-phase commitment. Three committed phases;
agent protocol, verification pipeline, breadcrumbs, and Rails intelligence move to
DESIGN.md §9 explicitly labeled uncommitted.

## D9 — No `rwr format`
**Decided.** Contradicted the plan's own separation of rewriting from formatting. Shell
out to an existing formatter.

## D10 — Name `rwr`
Crate name confirmed unclaimed on crates.io (2026-08-20). Consistent with `rq`, `pe`, `gqls`.

## D11 — rwr and rq stay separate; integrate at the interface, not the code
**Decided.** rq (the author's tree-sitter symbol-navigation CLI) has real surface overlap
with rwr's Phase 2 symbol index, but `rq/src/core/symbol.rs` states the decisive fact:
its model covers *"definitions only: call graphs, references, and inheritance are explicit
non-goals."* Everything rwr Phase 2 needs — references, call relationships, inheritance,
receiver resolution — is on rq's non-goals list. Its `Kind` vocabulary is also deliberately
language-agnostic (no `Constant` variant), everything but `cli` is `pub(crate)`, and it's
tree-sitter where rwr is Prism.

rwr builds its own Ruby symbol layer from the Prism parse it is already performing. That
duplicates roughly 400 lines of rq's `lang/ruby` extraction, which is cheaper than a
cross-parser dependency and much cheaper than generalizing rq's model to carry Ruby
specifics it was designed not to carry.

**Integration happens at the interface:** rwr emits the same JSON code-location shape and
exit-code conventions as rq and gqls, so an agent can orient with rq and act with rwr.
Composable Unix primitives (principle 8), not a shared crate.

*Reverses if:* rq's roadmap adds references/call-graph as a first-class goal, at which point
the overlap becomes real rather than superficial.

## D12 — Reimplement the matcher; no ast-grep dependency
**Decided.** ast-grep is tree-sitter-based, so consuming it for matching reintroduces the
exact fidelity problem D1 was decided to avoid, and adds a permanent mapping layer between
two node vocabularies.

The matcher is small: parse the pattern with Prism, walk pattern and target in lockstep,
metavariable nodes act as wildcards, bind captures into an environment. A recursive tree
comparison. The hard parts are the *semantic* decisions (Q4: sequence metavariables,
repeated-metavariable equality, whether captures bind keyword args / blocks / splats),
which must be made either way — ast-grep would save the tree-walk and none of the decisions.

**Scope discipline: implement the matcher, not ast-grep's rule algebra.** Pattern plus a
flat `where:` block first. Relational rules (`inside`, `has`, `follows`, `all`/`any`/`not`)
only if the Phase 0 corpus demonstrates they are needed.

ast-grep remains a Phase 0 measured baseline (D4), not a dependency.

*Superseded:* an earlier suggestion to use ast-grep as an over-approximate candidate
generator with Prism verifying. Rejected on inspection — it requires parsing the repo twice
with two parsers plus a range-mapping layer, and the speed win is illusory because Prism
parse is already fast.

## D13 — Rewriter is a port of TreeRewriter's action tree
**Decided.** `Parser::Source::TreeRewriter` maintains a tree of pending actions with three
invariants: children strictly contained by parent, siblings disjoint and ordered, only
non-replacing actions may have children. Order-independence follows *by construction*, and
partial overlap produces a clean clobbering error rather than corrupt output.

GritQL converged on the same invariant independently — good evidence it is the right shape
rather than an accident of RuboCop's history.

Adopt its three named policies for ambiguous cases (`:accept | :warn | :raise`):
`crossing_deletions`, `different_replacements`, `swallowed_insertions`.

## D14 — `effective_range()` is the only splice-able range; raw `.location` is never exposed
**Decided.** Resolves Q5. A node's effective range is the transitive closure over its
descendants unioning each heredoc's `closing_loc`, computed at splice time; insertion points
are phrased relative to it too.

Prism's `StringNode#location` for a heredoc is only `opening_loc` — the trap is in the
default. `whitequark/parser` needed a hardcoded `Map::Heredoc` for the same reason.

**The enforcement is the decision, not the rule.** RuboCop has had this data for a decade
and still ships heredoc-corruption bugs (#10895, #10320, #6653, #11621). So the capture API
does not expose raw `.location` at all. Unsafe operation unrepresentable, not documented.

## D15 — Overlap: find is reentrant, rewrite is outermost-only, partial overlap aborts
**Decided.** Resolves Q3. Conflict unit is the **edit** range, not the match range — minimal
edits mean nested matches usually produce disjoint edits and both apply cleanly.

ast-grep's `Visitor { reentrant: bool }` × pre/post-order is the clean orthogonal model
(its `replace_all` hardcodes `.reentrant(false)` with a TODO conceding bounded reentrancy is
unsupported). rwr: `find` reentrant with nesting metadata, `rewrite` outermost-only, abort on
partial overlap, no auto-fixpoint inside one invocation.

Comby (`Rewrite.substitute_matches`, no guard) and Semgrep (#3577, #3388, #4428) both
silently emit **unparseable output** on overlapping edits at **exit 0**. Aborting instead is
rwr's cheapest credible correctness claim — build the demonstration in Phase 0.

## D16 — Metavariable semantics: occurrence counts + AST equality
**Decided.** Resolves Q4. Adopt IntelliJ SSR's **min/max occurrence counts**, which unify
single, optional, sequence, and must-not-appear under one mechanism instead of four sigils.

Repeated metavariables require **AST equality** (Semgrep's choice), not textual equality
(Comby's `==`, a known mistake rwr can simply avoid).

Deferred but noted: GritQL's `bubble` frames repeated-metavar equality and multiple-matches
as the same knob; Coccinelle's `position` and `fresh identifier` metavariable kinds are both
directly useful later.

## D17 — CLI, JSON, and exit-code contract inherited from rq
**Decided.** Specified in `docs/cli-conventions.md`. Public contract from v0.1, same status
as D2. Key points: `-j/--json` and `-J/--ndjson` on every command that prints; stable field
names; scoping via repeatable `-p/--path` plus rg-style positional dirs; gitignore and
vendored/generated exclusion by default; progress UI suppressed when not a TTY.

Exit codes distinguish **retryable** (2 — matches skipped inside a rewritten range, rerun
makes progress) from **terminal** (3 — ambiguity needs judgment). Collapsing them makes an
agent either abandon recoverable work or spin on unrecoverable work. Never Comby's
always-0.

---

# Amendments

## D1 amended (rationale corrected, conclusion unchanged)
The original rationale leaned on "tree-sitter's error recovery silently guesses." That is
**overweighted** — tree-sitter marks ERROR/MISSING nodes explicitly, so a refuse-on-error
policy neutralizes malformed input entirely.

The decisive argument is **silently different-shaped trees for valid Ruby**, which are
undetectable because nothing is marked. Confirmed concretely by the prior-art survey:
Semgrep's Ruby grammar excludes heredoc bodies from the CST entirely (#2258); Comby
truncates `def…end` when a heredoc body contains `end`.

Stronger still, and connecting D1 to the product thesis: **a tree-sitter-based tool
structurally cannot report its own parse blind spots, because its parser never admits to
having any.** D1 and the residue-reporting differentiator are the same argument at different
levels.

Reversal cost corrected: rewriting the matcher against a new node vocabulary — weeks, not
days. The earlier implication that parser-agnostic structure makes this cheap was wrong.

## D5 amended — cut the cache entirely, pending measurement
Original framing (cache vs index) was the wrong comparison; the real one is
**cache vs nothing**. Prism is a fast C parser and deserialization plus hash invalidation may
not beat reparsing. Phase 1 ships no persistence at all. Revisit only if Phase 0
measurement (d) shows cold parallel parse is too slow.

*Counter-argument on record:* rq's `--wait DUR` / `--no-wait` pattern makes a warming index
safe for agents rather than merely tolerable (answer from what is committed, report a
retryable status, never hang). If persistence returns, it returns with that pattern.

## D7 amended — known-unknowns narrowed to name-scoped residue reporting
The original formulation ("which dynamic-dispatch sites may reach this name") is a dataflow
problem and will not be low-noise at 1M LOC. The proposed fallback — a repo-wide aggregate
of fully-dynamic sites — is worse than noise: constant per repo, identical for every query,
zero information about the rewrite at hand.

Replaced by **name-scoped residue reporting** (DESIGN.md §4), which is lexical plus AST
context and needs no dataflow. Two consequences: it applies to **name-anchored rules only**
(a rename has a target identifier; `return nil → return` does not), and the truly-dynamic
inventory becomes a separate one-time `rwr audit` if ever wanted.

Novelty re-checked: the *shape* is standard (Semgrep's 17-variant `skip_reason`), the
*content* is not — every existing implementation reports mechanical failures ("could not
process these bytes"), never semantic ones ("this file scanned fine and there is still a
`send(name)` on line 47").

**"Name-anchored" needed a test, and for a while did not have one.** `anchors()` collected
every literal call name anywhere in a pattern, which makes a rule about a *shape* look
name-anchored: `$R.select { |$P| $B }.first` anchored on `select` and `first`, and reported
every `.first` in the repo — 3,752 occurrences on Discourse, burying the account the feature
exists to give. A rule is name-anchored when the pattern **is that name applied to
metavariables**: the root is a call with a literal message, and its receiver, arguments and
block are metavariables or absent. `$R.display_name` and `$R.set_size($A)` qualify;
`$R.select { }.first` does not, because a bare `.first` elsewhere is a different program,
not a site the rule failed to convert. Pinned by
`residue::tests::a_shape_rule_is_not_name_anchored`.

## D18 — Metavariables are substituted before parsing, not parsed as globals
**Decided.** Resolves the largest hole the case studies found (`docs/use-cases.md`
finding 1).

D2 says a pattern is Ruby source parsed by Prism. That makes `$M` a *global variable*
token, which lexes only where a gvar is legal: expression positions. `foo($A, $B)` works;
`$X.$M`, `def $M`, `:$M`, `$K: v`, and `Foo::$C` do not lex at all — killing method-name,
symbol, keyword-name, and constant matching, and with them the whole modernization family.
The product's core bet on Prism strictness caused this.

**Resolution — pre-lex placeholder substitution.** Rewrite each `$NAME` to a syntactically
valid identifier before parsing, recognize the placeholder in the resulting tree, and map
back to the metavariable. Constants require a capitalized placeholder and everything else a
lowercase one; since the required case is not knowable before parsing, parse with the
lowercase form and retry with the constant-cased form on failure. Patterns are tiny, so the
retry is free, and the procedure stays deterministic.

*Reverses if:* a placeholder-collision or round-trip-fidelity problem makes substitution
unreliable, in which case the fallback is a documented contract limit on where
metavariables may appear — which would cost several transformation families.

## D19 — Relational constraints ship in v0.1, not deferred
**Decided.** Amends D12's "implement the matcher, not ast-grep's rule algebra; add relational
rules only if the corpus demands them." The corpus demanded them at entry #1: the first
realistic rename needs `inside: class User` to catch implicit-self call sites.

`inside:` ships in v0.1. The rest of the algebra (`has`, `follows`, `all`/`any`/`not`) stays
deferred under D12's original reasoning.

## D20 — Residue keys are receiver-qualified where a receiver is known
**Decided.** The "identifier too common; N occurrences, not enumerated" degradation in
DESIGN.md §4 collides with Rails naming: `create`, `update`, `call`, and `perform` are
simultaneously the likeliest migration targets and the names the clause abandons. The
differentiator zeroes out exactly where it is needed most.

Where a receiver resolves, residue is keyed on **co-occurrence** — `Payments::Charge` near
`create` — rather than on the bare identifier. This depends on the symbol layer, so full
benefit lands in Phase 2; Phase 1 degrades to bare-name residue and says so.

**Consequence for Phase 0 measurement (a):** the spike must include at least one *common*
Rails-shaped name, not only a distinctive one like `calculate_payroll_tax`. Testing only
distinctive names measures the easy case and passes for the wrong reason.

## D21 — Preview by default; `--write` is the only thing that touches disk
**Decided.** Reverses `cli-conventions.md`'s original `--dry-run` framing, which implied
write-by-default.

Every peer defaults to preview and requires an explicit flag to mutate: ast-grep
(`-U/--update-all`), semgrep (`--autofix`), ruff (`--fix`), biome (`--write`), rubocop
(`-a`/`-A`).

The argument is asymmetry: **a forgotten `--dry-run` is unrecoverable; a forgotten `--write`
is recoverable** — the agent sees no files changed, reads the diff, retries. It also deletes
a flag, since preview-as-default makes `--dry-run` unnecessary.

Naming follows biome, which renamed `--apply` to `--write` in v1.8 for consistency and
discoverability. Explicitly *not* rubocop's `-a`/`-A`, which differ only by case — trivially
mistyped by a model composing a shell string, and indistinguishable in a log.

## D22 — `rwr check`: an enforcement verb with inverted polarity
**Decided.** Two independent investigations converged on needing it — the case studies
(`docs/use-cases.md` finding 8) and the UX research (`docs/ux-research.md` §3.2).

In search mode "no match" is a negative result. In enforcement mode "no match" *is* success.
pre-commit's contract is "the hook must exit nonzero on failure," so a verb with search
polarity would block every commit where a rule correctly matches nothing — the common case.
The same exit code cannot mean both, so the verb carries the polarity.

Precedent: ast-grep's `run` exits 0 on a match (grep semantics) while `scan` exits 1 on a
rule match (lint semantics) — one binary, opposite conventions, chosen per subcommand.

`check` never writes. Mutation stays on `rewrite --write` (D21) rather than a fourth verb.

## D23 — Output is a tagged event stream, not an array of matches
**Decided.** `cli-conventions.md` originally specified `--json` as a pretty array. A
homogeneous array cannot carry heterogeneous records — matches, skipped files, residue
occurrences, conflicts, and a summary.

`--ndjson` follows cargo's `--message-format json`: one object per line with a
discriminator (`match`, `edit`, `skip`, `residue`, `conflict`, `error`, `finished`).
`--json` follows semgrep's multi-array object.

**The `finished` terminator is load-bearing**, not decoration: an agent reading a truncated
stream cannot otherwise tell "done" from "died halfway." ripgrep ships the equivalent as its
`summary` message.

---

## D17 amended — exit-code layout corrected
Three errors in the original layout, all found by `docs/ux-research.md` §3.2:

1. **`2 = retryable` collided with a near-universal `2 = error`** (grep, ripgrep, ruff,
   rubocop, biome, jq, semgrep). D11 wants an agent that learned rq to transfer, and this was
   the one number guaranteed to mislead it. Error is now 2; retryable moved to 4, refused to 5.
2. **A single polarity could not serve both search and enforcement** — see D22.
3. **Pattern-parse errors were not distinguished from source-parse errors.** They demand
   different responses (fix the rule vs skip the file) and now get their own code, 3,
   following jq's compile-vs-runtime split.

Also settled: **error does not win over match.** ripgrep returns 2 if anything errored even
when it matched; rwr returns 0 with the skip recorded in the JSON, consistent with DESIGN.md
§4's "reported and skipped." This differs from rg and is therefore documented explicitly.

## D24 - MCP: five tools on the read/write boundary, writes gated by a plan id
**Decided** (design; ships post-Phase-2). `rwr_find`, `rwr_check`, `rwr_plan`, `rwr_apply`,
`rwr_info` - never one fat tool with a mode flag, because MCP annotations are per-tool and a
write mode permanently taints the read path.

**`rwr_apply` accepts only a `plan_id` that `rwr_plan` mints.** Preview stops being a
forgettable flag and becomes the only path to a write - principle 5 (make unsafe operations
unrepresentable) applied to the API rather than only the capture surface.

Context: this MCP space is empty rather than merely under-served - no servers for comby,
jscodeshift or OpenRewrite, and ast-grep's is dormant, returning 194k-token results against a
25k cap with the maintainer rejecting pagination on principle. But a counter-argument is on
record that a *skill over the CLI* beats an MCP server outright. rwr's MCP therefore earns
its keep through **the transaction**, not through search.

## D25 - Edit preconditions are content-addressed per edit, not per file
**Decided.** Amends DESIGN.md section 7. A per-file content hash over-rejects: any unrelated
line moving anywhere in the file kills the whole transaction, which in a busy 1M-LOC repo
means a large rewrite can never land.

Each edit carries `<path>:<index>@<digest>`, the digest covering that edit's own effective
range. Serena arrived at the same contract independently.

## D26 - No config file in v1; ship `--isolated` anyway
**Decided.** ESLint's flat-config postmortem is the strongest evidence any tool has produced
against hierarchical config, and ruff - which avoided the cascade merge - still accumulated
six issues around hierarchical `exclude`, including `ruff --force-exclude .` reporting "no
Python files" at exit 0.

Rules are files passed explicitly; there is nothing a config file must hold in v1.

Ship `--isolated` regardless, as a no-op that harnesses and hook definitions can pass
forward-compatibly. If config ever arrives, every existing caller is already immune to it.

## D27 - MCP `plan`/`apply` collapse into one `rwr_rewrite`
**Decided.** Reverses the five-tool shape in D24, which was recorded from a research
recommendation without being pressure-tested.

D24's justification - "MCP annotations are per-tool, so a write mode taints the read path" -
is sound, but it supports splitting **read tools from write tools** (`rwr_find`/`rwr_check`
vs `rwr_rewrite`). It does not independently support splitting `plan` from `apply`. The two
splits were conflated.

Three problems with the plan/apply split:
- **It contradicts the CLI.** D21 already made preview the default and `--write` the
  mutation. An MCP with a different shape makes an agent learn the tool twice.
- **It introduces state with no compensating gain** - a `plan_id` needs a home, a lifetime,
  expiry and invalidation. Under D5 (no persistence) the process exits between calls, so the
  id would reference data, not a warm parse. There is not even a performance argument.
- **It manufactures the race D25 exists to fix**: plan at T0, apply at T1, files change in
  between.

**Replacement - stateless optimistic concurrency.** `rwr_rewrite(rule, write: false)` returns
the plan with per-edit digests; `rwr_rewrite(rule, write: true, expect: [digests])` applies
only if they still match. The plan is data the agent holds, not server state. `expect` is
optional, since CI wants a blind apply.

Four tools: `rwr_find`, `rwr_check`, `rwr_rewrite`, `rwr_info`.

## D28 - No undo journal; a clean-tree precondition instead
**Decided.** rwr does not implement revert/undo.

An undo journal is a *recovery* mechanism in a design that has consistently chosen
*prevention*: D14 makes unsafe splicing unrepresentable, D25 uses preconditions rather than
rollback, and apply is already atomic so a half-written state cannot occur. A journal would
also duplicate git badly - retention, staleness, interaction with stash and rebase, and the
"undo after I already committed" confusion.

The real gap it would have filled: `git checkout -- .` also destroys the user's *own*
uncommitted work, so "undo only rwr's changes" is genuinely not expressible in git alone.

**Closed by precondition instead.** rwr refuses to write a file whose worktree content
differs from the git **index**, unless `--allow-dirty` is passed. Then
`git checkout -- <files>` is an exact and complete undo, and rwr prints that command.

**Index, not HEAD** - the distinction is load-bearing. Inside a pre-commit hook (D22) files
are staged by definition, so a HEAD-based precondition would refuse on every autofix run and
force `--allow-dirty` into the common path, tripping this decision's own reversal condition
on day one. `git checkout -- <file>` restores from the index regardless, so index-clean is
simultaneously the weaker precondition and the one that makes the undo exact. For the
`--allow-dirty` case, emit the reverse patch in the JSON so `git apply -R` works.

*Reverses if:* real usage shows `--allow-dirty` is the common case rather than the exception,
which would mean the precondition is not actually closing the gap.

---

## D25 withdrawn - no edit preconditions at all
Per-edit content addressing existed to fix per-file hashing's over-rejection. With the
precondition itself removed (D28 withdrawn, below) there is nothing left to fix.

## D28 withdrawn - no clean-tree precondition, no `--allow-dirty`
Overbuilt. `sed -i`, `rubocop -a`, `ruff --fix` and `prettier --write` all write without
policing git state, and D21's preview-by-default already covers the risk the precondition
was guarding. "Refuses to run because your tree is dirty" is a well-earned annoyance, and the
flag existed only to escape a restriction rwr imposed on itself.

The gap D28 named is real but rare - `git checkout -- .` also discards the user's own
uncommitted work - and the near-free mitigation survives without any of the machinery: the
preview diff **is** the reverse patch, so `git apply -R` works on output rwr already emits.

## D27 amended - `rwr_rewrite(rule, write: bool)`, no `expect`
The optimistic-lock parameter is dropped.

Within a single invocation, read and write are milliseconds apart - there is no race. The gap
exists only between an MCP preview call and a later write call, and the write call recomputes
from current state regardless. So digests were preventing *surprise*, not *damage*: atomic
apply and reparse-verify already prevent corruption.

And the surprise is acceptable, because **the unit of review is the rule, not the edit
list.** If a file changed and the recompute yields 84 edits where the preview showed 83, the
84th is a correct application of an already-approved rule. Recomputing beats replaying a
stale plan.

Residual risk, stated rather than defended against: an apply may differ from its preview.

*Reverses if:* real usage shows agents materially harmed by preview/apply divergence - in
which case the fix is per-edit digests (the withdrawn D25), not a server-side plan.

## D21 amended - `--dry-run` exists, and overrides `--write`
Preview remains the default. `--dry-run` is added back, but *not* as a synonym for the
default - it is an override that beats `--write`.

The case the default cannot serve: a hook definition or CI job has `--write` baked into a
fixed command line, and rehearsing it should not require editing it.

```
rwr rewrite timecop.yml --write              # as shipped
rwr rewrite timecop.yml --write --dry-run    # same line, rehearsed
```

Secondary benefit: `--dry-run` is the flag a user or agent types from habit, and it should do
the expected thing rather than error. Costs one no-op flag.

## D29 - The verb carries the mode: `check` looks, `rewrite` does
**Decided.** Supersedes D21 and its amendment. Both `--write` and `--dry-run` are removed.

D21 chose preview-by-default with `--write`, copied from ruff/biome/rubocop. The amendment
then added `--dry-run` back as a `--write` override. The result was three spellings for two
behaviours - incoherent, and the incoherence was a symptom rather than the disease.

**The disease was the verb name.** Preview-by-default is right for tools whose verb is
already a reporting word (`ruff check`, `biome check`, `rubocop` the linter). Ours says
`rewrite`. A command named `rewrite` that does not rewrite is a mismatch no documentation
fixes, and the design kept growing flags to explain it away.

**Resolution:** mode moves onto the verb, exactly as D22 already moved polarity onto the
verb. `rwr check` = what would happen. `rwr rewrite` = do it. Zero mode flags.

Supporting evidence: dprint's `fmt`/`check` split - the subcommand says whether it mutates -
was ranked *above* biome's `--write` by the UX research, and `cargo fmt` writes by default
because its verb is an action word.

The "forgotten flag is unrecoverable" argument that justified D21 does not survive the
change, because there is no flag to forget. Typing `rewrite` when you meant `check` is a
rarer and more deliberate error than omitting a flag on a command you fully intended to run.

`check` serves both audiences on one polarity: exit 1 means "violations" to CI and "there is
work to do" to a human previewing. Same number, both readings correct.

*Reverses if:* users report accidental writes at a rate that a flag would have prevented -
though note the accident would then be typing the wrong verb, which a flag does not guard.

## D30 - Positional shorthand, read-only by construction
**Decided.** `rwr <pattern> [replacement]` works without a subcommand:

```
rwr 'foo($A, $B)'                          # = rwr find
rwr 'foo($A, $B)' app/models lib/          # scoped to those paths
rwr 'foo($A)' -r 'bar($A)'                 # = rwr check (preview)
rwr rewrite 'foo($A)' -r 'bar($A)' app/    # write - verb still required
rwr rewrite rule.yml app/models            # write, with where: constraints
```

Matches rq's shape (`rq <query>` as the default command), so the tool family stays
learnable as one thing - the interface-level integration D11 chose.

**The load-bearing constraint: the shorthand cannot write.** A two-argument form that
silently mutated a repo would reintroduce write-by-default in its most dangerous shape - no
verb admitting what is happening - undoing D29 through the back door. Terseness must never
buy a foot-gun, so the shorthand desugars only to `find` or `check`, and mutation always
requires typing `rewrite`. Pinned by
`tests/cli_e2e.rs::shorthand_with_replacement_cannot_reach_rewrite`.

Pattern-plus-replacement sugar is available to `rewrite` too, so the one-liner path to a
write exists - it just says so. This also gives natural progressive disclosure: bare
pattern/replacement for simple cases, a rule file exactly when `where:` is needed.

Agents, MCP and CI use explicit verbs; the shorthand is a human affordance.

*Ambiguity note:* a pattern that is literally a subcommand name (`rwr find`) resolves as the
subcommand. Acceptable - rq has the same collision - and `rwr find 'find'` disambiguates.

## D31 - Trailing positionals are paths; the replacement is `-r`
**Decided.** Every verb takes `[PATH...]` as trailing positionals - files or directories,
rg-style, sugar for repeated `--path`. The replacement template moves to `-r/--replace`.

**Why the replacement cannot stay a positional.** With both, `rwr 'foo($A)' app/models` is
ambiguous: is the second argument a replacement or a path? The only way to decide is to probe
the filesystem, which is a *guess* - and principle 2 is refuse rather than guess. The
ambiguity is removed structurally rather than by convention.

**Why paths win the positional slot.** Scoping a search is far more frequent than a one-liner
rewrite; rg users type paths constantly. `-r` also matches ast-grep, and the tool's own
one-line pitch is "rg/sed for Ruby programs" - so trailing paths are the shape users arrive
expecting.

Files and directories are not distinguished, exactly as in rg. `--path` remains available as
the explicit repeatable form, matching rq.

Pinned by `tests/cli_e2e.rs::trailing_positionals_scope_the_search`, which fails if a path is
ever routed as a replacement.

## D32 - Metavariable surface syntax
**Decided.** Completes D16, which settled the *semantics* (occurrence counts, AST equality)
but left the spelling unwritten - the last piece of D2's v0.1 public contract, and the one
every case study needed.

Two orthogonal axes rather than four forms to memorise: `*` means many, `_` means don't care,
`$NAME` binds.

|            | one node | zero or more |
|------------|----------|--------------|
| anonymous  | `_`      | `*_`         |
| captured   | `$NAME`  | `*$NAME`     |

**All four are valid Ruby**, which is the point. `*` is Ruby's own splat and `_` its own
throwaway, so the audience needs no new concepts, and Ruby's grammar validates position for
free: sequences are legal exactly where splats are - argument lists, arrays, block params,
destructuring - which is exactly where they are wanted. `*_` is already idiomatic Ruby for
"rest, ignored".

Optional, exact counts and must-not-appear are `where:` refinements over D16's occurrence
counts; `$NAME` defaults to `{min: 1, max: 1}` and `*$NAME` to `{min: 0, max: unbounded}`.
Inline covers the common cases, `where:` the precise ones - so the CLI one-liner (D30)
reaches sequences but not must-not-appear, the intended progressive disclosure.

**Collision - Ruby globals are spelled `$foo`.** If every `$X` were a metavariable, `$stdout`
could never be matched literally. Resolved by case: captures are uppercase, so `$stdout`,
`$_`, `$1` and `$:` are literals needing no escape. Only uppercase globals (`$LOAD_PATH`,
`$DEBUG`, `$PROGRAM_NAME`) remain ambiguous and escape as `\$LOAD_PATH` - rare enough in
codemod work to prefer an escape over a worse rule.

**No exceptions.** An earlier draft used `$$$NAME` for sequences and `$_` for anonymity,
which forced a documented carve-out because `$_` is a real Ruby global. Spelling anonymity as
plain `_` removes the carve-out rather than excepting it, and hands `$_` back as a literally
matchable global.

Implemented as a pure lexical scanner in `src/pattern/metavar.rs`, ahead of any parsing,
since D18 substitutes metavariables before Prism sees the pattern. Seven tests pin the rules,
including the global collision, the `_` word-boundary rule, and that a bare `*` is
multiplication.

*Reverses if:* the escape proves load-bearing in practice (i.e. matching uppercase globals is
common), which would argue for an explicit sigil instead of a case rule.

### Superseded draft: `$$$NAME` for ast-grep compatibility

The first version of D32 used ast-grep's `$$$NAME` / `$_`, chosen for switching cost on the
theory that "the patterns you already have, better results" is the adoption story.

Dropped in favour of the 2x2 above, which is smaller (no exceptions), self-documenting to
Ruby programmers, and natively parseable. The migration cost is paid once, in code, by the
deferred `rwr import` - rather than forever, in a syntax every reader must learn.

Note the assumption behind the original choice was never tested: there is no evidence anyone
writes ast-grep Ruby rules at volume. Phase 0 can check cheaply before any importer is built.

### Considered and rejected: a cleaner alias alongside `$$$`

Supporting both `$$$NAME` (ast-grep compatible) and a nicer spelling such as `$..NAME` or
`$*NAME`.

**Rejected on two grounds.**

First, the readability advantage is smaller than it looks, because **every punctuation sigil
collides with a Ruby global**. That namespace is nearly exhausted: `$*` (ARGV), `$$` (pid),
`$.` (line number), `$_`, `$~`, `$&`, `` $` ``, `$'`, `$+`, `$!`, `$@`, `$/`, `$\`, `$,`,
`$;`, `$?`, `$:`, `$"`, `$<`, `$>`, `$0`-`$9`. So `$..` collides with `$.`, and `$*NAME` -
the most Ruby-idiomatic candidate, since `*` is Ruby's own rest operator - collides with
ARGV. Collision was cited against `$$$` when D32 was made; it is in fact not a discriminator,
because there is nowhere clean to stand. That leaves taste versus migration cost, and only
one of those is measurable.

Second, two spellings for one concept is a permanent tax with **no offsetting reduction**. A
canonical form must still be chosen for documentation, `--explain` output and error messages,
so the alias is pure addition - and being a public contract, removing it later would be a
breaking change. Every rule a reader encounters, they must know both spellings.

**The better shape for the same instinct is an importer, not a dialect:** `rwr import
<ast-grep-rule>` converting to canonical rwr syntax once. Strictly stronger as adoption
("we port your rules" beats "we tolerate your syntax") and it costs nothing at read time
forever after - the same pattern as D26's `--isolated`. Deferred to vision until Phase 0
shows whether ast-grep migration demand is real; building it now is speculative.

## D33 - Sequence transforms in rewrite templates: a closed set
**Decided.** Rewrite templates may apply a transform to a *sequence* capture:

```yaml
match:   '[*$ITEMS]'
rewrite: '[*$ITEMS.sort]'
```

**Disambiguation is by arity, not by name.** `$X.sort` on a single capture is legitimate
*literal* output - `foo($X)` -> `$X.sort` is a rewrite someone would want - so `.sort` cannot
blanket-mean "transform". A sequence capture is not a value but a list of nodes spliced into
a position, so `*$ITEMS.sort` has no competing literal reading. It also matches Ruby's own
semantics: `[*a.sort]` splats the sorted thing.

An unrecognised transform name is an **error**, never silent literal output (principle 2).

**The closed set, defined so it cannot erode.** Principle 8 forbids embedded scripting, and
"just add `sort_by`" is exactly how that boundary would go. So the set is not a list of
blessed names but a definition: **zero-argument, deterministic, total, sequence-to-sequence.**
No blocks - a block is user code, which is the line.

That admits `sort`, `uniq`, `reverse` and excludes `sort_by`, `select`, `map` by
construction.

**Ordering:** `sort` compares **effective source text** (D14's range). The only general answer
for arbitrary expressions, and what "alphabetise" means to a human. Applies uniformly to
`%w[]` literals, symbol arrays, and hash pairs (sorting `AssocNode`s by their source).

*Reverses if:* real rules need an ordering source text cannot express - e.g. sorting by symbol
name where elements carry differing quoting - which would call for a small set of named
comparators rather than an expression language.

## D34 - Indentation repair, not formatting
**Decided.** Resolves the tension between principle 7 (rwr does not own style) and the fact
that every rewrite disturbs indentation.

The distinction:

> rwr does not impose style. rwr avoids *breaking* existing style.

**In scope:** re-indenting the regions rwr rewrote, so inserted multi-line text lands at the
correct column. This is not a style feature - without it the "minimal diff" promise is already
broken, because the diff contains whitespace damage rwr itself caused. Repair what was
disturbed, touch nothing else.

**Out of scope:** formatting a file, enforcing a layout style, or anything resembling a
general Ruby formatter. Those shell out - syntax_tree is the natural target, being written by
the Prism author and therefore sharing our parse lineage. `rubocop -a Layout` and standardrb
also work.

**On rubyfmt as a reference** (author asked): little to borrow for the narrow job. Its hard
problems - line-breaking decisions, idempotence, comment placement under reflow - are
full-formatter problems. Indentation repair needs only a target column and leading-whitespace
adjustment. *(Its current maintenance status was not verified; the session's web search budget
was exhausted.)*

The exception is **comment placement**, which is genuinely relevant because sorted arrays
(D33) made comment-attachment-under-movement load-bearing. syntax_tree is likely the better
reference there than rubyfmt.

## D35 - Comment attachment under movement
**Decided** at principle level; details deliberately left to iterate (author, this session).
Unblocks D33's sorted arrays, which made this load-bearing.

**The rule: a comment moves with the element it belongs to, and belonging is by adjacency.**

| Case | Attachment | Behaviour |
|---|---|---|
| **Trailing** - same line, after an element | that element | moves with it |
| **Leading** - own line(s) directly above an element, no blank line between | that element | moves with it |
| **Interior** - inside a multiline element | that element | moves automatically (already inside its `effective_range`, D14) |
| **Dangling** - after the last element, or separated by a blank line | the container | stays put |
| **Ambiguous** - shares a line with more than one element | none | **refuse** (exit 3) when the transform reorders |

Interior and dangling need no machinery - the first falls out of `effective_range`, the second
out of not attaching.

**Why ambiguous refuses rather than guessing.** A comment on a multi-element line has no
unambiguous owner, and silently reattaching it to a neighbour is exactly the quiet wrongness
the design exists to prevent (principle 2). The case is narrow - it fires only on a
multi-element line *with* a comment *during a reordering transform* - and the escape is
obvious: put one element per line and rerun. `--explain` names the comment and the line.

**Deliberately not settled yet**, to be driven by real rules rather than speculation:
- Blank-line handling around moved leading comments.
- Whether a leading comment block separated from its element by other trailing comments
  attaches, and to what.
- Magic comments and `# rubocop:disable` directives, which have semantics beyond position and
  probably should never move.
- Whether refusal is too strict in practice, or should degrade to a warning with the comment
  left in place.

## D34 confirmed - indentation repair is in scope
Author confirmed (this session) that both comment handling and indentation repair should ship,
on the reasoning in D34: without them the minimal-diff promise is broken by damage rwr itself
caused. Details may iterate; the commitment does not.

## D20 amended - residue degradation keys on receiver-shape diversity, not frequency
Phase 0 measurement (b) corrects the premise. D20 assumed the "identifier too common; N
occurrences, not enumerated" fallback would fire on the commonest Rails names, and that this
was where the differentiator would be weakest.

Measured on rails (309,683 call sites), commonness and tractability turn out to be
**different axes**:

| name | sites | constant receiver | tractable? |
|---|---:|---:|---|
| `create` | 875 | **75%** | yes - mostly `Foo.create`, statically known |
| `name` | 2,067 | 2% | no - six receiver shapes, nothing pinned |
| `id` | 1,608 | 0% | no |
| `value` | 335 | 0% | no |

`create` is among the commonest names *and* among the most tractable, because three quarters
of its call sites name their receiver outright. Keying the degradation on raw frequency would
abandon it needlessly.

**Amended rule:** degrade to "not enumerated" based on **receiver-shape diversity** at the
call sites - how much of the name's usage the index can actually pin - rather than on how
often the name appears. A name used 2,000 times with a known receiver is fine; a name used 300
times across six unresolvable shapes is not.

This also sharpens the Phase 0 (a) residue spike: it should pick targets by *shape diversity*,
not by picking one rare and one common name.

## D36 - The comparator is generated from Prism's schema; locations never compare
**Decided** after an independent staff-engineer review rejected the first proposal.

**What was rejected.** "Two nodes are equal iff same variant, same children, and equal
*interstitial source* (bytes not covered by a child, whitespace-normalised)." It promised zero
per-node-type code and free concrete-syntax sensitivity. It was wrong twice over:

- **It contradicts decisions already recorded.** DESIGN.md section 2 says `and`/`&&` and
  rocket-vs-shorthand are distinguishable *by a `where:` predicate* - core equality is
  tree-equality and spelling is opt-in. Interstitial equality inverts that, making spelling
  mandatory with no way to opt out. It also reintroduces exactly the textual comparison D16
  rejected as "Comby's known mistake", at the heart of node equality: `1_000` != `1000`,
  `"x"` != `'x'`, `foo a` != `foo(a)`.
- **Two decisive failures.** Trailing commas: `foo(a, b,)` vs `foo(a, b)` differ only in
  interstitial text that whitespace normalisation does not touch, so every multiline literal
  silently fails to match - and trailing commas are on the author's wanted-rules list.
  Heredocs are worse and in the worse direction: per D14 a heredoc node's range is
  `opening_loc` only, so interstitial comparison never sees the bodies and two calls with
  *different* heredoc content compare **equal**. A silent false match is the worst outcome the
  design admits.

**What replaces it.** Two nodes are equal iff **same variant, equal atoms, and pairwise-equal
children** - where an atom is a name, value, or semantic flag. **Locations never participate.**

`ruby-prism` vendors `config.json`, the machine-readable schema Prism's own `build.rs`
consumes. Field census across all 151 nodes: 190 child fields, **228 location fields**, 67
`constant` fields (where `CallNode.name` hides - the trap that started this), 18 value fields,
15 flag families. Only 66 of 151 node types carry atoms at all.

**So the comparator is generated from that schema, not hand-written**, and drift is defined out
of existence twice:
- A new Prism *variant* fails a non-exhaustive match -> **compile error**.
- A new *field* on an existing variant - the genuinely silent case - fails a **parity test**
  that reads the vendored `config.json` and asserts every field of every node is classified as
  child, atom, or ignored-location. Refuse-rather-than-guess, applied at the meta level.

**Atom policy** (the part deserving judgement - a table, not a codebase):

| field kind | rule |
|---|---|
| `constant*` | compare **resolved bytes**, never ids - pattern and target come from different parses with different constant pools |
| `string` | compare **unescaped values**, so `"x"` == `'x'` and **heredoc bodies compare correctly with zero heredoc-specific code** - the case that killed interstitial becomes free |
| `integer` / `double` | value compare: `1_000` == `1000`, `0x10` == `16` (base is spelling) |
| flags | per-family semantic mask: ignore parse artifacts (`VARIABLE_CALL`, so `foo` == `foo()`), compare semantic bits (regex `i`/`m`/`x`) |
| `location*` | never compared. `AndNode::operator_loc` and friends stay `where:` predicates, as section 2 promised |

**One comparator, three consumers**, for conceptual integrity: matching, D16's repeated-metavariable
`ast_eq`, and section 7's reparse-verify. If each grows its own equality they drift, and the
verify step stops guarding the matcher. The equality semantics are a **public contract** and
belong documented in one place.

Performance falls out: discriminant plus root atom rejects nearly every candidate in O(1), so
matching is O(target nodes).

## D18 amended - repair is per-placeholder, not per-pattern
D18 proposed parsing with lowercase placeholders and retrying capitalised. That cannot serve a
pattern needing both: `class $C; [1].each { |$P| $P }; end` requires an uppercase class name and
rejects an uppercase block parameter.

So the retry is per-placeholder: on a parse error, flip the case of the placeholder nearest the
error offset and try again, bounded by the placeholder count. Patterns are tiny, the loop is
cheap, and it stays deterministic. Implemented in `src/pattern/prepare.rs`.

**A hazard repair cannot see.** `Foo::bar` is a method call and `Foo::Bar` is a constant, so
`Foo::$C` parses under *either* casing and the placeholder's case silently decides what the
pattern means. There is no parse error to react to. The matcher must therefore treat a
placeholder as a wildcard by name rather than trusting the node type it landed on - pinned by
`scope_resolution_parses_under_either_casing`.

## D37 - Comparator implementation: visitor for children, explicit arms for atoms
**Decided.** Implements D36, deviating from its "generate everything from config.json"
recommendation for a reason found while building.

**The constraint.** `ruby-prism` exposes no typed-node -> `Node` conversion: no `as_node()`,
no `From<CallNode> for Node`. Typed accessors return *typed* values - `CallNode::arguments()`
gives `Option<ArgumentsNode>`, not `Option<Node>`. Generating a generic child walk would
therefore mean reconstructing `Node` enum variants from raw pointers per type, which is fiddly
and fragile in a way the codegen was supposed to avoid. D36's recommendation assumed that
conversion existed.

**So the two halves are built differently:**

- **Children: the `Visit` trait's depth-tracked enter/leave hooks.** These yield `Node`
  uniformly, so no conversion problem arises and no codegen is needed. D36 proposed dropping
  this in favour of generated code; the missing conversion reverses that call.
- **Atoms: explicit per-variant arms**, since extracting `ConstantId::as_slice()`,
  `unescaped()`, `value()` requires the typed accessor either way. Only 66 of 151 node types
  carry atoms.

**How the drift risk of hand-written arms is contained** - this is the part that matters, and
it keeps D36's guarantee without its mechanism:

1. The schema parity test (already built) catches a new *field type*.
2. A second parity test asserts every node type that config.json says carries atoms either has
   an arm **or** appears on an explicit `UNIMPLEMENTED` list. A new atom-carrying node cannot
   be silently skipped.
3. **The matcher refuses on an unimplemented atom-carrying node** rather than comparing
   incompletely. This is the right behaviour independent of implementation strategy: comparing
   a node while ignoring atoms it carries is precisely the `foo(a)` == `bar(a)` bug that
   started D36, and refusing is the design's answer to not knowing (principle 2).

That third point converts hand-written arms from a correctness risk into a *coverage* one:
incomplete arms make rwr decline work, never do it wrongly. Coverage can then grow with the
corpus rather than needing to be complete on day one.

*Reverses if:* a future `ruby-prism` adds a typed-node -> `Node` conversion, at which point
full codegen becomes as clean as D36 assumed and should be adopted.

## D49 - Class and instance methods are different methods
**Decided**, after the author asked whether rwr distinguishes `Account#display_name` from
`Account.display_name`. It did not, and that was a correctness bug: `type: Account` rewrote
both, so renaming an instance method silently renamed the unrelated class method too - the
silent wrong edit the whole design exists to prevent.

Receiver resolution yields `Instance(C)` or `Class(C)` rather than a bare name:

| receiver | resolves to |
|---|---|
| `Account` (a constant) | `Class(Account)` - a constant names the class *object* |
| `Account.new` assigned to a local | `Instance(Account)` |
| `self` in an ordinary method body | `Instance(enclosing)` |
| `self` in `def self.x` or `class << self` | `Class(enclosing)` |

Constraints take `kind: instance | class`, defaulting to **instance** - it matches Ruby's own
`Account#foo` notation and is the commoner case.

## D50 - Renames are written in Ruby's method notation
**Decided.** `method: Account#display_name` / `rename: full_name` expands to the rule set a
complete rename needs, rather than making the author write three rules by hand:

1. the definition - `def display_name` (or `def self.display_name` for the dot form), scoped
   `inside:` the class;
2. explicit-receiver calls - `$R.display_name` narrowed by class *and* kind, which also covers
   `self.display_name` since that resolves as an instance receiver;
3. implicit-self calls - a bare `display_name` scoped `inside:` the class, reaching the
   largest receiver bucket (43.5% of rails calls).

The notation is not incidental sugar: `#` versus `.` *is* the instance-versus-class
distinction D49 exists to respect, so the spelling a Ruby developer already uses carries
exactly the information the rename needs. The dot form emits two rules rather than three -
implicit self inside a class-method body is not yet reachable, since `inside:` does not track
singleton context.

## D51 - Inheritance is not modelled; residue is the safety net until it is
**Decided** (roadmap), after the author asked whether rwr handles inheritance and whether
`self` should be distinguished from the lexical class. It does not, and the honest answer has
two halves.

**`self` is an approximation.** Inside `class Account`, `self` is an instance of Account *or
any descendant*. rwr resolves it to `Instance(Account)`, which is right for finding Account's
own definition and its self-calls, and wrong in the strict sense that a subclass override
could be what actually dispatches. For a rename the approximation is usually harmless, because
a rename covers the hierarchy - but it is an approximation and should be named as one.

**Inheritance is unmodelled, and it cuts both ways:**

- *False negatives.* `premium.display_name`, where `Premium < Account`, does not match
  `type: Account`, so a rename misses it - and the method it called has just been renamed away.
  Demonstrated: rwr renamed the definition and the self-call, left the subclass call site, and
  the file no longer ran.
- *Override protection.* If `Premium` overrides `display_name`, renaming `Account`'s must not
  touch `Premium`'s. rwr gets this accidentally right today by matching neither.

**Why this is shipped rather than blocking:** wiring residue into `check` and `rewrite` turns
the gap from *silently broken code* into *reported*. The same run now says:

```
rewrote 2 site(s)
1 occurrence(s) this rule could not account for (0 symbol, 0 string, 1 call, 0 definition):
  inherit.rb:15:1: Call: premium.display_name
```

That is the design working as intended - do what can be proven, refuse the rest, and account
for it - rather than a gap papered over.

**Resolved by D52.** The class-hierarchy index is built.

## D52 - Class hierarchy, built per run
**Decided.** Implements the roadmap item D51 recorded, and closes the gap that produced
demonstrably broken code: renaming `Account#display_name` left `premium.display_name` behind
and shipped a `NoMethodError`.

`src/hierarchy` walks every Ruby file collecting `class X < Y`, giving `descends_from` and
`descendants_of`. Two constraints consume it:

- `where: { $R: { type: Account, subclasses: true } }` admits receivers whose class descends
  from Account, reaching subclass call sites.
- `scope: { inside: Account, subclasses: true }` admits descendant classes, reaching an
  **override's definition** as well as the original's.

**Both halves are needed together.** Reaching subclass call sites without renaming an
override's definition breaks the code, and renaming the definition without the call sites
breaks it the other way. The method notation (D50) therefore sets both, because that is what a
rename means.

**Off by default.** Narrowing may only ever narrow (D49's principle), so a bare `type:`
constraint stays exact. A rename opts in; an ad-hoc query does not.

**Built per run, not cached.** Phase 0 measurement (d) found a full rails parse takes under
200ms, so there is no staleness to manage and D5 still holds -- the index that Phase 1
deliberately avoided turns out to be affordable precisely because parsing is cheap. It is
built only when a rule actually asks for subclasses, so an ad-hoc query pays nothing.

A superclass named through a path (`class Premium < Billing::Account`) resolves by its final
name, matching what a `type:` constraint names. Cycles -- impossible in valid Ruby, possible
in a half-written file the walk reads -- terminate rather than hang.

## D53 - Trailing commas belong to the formatter, not to rwr
**Decided**, and by evidence rather than by taste.

A trailing comma is **invisible to rwr's equality**: `[a, b]` and `[a, b,]` compare equal, as
do `foo(a, b)` / `foo(a, b,)` and `{a: 1}` / `{a: 1,}`. Pinned by
`compare::tests::a_trailing_comma_is_not_structure`.

By the tool's own definition of a program, adding one changes nothing. That places it in the
same class as indentation -- presentation rather than structure -- and therefore with the
formatter (principle 7, D34: rwr does not impose style, only avoids breaking it).

**This corrects `docs/rule-corpus.md`**, which listed trailing commas as reachable pending an
inter-node-source predicate. The predicate would have worked -- the comma is findable in the
gap between the last element and the closing delimiter -- but building it would have made rwr
a formatter through the back door.

**The line is where it should be.** Sorting an array *is* structural, since the element order
differs in the tree. Hash shorthand *is* structural, since `{foo:}` carries an implicit value
node that `{foo: foo}` does not. Trailing commas and indentation are not. rwr does the first
two and shells out for the last two, and the test above is what tells them apart rather than
an opinion.

*Reverses if:* rwr ever grows a concrete-syntax layer for its own reasons, at which point
this becomes nearly free -- but it should not be the reason for growing one.

## D54 - A rule pack is a directory, and every rule carries an id
**Decided.**

`rwr check rules` loads every `.yml`/`.yaml` under a directory, recursively, in path order. A
rule's id defaults to its path within the pack, so `rules/performance/detect.yml` reports as
`performance/detect`, and an explicit `id:` key overrides that.

Three things follow from the directory being the unit, and each was a choice:

**A subset is a subdirectory.** `rwr check rules/performance` is how a family gets turned on.
No config file listing enabled rules, no `--only` flag — the filesystem already expresses
selection, and a second mechanism for it would drift from the first.

**A malformed rule fails the pack.** Skipping it would leave a run that looks complete and is
not, which is the same failure as silently dropping an edit (principle 2). The error names
the file.

**The file is the unit of identity, not the rule.** A `method:` rename expands to three or
four rules, but it is one thing a user turned on and reports as one. Only a rule that names
itself gets its own id.

**Attribution is the point, not decoration.** Before this, a run over a pack could say "27
sites changed" and nothing more. That is not a reviewable answer, and it is the failure mode
of every linter that reports a total instead of a cop name.

*Reverses if:* rules ever need ordering that path order cannot express — a dependency between
rules, say — at which point a manifest becomes necessary and the directory becomes just its
default.

## D55 - Output counts sites, not edits
**Decided**, and it corrects output that shipped in 0.1.0.

A rewrite that changes shape is split by the structural diff into several edits — `select { }
.first` → `detect { }` is two, because the receiver and the block are carried across
unchanged and only the two ends differ. Reporting `edits.len()` therefore said "2 site(s)"
for one place a reader sees in the diff.

`plan` now tags each edit with the match that produced it and counts distinct surviving
matches. A site counts once however many edits it takes, and only if at least one survived
conflict resolution. The JSON field is renamed `edits` → `sites` to match, which is a
breaking change to output that was wrong.

The general form of the mistake: an internal quantity that is easy to reach is not the
quantity the reader is asking about. Edits are an implementation detail of minimal diffs;
sites are what a human reviews.

*Reverses if:* nothing plausible. Edit counts remain available internally where the
conflict-resolution logic needs them.
