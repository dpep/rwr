# Staff-eng design review

2026-08-20. Adversarial review of the pre-implementation plan (`DESIGN.md`, `docs/decisions.md`, `docs/open-questions.md`) — nothing implemented yet, second revision of the plan. Requested focus: Q1 tractability, Q3 semantics, contesting D1, phase sequencing, and gaps.

## Verdict

**Proceed with changes.** The core abstraction is sound and nameable in one sentence — *a rewrite is complete over what static syntax can see, and the tool itemizes what it couldn't see* — and the three-phase cut plus the reparse-verify invariant are the right bones. But the plan has a structural flaw: Phase 0's kill criteria test only the part the doc itself calls table stakes (syntactic matching vs ast-grep), while the headline differentiator (known-unknowns) has no Phase-0 deliverable and no number that can kill it. As written, Phase 0 can pass while the thesis dies. Fix that, cut the Phase 1 cache (it's incoherent for a fresh-process CLI), and resolve Q3/Q4 as public contract before v0.1 — then this is worth building.

## Load-bearing problems (ranked)

### 1. The kill criteria can't fire on the thing that matters

**Breaks:** DESIGN.md §2 says differentiation is (a) the semantic layer and (b) known-unknowns. Phase 0's kill criteria (§6) measure neither — they measure whether ast-grep covers syntactic transformations. Q1 says known-unknowns "needs a Phase 0 spike, not a Phase 1 assumption," but Phase 0's defined work doesn't include that spike, and the criteria don't reference it. Meanwhile the criteria are a conjunction (ast-grep must hit ≥95% recall on ≥8/10 **and** <3 transformations need inexpressible constraints) over transformations the author selects — include three "narrow by receiver class" cases, which ast-grep structurally cannot express, and the project survives regardless of evidence. That's a gamed gate, probably unintentionally.

**Fix:** Split the gate per differentiator, each with its own kill number, pre-registered before any tool runs:
- *Syntactic:* keep the current criterion, but score only the transformations expressible without semantics. If ast-grep passes, Phases 1's syntax work becomes "contribute upstream," per the doc's own words.
- *Known-unknowns:* add the Q1 spike to Phase 0 (it's a grep + classifier prototype, days not weeks — see Q1 verdict below for the shape). Kill number: for ~10 real renames on the monolith, median surviving flagged sites ≤ ~10 with hand-verified relevance well above half. If it's hundreds, or relevance is noise-grade, the differentiation collapses to "ast-grep with a better parser" and §2 says stop.
- *Receiver narrowing (Q2):* a feasibility sample on the **public** corpus — what fraction of call-site receivers resolve with symbol-index-plus-local-inference, no Sorbet. This is the D6 bet; measure it before building Phase 2.

### 2. D5's cache is incoherent as designed — and pre-empts the design's own best question

**Breaks:** "Memoize parses keyed by content hash" assumes the parse survives to the next invocation. rwr is a one-shot CLI: each invocation is a fresh process, and Prism's tree is C-allocated and borrows the parse buffer — it is not serializable. To persist parses across invocations you must design an owned, serializable AST representation plus invalidation — which is most of the hard work of the index D5 congratulates itself for avoiding. Within a single invocation there's nothing to memoize (each file parses once). So the cache as specified either does nothing or is secretly index-shaped. Worse, §3 correctly names the benchmark that matters — *is cold parallel Prism parse over 1M LOC already fast enough to need nothing?* — and then Phase 1 ships a cache and the CLI grows a `rwr cache` subcommand before that number exists.

**Fix:** Phase 1 ships **no cache**; delete `rwr cache` from §8. Prism is built to parse at application boot; cold parallel parse of 1M LOC is plausibly low single-digit seconds, which is fine for an agent loop — but let Phase 0 report the number. If persistence is ever needed, design it once, in Phase 2, for the symbol index — which genuinely needs cross-invocation persistence and invalidation anyway. The coherence problem D5 defers arrives in Phase 2 regardless; solve it there, once.

### 3. Known-unknowns needs one sharpening to escape the noise trap (see Q1 verdict)

The proposed middle ground is close but underspecified in the one load-bearing way: the report must be filtered by **per-query intersection** — a site is reported only if its name-expression *could evaluate to the specific name being rewritten* — and fully-dynamic sites must leave per-query output entirely. Without that, "aggregate the rest" is the same noise in a summary line. Detail below.

### 4. Pattern equivalence classes are an undesigned public contract

**Breaks:** "Shape is Ruby source with `$METAVARS`" (§5, D2) hides the hard problem: one pattern shape must match many concrete syntaxes, or the tool is useless. Does `foo($A, $B)` match `foo a, b` (no parens)? `x.foo(a, b)` (explicit receiver)? `foo(a, b) { ... }` (trailing block)? `foo(a, b, &blk)`? Does a `do...end` in a pattern match `{ }`? If the answer is "exact concrete syntax only," recall craters and the Phase 1 precision gate is met vacuously. If it's "match modulo semantically-equivalent syntax," that equivalence relation *is* the matcher's spec and nobody has written it down. Q4 gestures at metavariable semantics but this is bigger: it's the semantics of the pattern language itself, public from v0.1 per D3.

**Fix:** State the principle in DESIGN.md: matching is structural over the AST, insensitive to syntax that doesn't change the parse shape (parens, `do/end` vs `{}`), with explicit named exceptions. Enumerate the equivalence decisions alongside Q4 and settle both before v0.1. ast-grep's documented metavariable/matching semantics are the prior art to diff against, deliberately.

### 5. The agent-facing contract is the real API, and it's one word in the doc

**Breaks:** The primary consumer is an agent in a loop, yet the entire machine interface is `[--json]`. Undesigned: the JSON schema for matches/captures/spans/diagnostics/unknowns; the exit-code contract (matched / no matches / refused-ambiguous / conflict-aborted / partial-parse-failure are all different agent branches); whether `rewrite --dry-run` output is byte-identical in shape to `rewrite`. For an agent, exit codes and JSON shape *are* the product; pattern syntax is secondary.

**Fix:** Add a §"Machine contract" to DESIGN.md before Phase 1: JSON schema sketch, exit codes, and a stability promise (schema versioned, additive changes only). Cheap now, breaking later.

### 6. Q3 is resolvable now — resolve it (see Q3 verdict)

Outermost-first, non-overlapping, single pass, skipped-nested-matches reported. Detail below. It should move from open-questions to decisions before Phase 1, since it shapes the edit engine.

### 7. Refusal taxonomy and file discovery are unspecified

**Breaks:** "Refuse rather than guess" names a principle, not behavior. A 1M LOC monolith contains files that don't parse (fixtures, vendored code, templates), `.erb`/`.haml`/`.rake`/`Gemfile` files that are or contain Ruby, and encoding oddities. Does one unparseable vendored file abort a repo-wide rewrite (unusable) or get skipped-and-reported (correct, but unstated)? What's the file set — gitignore-respecting, `.rb` only, include/exclude globs? These numbers also feed the known-unknowns counts, so they're not cosmetic.

**Fix:** One paragraph in DESIGN.md: discovery = gitignore-respecting `.rb` (+ configurable globs), ERB explicitly out of scope for v0.1; per-file parse failure = skip and report in the unknowns section, never abort; refusal reasons enumerated in the JSON contract.

## Q1 / Q3 / D1 verdicts

### Q1 — tractable, in a sharper formulation than the doc's

The doc's middle ground ("report partially-static names, aggregate the rest") is half right. As stated it still fails, because "aggregate the rest" per query is a repo-constant number carrying zero information about *this* rewrite. The formulation that works, three buckets:

1. **Literal-name dynamic sites are matches, not unknowns.** `send(:foo, x)`, `delegate :foo, to: :engine`, `alias_method :bar, :foo`, `def_delegators :@x, :foo` — the name is statically known; these should be found and (where safe) rewritten like any call. A recognizer list for the top ~10 Ruby/Rails metaprogramming idioms converts the *majority* of a Rails codebase's "dynamic" sites into static ones. This is the cheap, high-yield part, and no competitor does it.
2. **Partially-static sites, filtered by per-query intersection.** `define_method(:"#{p}_at")` compiles to the pattern `*_at`; report it for a rewrite of `paid_at`, never for `foo`. Symbols drawn from a literal constant array one hop away resolve the same way. This is local constant folding plus string-pattern intersection — not dataflow — and it makes noise proportional to genuine risk instead of repo size. This is also the one thing an agent's fallback `rg` genuinely cannot do: grep for `paid_at` will never find `"#{p}_at"`.
3. **Fully-dynamic sites (`send(m)` where `m` is a parameter) leave per-query output entirely.** They can reach anything, so reporting them per rewrite is noise by construction. Put them in a cached repo-level dynamism inventory (`rwr audit`, or one summary line with a pointer) the agent consults once per repo.

Be honest in the README about the adjacent fact: buckets 1–2 minus confirmed matches is a classified, filtered version of the `rg ':foo\b|"foo"'` backstop a careful agent runs anyway. The value is that agents *don't* run it, or drown when they do, plus the interpolation intersection grep can't express. That's a real differentiator but a smaller claim than §4's framing — size the claim to the evidence.

Spike this in Phase 0 with the kill number from problem #1. If the intersection filter still yields hundreds of sites per rename on the monolith, the doc's own §2 logic says the project is "ast-grep with a better parser" — and should stop or reposition.

### Q3 — recommended rule: outermost-first, non-overlapping, single pass, skip-and-report

- `find` reports **all** matches, nested included (agents want the full picture; ranges disambiguate).
- `rewrite` sorts matches by position, takes each leftmost-outermost match, and **skips any match whose range overlaps one already taken**. Skipped matches are counted and reported ("N matches inside rewritten ranges skipped; re-run to process"). Output is not re-matched — no fixpoint mode.

Why outermost: it preserves the invariant that *a capture is a verbatim source span*. Innermost-first forces the outer match's captured text to be spliced with already-applied inner edits, which breaks minimal-edit reasoning and the heredoc ownership rule (Q5) simultaneously. Why no fixpoint flag: `foo($A) → foo(foo($A))` diverges under fixpoint; the agent already runs in a loop, so convergence-by-reinvocation defines that failure mode out of existence, and each pass independently gets the reparse-verify check. Why non-overlap rather than conflict-abort: nested matches are the *common* case (`foo(foo(a,b),c)` is ordinary code), and aborting the whole transaction on ordinary code trains users to expect refusal — the exact failure D6 was pulled forward to prevent. Overlap-skip also covers the rarer non-nested overlaps that heredoc effective ranges can create. The "apply both → unparseable output" worry disappears under non-overlap plus reparse-verify.

This matches where the incumbents landed after pain (ast-grep applies non-overlapping edits per pass; RuboCop loops corrections with clobber detection and an iteration cap) — read both before finalizing, but expect confirmation, not surprises. Move this to decisions.md before Phase 1.

### D1 — Prism: right decision, partially wrong rationale; make the reversal path concrete

Keep Prism. But two of the three stated reasons need repair, because a decision justified by wrong reasons gets relitigated:

- **"tree-sitter silently guesses" is overstated.** tree-sitter marks recovery explicitly with ERROR/missing nodes; a tool can detect them and refuse, same as reading Prism diagnostics. The real difference is *fidelity of the recovered tree* and that Prism's diagnostics say why. Not decisive alone.
- **The steelman the doc skips is ast-grep reuse.** Nobody picks tree-sitter here for incremental parsing (the doc is right that it's irrelevant); you pick it because ast-grep is built on it — choosing tree-sitter could make rwr an ast-grep rule-set-plus-post-processor instead of a new engine, inheriting a battle-tested matcher for free. That's a genuinely attractive road not taken, and the README's "why not ast-grep?" answer must address it. It loses because of the third reason, which is the actually decisive one:
- **Fidelity plus unified pattern parsing.** The pattern language *is* Ruby source (D2), so the pattern parser and target parser must be the same parser, and it must be the highest-fidelity one — tree-sitter-ruby's structural lag becomes a pattern-language bug, not just a matching bug. And Prism's exact heredoc/interpolation modeling is load-bearing for §7, which is where rwr's correctness story lives.

On reversibility: resist building an owned parser-agnostic AST mirror "to keep D1 cheap to reverse" — Prism has ~150 node types and mirroring them is a large, speculative cost (and with the cache cut, nothing else needs an owned tree). Match directly on Prism nodes. Parser-agnosticism means one thing only: no Prism type names or node vocabulary in the JSON output, rule semantics, or error messages. That keeps the blast radius of a parser swap inside the matcher, which is as cheap as reversal honestly gets.

## Missing areas

Beyond the ranked problems above (machine contract #5, equivalence classes #4, refusal/discovery #7), a design doc for this should also cover:

- **Multi-file atomicity mechanics.** "Atomic per invocation" (§4) is not achievable across files on a real filesystem. Specify what it means: validate everything (all preconditions, all reparse-verifies) before writing anything, then best-effort write via temp-file-plus-rename per file, and state what the agent sees if a write fails mid-flight. Honest partiality beats claimed atomicity.
- **Comment preservation in rewrites.** A template that drops or moves a capture drops the comments attached inside it, silently, and the reparse-verify invariant won't catch it (comments aren't in the AST). RuboCop's TreeRewriter has scars here. At minimum: detect comments inside a replaced-but-not-reused range and warn or refuse.
- **Rule-file versioning.** The YAML rule format is public from v0.1; it needs a version field and the same additive-only discipline as the JSON schema.
- **Test strategy beyond the corpus.** The Phase 0 corpus is regression, not coverage. Name the property tests: idempotency (rewriting the output matches zero times or is a reported re-run), reparse-verify under fuzzed inputs, byte-identity of non-matched regions.
- **Q6 (Ruby version targeting)** is correctly flagged but needs a default answer before v0.1: repo's `.ruby-version`, else latest, overridable per rule. Cheap to decide, annoying to change.

## What to cut

- **Phase 1 cache and the `rwr cache` subcommand** (problem #2). Ship nothing; let Phase 0's cold-parse number decide if anything is ever needed, and if so build it once for Phase 2's symbol index.
- **Sorbet ingestion out of committed Phase 2 scope**, into §9 vision. Q2 already concedes the OSS value proposition can't depend on Sorbet; building the Sorbet-free resolver first is both the honest benchmark and the smaller phase. Sorbet becomes an enhancement with its own evidence later.
- **Repo-wide dynamic-site aggregates from per-query output** (Q1 verdict, bucket 3) — a separate audit surface, not a line every invocation prints.
- **Any fixpoint/iteration flag for Q3** — the agent loop is the fixpoint; don't build the divergence hazard.

None of this shrinks the thesis. It moves the evidence-gathering to where the thesis actually lives, and deletes the two places the plan was quietly building infrastructure ahead of its own measurements.
