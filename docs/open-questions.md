# Open questions

**None are open.** Every question raised during design has been answered, and each is kept
here with its resolution — the reasoning is worth more than the verdict, and several were
answered *against* the assumption that raised them.

Four were answered by measurement rather than argument, and all four moved:

| | assumed | measured |
|---|---|---|
| Q1 residue | reviewable | 7/7 recall, but only reviewable for a distinctive name |
| Q2 receivers without Sorbet | mostly recoverable | yes for named receivers, never for chained |
| Q8 isomorphisms | a known trap, per Semgrep | Semgrep's deprecation was a batch sweep; the real signal is subtler |
| Q11 templates | a templates problem | half of it was Ruby rwr refused to open |

A question that gets closed by its own evidence is worth more than one that gets closed by
being forgotten, so this file records the route as well as the answer. New questions go at
the bottom, open, until something measures them.

---

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
want", and **that turned out to be right**: a user testing against a private monolith asked
for exactly it, unprompted, having hit the under-reporting the honest claim only *described*.
It now ships.

Templates are searched for the anchored identifier at whole-identifier boundaries and
reported as their own class, `Text`, apart from everything the parser found:

```
194 occurrence(s) in 356 template file(s), found by text search rather than parsed --
rwr does not read .erb/.haml, so these may be comments or unrelated text:
  app/views/accounts/show.html.haml:9:23: = account.name
```

On mastodon, a rename of `User#name` finds 145 parsed occurrences and 194 in templates —
so more than half the account was previously invisible. Some of the 194 are HTML attribute
names rather than Ruby, which is what grep-grade evidence costs and why it is labelled
rather than merged. Shipping the honest claim first was still right: nobody was misled in
the meantime, and the weaker class arrived already knowing it was weaker.

## Q10 — The refusal contract guards edit mechanics, not match semantics — **closed, partly**

The reframing was right and Phase 2 was the answer, as the question predicted: `type:`,
`kind:`, `subclasses:`, the class hierarchy and Sorbet signatures all exist to let a rule say
which class it means. The question's own suggestion — *refuse when a bare name matches across
more than N receiver shapes* — is now built, as a **warning** rather than a refusal:

```
warning: rewrote receivers of 2 different classes (Account, Company). These are different
         methods that share a name -- narrow with `where: { $R: { type: ... } }` if only
         one was meant.
```

**Warning, not refusal, deliberately.** A genuinely repo-wide rename is legitimate, and
refusing it would teach people to reach for the flag that turns the check off — at which
point the check protects nobody. What they need is to be told once, with the fix in the
message. It fires only for a rule set that narrows by no class at all, so saying which class
you meant silences it.

**Its reach is exactly receiver resolution's reach, and that is the honest limit.** The
warning sees a class only where the receiver resolves — a local from a constructor, a
constant, `self`, an ivar, a signature. D61 measured that chained receivers mostly do not
resolve, so a rule whose collateral is all unresolved receivers still passes quietly. The
residue report's `Definition` entries remain the other tell, and they need no resolution.

**Still open, and worth building later:** a corpus-wide check of *how many classes define
the name being renamed*, which is purely syntactic and would fire regardless of whether any
receiver resolves. That is strictly higher reach than what shipped, and the reason it did not
ship here is cost — it wants an index pass of its own — not doubt about its value.

## Q1 — Does residue reporting hit its pass bar? — **closed: yes on recall, conditionally on noise**

Measured in two halves, because one instrument cannot do both jobs.

### Recall — measured on `testbed/`, and it found two real defects

A purpose-built Ruby app where every site carries a `GT:` marker, written from the *Ruby*
side: enumerate how Ruby reaches a method name — `send`, `respond_to?`, `alias_method`,
`Symbol#to_proc`, `delegate`, `validates`, a serializer DSL, a subclass override,
interpolation, ERB, YAML — not how rwr classifies them.

**First run: 2 of 7 dynamic reaches reported.** The two defects:

- **Residue was computed only for files rwr had already changed.** A file that is nothing
  but dynamic reaches — a serializer full of `delegate` and `validates`, which is the
  dangerous case exactly — was never looked at.
- **The report was scoped to the target class**, which discards those same reaches: a
  delegation lives in a *different* class from the method it names, by construction.

**Now 7 of 7, with one budgeted false positive** — a string literal equal to the method
name, indistinguishable from `send("display_name")` without running the program. Pinned by
`tests/testbed.rs`.

Two further precision fixes came out of the same work: a symbol that is a *hash key* is not
a reach (57% of a 15,587-entry report on discourse was keyword-argument keys), and a call
that *defines* a method — `attr_reader :name` in an unrelated class — is not a reach for
another class's method of the same name.

### Noise — measured on discourse, and it depends entirely on how common the name is

| rename | sites rewritten | residue |
|---|---|---|
| `Post#cook` | 0 | 608 |
| `Topic#save` | 2 | 544 |
| `Topic#slug` | 3 | 1,364 |
| `User#name` | 8 | **8,114** |

**A distinctive name is reviewable; a common one is not**, and no amount of filtering
changes that: 5,293 of `User#name`'s 8,114 are calls whose receiver did not resolve, which
D61 measured as irreducible without a type source.

So D20's warning was right and the pass bar is met **conditionally**. What makes the
degradation honest rather than polite is that the tool now says so, and says where to start:
symbols first, because those are the ones that break; calls are mostly other classes'
methods; `-j` for machine filtering. The old message advised narrowing the rule, which is
wrong for the case that produces these volumes — a rename wants completeness, so narrowing
it would make it miss sites.

### What this settles

Residue reporting is worth its place in the product, and it is **not** worth trusting as a
completeness guarantee on a common identifier. Both halves of that need saying, and the tool
now says both.

## Q7 — Perf targets are unfalsifiable until Phase 0 — **closed, targets set**

The measurements exist now, so the targets are derived rather than picked. Five runs each,
warm; the first run of each set is cold-cache and reported separately because it is a
different question.

| corpus | files | `find` one pattern | a rename | the pack's safe rules |
|---|---|---|---|---|
| mastodon | 3,269 | 39 ms | 37 ms | 102 ms |
| rails | 3,321 | 65 ms | 67 ms | 205 ms |
| discourse | 11,006 | 175 ms | 292 ms | 478 ms |

*(Pack figures are post-D63; they were 178 / 348 / 970 ms when the targets below
were first set, and the targets were deliberately not lowered to match — see D63.)*

**The pack has since outgrown the third target, and that is the target's fault.** It was
written as "the whole shipped pack under 1.5 s" when the pack held ten rules; it holds
seventeen now, and `--unsafe` runs 1.3–1.5 s. A ceiling pinned to a collection that grows is
not a ceiling, so the number that carries over is the **marginal cost per rule** — about
40 ms per rule over 11,000 files, measured in D63 and unchanged since. Judge a pack against
that, not against a total that was only ever true of one pack.

Cold-cache first runs: 99 ms, 105 ms, 280 ms for `find`. The gap is page-in, not work.

**Targets, as regression ceilings rather than aspirations** — roughly 1.5× the measured
figure, so ordinary variance does not trip them and an order-of-magnitude regression does:

- a single-pattern `find` over ~10k files: **under 250 ms** warm
- a rename, which additionally builds the class hierarchy and reads signatures: **under
  500 ms**
- the whole shipped pack, ten rules: **under 1.5 s**

**No timing assertion goes into CI, deliberately.** A wall-clock test is machine-dependent,
and a suite that fails on a busy laptop teaches people to rerun until green — which is worse
than no check, because it also destroys trust in the checks that mean something. The guard
is this table plus `--profile`, run before a release. If a cheap deterministic proxy for the
same thing turns up — files surviving the prefilter, say — it belongs in CI and this does
not.

**What the numbers say about the design.** Cost tracks how many files mention an identifier
rather than repository size: discourse is 3.4× rails by file count and 2.7× by `find` time,
while the pack, which cannot prefilter as sharply because ten rules contribute ten literal
sets, scales closer to linear. That is the literal prefilter working as intended.

## Q8 — Are Coccinelle-style isomorphisms viable, or a known trap? — **closed: no user-defined layer**

The question asked for 30 minutes finding out why Semgrep pulled equivalences before
designing anything. Done, and it changes the answer twice over.

**The deprecation was a batch sweep, not a verdict.** Semgrep
[v0.61.0](https://github.com/semgrep/semgrep/releases/tag/v0.61.0) (2021-08-04) deprecated
four experimental features in a single bullet with no reason given:

> Deprecated the following experimental features: `pattern-where-python`, `taint-mode`,
> `equivalences`, step-by-step evaluation output

**`taint-mode` was in that list and is now one of Semgrep's headline features** — 727 code
hits in the repository today. So "Semgrep deprecated it" is much weaker evidence than this
question assumed: the batch was a sweep of experimental surface, and membership in it says
almost nothing about the individual feature.

**The real signal is what happened next, and it is sharper.** Five years on, taint came back
and equivalences did not. `equivalence` appears 42 times in the codebase, and every one is a
*built-in* the matcher applies itself — name resolution, constant propagation,
`(-) E vs -n`, matching through casts, `!=` normalised to `!(..=)`. Users still cannot define
one. Given a decade and a large team, **the curated closed set survived and the open-ended
user-facing form did not.**

**rwr already has the half that survived.** Layout, block spelling and integer spelling do
not affect equality; `{foo: 1}` and `{:foo => 1}` reach the same node so one pattern covers
both; locations never compare (D36). Those are built-in, curated, and closed — the same
shape as what Semgrep kept.

**So the answer is: keep adding built-ins on evidence, and do not build a user-definable
layer.** Each equivalence rwr has was added because a corpus entry needed it, which is a
different process from offering `$X + $Y <==> $Y + $X` and discovering what it does to match
counts. The failure mode a user-defined layer invites is precisely the one rwr cannot afford:
a match that is confident, clean, and not what the author meant.

*Reverses if:* a rule in the corpus genuinely needs an equivalence that cannot be expressed
as a built-in — and the test is that it must arrive as a *rule someone wanted*, not as a
capability someone imagined. That is the same bar `docs/rule-corpus.md` was created to
enforce.
