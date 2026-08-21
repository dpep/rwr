# Phase 0 conclusion

Phase 0 existed to answer one question — *should rwr exist at all?* — and to be willing to
answer no. This records the verdict, the evidence, and the places the evidence is thinner than
the plan asked for.

## Verdict: proceed

No kill criterion fired. The differentiators are built and demonstrated rather than argued,
and the incumbents fail on the corpus in ways that are reproducible rather than rhetorical.

## What the kill criteria said, and what happened

> Stop if ast-grep achieves >=95% recall at 100% precision on >=8 of 10 transformations, **and**
> fewer than 3 transformations require a constraint ast-grep/Comby cannot express.

| entry | tests | rwr | ast-grep | comby |
|---|---|---|---|---|
| 001 return-nil | the simplest possible rule | match | match | **corrupts** — rewrites a heredoc body, and turns `return nil_value` into `return_value` |
| 002 perf-detect | method-name alternation; shape-changing rewrite | match | matched, rewrote non-minimally | inexpressible |
| 003 hash-shorthand | cross-capture name equality | match | inexpressible | inexpressible |
| 004 sorted-array | sequence transforms, comment attachment | match | inexpressible | inexpressible |
| 004 refuses-shared-line | refusing on an ambiguous comment | refuses, exit 5 | — | — |
| 006 metaprogramming-residue | the differentiator under load | match | inexpressible | inexpressible |
| 007 receiver-rename | receiver narrowing | match | **inexpressible** | **inexpressible** |
| 008 heredoc-survival | D14 in every position a rename touches | match | — | comby truncates `def…end` on heredocs |
| 009 modern-syntax | pattern matching, endless methods, safe navigation | match | tree-sitter grammar — exactly what D1 doubts | — |
| 010 deep-inheritance | three levels, an override, an unrelated namesake | match | inexpressible | inexpressible |

Neither half of the criterion is met. ast-grep matches correctly on 001 and 002 but cannot
express 004 or 007, and its 002 rewrite collapses a multiline chain and rewrites a `do ... end`
block as braces. Comby fails the simplest rule in the corpus by producing **a different
working program**.

## What was measured

| measurement | result | consequence |
|---|---|---|
| (a) residue reporting | built; 8 reviewable items for a distinctive name, 4,547 for `name` | useful above a distinctiveness threshold; scoped by class anchor |
| (b) bare-name collateral | renaming `Foo#name` touches **2,067 rails sites** | D6's premise confirmed with a number |
| (c) receiver resolution | ~59% of call sites resolve with **no type inference** | Q2 answered favourably; no Sorbet needed |
| (d) cold parse | ~85-100 MB/s; rails under 200 ms | **D5 settled** — no cache, no index, no staleness |
| D1 fidelity | **zero parse failures** across 5,499 application files | Prism has no gaps on real Ruby |

## The differentiators, demonstrated

**Receiver narrowing.** `node_pattern` has no notion of a receiver, ast-grep's FAQ disclaims
type analysis, and Ruby LSP matches methods by bare name. Corpus 007 renames
`Account#display_name` across a hierarchy — the definition, an override, and call sites on
locals — while leaving `Company#display_name` and `Account.display_name` untouched.

**Residue reporting.** No other tool reports what it *could not* see. For
`$R.autoload_paths` on rails, rwr names the `attr_accessor :autoload_paths` a rename would
silently break.

**Minimal diffs.** On 002 ast-grep finds every correct site and rewrites non-minimally; rwr
edits only what changed, leaving layout and block spelling alone.

**Refusing rather than guessing.** A comment that cannot be unambiguously attached declines at
exit 5 with the source untouched.

## Where the evidence is thinner than planned

Stated plainly, because a conclusion that hides its weaknesses is not evidence.

- ~~The corpus is 4 entries, not the 10 the plan called for.~~ **Addressed** -- nine entries
  now, each testing something specific rather than padding a count: metaprogramming residue,
  heredocs in every position a rename touches, modern syntax (pattern matching, endless
  methods, safe navigation, rightward assignment), three levels of inheritance with an
  override, and cross-capture name equality. See `corpus/`.
- **Measurement (a) never ran against hand-verified ground truth.** Residue was measured for
  *volume and class* on rails, not for *recall* against renames a person had actually done.
  Its pass bar — "catches the dynamic reaches a human found" — remains unverified.
- **No private-monolith run.** All numbers come from public corpora. The scaling threshold
  (~150-200k files) and the residue noise floor are extrapolations until one lands.
- **ast-grep was scored on 4 entries by one author.** The corpus was written by the party whose
  project survives if the incumbents fail, which is the selection-bias risk the plan named.
  Mitigated by the entries being reproducible and by comby's failure being an outright
  corruption rather than a judgement call, but not eliminated.

## What Phase 0 changed about the design

The measurements were not a formality; they reversed decisions.

- **D5** — the cache was cut because parsing turned out to be cheap.
- **D20** — residue degradation keys on receiver-shape diversity, not frequency, because
  `create` turned out to be common *and* tractable.
- **D51/D52** — inheritance went from "roadmap" to built, because a rename over a subclass
  produced demonstrably broken code.
- **D53** — trailing commas left the rule list, because a test showed they are invisible to
  rwr's own equality.

## Next

Phase 1 and Phase 2 are, in substance, done: the engine, the semantic layer, and the corpus
all exist. What remains is release, real use, and the monolith measurement that would replace
the extrapolations above with numbers.
