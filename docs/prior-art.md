# Prior art

A survey of structural search-and-rewrite tooling, read for what `rwr` should **steal** and
what it should **avoid** — not a catalogue. Every tool here has already hit at least one of
`rwr`'s open questions in production, and several have shipped an answer, a scar, or both.
The survey covers the nearest competitors (ast-grep, Comby, Semgrep, RuboCop's
`node_pattern` + `Parser::Source::TreeRewriter`, Synvert), the intellectual ancestor
(Coccinelle/SmPL), the lossless-rewriting systems that solved `rwr`'s §7 hazard class in
other languages (recast, LibCST, OpenRewrite, nikic/PHP-Parser), and the second tier
(Rector, jscodeshift, GritQL, IntelliJ SSR, fastmod). Its purpose is to shorten Phase 0 by
importing settled answers to [Q3, Q4, Q5](open-questions.md) rather than rediscovering them,
and to test honestly whether `rwr`'s claimed differentiators are actually novel.

---

## Steal list

Ranked by value to `rwr`, most valuable first.

### 1. TreeRewriter's tree-of-actions as the edit model (RuboCop / `whitequark/parser`)

`Parser::Source::TreeRewriter` is the single best-designed thing in this survey and `rwr`
should port it more or less wholesale. It arranges pending edits in a tree keyed by source
range, with three invariants stated in [`action.rb`](https://github.com/whitequark/parser/blob/master/lib/parser/source/tree_rewriter/action.rb):

```
* Children are strictly contained by their parent
* Siblings are all disjointed from one another and ordered
* Only actions with `replacement == nil` may have children
```

Consequences worth having:

- **Order independence is a guarantee, not a hope.** The class docs state results "will
  always be independent on the order they were given," and prove it by construction —
  `ordered_replacements` emits a canonical linearization from the tree.
- **Partial overlap is a hard error.** Two actions on crossing (non-nested, non-disjoint)
  ranges raise `ClobberingError`. This is exactly `rwr`'s "refuse rather than guess."
- **Nesting is legal iff the outer action doesn't replace.** A `wrap` of `foo(...)` plus a
  `replace` of its inner argument compose cleanly; a full replacement of the outer range
  swallows the inner and fires the `:swallowed_insertions` policy.
- **Three named, tunable conflict policies**, each `:accept | :warn | :raise`:
  `crossing_deletions` (two overlapping deletions fuse into one), `different_replacements`
  (same range, different text), `swallowed_insertions`. `rwr` should ship the same three
  knobs with `:raise` as the default — the inverse of parser's `:accept` default, because
  `rwr`'s consumer is a machine that cannot eyeball the diff.
- **`merge!` / `import!`** give a composable "compute edits independently, combine
  atomically, fail as a unit" transaction — precisely DESIGN.md §4's "rewrites are atomic
  per invocation."

GritQL independently arrived at the same invariant ("ranges may nest or be disjoint but
never partially overlap") and applies edits sorted by `(end desc, start desc)`. Two
independent designs converging is good evidence this is the right model.

### 2. Coccinelle's isomorphisms — and Ruby needs them more than C does

[Isomorphisms](https://coccinelle.gitlabpages.inria.fr/website/docs/main_grammar.html) are
Coccinelle's mechanism for treating syntactically different but semantically equivalent code
as one pattern. They live in [`standard.iso`](https://github.com/coccinelle/coccinelle/blob/master/standard.iso)
as *named*, individually disableable rewrite rules with their own metavariable declarations:

```
Expression
@ is_null @
expression X;
@@
X == NULL => !X

Expression
@ plus_assoc @
expression X, Y, Z;
@@
 (X + Y) + Z <=> X + Y + Z
```

Key mechanics worth copying exactly:

- **Named and individually disableable** — `@rule disable is_null@` or `--disable-iso`.
  This is what keeps the feature compatible with "refuse rather than guess": the
  normalization set is explicit, auditable, and switchable per rule, not a hidden fuzz.
- **Directional (`=>`) vs bidirectional (`<=>`)** — some equivalences are safe only one way.
- **Typed by syntactic position, not just node kind.** `Expression`, `TestExpression`,
  `ToTestExpression`, `Type`, `Statement`, `ArgExpression` — `!X => X == 0` only applies in
  boolean-test position. Position-scoped isomorphisms are what stop `X => X != 0` from
  exploding everywhere.
- **Explicitly not a fixpoint, and honest about it.** From the file header: *"the order of
  the rules has some importance. As we don't do a fixpoint, changing the order may impact
  the result."* Plus `--iso-limit` (max application depth) and `--track-iso` /
  `--profile-iso` for observability.

**Why this matters more for Ruby than for C.** Ruby has far more surface variation per
semantic construct, and the closest competitor already fails on it: Semgrep
[issue #5222](https://github.com/semgrep/semgrep/issues/5222) records that `method("foo")`
does not match `method ("foo")` in Ruby. Prism normalizes some of this for free (a
paren-less call is still a `CallNode`) but not all — `ParenthesesNode` is a real node,
`IfNode` and `UnlessNode` are different types, `ImplicitNode` exists for `foo(bar:)`
shorthand. Candidate Ruby iso set for Phase 0:

| Iso | Variants unified |
|---|---|
| `paren` | `(x)` ⇔ `x` (a real `ParenthesesNode` in Prism) |
| `unless_not` | `unless x` ⇔ `if !x` (`UnlessNode` vs `IfNode`) |
| `nil_check` | `x == nil` ⇔ `x.nil?` ⇔ `!x` (unsound for `false` — must be opt-in) |
| `implicit_self` | `foo` ⇔ `self.foo` |
| `symbol_block` | `&:sym` ⇔ `{ |x| x.sym }` |
| `brace_hash` | `foo(a: 1)` ⇔ `foo({a: 1})` |
| `kw_shorthand` | `foo(bar:)` ⇔ `foo(bar: bar)` (`ImplicitNode`) |
| `lambda_form` | `->(x){}` ⇔ `lambda { |x| }` |
| `numbered_params` | `_1` ⇔ a named block param ⇔ `it` |
| `word_array` | `%w[a b]` ⇔ `["a", "b"]` |
| `safe_nav` | `x&.foo` ⇔ `x && x.foo` |
| `return_nil` | `return nil` ⇔ `return` |

Ship this as a `where: isomorphisms: [...]` / `disable_isomorphisms: [...]` list with a
conservative default set, and report which isos fired for each match (`--track-iso`'s idea)
so a match is always explainable. Nothing in the Ruby ecosystem has this.

**One serious caution.** Semgrep shipped exactly this — a top-level `equivalences:` rule key
with Coccinelle-style bidirectional rules (`$X + $Y <==> $Y + $X`) — and
[deprecated it in v0.61.0](https://docs.semgrep.dev/writing-rules/experiments/deprecated-experiments)
with no stated reason, keeping only a fixed, non-configurable built-in set (import aliasing,
constant propagation, associative-commutative operators). That is a real negative signal
from the closest competitor and Phase 0 should treat it as a hypothesis to test rather than
an oversight to exploit. The likely failure modes to measure: combinatorial blowup when
several isos apply to one pattern (Coccinelle's answer is `--iso-limit`, a depth cap, and
explicitly no fixpoint); and users unable to predict what a pattern matches. Mitigations
that Semgrep did not have and `rwr` should: a small closed default set rather than an open
authoring surface, per-rule opt-out by name, and mandatory reporting of which isos fired.
If Phase 0 can't keep the fired-iso count small and the results predictable, ship the
built-in set only and drop user authoring — Semgrep's actual landing place.

### 3. `effective_range()` as the only splice-able range (answers Q5)

The heredoc hazard has a general, sound rule — but only if it is enforced structurally
rather than remembered per rewrite.

Prism gives `StringNode#location` for a heredoc equal to its `opening_loc` (`<<~SQL`
only); `content_loc` and `closing_loc` live physically *after* the rest of the enclosing
line, and the enclosing `CallNode#location` also stops before the body. So **no node's
`.location` is a superset of its own subtree's bytes** once a heredoc is involved. This is
not a Prism quirk: `whitequark/parser` has a dedicated
[`Parser::Source::Map::Heredoc`](https://github.com/whitequark/parser/blob/master/lib/parser/source/map/heredoc.rb)
carrying separate `heredoc_body` and `heredoc_end` fields for exactly the same reason.

The rule:

```
effective_range(node) =
  [ node.location.start_offset,
    max(node.location.end_offset,
        max over descendants D where D.heredoc? of D.closing_loc.end_offset) ]
```

Transitive over *all* descendants, computed at splice time, never cached as a static field.
That handles nested heredocs and multiple heredocs on one line. It must be paired with a
second rule: **every insertion point must be phrased relative to `effective_range(node)`,
never relative to `node.location.end` or a raw token scan.**

The evidence that the rule alone is insufficient without enforcement is RuboCop. `parser`
has exposed `heredoc_body`/`heredoc_end` for a decade, and cops still corrupt heredocs on
autocorrect, repeatedly, written by different people over years —
[#10895](https://github.com/rubocop/rubocop/issues/10895) (`Style/RedundantParentheses`
inserts a comma after the heredoc terminator, producing `SQL,`),
[#10320](https://github.com/rubocop/rubocop/issues/10320) (`Style/FileWrite` truncates
heredocs), [#6653](https://github.com/rubocop/rubocop/issues/6653),
[#11621](https://github.com/rubocop/rubocop/issues/11621). Availability of the data is not
the fix; **making the raw location inaccessible is.** `rwr`'s capture/node API should not
expose `.location` for extraction at all — only `effective_range()`.

Same family, same rule: `%w[]`, `%i[]`, interpolation, `__END__`/`DATA`.

### 4. Semgrep's typed `skip_reason` enum for machine-readable blind spots

Semgrep's [output schema](https://github.com/semgrep/semgrep-interfaces/blob/main/semgrep_output_v1.atd)
has `cli_output = { results, errors, paths: scanned_and_skipped }` where
`skipped_target = { path, reason: skip_reason, ?details, ?rule_id }` and `skip_reason` is a
closed 17-variant enum: `always_skipped`, `semgrepignore_patterns_match`,
`cli_include_flags_do_not_match`, `cli_exclude_flags_match`, `exceeded_size_limit`,
`analysis_failed_parser_or_internal_error`, `excluded_by_config`, `wrong_language`,
`too_big`, `minified`, `binary`, `irrelevant_rule`, `too_many_matches`,
`gitignore_patterns_match`, `dotfile`, `nonexistent_file`, `insufficient_permissions`.

Alongside it, `errors[]` carries a separate typed `error_type` enum — `LexicalError`,
`ParseError`, `PartialParsing`, `Timeout`, `PatternParseError` — with
`{code, level, type_, rule_id?, message?, path?, spans?, help?}` per record. Two enums:
one for "I didn't look at this file," one for "I looked and something went wrong."

This is the right *shape* for DESIGN.md §4's known-unknowns block: closed enums of reasons,
one record per blind spot, `details` free text alongside a machine-switchable tag. Steal the
schema shape; see the novelty check for what Semgrep's version does *not* cover.

Three things to fix rather than copy:

- **`?skipped` is optional and populated only under `--verbose`.** `rwr`'s completeness
  account must be unconditional — it is the product.
- **`PartialParsing` is non-fatal and exits 0.** If one method in a file fails to parse,
  Semgrep scans the rest, reports the matches it found, adds a `PartialParsing` entry to
  `errors[]`, and returns success. Correct behavior, wrong ergonomics.
- **There is no aggregate incompleteness signal.** Nothing at the top level says "results
  are incomplete." A caller has to know to inspect `errors[]` and `paths.skipped[]`.
  `rwr`'s JSON should carry a top-level `complete: false` (or a
  `completeness: { statically_visible: 83, unanalyzed: 4 }` object) that an agent
  cannot miss, plus a real exit-code contract distinguishing clean / findings /
  invalid-pattern / parse-failure — Semgrep has one (0/1/2/3/4/5/7/8/13), Comby does not.

### 5. ast-grep's traversal×reentrancy matrix (answers Q3 at the match layer)

ast-grep separates the Q3 policy into two orthogonal knobs on its `Visitor`
([`traversal.rs`](https://github.com/ast-grep/ast-grep/blob/main/crates/core/src/tree_sitter/traversal.rs)):

```rust
pub struct Visitor<M, A = PreOrder> {
  /// Whether a node will match if it contains or is contained in another match.
  reentrant: bool,
  ...
}
```

with the module doc spelling out the design intent for `foo(foo())`: *"we can configure a
traversal to report only the inner one, only the outer one or both."* Pre-order ×
`reentrant: false` = outermost-only; post-order × `reentrant: false` = innermost-only;
`reentrant: true` = both. Default for `Visitor::new` is `reentrant: true` (search reports
nested matches); `replace_all` hardcodes `.reentrant(false)` with a candid TODO:

```rust
// TODO: support nested matches like Some(Some(1)) with pattern Some($A)
Visitor::new(&matcher).reentrant(false)
```

Steal the matrix. `rwr`'s answer should be: **`find` is reentrant (report every match,
including nested, with an explicit `nesting_depth` / `contained_by` field); `rewrite` is
outermost-only by default and refuses when two selected matches partially overlap.** The
orthogonality means one flag (`--nesting outermost|innermost|all`) covers every policy
without inventing a new vocabulary.

### 6. ast-grep's explicit strictness dial

ast-grep's [match algorithm](https://ast-grep.github.io/advanced/match-algorithm.html)
exposes five named levels rather than an implicit fuzzy match:

| Strictness | Named in pattern | Named in code | Unnamed in pattern | Unnamed in code |
|---|---|---|---|---|
| `cst` | Keep | Keep | Keep | Keep |
| `smart` (default) | Keep | Keep | Keep | Skip |
| `ast` | Keep | Keep | Skip | Skip |
| `relaxed` | Skip comment | Skip comment | Skip | Skip |
| `signature` | Skip comment, ignore text | Skip comment, ignore text | Skip | Skip |

`rwr` gets `ast`-level for free (Prism is an AST, not a CST — trailing commas and most
whitespace are already invisible), so the dial it needs is different: **the isomorphism set
is `rwr`'s strictness dial.** The transferable idea is the discipline — name the levels,
document exactly what each ignores, make it a per-rule setting, never make it implicit.

### 7. OpenRewrite's parse-time round-trip self-check

OpenRewrite parsers print the freshly-parsed tree and byte-compare against the original; a
mismatch demotes the file to
[`ParseError`](https://github.com/openrewrite/rewrite/blob/main/rewrite-core/src/main/java/org/openrewrite/tree/ParseError.java)
("a parsed LST that was determined at parsing time to be erroneous… if it doesn't faithfully
produce the original source text") rather than proceeding with a lossy tree. The
`FindParseFailures` recipe surfaces these in a dedicated data table.

For `rwr` this is cheap insurance orthogonal to the reparse-verify invariant in DESIGN.md §7:
*before* attempting any rewrite, confirm the file's parse round-trips; if not, refuse the
file and list it in known-unknowns with a reason. Catches Prism edge cases `rwr` hasn't
modeled yet, and costs one string compare.

### 8. IntelliJ SSR's min/max occurrence counts (answers half of Q4)

IntelliJ [SSR](https://www.jetbrains.com/help/idea/structural-search-and-replace.html) gives
every `$var$` a **Minimum count / Maximum count** pair (default 1/1). This single mechanism
subsumes four things other tools give separate syntax:

- `1..1` — exactly one node (`$A`)
- `0..1` — optional
- `0..∞` — sequence (`$$$A` in ast-grep, `$...` in GritQL, `...` in Semgrep)
- `0..0` — must not appear (negation, with no negation operator needed)

For `rwr` this collapses Q4's "do we need a separate `$$$` sequence form?" into a `where:`
constraint: `where: { ARGS: { count: 0 } }`. It also fits DESIGN.md §5's split cleanly —
shape stays copy-pasteable Ruby, arity lives in `where:`. Strong candidate; the counterweight
is that `$$$` is the established convention in ast-grep/Comby/Semgrep and departing from it
costs familiarity.

### 9. GritQL's `bubble` — the metavariable scoping decision Q4 must make

In GritQL, a metavariable binds **once per file** by default: `contains \`console.log($msg)\``
matches only the first call site, because a second binding of `$msg` to different text fails
the equality check. `bubble` opens a fresh binding scope per match; `bubble($name)` lets one
named outer variable pierce the scope.

The lesson is the framing, not the keyword: *repeated-metavariable equality and
multiple-independent-matches are the same knob.* `rwr` should pick the opposite default —
each match gets a fresh environment, repeated `$A` within one pattern requires equality —
but must say so explicitly in the v0.1 contract, and must define whether that equality is
**structural** (AST-equal, so `foo( a )` ≡ `foo(a)`) or **textual**. Recommend structural,
modulo the active isomorphism set, with the source text of the first binding used on
substitution.

### 10. Synvert's `Result#conflicted` — conflicts as data, not just an exception

[`node-mutation-ruby`](https://github.com/synvert-hq/node-mutation-ruby) sorts actions by
position, runs `get_conflict_actions` (interval overlap on `begin_pos`/`end`), and under
`NodeMutation::Strategy::THROW_ERROR` raises `ConflictActionError` — but it also exposes
`Result#conflicted` / `#affected?` as structured fields. Applying is a reverse-order flat
string splice (`source[start...end] = new_code`, back to front), which is entirely adequate
at file scale — no rope needed.

`rwr` should do both: refuse the transaction *and* emit, in JSON, the specific conflicting
edit pairs with their ranges and the rule/match that produced each. An agent that gets
`exit 1` with no detail cannot repair its own pattern. Also worth stealing: Synvert's
`group` — an explicit atomic bundle of sub-edits that conflicts as one unit, which maps
directly onto `rwr`'s multi-range rewrites.

Synvert's other strategy, `KEEP_RUNNING`, silently drops conflicting actions. That is the
exact behavior DESIGN.md §10 principle 2 forbids; offer it only behind an explicit flag, if
at all.

### 11. RuboCop's bounded fixpoint with checksum cycle detection

`RuboCop::Runner` ([`runner.rb`](https://github.com/rubocop/rubocop/blob/master/lib/rubocop/runner.rb))
runs `iterate_until_no_changes` with `MAX_ITERATIONS = 200` and a `check_for_infinite_loop`
that hashes each intermediate source and raises `InfiniteCorrectionLoop` — whose message
names the cops responsible for the cycle — if a checksum repeats. Rector runs the same
convergence loop with **no cap and no cycle detection**
([`FileProcessor::processFile`](https://github.com/rectorphp/rector-src/blob/main/src/Application/FileProcessor.php)),
which is a known footgun.

`rwr` should not iterate to fixpoint by default (DESIGN.md's determinism principle argues
for single-pass). But if `--fixpoint` ever exists: cap it, detect cycles by content hash,
and name the rules in the cycle. And note the deeper lesson — a fixpoint loop is where
"minimal diff" quietly dies, because each pass reformats a little more.

### 12. ast-grep's MCP surface: pattern-authoring tools, and no write tool

The [official ast-grep MCP server](https://github.com/ast-grep/ast-grep-mcp) exposes exactly
four tools: `dump_syntax_tree`, `test_match_code_rule`, `find_code`, `find_code_by_rule`.
Two observations:

- **Half the surface is pattern authoring, not searching.** `dump_syntax_tree` and
  `test_match_code_rule` exist because an LLM cannot reliably write a correct pattern
  first try; it needs to see the tree and dry-run the rule against a snippet. `rwr` should
  ship `rwr explain <pattern>` (show the Prism tree the pattern parsed to, and which
  isomorphisms are active) and `rwr test <rule.yml> --against <snippet>` as first-class CLI
  verbs, not just MCP tools.
- **There is deliberately no rewrite tool** — and [Semgrep's MCP server](https://github.com/semgrep/mcp)
  independently made the same call: it exposes `semgrep_scan`,
  `semgrep_scan_with_custom_rule`, `get_abstract_syntax_tree`, `semgrep_findings`,
  `supported_languages`, `semgrep_rule_schema`, and a `write_custom_semgrep_rule` prompt
  template — but nothing that applies an autofix and returns a diff. The agent reads
  `extra.fix` per result and splices it itself, which re-inherits the overlap-corruption risk
  from Q1. So the convergence is real but the outcome is bad: **both servers pushed the
  dangerous part back onto the agent rather than solving it.** `rwr` is better placed than
  either, because it can offer a rewrite tool that is safe by construction — atomic,
  overlap-checked, content-hash-preconditioned (DESIGN.md §7), and reversible. That is a
  differentiator, not a risk to avoid.

Note also that both servers ship an AST-dump tool. Two independent teams concluded an LLM
cannot write a correct pattern without seeing the tree.

### 13. Rector's `getRuleDefinition()` — every rule ships a runnable before/after example

`AbstractRector` requires `getRuleDefinition(): RuleDefinition` returning a description plus
non-empty `CodeSample[]` (before code, after code). Documentation is generated from it, and
the samples are executable tests. For `rwr`, requiring `examples:` in every rule YAML —
validated in CI by actually running the rule — makes DESIGN.md §6's ground-truth corpus a
byproduct of rule authoring rather than a separate effort, and gives an agent a worked
example to pattern-match against when writing its own rules.

### 14. GritQL's `AnalysisLog` as a separate JSON stream variant

GritQL's JSONL output emits a `MatchResult` enum with 9 variants; one of them,
`AnalysisLog` (file, severity, message, optional range), is a dedicated diagnostic channel
distinct from `Match` / `Rewrite` / `DoneFile`. `rwr`'s JSON should likewise keep matches,
edits, refusals, and known-unknowns in **separate typed streams** rather than overloading a
`message` string. JSONL (one record per line) is also the right shape for an agent loop —
it can act on the first records before the scan finishes.

### 15. Comby's whitespace-insensitivity as a free lexical isomorphism

Comby is parser-free and gets a large slice of the isomorphism benefit lexically: a single
space in a pattern matches any run of whitespace *including newlines* — *"Comby will match
the corresponding whitespace in the source code, but will not care about matching the exact
number of spaces, or distinguish between spaces and newlines"* — combined with Dyck-balanced
delimiter matching, so `:[hole]` stops at a newline outside delimiters but spans newlines
inside them. So `foo(a, b)`, `foo(a,b)`, and a call split across four lines are one pattern
with zero configuration.

`rwr` gets this for free from Prism and should say so — but the design lesson is sharper
than that. Comby demonstrates that **most of the practical value of "one pattern, many
layouts" is lexical, not semantic**, which is a useful prior when sizing the isomorphism
work in steal #2: implement the layout-invariance for free via the AST, then spend effort
only on the genuinely semantic Ruby variants (`unless`/`if !`, `&:sym`/block, `x&.foo`).

Take the lesson, not the method. Comby's Ruby is a table of literal delimiter pairs — `def`
closed by the token `end`, `do`/`end`, `class`/`end` — with no notion of heredocs, `%w[]`,
or `#{}` as sub-grammars, so a heredoc containing the word `end` truncates the enclosing
`def…end` match. Lexical balance is a fine way to get layout-invariance and a bad way to get
Ruby.

Comby's hole vocabulary is also the most honest about lazy matching of anything surveyed —
`:[hole]` is explicitly "zero or more characters in a lazy fashion," `:[[hole]]` is
identifier-shaped (alphanumeric + underscore), `:[hole:e]` is expression-shaped, `:[ hole]`
is whitespace-only, `:[hole\n]` is up-to-and-including-newline. Whatever `rwr` chooses for
`$$$`, document the laziness with the same directness.

Comby's `where` rules are the closest existing analogue to `rwr`'s `where:` block and worth
reading as a design precedent:

```
where :[left] == :[right], :[left] != "x == 500"
where match :[left] { | "x == 600" -> false | "x == 500" -> true }
where rewrite :[args] { ":[[k]]=:[[v]]" -> "\":[k]\": :[v]" }
```

Take: comma-separated conditions as implicit AND (GritQL agrees), `==`/`!=` between
captures, and a `match` form that constrains a capture against sub-patterns. Note that
Comby's `==` is **textual**, which is the choice `rwr` should *not* make (see Q4). Avoid:
*"Rewrite expressions always return true, even if they don't succeed in rewriting a
pattern"* — a nested transformation that silently no-ops is exactly the class of quiet
failure `rwr` refuses.

### 16. Semgrep's `metavariable-*` operator family and `focus-metavariable`

Semgrep's constraint vocabulary is the most mature precedent for what belongs in `rwr`'s
`where:` block, because Semgrep made the same architectural split — pattern is source-shaped,
constraints are separate YAML keys:

- `metavariable-regex` — constrain a binding's text (ast-grep's `constraints: { HOOK: { regex: '^use' } }`
  is the same idea).
- `metavariable-pattern` — constrain a binding by matching *another whole pattern* against it,
  optionally in a different language. This is the recursive form and it is what makes a
  constraint language stop needing a scripting escape hatch. **Take this.**
- `metavariable-comparison` — numeric/ordering predicates on a binding.
- `focus-metavariable` — report the *binding's* range as the match rather than the whole
  pattern's range. Directly useful for `rwr`: `find` on `PayrollService.calculate($A, $B)`
  where the caller wants the range of `$A`, and `rewrite` where the edit range should be a
  sub-node of the match. Without it, every "replace just this argument" rule has to
  restructure its pattern. **Take this.**
- Typed metavariables `($X: int)` / `(Logger $X).log(...)` — the type constraint lives in
  the *pattern*, not the `where:` block. `rwr` should put receiver-type narrowing in `where:`
  instead (as DESIGN.md §5 already does), because Ruby has no type syntax to borrow and
  inline types would break the copy-pasteable-Ruby property.
- `<... $X ...>` deep expression operator — "matches anywhere inside this expression."
  Equivalent to node_pattern's `` ` `` and GritQL's `contains`; three independent designs
  agree this operator is necessary.

Also worth noting for the isomorphism design: Semgrep's surviving built-in equivalences are
**import aliasing** (`subprocess.Popen(...)` matches an aliased import), **constant
propagation** (a hardcoded value tracked through an assignment), and **associative-commutative
operators**. The first two are semantic, not syntactic, and both have obvious Ruby analogues
(`include`d module method resolution; a symbol assigned to a constant then splatted). They
sit at the boundary between `rwr`'s isomorphism layer and its Phase 2 semantic layer.

### 17. node_pattern's expressiveness as the target for `where:`

What [`node_pattern`](https://docs.rubocop.org/rubocop-ast/latest/node_pattern.html)
expresses that Ruby-source-with-metavars cannot, and which of it belongs in `rwr`'s `where:`:

| node_pattern | Meaning | Verdict for `rwr` |
|---|---|---|
| `<a b ...>` | **any-order** sequence match | **Take.** This is how you match keyword args / hash pairs regardless of order — the single most Ruby-relevant operator here. |
| `` `return `` | descend: match anywhere in subtree | **Take** as `where: { contains: ... }` (GritQL's `contains`). |
| `^` / `^^` | ascend to parent | **Take** as `where: { within: ... }` (GritQL's `within`). |
| `[odd? positive?]` | intersection of constraints on one node | **Take** — `where:` is naturally an AND-list. |
| `{int float}` | union / alternation | **Take** — `where: { any: [...] }`. |
| `!int` | inline negation | **Take** — `where: { not: ... }`. |
| `int+`, `int*`, `int?` | repetition, captures as array | Covered by min/max counts (steal #8). |
| `_name` | named wildcard; repeats must be equal | Covered by repeated `$A` (steal #9). |
| `%1`, `%named` | pattern parameterization | **Take** — a parameterized rule is how you write one rule and run it for 40 method names. |
| `#divisible_by?(_value)` | predicate cross-referencing another binding | **Defer.** This is where a declarative `where:` starts wanting a scripting escape hatch. |
| `?method`, `#method` | arbitrary Ruby predicate calls | **Reject** — see avoid list. |

### Nothing to steal from fastmod — and that is the point

[fastmod](https://github.com/facebookincubator/fastmod) is a Rust rewrite of Facebook's
Python `codemod`: pure regex, interactive by default, and it deliberately *dropped* its
predecessor's `--start`/`--end`/`--count` flags and scripting API to stay small. It has no
structural awareness whatsoever and is widely used anyway. Two consequences for `rwr`: the
fast, simple, regex-shaped codemod niche is fully served and should not be contested; and
`rwr`'s entire value is the guarantee fastmod declines to offer, which means the guarantee
had better hold under adversarial input, because "structural but occasionally wrong" is
strictly worse than "regex and honest about it."

---

## Answers to the open questions

Keyed to [`open-questions.md`](open-questions.md), ordered by how conclusively prior art
resolved them — Q3 and Q5 are effectively answered, Q4 is a menu of settled options, Q1/Q2
are not answered by anyone. A section on agent/machine interfaces and a note on Q6 close it
out; Q7 (perf) has no useful prior art.

### Q3 — Overlapping and nested matches

Every tool surveyed resolves this, and they agree more than they disagree. Two layers must
be separated: **which matches to report** and **which edits may coexist**.

**Match layer:**

| Tool | Rule |
|---|---|
| ast-grep `find` | Reentrant — reports both outer and inner. |
| ast-grep `replace_all` | Non-reentrant pre-order — outermost only. Explicit TODO acknowledging `Some(Some(1))` is unsupported. |
| ast-grep rewriters | "Nodes on the higher level of AST… matched first"; per node, rewriters matched in order, first match wins. |
| Coccinelle | Token tagging — a transformed token is marked, and a second transformation of the same token raises *"already tagged token."* Refusal, not resolution. |
| GritQL | `bubble` controls whether one pattern produces N independent matches; `contains`/`within` are the explicit nesting relations. |
| RuboCop | Cops match independently; conflicts surface at the edit layer, plus a bounded fixpoint loop. |
| Comby | Outermost-only *by architecture, not by policy* — a single forward scan consumes each match and resumes after it, so overlaps cannot arise. Opt-in `-rule 'where nested'` re-runs the matcher inside each hole's text and flattens the results. |
| Semgrep | Reports **both** — the engine visits every expression and sub-expression independently. [#3991](https://github.com/semgrep/semgrep/issues/3991) confirms this is correct-as-designed; dedup is fingerprint-based, not containment-based. |

**Edit layer** — the convergent answer:

- **`TreeRewriter`:** nested is fine if the outer action doesn't replace; partial overlap is
  `ClobberingError`; overlapping deletions fuse; identical ranges merge per policy.
- **GritQL:** same invariant, enforced — nest or disjoint, never partial overlap; apply
  sorted `(end desc, start desc)`.
- **Synvert:** positional overlap detection, then `THROW_ERROR` or silent-drop.
- **ast-grep:** silently skips overlapping edits during template substitution
  ([`transform/rewrite.rs`](https://github.com/ast-grep/ast-grep/blob/main/crates/config/src/transform/rewrite.rs):
  `// skip overlapping edits / if start > pos { continue; }`) — silent, but at least it
  produces parseable output.
- **Comby:** *no overlap guard at all in the rewrite path.* `Rewrite.substitute_matches`
  assumes a sorted non-overlapping match list. Combine `-rule 'where nested'` with a rewrite
  and it emits genuinely unparseable output — silently, at exit code 0.
- **Semgrep:** no overlap guard either, with reproducible corruption on record —
  [#3577](https://github.com/semgrep/semgrep/issues/3577) (two fixes on one line produce
  `return wrap("wrap("b") + "b";`), [#3388](https://github.com/semgrep/semgrep/issues/3388)
  and [#4428](https://github.com/semgrep/semgrep/issues/4428) for multiline matches.
  [#2324](https://github.com/semgrep/semgrep/issues/2324) states plainly there is only one
  autofix per rule and the fix logic is "tightly coupled to matching logic."

The last three are the case for `rwr`. **Two of the three most-used structural rewrite tools
in the world will silently emit invalid code when edits overlap**, and the two that don't —
`TreeRewriter` and GritQL — both got there by making non-overlap a checked invariant rather
than an emergent property of scan order. That is a small amount of code and it is `rwr`'s
cheapest credible correctness claim.

**Recommendation for `rwr`:** `find` reports all matches with explicit nesting metadata.
`rewrite` selects outermost-only by default (`--nesting` to change it), builds a
TreeRewriter-style action tree, and **aborts the whole transaction on partial overlap** with
the conflicting ranges reported as data. Do not iterate to fixpoint. What goes wrong with
each alternative: innermost-only surprises on `foo(foo(a,b), c)` where the intent is usually
the outer call; all-with-apply produces output that doesn't parse; fixpoint destroys
minimal-diff and needs cycle detection (RuboCop) or hangs (Rector).

### Q5 — Lossless rewriting

Three architectures exist, and `rwr`'s is the third and cheapest:

1. **Diff-and-reprint (recast, nikic/PHP-Parser).** Keep the original tree; after mutation,
   structurally compare new-vs-old per subtree; splice verbatim original text where they
   match, pretty-print where they don't. recast's `findReprints`/`findChildReprints` walk
   the union of keys excluding `loc`; PHP-Parser's `printFormatPreserving` uses an
   `origNode` attribute set by `CloningVisitor` and reprints the original token range where
   the node is attribute-identical. **Correctness depends on the completeness of an
   equality predicate**, and every case the predicate doesn't model is a silent formatting
   regression: recast [#296](https://github.com/benjamn/recast/issues/296)/[#297](https://github.com/benjamn/recast/issues/297)
   (comments move parens, breaking round-trip), [#429](https://github.com/benjamn/recast/issues/429)
   (array insertion loses indentation because the parent must fully reprint),
   [#914](https://github.com/benjamn/recast/issues/914)/[#1191](https://github.com/benjamn/recast/issues/1191)
   (over-parenthesization with zero AST change), [#1386](https://github.com/benjamn/recast/issues/1386)
   (spurious JSX parens, open). PHP-Parser's own docs concede it "works on a best-effort
   basis and may sometimes reformat more code than necessary"
   ([#344](https://github.com/nikic/PHP-Parser/issues/344)).
2. **Concrete-tree-by-construction (LibCST, OpenRewrite).** Every byte lives in the tree —
   LibCST's typed `SimpleWhitespace` / `ParenthesizedWhitespace` / `TrailingWhitespace` /
   `EmptyLine` nodes, OpenRewrite's `Space prefix` on every `J` node. Printing is a pure
   depth-first fold (`Module.code_for_node`, `JavaPrinter.visitSpace`) with **no diffing at
   all** — losslessness is a parser invariant, not a runtime decision. Cost: every grammar
   production needs an explicit whitespace-ownership rule, exhaustively round-trip-fuzzed.
   LibCST's stated invariant is worth internalizing: *"A statement should own the comments
   directly above it, and any trailing comments on the same line. If we delete that
   statement, the whitespace should disappear with it."*
3. **Source-range splice (`rwr`, Comby, Semgrep's fallback path).** Never build a concrete
   tree; treat the source bytes as ground truth and emit new text only for ranges a rule
   explicitly touches. Comby is the extreme case — no IR at all, `Buffer.add_substring`
   copies every unmatched span verbatim — and it is trivially lossless *for a single
   non-overlapping match set*, which is exactly why its losslessness collapses the moment
   overlaps are allowed. `rwr` indexes the same splice by Prism locations, which gets it
   LibCST's *guarantee* (no equivalence predicate to be incomplete — untouched bytes are
   copied) at recast's *cost* (reuse an existing AST).
4. **Hybrid (Semgrep autofix).** Parse the `fix:` template to an AST, substitute bound
   metavariables into it, then pretty-print while *"recycling the original text of AST nodes
   taken unchanged from either the target or the fix template"* — regenerate only genuinely
   new subtrees, with a legacy line/column text-splice as fallback. The instructive part is
   the coverage: expression-level only, and only Python and JS/TS. **Ruby is on the naive
   offset-splice path.** A hybrid printer is real work per language, and Semgrep — with far
   more resources than this project — has shipped it for two.

**So: is there a general principle?** Yes, and it is the invariant that makes (3) as safe as
(2): *every splice-able range must be a closed superset of every byte its subtree is
lexically responsible for.* Prism violates this for heredocs, so `rwr` must restore it with
the transitive `effective_range()` of steal #3, and must make that the *only* range the
capture API exposes. Given that, the rule is general rather than per-transformation-shape —
with one caveat: insertion points must also be phrased relative to `effective_range`, since
the RuboCop bug class is about inserting at a physically-near-but-logically-wrong offset,
not about extracting a truncated range.

**Ruby heredocs appear to be genuinely unusual among mainstream languages.** Python's
implicit concatenation and f-strings, JS template literals and JSX, and Java text blocks are
all contiguous; none of recast, LibCST, or OpenRewrite has any "node owns bytes elsewhere"
concept because none of their languages need one. The only prior art is `whitequark/parser`'s
`Map::Heredoc`, and that is a hardcoded special case, not a general mechanism. `rwr` is
building the generalization.

Corollary worth writing down: **byte order is not tree order near heredocs.** Any algorithm
of the form "find the next node after this one" will misbehave.

### Q4 — Metavariable semantics

What the field has settled on:

**Single vs sequence.** Universal split: ast-grep `$A` / `$$$ARGS` (plus `$$UNNAMED` to
match unnamed CST nodes), Semgrep `$X` / `$...ARGS` / bare `...`, Comby `:[hole]` /
`:[[hole]]` / `:[hole:e]`, GritQL
`$x` / `$...`, Coccinelle's `list` modifier on metavariable declarations, node_pattern
`_` / `...`. IntelliJ SSR is the outlier and the more elegant design: one min/max count pair
per variable subsumes single, optional, sequence, and must-not-appear.

**Laziness matters and should be documented.** ast-grep's FAQ: `$$$MULTI` uses lazy
matching and stops at the first node matching what follows, chosen to keep matching linear
time. `rwr` will face the same choice on `foo($$$A, context: $C)`.

**Repeated metavariables — and the equality question.** Comby's `where :[a] == :[b]` is
explicitly a **textual** comparison, which means `foo( x )` and `foo(x)` are unequal. Since
`rwr` has a real AST, structural equality is available and strictly better, and the
isomorphism set (steal #2) defines exactly how far "structural" reaches. Say which one in
the v0.1 contract; this is the sort of thing that is unfixable after release.

Coccinelle enforces equality and adds `pure` as an explicit
marker that a metavariable is side-effect-free and therefore safe to duplicate in the
output — a distinction `rwr` will need the moment a `rewrite:` template mentions `$A` twice.
node_pattern's `_name` named wildcards require equality across occurrences. GritQL makes
the whole binding environment file-scoped by default (see `bubble`).

**Coccinelle's typed metavariable vocabulary is worth mining.** `expression`, `identifier`,
`statement`, `type`, `constant`, `local idexpression`, `parameter list`, `expression list`,
`declaration`, `field`, `iterator`, `declarer`, `position`, `symbol`, `fresh identifier`.
Two are directly useful for Ruby: **`position`** (bind a location without binding a node —
lets a rule report a site without transforming it, which is exactly what known-unknowns
needs) and **`fresh identifier`** (generate a guaranteed-unused name in the output — needed
for any rewrite that introduces a temporary).

**Recommendation for `rwr`, as public contract:**
- `$A` binds exactly one Prism node.
- `$$$A` binds a possibly-empty sequence in an argument/parameter/statement list. Document
  greedy vs lazy explicitly.
- Repeated `$A` in `match:` requires **structural** equality modulo active isomorphisms.
- Keyword args, blocks, and splats are matchable but need dedicated forms, because Prism
  models them as distinct node types (`KeywordHashNode`, `BlockNode`/`BlockArgumentNode`,
  `SplatNode`). `$A` should *not* silently match a block passed to the call it's inside.
  Arity and presence/absence go in `where:` via min/max counts.
- `rewrite:` may only mention metavariables bound in `match:`, plus explicit fresh-name
  generators. An unbound metavariable in `rewrite:` is an error at rule-load time, not a
  runtime empty string.

### Q1/Q2 (partial) — refusal and blind-spot reporting

See the novelty check below; the short version is that **file-level** blind-spot reporting is
standard (Semgrep, OpenRewrite, LibCST, jscodeshift) and **semantic** blind-spot reporting is
not shipped anywhere found.

Reporting taxonomies worth merging:

- **LibCST codemods:** `TransformSuccess` / `TransformFailure(error, traceback, warnings)` /
  `TransformSkip` / `TransformExit`, with an unconditional aggregate line — *"Transformed N
  files successfully. Skipped N files. Failed to codemod N files. N warnings were
  generated."*
- **jscodeshift:** exactly four per-file buckets — `error`, `ok`, `nochange`, `skip` — where
  `skip` is decided at discovery time and `nochange` means "ran, found nothing."
- **Semgrep:** the typed `skip_reason` enum above.

`rwr` should ship **five** buckets, splitting what jscodeshift conflates:
`changed` / `no_match` (fully analyzed, genuinely nothing) / `unsupported` (construct present
but outside coverage) / `refused` (matched but the edit was unsafe — heredoc detachment,
overlap, ambiguity) / `error` (parse failure). The `unsupported` and `refused` buckets are
the ones nobody else has, and they are the product.

### Agent and machine interfaces — what the field got right and wrong

**JSON output.** Semgrep has the best schema by a distance: generated from an
[`.atd` interface definition](https://github.com/semgrep/semgrep-interfaces/blob/main/semgrep_output_v1.atd)
so the schema is a checked artifact rather than prose, with typed enums for skip reasons and
error types, and byte offsets alongside line/column so a consumer can splice without
reparsing. Its flaws are the ones listed above: an opaque `metadata` blob, no top-level
completeness flag, `skipped` gated on `--verbose`. ast-grep is close behind — clean
`metaVariables: { single, multi, transformed }` with `range.byteOffset`, and three modes
(`--json=pretty|stream|compact`) — but has **no representation for errors or skipped files
at all**, which is unsurprising given tree-sitter never reports a failure. GritQL's JSONL
`MatchResult` enum with a dedicated `AnalysisLog` variant is the best *stream* shape.
Comby is the cautionary tale: per-file rather than per-match objects, `-1` line/column in
rewrite mode, always-0 exit, corrupted output under default parallelism.

For `rwr`: generate the schema from a checked definition (Rust types + `schemars`, published
in the repo), emit JSONL by default with typed record variants (`match`, `edit`, `refusal`,
`unknown`, `summary`), byte offsets *and* line/column everywhere, no untyped blobs, and a
`summary` record that always states completeness.

**MCP.** Both official servers ([ast-grep](https://github.com/ast-grep/ast-grep-mcp),
[Semgrep](https://github.com/semgrep/mcp)) converge on: dump-the-AST, validate-a-rule,
search — and no apply-a-fix tool. The AST-dump convergence is a genuine finding about how
agents fail (they can't write patterns blind). The missing write tool is not a finding to
copy; it is an unsolved problem `rwr` is better equipped to solve, per steal #12.

**Iteration ergonomics.** The pattern that recurs across ast-grep, Semgrep, GritQL, and
Rector is that an agent's loop is *write pattern → discover it matches nothing → repair*.
Everything that shortens that loop is high-value: an AST dump, a rule tester against a
snippet, and — the one nobody ships well — a **"why didn't this match?"** explainer.
Semgrep's `--matching-explanations` is the only attempt found; Coccinelle's `--track-iso`
is the nearest analogue for explaining why something *did*. `rwr` should treat
`rwr explain` as a first-class verb, reporting the Prism tree, the active isomorphisms, and
for a near-miss the first node where the pattern and the target diverged. That is probably
worth more to the primary consumer than any additional matching power.

### Q6 — Ruby version targeting

One practical note rather than prior art: Prism's C API supports version targeting
(`pm_options_version_set`), but the [`ruby-prism`](https://docs.rs/ruby-prism/latest/ruby_prism/)
crate's top-level `parse` takes only source. If per-rule Ruby version targeting is a
requirement, confirm the binding exposes it — or budget an upstream patch — before writing
it into the v0.1 contract.

---

## Avoid list

Mistakes these tools made that `rwr` is currently positioned to repeat.

1. **Silently dropping or blindly applying conflicting edits.** ast-grep's
   `// skip overlapping edits` and Synvert's `KEEP_RUNNING` produce a partially-applied
   rewrite with no signal; Comby and Semgrep produce *unparseable output* at exit 0. This is
   the single most direct violation of DESIGN.md §10 principle 2 available, it is what the
   two biggest tools in the category actually do, and it will arrive as a convenience during
   implementation.
2. **Leaving heredoc awareness to each rewrite rule.** This is RuboCop's actual state and it
   fails repeatedly across years and authors. Make the raw range structurally inaccessible.
3. **Best-effort format preservation with no caller-visible signal when it degraded.**
   PHP-Parser reformats more than necessary and says so only in prose. If `rwr` ever cannot
   produce a minimal edit, that must be a reported refusal, not a wider diff.
4. **Patching output with string regexes.** Rector's `BetterStandardPrinter` carries an
   `EXTRA_SPACE_BEFORE_NOP_REGEX` post-hoc fixup for whitespace its node-identity model
   introduced. When the range model leaks, fix the range model.
5. **Uncapped fixpoint iteration.** Rector's convergence loop has no cap and no cycle
   detection. If `rwr` ever iterates, cap it and detect cycles by content hash (RuboCop).
6. **Optional completeness reporting.** Semgrep's `paths.skipped` requires `--verbose`. If
   the account of what you couldn't see is opt-in, nobody sees it, and it stops being a
   differentiator.
7. **A second syntax for what the host language already says.** Synvert's NQL
   (`.send[receiver=.block[caller=.send[message=map]]][message=flatten]`) is exactly the
   node-constructor form DESIGN.md §5 rejected, wearing CSS clothes. Mine it for `where:`
   predicate ideas (`:has()`, `:not_has()`, dotted attribute paths, `.size`), not for syntax.
8. **An embedded scripting escape hatch in the constraint language.** IntelliJ SSR's Groovy
   script filters and GritQL's inline JS functions are arbitrary code execution during
   matching. For a CLI an agent drives autonomously, that is a security and determinism
   liability. `where:` stays small and declarative; the pressure to add scripting is real
   and should be answered with more `where:` predicates.
9. **Coarse error reporting for a machine consumer.** jscodeshift's summary gives an error
   *count*; Rector's JSON gives unified diffs but not new content
   ([#6888](https://github.com/rectorphp/rector/issues/6888) requested it after the fact).
   An agent deciding "repair my pattern or give up" needs per-record structure: file, range,
   reason tag, and the offending text.
10. **Docs that lag the mechanics that matter.** GritQL's conflict rules and ast-grep's
    overlap handling both had to be read out of source. Since `rwr`'s pattern syntax is a
    public contract from v0.1, Q3/Q4/Q5 semantics belong in the README, with examples, not
    in the implementation.
11. **Assuming your parser tells you when it failed.** Coccinelle silently drops files whose
    macros it can't resolve and then *reports success on code it never read.* This is the
    exact failure mode `rwr` exists to prevent, committed by the tool `rwr` most admires.
12. **An uninformative exit code.** Comby always exits 0 — "found nothing" and "ran fine" are
    indistinguishable without parsing stdout. Semgrep's documented table (0 clean, 1 findings
    with `--error`, 2 generic failure, 3 invalid syntax under `--strict`, 4 invalid pattern,
    5 bad YAML, 7 invalid rule, 8 unknown language) is the model. `rwr` needs at minimum
    distinct codes for: no matches, matches applied, matches found but refused, invalid
    pattern, and precondition/hash mismatch.
13. **Degrading to a weaker matcher instead of refusing.** Semgrep's own KB advises falling
    back from `metavariable-pattern` to `metavariable-regex` when the bound text contains a
    reserved word used as an identifier. A structural tool that quietly becomes a textual
    tool under load is worse than one that says it cannot answer.
14. **Implicit semantic magic in the match path.** Semgrep's constant propagation is on by
    default, so a pattern can match a call site via a literal that never textually appears
    there. `rwr`'s `where:` split exists precisely so that anything beyond the visible shape
    is opted into by name. Keep matching itself predictable.
15. **Lying about locations in one output mode.** Comby's rewrite-mode JSON hardcodes line
    and column to `-1`, so an agent that wants human-readable positions must make a second
    match-only call. Every `rwr` output mode should carry real byte offsets *and* real
    line/column.
16. **Unsynchronized parallel JSON output.** Comby's default 4-way parallelism interleaves
    and corrupts JSON-lines output ([#210](https://github.com/comby-tools/comby/issues/210),
    unfixed; the workaround is `-sequential`). If `rwr` parallelizes across files — and it
    will — each JSONL record must be written atomically under a lock.
17. **An opaque blob in an otherwise typed schema.** Semgrep's `extra.metadata` is raw JSON,
    which pushes parsing burden back onto the consumer and defeats the point of having a
    schema. Keep `rwr`'s output fully typed.

---

## Novelty check

Honest assessment of DESIGN.md §2's three claimed differentiators.

### Prism-grade Ruby fidelity — **real, and undersold**

Genuinely differentiated, and more so than DESIGN.md claims. ast-grep, GritQL, and Comby run
on tree-sitter or no parser at all; GritQL's Ruby is a bare tree-sitter submodule with no
Ruby-specific docs or examples; Comby is parser-free by design. The one competitor with equal
fidelity is RuboCop, on `whitequark/parser` — Ruby-native and battle-tested, and DESIGN.md is
right that the gap there is ergonomic, not parsing.

The strongest evidence is the one construct DESIGN.md §7 already singles out. **Every
competitor's Ruby heredoc handling is broken, in a different way, silently:**

- **Semgrep:** heredoc body content is not in the CST at all — the grammar declares it
  tree-sitter `extra` — so patterns cannot match into heredoc bodies, only around them
  ([#2258](https://github.com/semgrep/semgrep/issues/2258)). Nested heredocs with
  interpolation fail to parse ([#3151](https://github.com/semgrep/semgrep/issues/3151));
  squiggly-heredoc interpolation is mishandled
  ([#1580](https://github.com/semgrep/semgrep/issues/1580)). Plus the paren-normalization
  gap ([#5222](https://github.com/semgrep/semgrep/issues/5222): `method("foo")` ≠
  `method ("foo")`).
- **Comby:** a heredoc whose body contains the word `end` truncates the enclosing
  `def…end` match, because Ruby's `def`/`end` pair is a literal token pair and the heredoc
  body is scanned as ordinary text. Escaped quotes inside a double-quoted string can
  truncate a block match for the same reason. Silent, both.
- **RuboCop:** has the data (`Map::Heredoc`) and still ships corruption bugs, per steal #3.

Two more things fidelity buys that DESIGN.md underweights: Prism returns real `Diagnostic`s
(errors *and* warnings) and marks syntax-error sites with an explicit `MissingNode`, where
tree-sitter's recovery silently invents a plausible tree. That is not a nicety — it is the
only reason a completeness claim is possible at all. **A tool built on tree-sitter cannot
report parse blind spots, because its parser never admits to having any.** This is the
strongest available form of the D1 argument and it belongs in the README.

Verdict: **DESIGN.md undersells this.** "Table stakes" is right for pattern matching on
ordinary calls, but heredocs, `%w[]`, and interpolation are not exotic in a Rails monolith,
and on those the competition doesn't fail loudly — it silently returns fewer or wrong
matches. That is a Phase 0 benchmark waiting to be written: include two transformations
that touch heredoc-bearing call sites and score recall. It is still a precondition for the
other two claims rather than a standalone product, but it is a demonstrable one.

### Known-unknowns reporting — **the shape is standard; the content is not**

The shape — a structured account of what the tool didn't cover — is thoroughly precedented.
Semgrep has a 17-variant typed `skip_reason` enum in its JSON schema. OpenRewrite demotes
non-round-tripping files to `ParseError` and surfaces them via `FindParseFailures`. LibCST
reports `TransformFailure`/`TransformSkip` counts unconditionally. jscodeshift buckets every
file. `rwr` should not claim novelty for "reports what it skipped."

Semgrep goes furthest and gets closest: `PartialParsing` is a *sub-file* blind spot — one
method fails to parse, the rest of the file is still scanned, and the gap is recorded with a
span. That is genuinely the same species as what DESIGN.md §4 wants.

But every one of these is **mechanical**: *the analyzer could not process these bytes, and
here is the category.* None answers the question §4 actually poses: *this file parsed and
scanned perfectly, and there is still a `send(method_name)` on line 47 that might reach the
symbol you are rewriting.* That is a **semantic** blind spot inside successfully analyzed
code — not a failure of the analyzer, a limit of static analysis on a dynamic language — and
no tool surveyed emits it.

Two further gaps `rwr` can occupy even in the mechanical category, both cheap: nobody emits
a **top-level completeness flag** (Semgrep returns exit 0 with `PartialParsing` in a nested
array), and nobody makes the blind-spot report **unconditional** (Semgrep's `skipped` needs
`--verbose`; Comby swallows per-file exceptions with no output at all unless `DEBUG_COMBY`
is set).

Two near misses worth knowing about:

- **RubyMine's rename refactoring** has a "Search for dynamic references" checkbox and a
  conflicts dialog listing problems with "Refactor Anyway" / "Open in Find Window." This is
  the same *idea* — surface the dynamic-dispatch risk before applying — but it is an
  interactive human-in-the-loop dialog, not a machine-readable completeness report, and it
  is not composable into an agent loop.
- **Ruby LSP's Claude Code integration** (March 2026) exposes definition, find-all-
  references, and call hierarchy to a coding agent, and explicitly courts the rename use
  case: the agent queries the LSP for the complete reference list before changing anything.
  It claims to handle "common" metaprogramming cases but does not quantify accuracy or
  document failure modes. This is now the closest competitor to `rwr`'s *semantic* layer for
  the rename transformation specifically, and it is free, installed, and agent-integrated.

Verdict: **genuinely novel as specified**, with two qualifications. First, `rwr` should not
market "reports what it skipped" — it should market *"reports the dynamic-dispatch sites
that could reach your target and explains why it can't decide."* Second, Q1's tractability
risk is undiminished by this survey: nobody has shipped it, which is weak evidence it is
hard, not that it is unclaimed. Coccinelle's `position` metavariable (bind a site, report it,
transform nothing) is the closest usable mechanism found for expressing it.

### Semantic receiver-narrowing — **shipped elsewhere, not shipped for Ruby structural search**

The mechanism has strong prior art. LibCST's `QualifiedNameProvider` resolves `foo.bar()`
back through import bindings to `pkg.mod.bar` and tags each result with a
`QualifiedNameSource ∈ {IMPORT, BUILTIN, LOCAL}`, surfacing multiple candidates under
shadowing. OpenRewrite's LST carries resolved type attribution, which is the whole basis of
its recipe precision. IntelliJ SSR has a per-variable Type filter with "Exact match" and
"Apply within type hierarchy" toggles. Semgrep has typed metavariables. So `rwr` is not
inventing the concept.

What is absent is the combination: **no structural search-and-rewrite tool for Ruby offers
receiver-type narrowing.** ast-grep's FAQ states flatly that it does not support scope
analysis, type information, or dataflow. Synvert, Comby, and GritQL have no semantic layer.
RuboCop cops can hand-roll receiver checks in Ruby, per cop, imperatively.

Two honest caveats. First, Ruby's dynamism makes any such provider best-effort where
LibCST's is close to sound — which means it must ship with a confidence signal, not a
boolean. Second, Ruby LSP already answers the narrow rename case for agents. `rwr`'s
defensible ground is arbitrary *rewrite shapes* narrowed by receiver, not reference lookup —
`PayrollService#calculate(a, b)` → `PayrollService#calculate(a, b, context:)` is something
an LSP cannot express at all.

Verdict: real, but Q2 remains the right question — measure how far a symbol index plus local
inference gets on the public corpus before committing Phase 2.

---

## Sources

**ast-grep** — [rule config](https://ast-grep.github.io/guide/rule-config.html) · [match algorithm & strictness](https://ast-grep.github.io/advanced/match-algorithm.html) · [core concepts](https://ast-grep.github.io/advanced/core-concepts.html) · [FAQ](https://ast-grep.github.io/advanced/faq.html) · [rewrite code](https://ast-grep.github.io/guide/rewrite-code.html) · [transformation object](https://ast-grep.github.io/reference/yaml/transformation.html) · [JSON mode](https://ast-grep.github.io/guide/tools/json.html) · [`traversal.rs`](https://github.com/ast-grep/ast-grep/blob/main/crates/core/src/tree_sitter/traversal.rs) · [`tree_sitter/mod.rs` (`replace_all`)](https://github.com/ast-grep/ast-grep/blob/main/crates/core/src/tree_sitter/mod.rs) · [`config/src/transform/rewrite.rs`](https://github.com/ast-grep/ast-grep/blob/main/crates/config/src/transform/rewrite.rs) · [issue #1427 (parent/child ambiguity)](https://github.com/ast-grep/ast-grep/issues/1427) · [ast-grep-mcp](https://github.com/ast-grep/ast-grep-mcp)

**RuboCop / `whitequark/parser`** — [node_pattern reference](https://docs.rubocop.org/rubocop-ast/latest/node_pattern.html) · [`tree_rewriter.rb`](https://github.com/whitequark/parser/blob/master/lib/parser/source/tree_rewriter.rb) · [`tree_rewriter/action.rb`](https://github.com/whitequark/parser/blob/master/lib/parser/source/tree_rewriter/action.rb) · [`source/map/heredoc.rb`](https://github.com/whitequark/parser/blob/master/lib/parser/source/map/heredoc.rb) · [`runner.rb`](https://github.com/rubocop/rubocop/blob/master/lib/rubocop/runner.rb) · heredoc autocorrect bugs [#10895](https://github.com/rubocop/rubocop/issues/10895), [#10320](https://github.com/rubocop/rubocop/issues/10320), [#6653](https://github.com/rubocop/rubocop/issues/6653), [#11621](https://github.com/rubocop/rubocop/issues/11621) · [clobbering in nested UnlessElse #10375](https://github.com/rubocop/rubocop/issues/10375)

**Coccinelle / SmPL** — [SmPL grammar](https://coccinelle.gitlabpages.inria.fr/website/docs/main_grammar.html) · [`standard.iso`](https://github.com/coccinelle/coccinelle/blob/master/standard.iso) · [semantic patches overview](https://coccinelle.gitlabpages.inria.fr/website/sp.html) · [`spatch(1)`](https://manpages.debian.org/testing/coccinelle/spatch.1.en.html) · [The Semantics of "Semantic Patches" in Coccinelle](https://scispace.com/pdf/the-semantics-of-semantic-patches-in-coccinelle-program-40zj6qem77.pdf) · [already-tagged-token #390](https://github.com/coccinelle/coccinelle/issues/390) · [parse error detail #122](https://github.com/coccinelle/coccinelle/issues/122)

**Semgrep** — [pattern syntax](https://docs.semgrep.dev/writing-rules/pattern-syntax) · [rule syntax](https://docs.semgrep.dev/writing-rules/rule-syntax) · [autofix](https://docs.semgrep.dev/writing-rules/autofix) · [AST-based autofix (blog)](https://semgrep.dev/blog/2022/autofixing-code-with-semgrep/) · [CLI reference](https://docs.semgrep.dev/cli-reference) · [JSON and SARIF fields](https://docs.semgrep.dev/semgrep-appsec-platform/json-and-sarif) · [`semgrep_output_v1.atd`](https://github.com/semgrep/semgrep-interfaces/blob/main/semgrep_output_v1.atd) · [deprecated experiments (equivalences)](https://docs.semgrep.dev/writing-rules/experiments/deprecated-experiments) · [semgrep-ruby grammar](https://github.com/semgrep/semgrep-ruby) · [semgrep/mcp](https://github.com/semgrep/mcp) · issues [#5222 (paren normalization)](https://github.com/semgrep/semgrep/issues/5222), [#3991 (nested match suppression)](https://github.com/semgrep/semgrep/issues/3991), [#3577](https://github.com/semgrep/semgrep/issues/3577) / [#3388](https://github.com/semgrep/semgrep/issues/3388) / [#4428](https://github.com/semgrep/semgrep/issues/4428) (autofix corruption), [#2324 (one autofix per rule)](https://github.com/semgrep/semgrep/issues/2324), [#2258 (heredoc bodies absent from CST)](https://github.com/semgrep/semgrep/issues/2258), [#3151](https://github.com/semgrep/semgrep/issues/3151) / [#1580](https://github.com/semgrep/semgrep/issues/1580) (heredoc parsing)

**Comby** — [syntax reference](https://comby.dev/docs/syntax-reference) · [basic usage](https://comby.dev/docs/basic-usage) · [advanced usage (rules)](https://comby.dev/docs/advanced-usage) · [`matcher_engine.ml`](https://github.com/comby-tools/comby/blob/master/lib/kernel/matchers/matcher_engine.ml) · [`rewrite.ml`](https://github.com/comby-tools/comby/blob/master/lib/kernel/matchers/rewrite.ml) · [`metasyntax.ml`](https://github.com/comby-tools/comby/blob/master/lib/kernel/matchers/metasyntax.ml) · [`languages.ml`](https://github.com/comby-tools/comby/blob/master/lib/kernel/matchers/languages.ml) · issues [#318 (textual vs semantic equality)](https://github.com/comby-tools/comby/issues/318), [#330 (keyword substrings in identifiers)](https://github.com/comby-tools/comby/issues/330), [#298 (silent zero-match on unbalanced pattern)](https://github.com/comby-tools/comby/issues/298), [#210 (parallel JSON-lines corruption)](https://github.com/comby-tools/comby/issues/210)

**recast / LibCST / OpenRewrite** — [recast `patcher.ts`](https://github.com/benjamn/recast/blob/master/lib/patcher.ts), [`lines.ts`](https://github.com/benjamn/recast/blob/master/lib/lines.ts), [`comments.ts`](https://github.com/benjamn/recast/blob/master/lib/comments.ts), issues [#296](https://github.com/benjamn/recast/issues/296) [#297](https://github.com/benjamn/recast/issues/297) [#429](https://github.com/benjamn/recast/issues/429) [#914](https://github.com/benjamn/recast/issues/914) [#1191](https://github.com/benjamn/recast/issues/1191) [#1386](https://github.com/benjamn/recast/issues/1386) · [LibCST motivation](https://libcst.readthedocs.io/en/latest/motivation.html), [nodes](https://libcst.readthedocs.io/en/latest/nodes.html), [metadata](https://libcst.readthedocs.io/en/latest/metadata.html), [codemods](https://libcst.readthedocs.io/en/latest/codemods.html), [`whitespace.py`](https://github.com/Instagram/LibCST/blob/main/libcst/_nodes/whitespace.py), [`codemod/_cli.py`](https://github.com/Instagram/LibCST/blob/main/libcst/codemod/_cli.py) · [OpenRewrite LST](https://docs.openrewrite.org/concepts-and-explanations/lossless-semantic-trees), [`Space.java`](https://github.com/openrewrite/rewrite/blob/main/rewrite-java/src/main/java/org/openrewrite/java/tree/Space.java), [`ParseError.java`](https://github.com/openrewrite/rewrite/blob/main/rewrite-core/src/main/java/org/openrewrite/tree/ParseError.java), [`FindParseFailures`](https://docs.openrewrite.org/recipes/core/findparsefailures)

**Synvert / GritQL / Rector / jscodeshift / SSR / fastmod** — [synvert-core-ruby](https://github.com/synvert-hq/synvert-core-ruby), [node-query-ruby](https://github.com/synvert-hq/node-query-ruby), [node-mutation-ruby](https://github.com/synvert-hq/node-mutation-ruby) · [GritQL patterns](https://docs.grit.io/language/patterns), [conditions](https://docs.grit.io/language/conditions), [bubble](https://docs.grit.io/language/bubble), [biomejs/gritql](https://github.com/biomejs/gritql) · [Rector custom rules](https://getrector.com/documentation/custom-rule), [`FileProcessor.php`](https://github.com/rectorphp/rector-src/blob/main/src/Application/FileProcessor.php), [PHP-Parser format-preserving printing](https://github.com/nikic/PHP-Parser/blob/master/doc/component/Pretty_printing.markdown), [#344](https://github.com/nikic/PHP-Parser/issues/344), [Rector #6888](https://github.com/rectorphp/rector/issues/6888) · [jscodeshift `Runner.js`](https://github.com/facebook/jscodeshift/blob/master/src/Runner.js) · [IntelliJ SSR](https://www.jetbrains.com/help/idea/structural-search-and-replace.html), [search templates](https://www.jetbrains.com/help/idea/search-templates.html), [RubyMine rename refactorings](https://www.jetbrains.com/help/ruby/rename-refactorings.html) · [fastmod](https://github.com/facebookincubator/fastmod)

**Ruby ecosystem** — [`ruby-prism` crate docs](https://docs.rs/ruby-prism/latest/ruby_prism/) · [Ruby LSP](https://shopify.github.io/ruby-lsp/) · [Ruby LSP + Claude Code](https://www.damiangalarza.com/posts/2026-03-13-ruby-lsp-claude-code/)
