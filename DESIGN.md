# rwr — Ruby structural rewriting

**Status:** design draft, pre-Phase-0. Revision 3 — incorporates the staff-engineer review
(`docs/internal/review-staff-eng.md`) and the prior-art survey (`docs/internal/prior-art.md`). Nothing here is
committed until Phase 0 reports numbers.

Companion docs: `docs/internal/decisions.md` (what was decided and what reverses it),
`docs/internal/open-questions.md` (unresolved risk), `docs/internal/cli-conventions.md` (CLI/JSON contract),
`docs/internal/prior-art.md`, `docs/internal/review-staff-eng.md`.

## 1. What this is

`rg`/`sed` for Ruby *programs* rather than Ruby *text*. A fast CLI that finds code by
structure, rewrites only what matches, preserves everything else, and refuses when it
can't be sure.

The intended user is a coding agent invoking it in a loop, with humans secondary. That
ordering drives the design: machine-readable output, deterministic results, meaningful exit
codes, and an explicit account of what the tool could not see.

## 2. Positioning

Structural search and rewrite for Ruby already exists.

| Tool | Has | Lacks |
|---|---|---|
| [ast-grep](https://ast-grep.github.io/) | Rust, fast, metavariable patterns, YAML rules, JSON out | tree-sitter Ruby fidelity; FAQ disclaims scope/type/dataflow analysis outright |
| RuboCop (`node_pattern` + `TreeRewriter`) | Ruby-native patterns, an order-independent action tree, huge install base | requires authoring a cop; runs in Ruby; batch-oriented; no ad-hoc queries |
| Semgrep | Mature skip/error taxonomy, AST-equality metavariables | Ruby grammar excludes heredoc bodies from the CST entirely (#2258) |
| Comby | Language-agnostic, proven, easy patterns | truncates `def…end` when a heredoc body contains `end`; always-0 exit code |
| Ruby LSP + Claude Code (Mar 2026) | Agent-facing find-all-references for renames | undocumented failure modes; not a rewriting engine |

**Who this is for, concretely.** The goal is not to displace RuboCop. It is to be a faster,
more ergonomic engine for a *personal* rule corpus - a handful of ported favourites, plus
custom rules RuboCop cannot express because `node_pattern` is purely syntactic - used to fix
the author's own repos. `.rubocop_todo.yml` draining is a named target. That framing removes
the coverage-threshold problem: twenty rules someone wants beats five hundred they do not.

**rwr is not novel at the syntax layer.** Phases 1–2 aim to match ast-grep's bar with better
Ruby fidelity. That is table stakes and should be understood as such when scope is under
pressure. The differentiation is three things:

1. **Ruby fidelity that is structurally reportable.** A tree-sitter-based tool cannot report
   its own parse blind spots, because its parser never admits to having any. Prism reports
   diagnostics. This is the same argument as (2), one level down.
2. **Semantic residue reporting** (§4) — what the tool could *not* see, per query.
3. **Receiver-narrowing** (§6, Phase 2) — mechanism has prior art (LibCST
   `QualifiedNameProvider`, OpenRewrite type attribution), but no Ruby structural tool offers it.

Plus one correctness claim that is nearly free: **Comby and Semgrep both silently emit
unparseable output when edits overlap, at exit 0.** rwr aborts. That is a reproducible
demonstration, not a marketing line — build the test case in Phase 0.

### What rwr cannot do

A boundary, not a bug - stated here and in the README rather than discovered by a frustrated
user (Q12).

**Genuinely out of reach:** non-anchored insertions (add an `include`, a `require`, a magic
comment - there is no node to anchor to), coordinated multi-site edits (a paired
`Timecop.freeze` and its matching `return`), sub-identifier matching and name transforms
(`find_by_*` -> `find_by(...)`), semantic guards (is this expression pure? is it truthy?),
and non-Ruby templates (ERB, Haml, jbuilder).

**Reachable after all - correction.** An earlier revision claimed concrete-syntax
transformations (`and` -> `&&`, hash rockets) were impossible because the trees are
identical. That is false: the node *types* match, but Prism retains operator locations, so
`AndNode::operator_loc()` recovers the spelling and `AssocNode::operator_loc()` is `None` for
`a: 1` and `Some` for `:a => 1`. Distinguishable by a `where:` predicate, with no raw source
reading. Pinned by `source::tests::operator_spelling_survives_parsing` and
`hash_rocket_is_distinguishable_from_shorthand`.

So the modernization family is largely in scope, and Phase 0's corpus should include it.

## 3. Parser: Prism. Decided (D1).

[`ruby-prism`](https://crates.io/crates/ruby-prism) — official Rust bindings, MIT.

The decisive argument is **silently different-shaped trees for valid Ruby**, not error
recovery. tree-sitter marks ERROR/MISSING nodes explicitly, so refuse-on-error-node
neutralizes malformed input entirely. What it cannot neutralize is valid Ruby that the
community grammar shapes differently or drops — heredoc bodies excluded from the CST, the
`a /b/` regex-vs-division ambiguity, lexer-feedback cases — because nothing is marked and
the tool has no way to know.

Incremental parsing is correctly dismissed: it is an editor keystroke-latency feature this
architecture never consumes.

Reversal cost is honest: rewriting the matcher against a new node vocabulary. Weeks, not days.

## 4. The safety contract

Ruby cannot support "I found 83 matches and changed all 83." `send`, `public_send`,
`define_method`, `method_missing`, `const_get`, `eval`, and monkey patches all defeat static
enumeration.

### Completeness over static syntax
83 matches, 83 rewritten, 0 conflicts. Reported always.

### Name-scoped residue reporting
**For name-anchored rules only** (a rename, a signature change — anything with a target
identifier). For `return nil → return` there is no name to track and this section is absent.

After the structural match, enumerate every remaining occurrence of the target identifier
the match did not account for, classified by syntactic context: `:foo` symbol literals,
`"foo"` strings, interpolation fragments whose static parts are consistent, elements of
literal arrays feeding `send`/`define_method`/`delegate`/`alias_method`.

This is what a careful human does with `rg` after a rename. Lexical plus AST context, no
dataflow. Rails metaprogramming overwhelmingly flows *literal* symbols through macros, so
the classifiable fraction is large. Noise degrades with name commonality exactly as
syntactic matching does — for `calculate_payroll_tax`, near-zero noise; for `call`, the tool
reports "identifier too common; N occurrences, not enumerated," which is honest and graded.

**Receiver-qualified where possible (D20).** The "identifier too common" degradation
collides with Rails naming — `create`, `update`, `call`, `perform` are simultaneously the
likeliest migration targets and the names that clause abandons. Where a receiver resolves,
residue keys on *co-occurrence* (`Payments::Charge` near `create`) rather than the bare
identifier. Full benefit lands in Phase 2; Phase 1 degrades to bare-name residue and says so.

**Blind spot — non-Ruby templates (Q11).** ERB, Haml, Slim, and jbuilder are not parsed, so
residue's "here is everything left" is false repo-wide in a Rails app. The claim must be
narrowed to `.rb` in every report until a fallback exists. An over-claim here directly
undermines the differentiator.

**Rejected:** a repo-wide count of fully-dynamic dispatch sites. It is constant per repo,
identical for every query, and therefore carries zero information about the rewrite at hand.
If that inventory is ever wanted it is a separate one-time `rwr audit`.

**Novelty, honestly:** the *shape* is standard — Semgrep ships a 17-variant `skip_reason`
enum. The *content* is not: every existing implementation is mechanical ("I could not
process these bytes"), never semantic ("this file scanned fine and there is still a
`send(name)` on line 47").

### What refusal does *not* cover (Q10)
Refusal guards **edit mechanics** — conflicts the tool can detect. It does not guard **match
semantics**. Seven realistic case studies produced no natural refusal, and two confidently
*wrong* rewrites at exit 0: a `Company#full_name` site matched when `User#full_name` was
meant, and an impure repeated-metavariable match.

The dangerous failure is a clean, confident, wrong match. Nothing guards it before Phase 2 —
which reframes the symbol layer from "makes matching usable" to "the only thing between the
user and silent breakage."

### Refusal granularity
Whole-transaction abort is agent-hostile at 1M LOC — an 83-file rewrite aborted by one
vendored parse error teaches the agent to shrink invocations until atomicity is meaningless.

- Unparseable or excluded files: **reported and skipped**. They are a known-unknown and the
  design already has vocabulary for that.
- Ambiguous sites within a parseable file: **abort the transaction**.
- Overlapping edits: **abort** (§5).
- JSON says which, and the exit code distinguishes retryable from terminal
  (`docs/internal/cli-conventions.md`).

## 5. Matching and rewriting

### Pattern syntax (D2)
Shape is Ruby source with `$METAVARS`; constraints live in a separate `where:` block.

```yaml
match: foo($A, $B)
where:
  keywords: none
  receiver_type: PayrollService    # Phase 2
  inside: class User               # v0.1 — see D19
rewrite: foo($A, $B, context: context)
```

### Metavariables must be substituted before parsing (D18)

Parsing patterns with Prism makes `$M` a *global variable* token, which lexes only where a
gvar is legal. `foo($A, $B)` works; `$X.$M`, `def $M`, `:$M`, `$K: v`, and `Foo::$C` do not
lex at all — which would kill method-name, symbol, keyword-name, and constant matching, and
the whole modernization family with them.

So each `$NAME` is rewritten to a syntactically valid placeholder identifier *before*
parsing, recognized in the resulting tree, and mapped back. Constants need a capitalized
placeholder and everything else lowercase; parse with the lowercase form and retry with the
constant-cased form on failure. Patterns are tiny, so the retry costs nothing.

### Metavariable syntax (D32) and semantics (D16)

Two orthogonal axes, not four forms to memorise: `*` means many, `_` means don't care,
`$NAME` binds a capture.

|            | one node | zero or more |
|------------|----------|--------------|
| anonymous  | `_`      | `*_`         |
| captured   | `$NAME`  | `*$NAME`     |

```ruby
charge($AMOUNT, *$REST)
delegate(*$METHODS, to: $TARGET)
hash.each { |_, $V| ... }
```

**All four are valid Ruby.** `*` is Ruby's own splat and `_` its own throwaway, so the
language's grammar validates position for you: sequences are legal exactly where splats are —
argument lists, arrays, block params, destructuring — which is precisely where they are
wanted. `*_` is already idiomatic Ruby for "rest, ignored".

Everything else — optional, exact counts, must-not-appear — is a `where:` refinement using
D16's occurrence counts. `$NAME` defaults to `{min: 1, max: 1}`, `*$NAME` to
`{min: 0, max: unbounded}`.

```yaml
match: charge($AMOUNT, *$REST)
where:
  $REST:  { min: 1 }   # at least one
  $BLOCK: { max: 0 }   # must not appear
```

A capture name is `$` plus an **uppercase** letter, then uppercase letters, digits or
underscores. The case rule keeps ordinary Ruby globals matchable as literals with no escape —
`$stdout`, `$_`, `$1`, `$:` are all common and all literal. Only uppercase globals
(`$LOAD_PATH`, `$DEBUG`) are genuinely ambiguous, and they escape as `\$LOAD_PATH`.

There are **no exceptions to the rules above**. Because anonymity is spelled `_` rather than
`$_`, Ruby's `$_` global stays literally matchable like any other.

Repeated names bind once and are checked for **AST equality** (D16), never textual equality.

Position is otherwise a non-issue: D18's pre-lex substitution means `$M` is spelled
identically in method-name, symbol, keyword-name and constant positions.

### Overlap and nesting (D15, resolves Q3)
The conflict unit is the **edit range**, not the match range. Because edits are minimal,
nested matches usually produce *disjoint* edits: `foo(foo(a,b), c)` under
`foo($A,$B) → foo($A,$B, context: context)` is two insertions before two different closing
parens, and both apply cleanly.

- `find` is **reentrant** — reports all matches including nested, with nesting metadata.
  Find is observation; suppressing lies.
- `rewrite` is **outermost-only**. Inner-first would invalidate the outer match's captures
  against original source; outer-first merely obsoletes the inner match, which a rerun
  re-finds cleanly.
- **Partial overlap aborts.** This is the correctness claim from §2.
- **No auto-fixpoint inside one invocation.** `foo($A) → foo(bar($A))` matches its own
  output and diverges. The consumer is already a loop — report the residual count, exit
  retryable, let the agent iterate. This is a *choice*, not a necessity: ruff ships bounded
  fixpoint in production (100-iteration cap, full revert on syntax error) and RuboCop caps at
  200 with checksum cycle detection. We push the loop to the caller because the caller is
  already a loop, not because it cannot be done.

### Rewriter implementation (D13)
Port `Parser::Source::TreeRewriter`'s **tree of actions**. Invariants: children strictly
contained by parent, siblings disjoint and ordered, only non-replacing actions may have
children. This yields order-independence *by construction* and a clean clobbering error on
partial overlap. GritQL converged on the same invariant independently, which is good
evidence it's the right shape.

Three named policies for the ambiguous cases, following TreeRewriter's
`:accept | :warn | :raise`: `crossing_deletions`, `different_replacements`,
`swallowed_insertions`.

## 6. Phases

Three committed phases. Everything else is §8.

### Phase 0 — validate the differentiators, not the table stakes

**Corpus.** Two, because one cannot do both jobs:
- Your monolith (>1M LOC) — real transformation fidelity and perf; unpublishable.
- A public repo (Discourse / Mastodon / GitLab) — reproducible benchmarks a skeptic can rerun.

~10 real transformations with hand-verified ground truth, **partitioned into syntactic and
semantic before scoring**. The syntactic partition judges whether Phase 1 deserves to exist
as a new engine; the semantic partition judges Phase 2. ast-grep failing the semantic
partition proves nothing — it definitionally cannot pass — and must not count toward survival.

**Four measurements, all of which gate:**

- **(a) Residue-reporting spike.** 3 real monolith renames with hand-enumerated ground
  truth. Pass bar: residue reporting catches the dynamic reaches a human found, with a
  reviewable — not screen-filling — false-positive list. **At least one target must be a
  common Rails-shaped name** (`create`, `perform`), not only a distinctive one — testing
  only distinctive names measures the easy case and passes for the wrong reason (D20).
- **(b) Bare-`foo(...)` false-positive rate** on the corpus. Validates or refutes D6's
  premise with a number instead of an assertion.
- **(c) Receiver resolution for *methods*, without Sorbet** - what fraction of method call
  sites resolve their receiver from a symbol table plus local inference, on the **public**
  corpus. Methods specifically: Ruby LSP already resolves constants well (Q9), so constant
  navigation is taken and this is the only part no plausible LSP exposure covers.
- **(d) Cold parallel Prism parse time**, both corpora. Decides D5.

**Decision table — three branches, not two:**

| Outcome | Action |
|---|---|
| ast-grep passes syntactic partition **and** (a) fails | Contribute Ruby fixes upstream. Stop. |
| ast-grep fails syntactic partition | Build Phase 1 as designed. |
| ast-grep passes syntactic partition **and** (a) passes | **Evaluate rwr as a layer** over ast-grep's JSON, owning only rewrite/verify. Note the tension: this reintroduces tree-sitter fidelity, which §3 calls decisive. Resolve with numbers. |

Two correctness demonstrations, both cheap and both reproducible by a skeptic:
- Comby and Semgrep emitting **unparseable output at exit 0** on overlapping edits (D15).
- RuboCop's known heredoc-corruption autocorrect bugs (#10895, #10320, #6653, #11621), which
  D14's `effective_range` is specifically designed to prevent. A *safety* claim against the
  incumbent, not a speed one.

### Phase 1 — engine

Prism parse, metavariable matching, captures, `effective_range` splicing (§7), the
TreeRewriter action tree, atomic apply, JSON + exit codes per `docs/internal/cli-conventions.md`,
scoping flags, residue reporting for name-anchored rules.

**No persistence.** Parse, answer, exit. D5's cache is unmeasured; §3's question about the
index applies equally to it. Deletes `rwr cache`, the invalidation bug class, and a
coherence surface. Revisit only if (d) says so.

*Gate:* 100% precision on the corpus. Hard gate, not a target — one incorrect rewrite
invalidates the thesis.

### Phase 2 — symbols

Symbol index built from the Prism parse already happening: definitions, inheritance,
includes, constant resolution. Enough to resolve receivers and narrow by defining class.

Pulled forward from the original Phase 4 because at >1M LOC bare `foo(...)` matching has a
false-positive rate high enough that the refusal contract fires constantly and trains users
to ignore it — **pending confirmation by measurement (b)**.

Sorbet ingestion is **not** in committed scope (moved to §8). The no-Sorbet path must stand
alone; commit to Sorbet only if (c) shows it cannot.

## 7. Engineering hazards

**Heredocs, and the general rule (D14, resolves Q5).** A heredoc body lives far from its
`<<~FOO` token; Prism's `StringNode#location` is only `opening_loc`. Moving a captured node
silently detaches the body — **and the result still parses**, so reparse-verify won't catch it.

The general rule *is* derivable: a node's **`effective_range()`** is the transitive closure
over its descendants, unioning each heredoc's `closing_loc`, computed at splice time.
Insertion points must also be phrased relative to it.

But the rule is not the hard part — the enforcement is. RuboCop has had this data for a
decade and still ships heredoc-corruption bugs (#10895, #10320, #6653, #11621). Therefore:
**never expose raw `.location` from the capture API.** `effective_range()` is the only
splice-able range. Make the unsafe operation unrepresentable rather than documented.

**Comment attachment under deletion and movement (D35).** A comment moves with the element it
belongs to, and belonging is by adjacency: trailing (same line) and leading (own line directly
above) attach to that element; interior comments already move via `effective_range`; dangling
comments attach to the container and stay. A comment sharing a line with several elements has
no unambiguous owner, so a reordering transform **refuses** rather than reattaching it to a
neighbour.

**Reparse-verify.** After computing edits, reparse and assert the resulting AST matches what
the transformation intended. Mismatch discards the whole transformation.

**Identity-rewrite property test.** Match with no template change → byte-identical output,
over both corpora. Cheap, and catches the range-arithmetic bug class at dev time.

**No write preconditions, and no undo (D28 withdrawn).** rwr writes files, as `rubocop -a`,
`ruff --fix` and `prettier --write` do; git is the safety net and D21's preview-by-default is
the protection. Within one invocation read and write are milliseconds apart, so there is no
race to guard. Across a preview call and a later write call the second call recomputes from
current state - which is *more* correct than replaying a stale plan, because the unit of
review is **the rule, not the edit list**: an edit that appears in code touched since the
preview is still a correct application of an approved rule. The residual effect is that an
apply may differ from its preview; that is stated, not defended against.

**Read-write race - content-addressed edit ids (D25).** The agent flow searches, inspects,
then writes; files change in between. A *per-file* content hash over-rejects: any unrelated
line moving anywhere in the file kills the whole transaction. Each edit instead carries
`<path>:<index>@<digest>` where the digest covers the edit's own effective range. Serena
arrived at the same contract independently.

## 8. Vision — explicitly not committed

Recorded so the design doesn't foreclose it. Not a roadmap.

- Sorbet ingestion; fuller type layer
- Rails-aware constructs (associations, callbacks, routes, concerns)
- Coccinelle-style **isomorphisms** — named, individually disableable, position-typed
  equivalence rules. Ruby needs them more than C does. **But** Semgrep shipped exactly this
  and deprecated it in v0.61.0, so this is a Phase 0 hypothesis, not a free win (Q8).
- `rwr audit` — one-time repo-wide dynamic-dispatch inventory
- Verification pipeline: format → typecheck → affected tests
- Breadcrumbs
- **Editor integration** - "apply this rule here" on the file already open, since a lot of
  real usage is a spot rewrite rather than a batch sweep. The architectural tension to settle
  first: rwr is a single static binary with no daemon, no index and no persistent state (D5),
  while an LSP is a long-running stateful process. The cheap version avoids that entirely --
  a VS Code extension that shells out to `rwr check <rule> <file>:<line> -j` per invocation
  buys the spot-rewrite affordance at zero architectural cost, because a single file is
  already fast enough that a warm index buys nothing (see docs/internal/scaling.md). Reach for a real
  language server only when something needs to be *resident*, and say what that something is
  before building it.
- **GitHub integration, and the cheap path to it.** A Marketplace app is the expensive
  spelling: hosting, OAuth, permissions, a webhook endpoint, a release channel rwr does not
  otherwise need. The same outcome -- findings as annotations on a pull request -- is reached
  by emitting **SARIF** from `check` and letting `github/codeql-action/upload-sarif` ingest
  it, which is a self-contained output mode in a tool that already has two (D17's `-j`/`-J`).
  Requested by a user in the same breath as the app, and it is the half worth building first:
  it is a day of work, it composes with every other SARIF consumer, and it makes the app
  question empirical rather than speculative. Do not build the app until SARIF-in-Actions is
  in real use and something concrete is still missing.
- **`rwr import`** - convert an ast-grep rule to canonical rwr syntax. Compatibility as a
  one-time conversion rather than a permanent second spelling; see D32's rejected-alias note.
  Build only if Phase 0 shows real migration demand.
- **MCP server** - four tools split on the read/write boundary: `rwr_find`, `rwr_check`,
  `rwr_rewrite`, `rwr_info`. Never one fat tool with a mode flag, because MCP annotations are
  per-tool and a write mode would permanently taint the read path. `rwr_rewrite` mirrors the
  CLI exactly: preview by default, `write: true` to mutate. Neither ast-grep's nor Semgrep's
  MCP ships apply-fix at all; they pushed the dangerous splice onto the agent.
  Counter-argument on record: a skill over the CLI may beat an MCP server outright, so rwr's
  earns its keep through **the transaction**, not the search.

## 9. Principles

1. Determinism over cleverness.
2. Refuse rather than guess — and **never silently drop an edit** (ast-grep and Synvert both do).
3. Minimal diffs.
4. Report what you couldn't see — **unconditionally**, never behind `--verbose` (Semgrep's mistake).
5. Make unsafe operations unrepresentable, not documented.
6. Separate syntax from semantics; semantics enhances matching, never gates it.
7. Separate rewriting from formatting.
8. No embedded scripting in the constraint language (SSR's Groovy, GritQL's inline JS).
9. Never silently degrade to a weaker matcher.
10. Ruby-first. Depth over multi-language generality.
11. Measure before replacing.
