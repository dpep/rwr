# Phase 0 results

Running record of the measurements that decide whether rwr should exist. Each entry states
what was measured, on what, and what it settles.

Corpora are local checkouts under `~/code/lib/ruby`. Reproduce with:

```sh
cargo test --test phase0_parse -- --nocapture
```

---

## (d) Cold parse throughput — **measured**

8 threads, Apple Silicon, warm page cache.

| repo | files | MB | ms | MB/s | unparsed |
|---|---:|---:|---:|---:|---:|
| rails | 3,254 | 16.2 | 188 | 86 | 0 |
| rubocop | 1,531 | 7.5 | 85 | 88 | 0 |
| graphql | 714 | 3.4 | 39 | 88 | 0 |
| ruby (CRuby) | 7,858 | 34.8 | 360-414 | 84-97 | 3 |
| **total** | **13,357** | **62.0** | **612-726** | **85-101** | **3** |

Two runs varied 612-726 ms; both had a warm page cache, so treat ~85-100 MB/s as the range
rather than either endpoint as a figure. Cold-cache numbers will be worse and are not yet
measured.

### This settles D5: Phase 1 ships no persistence

Rails — 3,254 files, 16 MB — parses **in under 200 ms**. A 1M-LOC monolith is roughly 35 MB,
so a full cold parse lands near half a second on this hardware.

That is fast enough that a parse cache would be solving a problem which does not exist. D5 was
amended to cut the cache on the *argument* that cache-vs-nothing was the real comparison and
had never been measured; this is the measurement, and it agrees. No cache, no index, no
invalidation bug class, no coherence surface.

The staff-engineer review predicted this outcome and it was right.

## D1 fidelity — **measured, and stronger than claimed**

**Zero parse failures across 5,499 application-shaped Ruby files** (rails, rubocop, graphql).

Three failures in CRuby's own tree, all explained, none a Prism gap:

| file | why |
|---|---|
| `spec/ruby/command_line/fixtures/bad_syntax.rb` | deliberately invalid — that is the fixture's purpose |
| `test/ruby/test_call.rb` | CRuby's error-path test suite |
| `tool/fetch-bundled_gems.rb` | `#!ruby -an` — the `-n` flag wraps the file in an implicit `while gets` loop at runtime, so its top-level `next` is not valid standalone Ruby. Prism is correct to reject it. |

DESIGN.md §3 argues Prism over tree-sitter on fidelity for *valid* Ruby. This is the first
evidence: across 13,357 files of real-world Ruby — including CRuby's own source, which
exercises syntax no application ever will — Prism's only rejections are files that genuinely
are not valid standalone Ruby.

It does not yet prove the converse (that tree-sitter's Ruby grammar *would* differ), which
needs a direct comparison. But it removes the possibility that Prism itself has gaps here.

## (b) Bare-name match collateral — **measured**

Rails: **309,683 call sites, 12,686 distinct method names.** No matcher needed — collect every
`CallNode`, group by name, classify each receiver by syntax alone.

```sh
cargo test --test phase0_receivers -- --nocapture
```

### D6's premise is confirmed, with a number

| name | call sites | implicit | constant | receiver shapes |
|---|---:|---:|---:|---:|
| `name` | 2,067 | 15% | 2% | 6 |
| `id` | 1,608 | 4% | 0% | 6 |
| `create` | 875 | 2% | 75% | 5 |
| `call` | 686 | 0% | 4% | 6 |
| `save` | 585 | 1% | 0% | 4 |
| `execute` | 564 | 17% | 2% | 5 |
| `build` | 394 | 20% | 10% | 5 |
| `value` | 335 | 27% | 0% | 6 |

Renaming `Foo#name` by bare identifier touches **2,067 sites in Rails alone**, essentially all
of them wrong. That is what Ruby LSP's `ReferenceFinder` does today (bare name equality, no
receiver check), and it is the disease D6 and the semantic layer exist to cure. The premise
was previously asserted; it is now measured.

### A nuance that corrects the case studies

`create` is **75% constant-receiver** (`Foo.create`) — statically known, and therefore the
*easy* case. The case studies worried that `create`/`update`/`call` were hopeless because
residue degrades on common names. That is right for `name`, `id` and `value`, and wrong for
`create`. Commonness and tractability are not the same axis, and D20's "identifier too common"
degradation should key on **receiver-shape diversity**, not raw frequency.

### Receiver shapes across all 309,683 call sites

| shape | count | share | resolvable without type inference? |
|---|---:|---:|---|
| Implicit (`foo`) | 134,642 | 43.5% | **yes** — lexical scope names the enclosing class |
| Local (`x.foo`) | 55,563 | 17.9% | sometimes — local inference from `Foo.new` etc. |
| Chained (`a.b.foo`) | 49,051 | 15.8% | hardest — needs the chain's type |
| Constant (`Foo.foo`) | 44,436 | 14.3% | **yes** — statically known |
| Ivar (`@x.foo`) | 17,668 | 5.7% | sometimes |
| Self (`self.foo`) | 2,402 | 0.8% | **yes** |
| Other | 5,921 | 1.9% | no |

### Preliminary answer to Q2 — favourable

**~58.6% of call sites (implicit + constant + explicit self) are structurally resolvable from a
symbol index plus lexical scope, with no type inference and no Sorbet.**

That is better than expected, and the reason is the shape of the data rather than any
cleverness: the dominant bucket is *implicit self*, and knowing which class you are lexically
inside is pure symbol-index work.

Stated carefully, because this is an upper bound on the easy cases rather than a finished
result: resolving an implicit-self call still requires the index to know the enclosing class
*and* that the method is defined on it or an ancestor. Local and ivar receivers (23.6%
combined) would push the number higher with modest inference. Chained receivers (15.8%) are
where a type layer would actually earn its keep.

This does not close Q2 — that needs the index built and measured — but it removes the
worry that receiver narrowing is unworkable without Sorbet. The largest bucket needs no types
at all.

## Syntactic partition vs incumbents — **first scores**

```sh
./script/score.sh
```

| entry | ast-grep | comby |
|---|---|---|
| 001 return-nil | **match** | **WRONG** - rewrites a heredoc body and mangles `nil_value` |
| 002 perf-detect | matched correctly, **rewrote non-minimally** | inexpressible |
| 004 sorted-array | inexpressible | inexpressible |
| 007 receiver-rename (semantic) | inexpressible | inexpressible |

### 001: ast-grep passes, comby fails it in two ways

**A methodology note first, because it changed the result.** An earlier run scored comby as a
match. That was an artefact: comby had been invoked with a bare `.rb` extension filter, which
made it recurse the working directory and **rewrite the corpus fixture in place** — so the
comparison was checking comby's output against a file comby had already written. Circular, and
it read as success. The corpus well-formedness test caught it (`in/basic.rb` had become
identical to `out/basic.rb`), the fixture was restored from git, and competitor runs now
execute with cwd inside a temp directory so a careless runner can only damage the copy.

Correctly contained, comby fails the simplest rule in the corpus twice:

```ruby
# rewrote prose inside a heredoc body
  This heredoc body says return nil   ->   says return

# mangled an identifier -- and the result still parses
  return nil_value                    ->   return_value
```

The second is the failure the entire design exists to prevent: matching `return nil` as a
*prefix* of `return nil_value` and silently producing a different working program. It is a
stronger correctness claim than the overlap-corruption one, and it falls out of the simplest
rule anyone would write.

ast-grep produces the expected output exactly. `rg` finds 7 matches where 3 are real - a
2.3x over-match from the comment, the heredoc body and the string literal - which is the case
for structural tooling at all, but not a case for *this* structural tool.

DESIGN.md section 2 already concedes Phase 1 is table stakes. This is that concession measured
rather than asserted, and it is what the kill criteria are watching.

### 002 relocates the differentiator from matching to rewriting

ast-grep **found every correct site**. The output differs in layout:

```ruby
# expected (minimal diff - only the changed tokens move)
    accounts
      .detect { |account| account.name.include?(term) }

# ast-grep
    accounts.detect { |account| account.name.include?(term) }
```

and a multiline `do ... end` body came back as `... positive? end`, its trailing newline lost
on splice.

**Being fair about attribution:** the single-line collapse is the *invocation's* fault - the
rewrite template was written on one line, so one line is what it emitted. The lost newline is
ast-grep's splice.

But the fair framing is more interesting than a scoreline: **a template-based rewriter cannot
preserve layout it never captured.** ast-grep renders a template; rwr splices source ranges
(D13's action tree over `effective_range`). That is exactly why section 3C made minimal diffs a
requirement rather than a nicety.

So the syntactic partition's real finding so far is that **rwr's edge at this layer is
rewriting, not matching** - which sharpens section 2's positioning rather than contradicting it.

### A second finding, from writing the invocation

ast-grep has no method-name alternation in a bare pattern, so `select` and `find_all` need
separate passes. Corpus 002's runner does two. That is a small ergonomic tax, and it is the
first evidence for the `where:` predicate the backlog ranks first.

## rwr scored against its own corpus — **first run**

### 001: rwr matches exactly

```
corpus/001-return-nil: MATCH
```

The same fixture on which comby rewrites a heredoc body and turns `return nil_value` into
`return_value`. rwr changes three sites and leaves the comment, the string literal and the
heredoc body untouched.

### 002: partially fixed by structural diffing; shape-changing rewrites still fall back

The first run of this corpus caught rwr doing exactly what this document had criticised
ast-grep for -- a multiline chain collapsing and a `do ... end` block coming back as braces --
because the template governed layout.

**Structural diffing fixes the same-shape case.** Rather than rendering the whole template,
pattern and template trees are aligned and edits emitted only where they *differ*. A subtree
carried across unchanged is never spliced, so its bytes survive exactly:

```ruby
foo(  x  +  1  )   ->   bar(  x  +  1  )     # was bar(x  +  1)
foo(\n  a,\n  b,\n)  ->  bar(\n  a,\n  b,\n)   # layout untouched
```

Three consequences, two of them unplanned:

- **The heredoc refusal dissolved itself.** A capture that does not move is never spliced, so
  the discontiguity hazard never arises. `foo(<<~SQL) -> bar(<<~SQL)` now simply works, and the
  `DiscontiguousCapture` refusal no longer fires for renames.
- **Nested matches both apply in one pass.** Minimal edits over `foo(foo(1))` are two method
  names in different places -- genuinely disjoint -- so the result is `bar(bar(1))` with no
  rerun. This is D15 working as *written*: the conflict unit is the edit range, not the match
  range.
- **Shape-changing rewrites still fall back to full replacement.** `$R.select { .. }.first ->
  $R.detect { .. }` drops an enclosing call, so the trees diverge and alignment gives up.
  Corpus 002 therefore still differs from its expected output. Giving up is *correct*, merely
  non-minimal, which is the right direction to be wrong in -- but the minimal-diff claim holds
  today only for same-shape rules.

## (a) Residue reporting — **built and measured; the answer is conditional**

The differentiator, and Q1's actual question: is name-scoped residue *useful*, or noise?

### It works, and catches exactly what a rename breaks

`rwr '$R.autoload_paths' rails` — 21 matches, **8 residue occurrences**:

```
attr_accessor :autoload_paths                Symbol
attr_reader   :autoload_paths                Symbol
attr_writer   :..., :autoload_paths          Symbol
def autoload_paths                           Definition
autoload_paths.each do |autoload_path|       Call    (implicit self)
autoload_paths << lib.to_s                   Call    (implicit self)
%w(autoload_lib autoload_paths)              String
```

Every one is a site a rename would break and a syntactic match would miss. The
`attr_accessor :autoload_paths` case is the whole argument: rename the method and that macro
silently keeps defining the old name. Eight items is reviewable.

### It degrades badly on common names, as D7 feared

| anchor | matches | residue | verdict |
|---|---:|---:|---|
| `autoload_paths` | 21 | **8** | reviewable |
| `perform` | 15 | 101 | borderline — mostly implicit-self calls, arguably signal |
| `save` | 570 | 167 | borderline |
| `name` | 1,750 | **4,547** | noise |

**Q1's answer is therefore conditional: the reformulation is tractable and genuinely useful,
but only above a distinctiveness threshold.** 4,547 items is not a report anyone reads.

### What this settles, and what it does not

It settles that residue is *implementable* without dataflow and does find real metaprogramming
reaches — the doubt D7 recorded. It does not settle where the threshold sits.

D20 as amended already prescribes the answer: degrade on **receiver-shape diversity**, not raw
frequency. Measurement (b) supplies the input — `name` carries six receiver shapes with 2%
constant receivers, while a distinctive name is pinned by its receiver almost everywhere. The
threshold should be *derived* from that rather than picked.

`perform` is the instructive middle: 101 residue items against 15 matches, almost all
implicit-self calls. Those are not noise — they are real call sites a receiver-qualified
pattern genuinely missed — which suggests the report should **separate** "reachable but
unmatched calls" from "the name appearing in metaprogramming", since only the second is a
blind spot.

## Still outstanding

| measurement | status | needs |
|---|---|---|
| (a) residue-reporting spike | not started | a matcher, plus real renames with hand-verified ground truth |
| (b) bare-name collateral | **done** — see above | — |
| (c) receiver resolution for methods, no Sorbet | **preliminary, favourable** (~59% structurally resolvable) | symbol index to confirm |
| syntactic-partition scoring vs incumbents | **started** - 4 entries scored | more entries; rwr itself |

(b) was previously blocked on the author's private monolith. Rails is a legitimate public
substitute, and being public it makes the result reproducible — which for an OSS tool
positioned against an incumbent is worth more than a bigger private number.
