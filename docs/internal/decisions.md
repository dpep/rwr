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

## D56 - The structural diff aligns across sequence placeholders, and localizes per child
**Decided**, by a bug found in the field rather than in a test.

Two gaps in `structural_diff` made it give up far more often than it needed to, and giving
up means whole-node replacement -- correct, but it re-imposes the template's layout on
everything it covers.

**Sequence placeholders were not aligned.** A pattern child list containing `*$REST` or
`**$REST` is not the same length as the target's, so the diff bailed on length alone. Every
rule using a sequence placeholder therefore lost minimal diffs. `align()` now walks the two
lists together, consuming as many target children as the environment says each sequence
captured, and requiring the same sequence at the same position in the template.

**A diverging child aborted the whole node.** The recursion propagated `None` upward, so one
changed leaf re-rendered its entire ancestor. A child that cannot be diffed is now replaced
*in place*: its template counterpart is restored from placeholder spelling back to `$NAME`,
rendered against the environment, and spliced over that child's `effective_range`. Siblings
keep their bytes.

**Why this mattered more than it looks.** Hash shorthand is the highest-volume rule in the
shipped pack -- 1,759 sites on Discourse -- and it is spelled with double-splat sequence
placeholders. Before this, every multiline hash came back on one line with its trailing comma
gone. rwr was breaking principle 6 and D53's own line about not owning style, on the rule
most likely to be run. The corpus fixture had encoded that output as *expected*, which is the
part worth remembering: a test written after the behaviour confirms the behaviour.

**One trap.** `ImplicitNode` -- the value half of `{foo:}` -- borrows the *key's* location
rather than having one of its own, so a zero-width check does not catch it and localizing
onto it writes the key over the value. It is excluded by node kind, and the parent handles
the assoc whole.

*Reverses if:* nothing plausible. The escape hatch is unchanged -- anything that still fails
to align falls back to whole-node replacement, and `verify` reparses either way.

## D57 - No per-rule options; `unsafe:` carries the caveat instead
**Decided**, and it is the deliberate divergence from RuboCop's configuration model.

RuboCop configures a cop because a cop is Ruby code the user cannot edit: `EnforcedStyle`,
`AllowedMethods`, `Max` all exist to parameterise something opaque. An rwr rule is four lines
of YAML. The rule **is** the option -- there is no direction to configure because the rule
encodes one outright, and the way to get the other direction is to copy the file and swap
`match` with `rewrite`. Adding an options layer would mean a schema, defaults, and a merge
order per rule: reimplementing `.rubocop.yml` to configure something already declarative.

What genuinely cannot live in the rule body is **whether the rewrite can change behaviour**,
because that is a property of Ruby rather than of the pattern. `inject(:+)` returns nil for
an empty collection where `sum` returns 0. `select` on an `ActiveRecord::Relation` names
columns rather than filtering rows. `tr` gives `^` and `\` special meaning that `gsub` does
not. Each is a real input that breaks a rule that matched correctly.

So a rule carries `unsafe: <reason>`. **Presence means unsafe and the value is the reason** --
there is no boolean to set without saying what for, which is the design's whole point.
Unsafe rules are held back unless `--unsafe` is passed, and three things follow:

- **The holding back is reported.** A rule that was not run produces the same zero as a rule
  that found nothing, and those must not look alike -- the same failure as `performance/count`
  matching only `.size` and reading as a clean codebase.
- **The reason prints when the rule fires**, next to the diff. RuboCop has this information
  as `SafeAutoCorrect: false`, in a config file nobody reads at the moment of the edit.
- **Selection stays the filesystem.** A family is a directory, so turning a subset on needs no
  second mechanism.

**This is where receiver narrowing pays.** Half the caveats above are "unless the receiver is
an ActiveRecord relation", which a `where: { $R: { type: Array } }` answers outright. The
unsafe marker is the honest default *until* the rule is narrowed, not a permanent property.

*Reverses if:* a rule ever needs a genuinely numeric or list-valued parameter that cannot be
spelled as a distinct rule -- a `Max:` threshold, say. Nothing in the corpus wants one.

## D58 - Two literal predicates: `is:` and `length:`
**Decided.** Both were forced by rules someone wanted, and each unblocks more than its
requester.

`is: constant|symbol|string|integer|array|hash` constrains a capture's node kind. `length: N`
constrains a string or symbol literal's content in *characters*. Together they are what makes
`gsub` -> `tr` safe rather than plausible: `tr` maps character by character, so the rewrite is
valid only for one-character string literals, and `is: string` additionally keeps an
interpolated `"#{x}"` out, since interpolation is a different node.

**`is: constant` also seeds the substitution, and had to.** The case-repair loop in D18 flips
a placeholder only when the parse *fails*, which cannot help where both casings parse and
mean different things: `rwr_mv_1 = [1]` is a local-variable write and `RwrMv1 = [1]` a
constant write. A pattern for the latter silently became one for the former, matching every
local assignment and no constant. A rule that says `is: constant` has already answered the
question, so the declaration seeds the substitution rather than a second mechanism being
invented for it.

**A capture in name position binds an identifier, not a node.** `$C` in `$C = [...]` and `$M`
in `$R.$M` are atoms Prism carries on the parent, so there is no node to classify. For those
the identifier's own spelling answers the only question available -- Ruby constants start
uppercase and nothing else does. Pinned by
`matcher::tests::is_constant_reaches_a_constant_assignment`.

*Reverses if:* the predicate set grows past what a closed enum can carry, at which point it
wants a general value-matching language -- which should be resisted, since it is how a
structural tool turns into a regex engine.

## D59 - `--diff` scopes a run to the lines a change touched
**Decided.**

`rwr check` on a codebase that has never run it reports every pre-existing site, which is
useless as a gate: a pull request adding three lines fails on two thousand it did not write.
`--diff` restricts both matching and rewriting to lines the change touched, which is what
makes the tool adoptable without a todo file to go stale — RuboCop needed
`--auto-gen-config` for the same reason, and that file is a liability the moment it exists.

**`--diff` is the uncommitted work; `--since REV` is three-dot.** `git diff main` compares
against main's *tip*, so anything main gained meanwhile is reported as though this branch had
written it. `main...HEAD` is the change this branch introduces. Verified against a diverged
branch: two-dot picked up an unrelated file committed to main, three-dot did not
(`cli_e2e::a_named_base_excludes_what_the_base_gained`).

*Amended by D68* — this was one flag, `--diff [<REV>]`, until the optional value was found to
swallow a following path.

**Overlap, not containment.** A match spanning a changed line and some unchanged ones belongs
to the change. Containment would let a one-line edit inside a multi-line expression escape the
gate.

**A git failure is an error, not an empty scope.** "No lines changed" and "git could not tell
me" produce the same clean exit otherwise, and only one of them means the tree is clean — the
same principle as D57's held-back rules.

**The repository is the one being scanned, not the one you are standing in.** `rwr check
~/other/repo --diff` asks git in `~/other/repo`. Using the process's own directory silently
scoped a run to a diff from somewhere else entirely, which is a wrong answer that looks
right.

*Reverses if:* nothing plausible. The scope is opt-in and the unscoped run is unchanged.

## D60 - The whole-repo hierarchy constructors are deleted
**Decided**, as cleanup rather than design.

`Hierarchy::build`, `from_files`, `from_files_counted` and `descendants_of` were superseded by
`reachable_from` (D52) and had no caller outside a test. They are gone, with the test and the
three imports that went with them.

Worth recording only for how it surfaced: `clippy` had been reporting them as dead for some
time, and a cached lint result hid it until an unrelated new module forced a full re-lint. A
gate that passes from cache is not the same as a gate that passes.

## D61 - Chained receivers resolve only when the chain carries its own answer
**Decided by measurement, and the measurement is mostly a negative result.**

`Widget.new.foo` and `thing.dup.foo` resolve; `user.account.foo` does not, and will not.

Three measurements across rails, discourse and mastodon, each of which changed the plan:

1. **The bucket is not one problem.** Chained receivers are 15.8-27.4% of call sites, but
   `X.new` — the case worth building first on intuition — is **under 4% of chains**.
   Everything larger needs a method's return type.
2. **Return types are mostly not in the syntax.** Only **2.3-4.5%** of method definitions have
   a last expression that resolves to a class. **Seventy percent end in another call**, so an
   index built from syntax recurses into more unknowns. That is not an implementation gap; it
   is what dynamic typing means.
3. **A quarter of the bucket is spec DSL.** `expect(...)` alone is 20-25% of every chained
   receiver in both Rails applications. Nobody narrows a rename by an RSpec matcher, so the
   headline percentage overstates the reachable problem badly.

So: build the free part, and stop. Constructors and identity methods (`freeze`, `dup`,
`clone`, `itself`, `tap`) need no index, compose recursively, and cannot be wrong. `then` is
excluded because it returns the *block's* value, and `presence` because it may return nil —
both would have been plausible-looking bugs.

**What this settles about DESIGN.md §6.** The no-Sorbet path was required to stand alone.
It does, for receivers named directly — locals, ivars, constants, `self`, constructors — and
it does **not** reach chained receivers. Those need a type source or they stay residue. The
honest default is residue: rwr does not match them, does not rewrite them, and reports them,
which is under-matching in the safe direction.

*Reverses if:* a repository carries RBS signatures or Sorbet RBI, which state return types
outright and turn the 70% into data rather than inference. That is the case for ingesting
them — a much stronger one than "it would help receiver narrowing generally", which this
measurement refutes.

## D62 - Sorbet signatures are read as a return-type index; RBI is not parsed
**Decided.** This is the narrow case D61 said RBS/Sorbet ingestion would have to make, and
it makes it.

D61 measured that syntax states a method's return type for only 2-4% of definitions, leaving
chained receivers unreachable. A `sig` block states it outright. On graph_weaver — a real
Sorbet project, `srb tc` in its Makefile — **64% of signatures name a class rwr can use**,
against 3.9% inferable from that repo's syntax. Sixteen times the per-method yield.

**No Sorbet, no RBI parser, no new file format.** A signature is ordinary Ruby: `sig {
returns(String) }` is a method call with a block, already in the tree rwr parses. The whole
feature is reading a shape out of an AST that exists.

**It composes with what was already there.** `p = P.new; p.widget.display_name` resolves the
local from its constructor, then the signature of `P#widget`. And implicit self — the largest
slice of the chained bucket at 53-66% — needs no resolution at all, because the enclosing
class *is* the receiver.

**Partial by construction.** `T.untyped`, `T.any(...)` and `void` name no single class to
dispatch on, so they yield nothing rather than a guess. `T.nilable(X)` yields X, because a
value that reaches a call site is not nil there. `T::Array[Widget]` yields Array: the element
type is erased, and what dispatches is the collection.

**Cost when a repository has no signatures: none, and measured.** The prefilter is `sig `
with the trailing space — the bare word occurs inside "design", "assign" and "signature", and
filtering on it parsed 1,584 files of Discourse to find nothing, 46% of that run. With the
space, Discourse parses zero files. Five runs each: 159-164ms with the signature pass against
172-185ms without, i.e. **slightly faster with it on**, because the pass faults the mmapped
pages in parallel before the scan needs them. The profile's phase table over-attributes here,
and reading it alone would have said the feature cost 190ms.

**RBI files are out of scope for now.** `sorbet/rbi/gems/*.rbi` describes *dependencies*,
which would reach a different and larger class of receiver, and `.rbi` is parseable Ruby so
the same machinery would work. It is not built because nothing has asked for it yet, and
gem-typed receivers are a different rule population from a repository's own classes.

*Reverses if:* nothing about this one. The feature is pure upside — a repository without
signatures is unaffected, and a wrong signature narrows to a class that simply does not
match, which under-matches rather than mis-rewrites.

## D63 - Three structural cuts in the scan, and the ones not taken
**Decided**, and every number here is five warm runs on discourse (11,006 files, 39 MB).

The pack went **970 ms to 565 ms, −42%**, and a rename 312 ms to 272 ms. Nothing clever
happened; three pieces of redundant work were deleted.

**One parse per generation, not per rule.** The scan reparsed every candidate file once per
rule, whether or not the previous rule had changed a byte. Measured first, with eight rules
that matched *nothing*: **~85 ms per additional rule**. One parse now serves every rule until
a rule actually rewrites something, at which point the bytes have changed and a reparse is
owed. Marginal cost per rule halved to ~40 ms.

**A per-rule literal gate.** The prefilter decided whether to *read* a file, using the union
of the set's literals — and then every rule walked the whole tree of every file that any rule
wanted. For a ten-rule pack that is nine wasted walks per file. Each rule now checks its own
literals first. Ten rules: 1,440 ms to 1,173 ms.

**A copy and a syscall per file.** `original` was allocated, copied into `current`, and never
read again — a redundant copy of every candidate file, up to 39 MB a run. And
`path.canonicalize()` ran for all 11,006 files to serve `--diff`, which is usually off; it is
resolved lazily now. 643 ms to 565 ms.

### Rejected, with the numbers

**Hoisting the pattern reparse.** Each rule reparses its own tiny pattern per file — 56,000
parses for an eight-rule run. Microbenchmarked at **57 ms single-threaded**, so roughly 7 ms
of wall clock across eight threads. Sharing them would need thread-local storage of a
self-referential parse, since `ParseResult` borrows its source and is not `Sync`. Not worth
it for 7 ms.

**Memory-mapping `find`'s reads.** `find` uses `std::fs::read` where `check` maps, and
scaling.md records mapping as a 28% win — so this looked like an oversight. It is not.
Measured both ways on discourse, same build, same warmth: `fs::read` **170-179 ms**, mapped
**181-187 ms**. Mapping is ~3% *slower* here.

The reason is that the 28% came from *reuse*, not from mapping. `check` reads every source
once and three phases consume it — hierarchy, signatures, scan — so the mapping is amortised.
`find` touches each file exactly once, and then mmap and munmap are two syscalls and a page
fault where a read is one syscall into a buffer. A win that came from an access pattern does
not transfer to a different access pattern, however similar the code looks.

**A multi-pattern matcher** — walk the tree once and match every rule simultaneously. This is
the only thing left that would attack the remaining `scan` cost, which is now genuine
matching work. Rejected on architecture rather than effort: it taxes the file rules are added
to, which is the highest-traffic edit in the project, and it buys a run already at 38% of
its budget.

### The targets are not lowered to match

Q7's ceilings stay at 250 ms / 500 ms / 1.5 s. A budget that tracks the best number ever
measured is not a budget — it is a ratchet that turns every ordinary regression into a
failure, and it would have to be re-derived after every change of this kind.

*Reverses if:* the pack grows enough that per-rule tree walks dominate again, at which point
the multi-pattern matcher stops being premature and its cost to rule-authoring is worth
paying.

## D64 - `contains:` relates a sub-pattern to the outer bindings
**Decided.** A pattern matches a *shape*; this is how a rule says "and somewhere inside it,
this".

```yaml
match: $R.each { |$X| $B }
where:
  $B: { contains: $X.$ASSOC.$FIELD }
```

**The agreement is the feature.** A containment that ignored the outer bindings would match
any call on anything inside the block, which is nearly vacuous. Metavariables shared between
the two patterns must refer to the same thing, so `other.customer.name` inside
`orders.each { |order| ... }` does *not* match.

**Agreement is by identifier where both bindings name one, and by span otherwise.** This is
the part that took a wrong turn first: in `$R.each { |$X| $B }` the outer `$X` binds the
block's **parameter** and the inner one binds a **read** of it. Same variable, different
nodes, different spans — and comparing spans said they disagreed, so nothing matched at all.

**What it costs.** Sub-patterns are prepared once per rule rather than per candidate match,
since preparing is a parse-and-retry loop. The search itself runs only on the bound subtree
of a match that already passed everything else.

**What it unlocked, measured.** `performance/possible-n-plus-one` narrows discourse's 637
`each`/`map` blocks to **51 candidates** — a 92% cut, and reviewable. About half are real:
`posts.map { |p| p.topic.category_id }` is a textbook N+1. The rest are conversion chains
like `names.map { |n| n.to_s.downcase }`, which the shape cannot distinguish.

This corrects an estimate made before building it, that a containment-only N+1 rule would
"flag every block" and be useless. Requiring two levels of reach *through the block's own
parameter* turns out to be a strong filter. The estimate was wrong in the useful direction,
and only measuring said so.

**What it still cannot do.** N+1 proper needs "and nothing upstream eager-loaded this", a
negative condition over a scope rwr does not analyse — usually a different method entirely.
So the rule ships as a lint that finds *candidates*, and says so in its own description.

**A note on YAML, because it bit.** `{ contains: $X.$ASSOC.$FIELD }` reads better than three
indented lines and needs no quotes -- `$` and `.` are ordinary characters. But inside a flow
mapping, `,` `{` `}` `[` and `]` belong to YAML, so `{ contains: log($A, $B) }` arrives as
`log($A`. That pattern then fails to prepare, and `contained()` was *swallowing* the failure
with `.ok()?` -- leaving a rule that ran clean, matched nothing, and said nothing. It now
refuses at exit 3 and names the cause.

The general lesson is not about YAML. A constraint that cannot be built must not degrade into
a constraint that is never satisfied, because the two are indistinguishable from the output
and only one of them is a bug.

*Reverses if:* nothing. It is opt-in per constraint and costs nothing where unused.

## D65 - ERB is parsed by stitching its tags into one Ruby program
**Decided**, after measuring the alternative first.

**A tag on its own usually does not parse.** `<% posts.each do |p| %>` opens a block that
`<% end %>` closes three tags later. Across discourse and mastodon, **60%** of tag bodies
parse standalone; the failures are exactly the control-flow halves.

**Stitched in document order they are one program, and 95% of templates parse** — 159 of
168. That is the measurement the whole feature rests on, and it was taken before any of it
was built.

So: extract each tag body with its template byte range, join them, parse the result, and keep
a fragment map. Matching, residue and rewriting all run on the Ruby; every edit is mapped
back through the map to the template it came from.

**An edit spanning two tags is refused.** Those bytes include template text that is not Ruby
at all, and splicing through them would put HTML inside an expression. It is rare — an
expression seldom spans two tags — and refusing is the only safe answer.

**A template that does not stitch falls back to the text search**, which already existed and
already says it is weaker. On discourse 114 of 124 templates parse and 10 fall back. On
mastodon almost everything falls back, because mastodon is Haml.

**Measured payoff.** For a rename of `User#name` over discourse's `app/`: 53 occurrences
found by *parsing* templates against 49 by text. More than half the template account is now
real evidence rather than a string that looked right — and the parsed half can be rewritten,
which the text half never could.

**Naive by design.** The template pass re-translates per rule rather than threading a live
map through the rewrite loop. There are a few hundred templates and they are small; the
simple version costs nothing worth the complexity.

*Reverses if:* nothing here. Haml wants the same treatment and is a separate job — its Ruby
is line-oriented rather than delimited, so extraction is a different problem.

## D66 - An empty template is a deletion, and deletion means the unit
**Decided.** `-d`, `-r ''` and `rewrite: ''` are three spellings of one mechanism, because
one mechanism is easier to trust than three.

**Deletion means the *unit*, not the node.** Removing only a method's own bytes leaves its
doc comment stranded above a blank gap, which is not what anyone means by deleting a method.
The unit is the match, the comment lines written directly above it, its own line, and one of
the blank lines that separated it from its neighbours -- so the survivors stay spaced exactly
as they were. `unit_for` already computed this for sequence sorting (D35); deletion reuses it.

A comment block belonging to the *neighbour* is not taken: the walk upward stops at the first
line that is not a comment, and a blank line is not a comment.

**A partial match refuses.** This is the important half, and the naive version got it wrong
in the worst possible way. Deleting `a.display_name` from `x = a.display_name` leaves `x = `,
which then swallows the line below into `x =   y = 2` -- **valid Ruby, wholly wrong, exit 0**.
That is the clean-confident-wrong failure this whole design exists to prevent, and it was
sitting in the first working version until a test caught it.

So a deletion whose match does not occupy whole lines of its own is refused
(`Refusal::PartialDeletion`, exit 5), and the file is left alone. Pinned by
`cli_e2e::delete_refuses_a_partial_match`.

*Reverses if:* nothing. Deleting a sub-expression has no correct answer to fall back on --
there is no "smaller" thing to do than refuse.

## D67 - Comments are reported, never matched or rewritten
**Decided.** The question was whether rwr can match and rewrite comments. It reports them
instead, and the distinction is the product.

**The gap was real.** Renaming `Account#display_name` left `# Returns the display_name for
the header.` behind and rwr said *nothing at all* — comments are not in Prism's tree, so
neither the matcher nor the residue pass saw them. A report that claims to list what was left
over was silently omitting a whole category.

Fixed: comments get their own pass, scoped by position. A comment has no place in the tree to
read its lexical scope from, so the innermost class or module whose span contains it supplies
one — without that, every comment would escape the class scoping that keeps the rest of the
report readable. On discourse, renaming `Topic#slug` finds 3 comment occurrences, all
genuinely stale; renaming `User#name` finds none, because the scoping holds.

**Matching stays off, deliberately, and this is the whole thesis.** `rwr 'return nil'` finds
22 sites on rails where ripgrep reports 40, because comments and strings are not code. A
matcher that read comments would be ripgrep with extra steps.

**Rewriting stays off for a different reason: it would be a guess.** A name in prose may be a
reference, an example, a changelog entry, or an ordinary English word. `# See also
#display_name on Company` is about a *different class* and must not change; `# Returns the
display_name` must. Nothing in the text distinguishes them, and rwr does not guess.

Reporting is a fact and rewriting would be a guess, so rwr reports — and having named the
exact lines, the manual fix is cheap. That is the same trade the whole residue design makes.

*Reverses if:* never for matching. For rewriting, only if a rule could express *which* prose
mentions it means, which is a natural-language problem rather than a structural one.

## D68 - `--diff` and `--since` are two flags, and a path may name its lines
**Decided.**

D59 shipped one flag with an optional value, `--diff [<REV>]`. A flag that *may* take a value
consumes the next token whatever it is, so `rwr check all --diff app/` built the revision range
`app/...` and failed inside git. The bug was reported against the opposite ordering; the order
the `--help` usage line implies (`--diff` last) always worked, which is a good illustration of
how badly an optional-value flag reads.

**The fix is to remove the optionality, not to disambiguate it.** `--diff` now takes no value
and `--since REV` requires one. A flag taking zero arguments cannot consume the next token; one
taking exactly one always does. No token's role depends on what it looks like, so nothing has to
probe the filesystem to decide — which is the guess D31 already refused for `-r`. Requiring
`--diff=REV` would have worked too, but two flags keep the space-separated spelling and make the
combination below expressible.

**Together they mean the merge base against the working tree.** `--since main` is
commit-to-commit, so uncommitted work is silently outside it — measured, not assumed:
`git diff --unified=0 main...` omits an unstaged line that `git diff $(git merge-base main HEAD)`
reports. `--since main --diff` is "what this branch introduces, including what I have not
committed", which is what a human at a terminal usually means and which one flag could not say.

**Untracked files are in the uncommitted scope.** `git diff` cannot see a file git is not
tracking, so a brand-new file full of violations reported as a clean tree — the pre-commit case
failing exactly when a change is largest. `git ls-files --others --exclude-standard` folds them
in, every line of a new file being a new line.

**A path that does not exist is an error.** `rwr check all app/typo` exited 0 and reported a
clean tree; in CI that is a green gate that checked no files at all. This is the same vacuous
pass that ruled out inferring a default branch (below), and it is now exit 2.

**`PATH:N` and `PATH:N-M` name lines directly.** The same scope git produces, supplied by hand.
rwr already *prints* `file:line`, so an output line pastes back in as an input. A filename that
genuinely ends in `:3` is the fallback reading rather than a coin flip, and when neither reading
exists the error names both. Mixing a bare path with a scoped one is refused: a `Changed` covers
the files it names and nothing else, so `app/ lib/x.rb:3` would check three lines and call `app/`
clean. Combining it with `--diff`/`--since` is refused for the same reason — two answers to which
lines to check, and picking one silently is how a scoped run becomes an unscoped one.

**Rejected: inferring the default branch.** `--diff` could have defaulted to
`origin/HEAD...HEAD`, saving the four characters of `main`. Measured: `origin/HEAD` is present in
all seven local repos here, and *absent* after a `--single-branch` clone — which is
`actions/checkout`'s shape, so it fails in CI, the one place it would matter. CI does not need it
either, since a pull-request gate is handed `$GITHUB_BASE_REF`, and that is *more* correct than
the default branch for a PR targeting a release branch or a stacked PR. The fallbacks are worse:
guessing `main`-then-`master` picks silently wrong in a repo that has both — a wrong scope with a
clean exit, which is the failure this decision spends its length avoiding. It also has nowhere to
live: hanging it off bare `--diff` would turn a pre-commit hook's scope from "the three lines I am
about to commit" into "everything across ten commits", silently.

*Reverses if:* `--since main` proves to be typed often enough to want a spelling that infers the
base — in which case it wants its own flag, taking no value, reading `origin/HEAD` only and
erring when that is absent rather than guessing.

## D69 - Fixtures live in the rule file, and `rwr test` runs them
**Decided.**

A hand-written rule had nothing pinning its behaviour: the shipped pack changed shape between
two releases, which is reasonable for an actively developed tool, but it means a user's own rule
can start doing something different after an upgrade and say nothing. A rule now carries
`tests:` -- an input snippet and what should happen to it -- and `rwr test` checks them.

**A verb, not a flag on `check`.** The object is the *rule*, not a codebase: `find`, `check` and
`rewrite` all take (rule, codebase) and act on code. As `check --test` it would have had to
reject the PATH positional plus `-p`, `--include-vendored`, `--diff`, `--since`, `--ruby`,
`--unsafe`, `-r` and `-d` -- two thirds of the surface -- because fixtures walk nothing. A flag
that invalidates most of its siblings is a verb wearing a flag's clothes, and D29 already
refused the same shape pointing the other way (`--write`/`--dry-run` on one verb). Overloading
`rwr check rule.yml` was ruled out separately: with no PATH it already means "check the current
directory", so fixtures would have silently changed an existing command's meaning.

**This is not a per-rule option (D57).** An option parameterizes behaviour and needs defaults
and a merge order; a fixture parameterizes nothing and cannot change what the rule does to any
file. It is a falsifiable claim *about* the declared behaviour, beside the declaration for the
same reason `unsafe:`'s reason is. D57 is strengthened rather than bent: the rule is now also
its own proof.

**A case that asserts nothing is unrepresentable.** `input:` alone, `output:` together with
`unchanged:`, `unchanged: false`, and `finds:` on a set that rewrites are all refused at load
(exit 3). The whole value of a fixture suite is lost the moment a case can pass without
claiming anything.

**An unparseable snippet fails; it is not skipped.** In `check`, skipping a file that does not
parse is the contract. Here the identical behaviour would make a typo'd snippet -- the
commonest fixture bug there is -- pass every negative assertion vacuously. This is the same
family as the nonexistent path that exited 0 (D68): a green result that checked nothing.

**A set with no fixtures exits 2 rather than passing.** For the same reason, and the untested
rules are named rather than counted so a partly-covered pack cannot read like a covered one.

**Gates do not apply.** `unsafe:` holdback and `ruby:` version checks are application-time
policy about *whether* to run a rule; a fixture tests what it does. Without this, most of the
interesting pack would be untestable without a flag, and the flag would be D57's disease.

**A case runs the whole document's rule set**, not the single rule its `tests:` key sits on.
D54 makes the file the unit of identity, and a `method:`/`rename:` pair expands to several rules
that only mean anything together; per-rule execution would test a mode that never occurs in
production, which is the drift this feature exists to kill.

*Reverses if:* fixtures prove unable to express the context-dependent rules that matter --
`type:` narrowing and `scope: inside:` both need the snippet to carry a class or an assignment,
and if authors find themselves unable to write those snippets, the answer is a declared context
block rather than abandoning the feature. Deliberately not built yet: two cases is not a
pattern.

## D70 - `-e` reports why a candidate was declined
**Decided.**

`--explain`'s own help had said it explains "which constraint rejected a candidate" since it
shipped, and it did not: a site declined by `type: Widget` produced no output at all. The
matcher computed the reason -- `Verdict::BadBinding` drives the rebind loop (Q13) -- and threw
it away once rebinding was exhausted. The work was surfacing it, not deriving it.

**A scoped `-e`, not a new verb.** `rwr check r.yml app.rb:5 -e` is the rule-authoring loop, and
both halves already existed: line addressing from D68 and a global `-e`. A `rwr why` verb would
duplicate scoping, rule loading and output framing to deliver what two orthogonal features
compose into.

**Behind the flag, deliberately, and this is not a breach of principle 3.** Residue reports
unconditionally because it is the account of what rwr *could not see*. A rejection is the
opposite: a site the rule correctly refused, and the report says so. Detail about correct
behaviour is debugging; the blind-spot account stays unconditional.

**The distinction worth the whole feature is `Type { resolved: None }`.** Receiver narrowing is
conservative -- a receiver rwr cannot resolve does not match -- which makes "could not resolve
this at all" and "resolved to the wrong class" different problems with different fixes. They
were indistinguishable, and the first is the most-documented source of surprise in the design.
A third case joins them: resolved to the right class, but as the other of instance/class than
the rule means.

**A rule bug is no longer a scope miss.** `verdict()` returned `WrongScope` for a constraint
naming an unbound capture and for a `contains:` that failed to prepare. Both are pre-validated
by `Rule::validate`, so reaching them means the pre-validation has a hole -- and reporting them
as a scope miss sent an author looking at their `scope:` for a typo'd `where:` key. They are
`Verdict::Bug` now, reported as constraint `rule-bug`.

**Rejections are buffered per candidate and discarded when one binding works.** A node may admit
several bindings and only a later one may satisfy the rule; a site that ultimately matched has
nothing to explain, and reporting the attempts would make successful backtracking look like
failure.

**Report schema 3.** `rejections` is absent without `-e` rather than empty: nobody asking is not
the same as nothing being declined, which is the same present-versus-absent convention `residue`
already uses. One schema number across the CLI contract rather than one per command, since the
field names are shared.

*Reverses if:* a repo-wide `-e` proves too loud to use -- the obvious next move would be a cap
like residue's, but rejections only exist where a pattern matched structurally and a constraint
refused, so the volume is bounded by how narrow the rule already is.

## D71 - An accepted finding is one concept; a predicate is not a suppression
**Decided.**

Two requests -- a baseline file and inline directives -- are two spellings of one thing, an
*acknowledged* finding: "this is real, I have seen it, stop failing on it." They share one
engine, one report shape, and one rule: **a suppression whose finding is gone is itself a
finding.** A mechanism that can silence a run must never be able to silence itself, which is
exactly how RuboCop's todo file became a permanent monument.

A `where:` predicate is deliberately outside this system. It says the finding was *wrong* -- the
rule over-matched -- so a predicate-excluded site is not counted, not reported, and not debt.
The teaching rule, one sentence: **narrow before you suppress; if you would have to explain why
the finding is wrong, fix the rule.**

Four ways a finding can stop failing a run, each canonical for one situation:

| You believe | Mechanism | Lifetime |
|---|---|---|
| The finding is wrong | a `where:` predicate (`name_not:`) | permanent, portable to every repo |
| Touched code must be clean | `--diff` / `--since` | per run, no state |
| Existing stock accepted, new ones not | a baseline | temporary, drains |
| This one site is a deliberate exception | `# rwr:ignore` | permanent, visible at the site, reviewed with the code |

Cutting any one pushes its case onto a mechanism that serves it badly: without the baseline,
adoption means a 2,000-line cleanup PR; without directives, permanent exceptions live in the
baseline, invisible at the site and guaranteeing it never drains.

*Reverses if:* the two suppression surfaces develop genuinely different needs -- per-site expiry
dates, say -- at which point the shared engine is the wrong unification and they should be split
honestly rather than parameterized.

## D72 - Directives are node-scoped, rule-named, and never touch residue
**Decided.** Amends D67.

`# rwr:ignore <rule-id>`, trailing on a line or leading above one.

**D67 said comments are never matched or rewritten, and it stands.** Its prohibitions are on
treating comments as *code* (matching them is ripgrep with extra steps) and on rewriting prose
(which mention is meant is unknowable). A directive addressed to rwr is a third category:
instructions, not prose. They are read, never matched, never rewritten, and never counted as
residue. **Directives suppress findings and edits; they cannot touch the residue report**, which
is the account of blind spots and is the product.

**The unit is the node, not the line.** A directive attaches to its own line when code precedes
it and otherwise to the next line carrying code (comment lines skipped so it reaches past a doc
block; a blank line ends the search, per D35's adjacency). It then covers the *outermost node
starting on that line*. Line scoping was implemented first and was wrong in the common case: a
directive above `def three` covered only the signature while the violation sat in the body, so
it suppressed nothing and reported itself stale. Line scoping in a structural tool leaves the
whole advantage on the table -- rwr has the tree, and no line-based tool can offer this.

**No block form.** A `disable`/`enable` pair with a forgotten terminator silently suppresses the
rest of a file, which is the invisible blind spot this tool exists to refuse. A mandatory
terminator only converts that into "hope the reviewer notices it is 400 lines down". Node
scoping gets the useful part of a block with nothing to forget to close.

**Rule ids are mandatory.** A bare `# rwr:ignore` is a blanket blind spot that no staleness
check can audit, so it is reported as malformed and suppresses nothing. A directive naming a
rule outside the current run is left alone: it belongs to another pack, and it is neither
honoured nor stale. A bare-pattern run has no id and is not covered -- suppression is for
standing enforcement, and an ad-hoc query is exploration by someone who typed the pattern
seconds ago.

**`rewrite` honours directives identically to `check`**, because `check` is the preview of
`rewrite` (D29) and a preview that disagreed would be a lie. Draining is spelled by fixing the
code, not by a flag. Deletion (D66) takes a directive with it for free: a trailing one is on the
match's own line and a leading one is a comment directly above, both already inside the unit.

**Stale directives are reported, not failed.** The count and the sites print unconditionally,
never behind `-e`. They do not set the exit code: a stale directive cannot keep silencing
anything -- its finding is already gone -- so what remains is tidying, and tidying should not
block a commit. The recorded tradeoff is that nothing then *forces* the drain, which is the
mechanism by which todo files calcify; the self-expiring scope means a stale directive is inert
rather than dangerous, so the failure mode is clutter rather than silence.

*Reverses if:* real packs show a legitimate need for a wider-than-node exception that neither a
predicate nor a baseline serves -- the evidence would be users stacking many identical
directives in one file -- or if stale directives accumulate in real repos, in which case failing
on them is a one-line change and should be an amendment here rather than a flag.

## D73 - Derived fields are not syntax
**Decided.** Amends D36.

D36 made equality "variant + atoms + children", and `generated::atoms` emitted every `constant`
field Prism exposes. `locals` is one of those — the local-variable symbol table Prism attaches
to each node that opens a scope — and it is *derived from* the body rather than written by
anyone. Comparing it meant a pattern matched only bodies whose local set was identical to its
own, so `def foo; $B; end` matched a method with no locals and declined every method that
assigned one. The flagship one-line rename worked on one-liners and silently reported real
methods as residue.

**The failure was invisible in every direction that usually catches things.** The rename
reported the definition it declined, so it was honest; the testbed scored 7 of 7 because its
`Account#display_name` happened to be a single expression; and the pack's fixtures were all
one-liners. A corpus written from the Ruby side still missed it, because "a method with a
variable in it" is too ordinary to think of as an edge case.

Derived fields are excluded at the generator (`script/gen-compare.py`, `DERIVED`), not filtered
at the comparison site, so the exclusion is visible in the generated source rather than hidden
behind a runtime check.

**A lone metavariable in a body position binds the whole body.** Not an exception to D32's "one
node": a Ruby body holds a statements sequence and that sequence *is* one node. Comparing the
two sequences child by child was the second half of the same bug.

Measured before shipping, across ~/code/lib/ruby: 1,051 sites to 1,054, with nothing lost. A
correctness fix to a matcher should widen strictly and slightly; a large delta would have meant
the fix was wrong.

*Reverses if:* a field in `DERIVED` turns out to carry meaning a pattern should be able to
match. The test to write first would be the one showing two programs that differ only in that
field and should not be equal.

## D74 - Singleton context is inherited, and a subclass definition is the report's business
**Decided.**

Two faults from one corpus, both in the receiver narrowing that is rwr's headline claim.

**`class << self` puts its whole body in singleton context.** `walk` computed the flag as "does
this `def` have an explicit receiver", which cleared it on entry to every method *inside* a
singleton class. The definition rule additionally left `scope.singleton` unset. Together an
instance rename declined the `class << self` definition (correct, once the rule constrained it)
and then rewrote the call sites inside its sibling methods (wrong) -- introducing a
`NoMethodError` into a class method the rule had just correctly refused to touch. The pairing is
the worst available: a wrong rewrite, in the one place the tool had demonstrated it knew better.

A `.` rename had the mirror fault, missing the definition entirely, because `def self.name` is
not the only spelling of a class method. The expansion now carries both.

**`subclasses: true` was honoured by the matcher and ignored by the report.** `residue::scoped_to`
kept a `Definition` only when its lexical scope literally contained the anchor class, so an
override in a subclass -- the one occurrence *guaranteed* to break, since a rename that misses it
ships a `NoMethodError` -- was dropped from the account. Definitions in descendants are now kept.

*Reverses if:* nothing plausible for the singleton half. For the subclass half, if descendant
definitions prove noisy on a wide hierarchy, the answer is to rank them rather than drop them --
an override the rename did not reach is the highest-value line in the whole report.

## D75 - The testbed scores on a ratchet
**Decided.**

The corpus states what *should* happen, derived from Ruby semantics, so it necessarily describes
behaviour the tool does not yet have -- the original scored 2 of 7 and that was the point (see
`testbed/README.md`). Asserting perfection makes the suite permanently red and blocks every
commit; deleting the failing cases throws away the only honest measurement there is.

So the scorers pin today's number and fail on regression, and the assertion that recall
*improved* is also a failure -- it says "lower the constant to lock it in". A gap that quietly
closes and is not recorded can quietly reopen.

Every outstanding gap is named in the test, with its cause. The current ones share a root:
`scoped_to` compares scope names literally and the hierarchy carries `class X < Y` links only, so
anything a module contributes -- a concern's `included do`, a `prepend`ed override, a `refine`
block -- has an enclosing scope that never equals the anchor class. In Rails a large share of a
model's methods live in concerns.

*Reverses if:* the constants stop being lowered. A ratchet nobody tightens is a todo file with
better manners, which is exactly what D70 was written to avoid.

## D76 - The hierarchy records mixins, not just superclasses
**Decided.**

`class X < Y` was the only relation rwr knew, which answers "what is this class" and not "where
else are its methods written". In Rails the second question matters more: a concern, a
`prepend`ed patch and a refinement all put a method on a class without either file naming the
other in a superclass line. Every one of them was dropped from the residue report, and the
report said nothing about dropping them.

`Hierarchy` now carries `include`/`prepend`/`extend`/`refine` edges alongside superclass links,
and `residue::scoped_to` asks `contributes_to` as well as `descends_from`. `refine C do` is
recorded inverted -- the argument is the host, the enclosing module the contributor -- because
that is the direction the relation actually runs.

**The prefilter that fed it was the real bug, and it had been hiding behind a comment.**
`reachable_from` only parsed a file containing both `class` and `<`, which was exact while the
hierarchy held superclass links and wrong the moment it held mixin edges: `Account.prepend(Audit)`
contains neither word. The testbed's own patches file passed anyway -- because a *prose comment*
in it reads "Neither module names Account in a `class X < Y` line". The sentence asserting the
module never writes `class X < Y` was the literal reason the file was admitted to the scan.

That is worth recording beyond the fix. The corpus was green, the feature was broken, and the
thing bridging them was English. A ground-truth corpus can be right about what *should* happen
and still pass for a reason unrelated to the mechanism under test -- so when a fixture passes
and a minimal reconstruction of it does not, the reconstruction is the honest signal.

*Reverses if:* mixin edges prove noisy enough to bury the report on a wide corpus. Measured on
discourse the widening cost 10 ms of hierarchy time and changed no rewrite -- `contributes_to`
is consumed only by the report, and the matcher still uses `descends_from` alone.

## D77 - A Ruby file that does not parse is named
**Decided.**

Skipping an unparseable file is the contract (D-nn) and saying nothing about it is not.
Templates were counted in `templates_skipped` from the day they were searched; Ruby that failed
to parse had no counterpart, so a generator template under `lib/templates/` with a `.rb`
extension -- or any broken file -- contributed zero matches, zero residue, and no mention, with
the run exiting 0. A filter that over-fires is indistinguishable from a quiet corpus, which is
the failure class this codebase produces; this was that failure with the reporting half simply
absent.

**Only files that could have contributed are listed.** A broken file with no mention of the
anchor is declined by the prefilter before any parse is attempted, and naming those would bury
the report under every unparseable file in a repository -- which is the same lie in the other
direction.

*Reverses if:* nothing plausible. The count is small by construction and the field is additive.

## D78 - An active refinement refuses the file
**Decided.**

`using M` makes a refinement of the target class intercept exactly the call a rename would
rewrite. Rewriting it routes around the refinement: the class gains the new name, the refinement
keeps defining the old one, and the call silently stops being refined. No exception, nothing
that fails to parse, and `verify` passes -- the same "working code, changed behaviour" shape as
a rename colliding with a local, and the only unrecoverable outcome rwr has.

**Refuse the file, do not downgrade the rewrite to a report.** A refusal is loud, leaves every
byte alone, and costs a round trip; the alternative silently produces a file that runs and is
wrong. This is principle 1 applied where it matters most.

**Scoped to activation, not to existence.** A refinement nobody `using`s is inert, so a call
really does dispatch to the class and renaming it is correct -- verified both ways. Refusing on
the mere presence of `refine C do` anywhere would decline work that is perfectly safe, which is
the cost of getting the scope wrong in the other direction.

*Reverses if:* per-file granularity proves too coarse. `using` inside a class or module body
scopes to that body, so a file could in principle be part-refined; rwr refuses the whole file
today. Narrowing that is a precision improvement, not a correctness one.

## D79 - A rewrite that would shadow a local refuses
**Decided.**

Renaming `Account#display_name` to `full_name` in a scope that already had a `full_name` local
produced `full_name = full_name if profile?`. Valid Ruby, quietly evaluating to the local's
current value, passing the reparse check, reported by nothing. Every other defect this project
has found was a miss, a refusal, or noise; this one shipped code that ran and was wrong.

**The check is general, not rename-specific.** A rule's template may introduce identifiers its
pattern did not have; if one of them is already a local where the edit lands, the rewrite
collides. Comparing `prefilter::required` over pattern and template gives the introduced set for
free, so any rule gets this, not just `method:`/`rename:`.

**Prism's local table answers it exactly.** D73 removed `locals` from *equality*, because it is
derived rather than written and comparing it made a pattern match only bodies whose locals
matched its own. It is still the right answer to a different question -- "is this name already
taken here" -- and using it for that is not in tension with D73.

**Per scope, not per file.** Locals belong to the method that declares them. A file-level check
would be safe and far too blunt: the target of a rename is usually a short ordinary name, and
refusing every file that happens to contain it elsewhere would decline most of the work. Verified
both directions, and measured across ~/code/lib/ruby: zero refusals, 532 files and 1,054 sites
unchanged.

*Reverses if:* nothing plausible. The alternative to refusing is writing code that runs and is
wrong.

## D80 - The diff carries the same body rule as the matcher
**Decided.**

D73 let a body-position metavariable bind a whole body, and stopped at the matcher. That was
half a rule. `rewrite::structural_diff` compares the pattern against the template *and* the
target, and it had its own discriminant check -- so a `def` carrying `rescue`, whose body is a
`BeginNode` rather than a `StatementsNode`, was called diverged there even once the matcher
accepted it.

The consequence was worse than the original miss. The diff localized to the whole `def` and
emitted a second, wider edit alongside the correct one; the planner drops a contained edit
(D15), so the correct edit was dropped and the wider one applied -- and its text was the
original, so the file did not change while the run reported "rewrote 1 site" and exit 4, asking
to be run again forever. A silent miss became a loud lie.

**A matching rule that lives in two places must be written in both.** The matcher and the diff
each decide what "the same subtree" means, and they were allowed to disagree. Both now recognise
a lone body-position placeholder; the comments point at each other, because the failure mode
when they drift is not a failed match but a corrupt plan.

**Found by fixing the matcher and measuring the result rather than assuming it.** The first
attempt at D73's twin was reverted precisely because the rewrite path misbehaved, and the gap
sat recorded for a day until the splice side was understood. Reverting a fix that half-worked
was the right call: a reported miss is strictly better than a retry loop that lies about its
work.

*Reverses if:* nothing plausible.

## D81 - The testbed scores per file, not by total
**Decided.**

`every_site_that_must_change_changed` compared the total rewrite count against the number of
`GT:rewrite` markers, and passed at sixteen while two errors cancelled: a class nested inside
`class Account` was being rewritten as though it were Account (two sites it should not have),
and two definitions were being declined -- an arity-drifted override and a `rescue`-bearing body.
Sixteen expected, sixteen counted, both halves wrong.

A total is the one number that can be right while nothing else is. The check is per file now,
with the two known divergences named in the test rather than absorbed into a sum.

*Reverses if:* nothing plausible. Per-site would be better still; per-file is what the report's
shape supports today.

## D82 - A class is its qualified name, and nesting is not membership
**Decided.**

`scope: inside: Account` matched if *any* enclosing scope was called `Account`, and a scope name
was Prism's `name()` -- the last segment. Two faults from one shortcut:

- `class Account; class Row` declares `Account::Row`, a different class that does not inherit
  from `Account`. Its code was rewritten as though it were Account's. So was
  `module Billing; class Account`, which shares nothing with the top-level `Account` but a word.
- `class Account::Exporter` came back as `Exporter`, so nothing connected it to `Account` and a
  rule naming either spelling missed it.

A scope entry is now the constant *path*, and `inside:` compares against the innermost class's
fully-qualified name. `inside: Billing::Account` reaches the class it names; `inside: Account`
does not.

**A singleton body stays transparent.** `class << self` opens a context, not a class, so the
enclosing class is still the one that counts -- otherwise a class-method rename would lose every
definition written that way.

**Both directions were errors, which is what showed the question was never decided.** A rule that
over-matched *and* under-matched from the same shortcut is not a tuning problem; it is a missing
definition of what a class is.

*Reverses if:* users writing `inside: Account` inside a namespaced app expect it to mean
`Billing::Account`, the way Ruby's own constant lookup would. That is a real argument, and the
answer would be lexical resolution rather than a suffix match -- guessing which `Account` was
meant is exactly what this decision removes.

## D83 - `(*$P)` means any parameter list, including none
**Decided.**

An override whose arity has drifted from its parent's -- `def display_name(format = :long)` over
a zero-arity parent -- is ordinary legacy inheritance and was unmatchable by any spelling. A
definition pattern with no parameter list matched only a definition that had none; `def
foo(*$P)` matched nothing, because in a *parameter* position `*$P` is a real Ruby rest parameter
rather than the sequence placeholder it is in an argument list, so the splat machinery never
applied.

A lone rest-placeholder now means "any parameter list", and absorbs an absent one -- Prism gives
a zero-arity `def` no `ParametersNode` at all, so the pattern carries a child the target lacks.
The rename expansion uses it, which turns the one occurrence *guaranteed* to break into a
rewrite rather than a residue line.

**Three places had to agree, and getting two of them right produced a wrong rewrite.** The
matcher binds it; `match_children` lets it absorb nothing; `rewrite::align` carries it across so
the diff stays minimal. With only the first two, the zero-arity case fell through to whole-node
re-rendering and emitted `def full_name()` with the body reflowed. With the matcher rule left
unconstrained, it bound the `self` receiver of `def self.foo` and an *instance* rename renamed a
class method -- the one thing receiver narrowing exists to prevent, reintroduced by a fix.

Both were caught by the testbed within a minute of being written, which is the argument for
ground truth that scores per file: the second would have been invisible in a total.

*Reverses if:* nothing plausible. The spelling was already the one users would guess.

## D84 - A constant list of symbols is a name table
**Decided.**

`COLUMNS = %i[display_name email].freeze`, read back through `public_send`, is the most ordinary
dynamic reach a legacy exporter has, and it is stated entirely in literals -- nothing about it
needs inference. It was dropped from the report because residue labels a symbol by the call it
is an argument to, and these are an argument to nothing: the array is `freeze`'s receiver, or the
constant write's value.

A constant write now labels its own subtree, so those symbols are a reach like any other symbol
handed to something.

**Measured before shipping, because a heuristic that widens the report is a precision cost.** On
discourse's `app/`, a real rename went from 832 residue entries to 840 -- eight more, one
percent -- and it closed the last recall gap on the testbed. A gap worth one percent.

*Reverses if:* the ratio goes the other way on a corpus with large constant tables of
non-method symbols. The measurement is cheap to repeat and the rule is one match arm.

## D85 - `send` with a literal name is a call; with a computed one it is a caveat
**Decided.**

`account.send(:display_name)` was reported rather than rewritten, on the reasoning that dynamic
dispatch cannot be proved. That reasoning is right about the *name* and wrong about this case:
the name is a literal sitting in the source, and the only open question is which class receives
it -- which is exactly the question receiver narrowing already answers for `account.display_name`
two lines away. Declining it was a limitation, not a judgement, and it left the user to hand-edit
a call rwr could prove.

The rename now covers `send`, `public_send`, `__send__`, `try` and `try!`, with a symbol or a
string, under the same receiver constraint as an ordinary call. `unknown.send(:display_name)` does
not resolve and is reported, exactly as `unknown.display_name` would be.

**A corpus fixture had recorded the limitation as its expected output.**
`corpus/006-metaprogramming-residue` asserted that the `send` stays untouched -- written when it
had to. Its receiver is an ivar assigned from a constructor in the same file, and the plain call
beside it *was* being renamed, so the expectation was that rwr should ship a `NoMethodError`. The
fixture is updated and its notes now say why, because a fixture written after the behaviour is
how a limitation becomes a specification.

**The computed case gets a `Dynamic` residue context.** `send("display_#{attr}")` names nothing
rwr can enumerate, so this is not an occurrence of the anchor -- it is a location where the
completeness claim does not hold, which is the same product as the rest of the report.

**Scoped to the class the rule is about, and that scoping is the whole feature.** A dispatcher
appears in nearly every class; a computed name in an unrelated one says nothing about this
rename. Measured on discourse: unscoped it was 115 entries of 955, twelve percent of the report;
scoped to the target class, its descendants and the modules mixed into it, three. A caveat that
buries the report is not a caveat.

*Reverses if:* the dispatcher list proves too narrow or too wide. It is a closed set, which is
the point -- guessing from an interpolation's prefix whether it could produce the anchor is the
kind of inference this tool refuses.

## D86 - A definition's owner is its receiver, not its lexical nesting
**Decided.**

Ruby decides which class a definition attaches to from the *receiver*; lexical nesting only
supplies a namespace. rwr read the owner off the nesting, which is wrong three separate ways, and
each way is silent -- the run completes, the count looks plausible, and the wrong methods moved.

| written | owner | rwr said |
|---|---|---|
| `class ::Bar` inside `module Foo` | `Bar` -- `::` resets to top level | `Foo::Bar` |
| `class << Foo` inside `class Bar` | `Foo`'s singleton | `Bar` |
| `def Foo.bar` inside `class Bar` | `Foo`'s singleton | `Bar` |

The middle row is the worst of the three: a rename of `Bar.bar` rewrote a method living on `Foo`,
at exit 0, and a rename of `Foo.bar` reported *no residue* for the definition it had missed -- a
report claiming a completeness it did not have.

A scope entry may now be **rooted**, meaning it discards everything outside it rather than nesting
under it. Rooted names, `class << Constant` and `def Constant.name` all push one. An owner rwr
cannot name -- `class << obj`, `def obj.name` -- pushes a name no Ruby constant can spell, so a
rule naming a real class matches nothing inside: under-matching, which is the safe direction,
rather than silently borrowing the enclosing class's name.

`class << self` is untouched and stays transparent, because `self` *is* the enclosing class.

**Residue files a definition under its owner too.** The occurrence scan recorded every occurrence
against the scope it was written in, which is right for a *reference* and wrong for a *definition*:
`def Foo.bar` was filed under the class containing it, so the rename that cared could not see it.

**`def Constant.name` is now rewritten** (was reported). It was not a limitation of the matcher but
a shape the rename never emitted: the class-method expansion covered `def self.name` and a `def
name` inside `class << self`, and not the third spelling. It needs no scope, because the receiver
written in the source is what pins the class and nothing else can -- the definition most often sits
inside a *different* class, which is the whole reason it was missed.

**Kept, deliberately.** The constant in `class << Foo` is resolved lexically by Ruby and rwr takes
it as written, so a nested `Bar::Foo` under-matches. Closing that means resolving a constant
against the lexical nesting chain, and a wrong answer there converts a safe under-match into a
wrong rewrite -- the trade principle 2 refuses. Revisit if Prism exposes a resolved owner, or if
rwr grows constant resolution for its own sake; not before.

**Found by** reading Shopify rubydex's `docs/ruby-behaviors.md` and testing rwr against the
behaviours it catalogues. The testbed had `class << self` and nothing else in the family, so the
engine's blind spot and the testbed's were the same blind spot -- now `testbed/lib/account_owners.rb`.

## D87 - `extend self` and `module_function` collapse the two method tables
**Decided.**

A module that extends itself puts one method on both tables: `Util.foo` and `Util#foo` name the
same method. rwr treated `kind:` as decisive there, so a rename did half its job -- rewriting the
definition and filing every call as residue, or the reverse -- and the report looked complete
either way, because the half it missed was reported rather than dropped.

`Hierarchy` now records self-extending modules, and for those the kind check and the
`scope: singleton:` check both stop discriminating. `extend Other` is untouched: an ordinary mixin
does not collapse the extending module's own tables.

`module_function :foo` names particular methods and this does not track which -- it marks the
module. That over-admits *within one module* and never reaches outside it, and the alternative
under-reports: a missed call site is a NoMethodError, where an extra candidate is a call that still
resolves.

**The pre-filter was the real bug, and it hid the feature working.** The hierarchy admits a file
only if it contains `class` and `<`, or a mixin keyword. A module using `module_function` has none
of those, so the file was dropped before parsing and the module was never recorded -- the collector
was correct and never ran. `extend self` worked throughout because `extend` is a mixin keyword.
The pre-filter and the collector now read from one list, since a pre-filter naming less than its
collector is a silent under-report by construction.

This is the second time this pre-filter has done it. The comment above it already records the
first, and notes that the testbed scored the case anyway because a prose comment happened to
contain the words `class` and `<`.

**Also closed:** bare `attr` was missing from `DEFINERS`, so an unrelated class's own attribute
read as a reach where the `attr_reader` beside it did not. Every symbol `attr` takes names a method
on the enclosing class; its optional second argument decides whether a writer comes too and is
never a name.

**Not a gap:** constant multi-assignment. `FOO, BAR = [1], [2]` is a `MultiWriteNode`, so a rule
matching `$C = [...]` does not match it -- under-matching, which is the safe direction.

## D88 - A pattern's receiver is compared for presence, not just position
**Decided.**

`generated::children` returns the children a node actually has, so an absent optional field leaves
no gap. For a call that means `receiver` and `arguments` are both "the first child" when the other
is missing, and comparing children positionally lined them up: `$X.foo` matched `foo(bar)`, binding
`$X` to an `ArgumentsNode`. `bar.foo` is a `CALL` and `foo(bar)` an `FCALL` -- different programs,
reported as the same site. A block collided the same way, so `$X.foo` also matched `foo { 1 }`.

Receiver presence is now compared before the positional walk. **Receiver only**: a sequence
metavariable must be able to absorb nothing, so `foo(*$REST)` matches `foo()` where the pattern
carries an arguments node and the target does not, and comparing argument presence would break the
thing splats exist for. Once the receiver is pinned, nothing else can be the lone child in its
place.

**Why it survived.** Every pack rule using `$R.method` carries either more structure
(`$R.where($C).count > 0`) or a `type:` constraint, and a constraint cannot resolve an argument
list -- so renames declined these sites rather than rewriting them. The exposure was `rwr find` and
any hand-written rule with a bare `$R.method`. Find is observation, where over-reporting is as much
a lie as a miss.

**Found while** asking whether a rewrite should be validated beyond reparsing. It should, and the
question was worth asking: this bug was invisible to `verify`, which only checks that the file
still parses.

## D89 - The result is checked against the template, not only against the parser
**Decided.**

`verify` reparses the rewritten source and discards the whole transformation if it no longer
parses. That is the backstop for range arithmetic and it has a stated limit -- it cannot catch a
mistake that happens to stay valid. D-era evidence: `!$X.empty?` -> `$X.any?` wrote `any?xs`, which
Ruby reads as `any?(xs)`. Valid, wrong, and silent.

Each rewritten site is now re-matched against the template it came from. The question is the
obvious one to ask afterwards and nothing was asking it: *is what we wrote what we said we would
write?*

**Skipping is not failing.** Refusing a correct rewrite breaks a working run; missing an incorrect
one leaves things as they were before this existed, so every uncertainty resolves toward letting
the rewrite through. Skipped: an empty template (a deletion has no shape), a template carrying a
sequence transform (`*$ITEMS.sort` is an instruction, not output), a template that is not a single
expression, and any site whose node cannot be located again in the rewritten tree.

**Shape, not bindings.** Metavariables match freely, so this catches a mangled shape and not a
correct shape wrapped around the wrong capture. The second is a narrower bug and needs the match
environment threaded through; this is the cheap half and it is the half that failed in practice.

**Both checks are in memory, before the write.** `apply` -> `verify` -> `verify_template` all run
on a `String`, and `cli` writes only once the outcome comes back clean. There has never been a
partially-written file to undo, and there should not be one.

**Cost.** One extra parse of the rewritten source per rule application -- the same source `verify`
already parses, so the marginal cost is the match, which is bounded by the number of changed sites.
