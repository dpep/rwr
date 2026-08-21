# Open questions

Unresolved design risk, roughly by severity. Q3, Q4, and Q5 are **closed** by the prior-art
survey and recorded as decisions; kept here with their resolutions for continuity.

---

## Q1 — Does residue reporting hit its pass bar? *(highest risk — the thesis)*

Reformulated from "can we report dynamic-dispatch reachability" (intractable) to
**name-scoped residue reporting** (D7 amended, DESIGN.md §4). The reformulation is lexical
plus AST context and needs no dataflow, so it is clearly *implementable* — what is unproven
is whether it is *useful*.

Unvalidated: whether the classified residue for a real rename is reviewable rather than
screen-filling, and whether the "identifier too common, not enumerated" degradation is
honest-and-useful or just a polite failure.

**Now scheduled as Phase 0 measurement (a)**, with a concrete pass bar: 3 real monolith
renames with hand-enumerated ground truth; residue reporting must catch the dynamic reaches
a human found. If it fails and ast-grep passes the syntactic partition, the honest outcome
is to contribute upstream and stop.

## Q7 — Perf targets are unfalsifiable until Phase 0

No latency numbers are set because none are measured. Phase 0 must produce cold parse (both
corpora), single-file reparse, repo-wide query, and rewrite+verify. Targets get set *after*
those land — picking numbers now would be a guess wearing the clothes of a requirement.

## Q8 — Are Coccinelle-style isomorphisms viable, or a known trap? *(new)*

Named, individually disableable, position-typed equivalence rules — treating syntactically
different but semantically equivalent code as one pattern. Ruby arguably needs them more
than C does (`unless`/`if !`, `&&`/`and`, block vs `do…end`, safe navigation).

**But Semgrep shipped exactly this** (`equivalences:`, e.g. `$X + $Y <==> $Y + $X`) and
**deprecated it in v0.61.0 with no stated reason**, keeping only a fixed built-in set. That
is strong negative evidence from the closest comparable, and the missing rationale is the
interesting part — combinatorial blowup? unpredictable matches? nobody used it?

Treat as a Phase 0 hypothesis, not a free win. Worth 30 minutes finding out why Semgrep
pulled it before designing anything.

## Q10 — The refusal contract guards edit mechanics, not match semantics *(new, severe)*

Across seven realistic case studies (`docs/use-cases.md`) no natural exit-3 refusal could be
constructed — while **two confidently wrong rewrites shipped at exit 0**: a
`Company#full_name` site matched when `User#full_name` was meant, and an impure
repeated-metavariable match (`rows.shift.present? ? rows.shift : nil`).

The safety story in DESIGN.md §4 protects against *conflicts the tool can detect*. The
actual danger is a clean, confident, semantically wrong match — and nothing guards it before
Phase 2.

This reframes Phase 2 from "makes matching usable at scale" to "is the only thing between
the user and silent breakage," and it strengthens the case that Phase 0 measurement (b) is
the real go/no-go rather than a supporting number.

Open: is there any *syntactic* guard worth having in Phase 1 — refuse when a bare method
name matches across more than N distinct receiver shapes? Or is honest degradation
(ship Phase 1 as explicitly-unsafe-without-review) the better answer?

## Q12 - RuboCop positioning: resolved as complementary, personal-corpus-first

**Corrected premise.** This originally recorded concrete-syntax transformations as impossible
because `and`/`&&` and hash rockets parse to identical trees. False - Prism retains operator
locations, so both are distinguishable by a `where:` predicate (pinned by
`source::tests::operator_spelling_survives_parsing` and
`hash_rocket_is_distinguishable_from_shorthand`). The modernization family is in scope.

**Resolved goal (author, this session):** not to displace RuboCop industry-wide, but to be a
faster, more ergonomic engine for *a personal rule corpus* - port a handful of favourite
rules, write custom ones RuboCop cannot express, and use it to fix the author's own repos.

That dissolves the coverage-threshold objection entirely. The argument "a fast engine below
~500 cops is worthless" applies to *displacement*; it does not apply to a corpus of twenty
rules someone actually wants. It is also not a scope change - `rwr check` (D22) is already the
linter verb, and porting rules is just writing rules.

**Concrete target: draining `.rubocop_todo.yml`.** Mechanical, painful, high-volume, and
already in the author's tooling - the `mrclean` skill explicitly handles rubocop_todo entries,
so rwr becomes its engine rather than a parallel tool.

**Caution to resolve early:** some cops will not port. `Style/FrozenStringLiteralComment`
needs a file-level insertion with no node to anchor to, which is on the genuinely-out-of-reach
list, as is anything needing coordinated multi-site edits. The port list should be checked
against the boundary before Phase 0, not during it.

**Blocked on input:** which rules are the favourites. That determines which `where:`
predicates get built first (the case studies flagged three candidates), and it is a better
Phase 0 corpus than anything derived from invented case studies.

**Still open:** whether "safer autocorrect" is worth marketing. RuboCop ships
heredoc-corruption bugs that D14 is built to prevent; Phase 0 demonstrates it either way.

## Closed

### Q3 — Overlapping and nested matches → **D15**
Conflict unit is the edit range, not the match range. `find` reentrant, `rewrite`
outermost-only, partial overlap aborts, no auto-fixpoint. Bonus: Comby and Semgrep both
silently emit unparseable output here at exit 0, which is a free correctness claim.

### Q4 — Metavariable semantics → **D16**
IntelliJ SSR's min/max occurrence counts unify single/optional/sequence/must-not-appear
under one mechanism. Repeated metavariables use AST equality, not textual.

### Q9 - Ruby LSP as a Phase 2 competitor -> resolved: **Phase 2 survives**
Read from source rather than docs. `ReferenceFinder` matches methods by **bare name
equality** (`MethodTarget.new(node.name.to_s)`, then `node.name.to_s == @target.method_name`)
- no receiver, arity, or defining-class narrowing. That is precisely the false-positive
problem measurement (b) exists to quantify: the incumbent has it.

`rename` is constants-only even in the editor and is not exposed to agents at all. The
"handles metaprogramming" claim does not survive source reading - no visitor is registered
for `SymbolNode` or string nodes, so `send(:foo)`, `define_method`, `delegate` and
`alias_method` sites are invisible, and nothing reports that. It also requires a working
`bundle install`, silently excludes `*_spec.rb` from its index, and takes ~35s for one
find-references on a 40k-file repo.

**Consequences:** constants *are* resolved well - treat constant navigation as taken and do
not rebuild it. Phase 0 measurement (c) narrows to **receiver resolution for methods**, the
only part no plausible LSP exposure covers.

### Q13 - Post-hoc constraints missing alternative bindings -> **resolved**
A constraint rejection now *forbids that binding and re-matches*, which forces backtracking to
a different one, rather than discarding a completed match. Terminating, because each retry
forbids one more finite binding; `MAX_REBINDS` is a backstop rather than a budget.

`Criteria` moved inside `search`, so the search returns matches that satisfy the rule instead
of the caller filtering afterwards. The separation the original design prized -- a constraint
may not change *what* matched -- is preserved in substance: a constraint still cannot invent a
match, only decline one and let the search look again.

**The original framing was slightly wrong**, and the distinction matters. Two different things
look alike:

- *A rejected binding.* `{name:, size: size}` binds `$K` to `name:` first, which fails
  `same_name_as` because an implicit value has no identifier. This is what the retry fixes,
  and without it the second pass over a partly-shortened hash found nothing at all.
- *Several **valid** bindings on one node.* `{name: name, size: size}` -- both pairs satisfy
  the constraint, and `search` reports one match per node by design. That still needs a second
  pass, and correctly so: rwr does not iterate internally because `foo($A) -> foo(bar($A))`
  matches its own output and diverges (D15). The caller loops; the corpus runner applies to a
  fixpoint, as a real consumer does.

Pinned by `matcher::tests::a_rejected_binding_is_retried_not_abandoned` and by corpus 003,
whose two-pair hash converges in two passes.

### Q5 — Heredoc-safe rewriting → **D14**
A general rule *is* derivable: `effective_range()` = transitive closure over descendants
unioning heredoc `closing_loc`, at splice time. The enforcement matters more than the rule —
never expose raw `.location` from the capture API.


## Q6 — Ruby version targeting — **closed**

**A rule declares the floor its *output* needs; the codebase declares what it is.** Not the
other way round: the version is a property of the repository, and a pattern that had to name
one would stop being portable between repos, which was the original worry.

```yaml
ruby: '3.1'     # `{foo:}` is a syntax error before this
```

The premise the question rested on turned out to be false in practice: Prism can parse for a
specific Ruby version, but the `ruby-prism` crate exposes only `parse(source)` and passes a
null options pointer, so version-targeted parsing is not reachable. That is fine, because it
would have been the wrong mechanism anyway — the danger is not that rwr *reads* new syntax,
it is that rwr *writes* it.

**And `verify` cannot catch this one.** Reparsing the output proves it is valid Ruby to
*Prism*, which is modern Ruby. `{foo:}` sails through and then breaks on the target's 2.7.
This is exactly DESIGN.md §4's dangerous failure — clean, confident, wrong — and the only
guard is knowing what version the codebase targets.

Detection reads `.ruby-version`, a Gemfile `ruby` line, or a gemspec's
`required_ruby_version`, walking up to the repository root. All three were needed: rails
declares it only in a gemspec, discourse and mastodon only in a Gemfile. `--ruby X.Y`
overrides.

**An undetected version is not permission to assume the newest.** The rules are held back and
the run says so, the same shape as the unsafe gate (D57) — a rule that did not run must not
look like a rule that found nothing.

## Q2 — Does receiver-narrowing work without Sorbet? — **closed, with a boundary**

**Yes for receivers named directly; no for chained ones, permanently.**

Recoverable without a type source, and shipped: `self` (singleton-aware), constants, locals
and instance variables assigned from a constructor, and — since D61 — chains that carry their
own answer (`Widget.new.foo`, and identity methods passing a type through).

Not recoverable, and now measured rather than assumed: **chained receivers**, 15.8-27.4% of
call sites. Following a chain needs a method's return type, and only **2.3-4.5%** of method
definitions state one syntactically; **70% end in another call**, so inference recurses into
more unknowns. See `docs/phase0-results.md` for the three measurements and D61 for the
decision.

**The OSS value proposition survives**, because the failure is under-matching rather than
mis-matching: an unresolved receiver does not match, is not rewritten, and is reported as
residue. A rule narrowed by `type:` is exactly as correct on a repo with no signatures — it
simply reaches fewer sites, and says which ones it could not reach.

**This was also the real case for RBS/Sorbet ingestion**, and a much narrower one than "it
would help receiver narrowing": it turns that 70% from inference into data. **Since D62 it is
built, for Sorbet's inline signatures** — 64% of them name a class rwr can use, against 3.9%
inferable from the same repository's syntax. It needed no Sorbet and no RBI parser, because a
`sig` block is ordinary Ruby.

Still open, and deliberately: **RBS** (`sig/*.rbs`), which is a genuinely different grammar
rather than Ruby, and **RBI files for gems**, which would reach typed receivers from
dependencies rather than from the repository's own classes.

## Q11 — Non-Ruby templates are invisible, so residue over-claims — **closed**

The question turned out to be two, and only one of them was about templates.

**Half of it was not templates at all: it was Ruby rwr refused to open.** `.rb` is not the
whole language. Discourse keeps **11,854 lines** of Ruby in `.rake` files, a Gemfile and a
gemspec; rails keeps 3,102; mastodon 2,616. rwr walked past every one — so a rename skipped
them *and* the residue report claimed completeness without having read them, which is the
worse half. Fixed: `.rake`, `.ru`, `.gemspec`, `.jbuilder`, `Rakefile`, `Gemfile`,
`Vagrantfile` and the rest are Ruby now, and the list is deliberately narrower than
RuboCop's, which also claims `.spec` and `.schema` — extensions that are Ruby in some
projects and something else in others.

**The templates half is answered by narrowing the claim, which was option (a).** Every
report that makes a completeness claim now says what it did not read:

```
note: 356 template file(s) were not searched. rwr reads Ruby, and .erb/.haml embed it
      -- so this account covers Ruby only (Q11).
```

356 on mastodon, 106 on discourse. The note stands alone rather than riding on the residue
list: a rule that accounted for everything in Ruby still did not look at ERB, and a blind
spot that appears and vanishes with unrelated results is not a report.

Option (b) — grep-grade residue inside templates — was judged "probably what users actually
want", and may still be. It is a different feature from an honest claim, and shipping the
honest claim first means it can be added without anyone having been misled meanwhile.
