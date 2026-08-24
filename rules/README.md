# The shipped rule pack

The pack is compiled into the binary, so it works from any directory:

```sh
rwr check all                   # every safe rule, read-only
rwr check performance           # one family
rwr check style/return-nil      # one rule
rwr rewrite all app/ --unsafe   # apply, including the rules with caveats
```

A real path always wins over a built-in name, so `rwr check performance` reads
your own `./performance` directory if you have one. Pointing at a directory of
your own rules works the same way — a pack *is* a directory.

Rules apply in path order, each seeing the previous one's output, and the run
reports which rule accounted for what. A rule's id defaults to its path within
the pack; an `id:` key overrides it.

## Safety, and why there are no per-rule options

RuboCop configures a cop because a cop is code you cannot edit. An rwr rule is
four lines of YAML — the rule **is** the option. There is no `EnforcedStyle` to
misread, because a rule encodes its direction outright; if you want the other
direction, copy the file and swap `match` with `rewrite`.

What does need saying is **when a rewrite can change behaviour**, and Ruby being
dynamically typed, most interesting ones can:

```yaml
unsafe: >-
  `inject(:+)` returns nil for an empty collection; `sum` returns 0.
```

Present means unsafe, and the value is the reason — there is no boolean to set
without saying what for. Those rules are held back unless you pass `--unsafe`,
the run says how many were held back and why, and when one does fire its reason
is printed after the run, in an `unsafe rule(s) applied:` block. RuboCop
carries the same information as
`SafeAutoCorrect: false`, in a config file nobody reads at the moment of the
edit.

**`unsafe:` names a runtime behaviour change, and nothing else.** Every reason in
the pack finishes the sentence "your program will do this instead" — a nil where
a zero was, a different method on a non-relation, `^` turning special. Not "the
result is a little weaker", not "read it before you run it". `rspec/be-empty`
ships plain for this reason: `eq([])` pins the type where `be_empty` does not, so
the assertion gets weaker, but no input makes Ruby behave differently. That
caveat lives in the rule's `description:`, which rides along to `-j`, SARIF and
the pull-request comment.

The distinction is load-bearing because `--unsafe` is all-or-nothing. Every rule
marked for a softer reason taxes the person who wanted one specific rewrite, and
a flag holding back a dozen rules for two different kinds of reason stops
discriminating and becomes a reflex.

## Safe by design

`rwr rewrite all app/` should be something you can run unattended. Not "run it
and review carefully" — *unattended*, on a codebase you have not read, the way
you would run a formatter. That is the design constraint the pack is built to,
and it decides what goes in it.

Safety comes from a rule not being in the default set, never from a warning
attached to one. A warning is a request for vigilance, and vigilance does not
scale to ten thousand call sites; that is the house principle "make unsafe
operations unrepresentable", applied to the pack rather than to the API. So the
question for a candidate is not "is this usually right" — most rewrites are
usually right — but:

**Is there an input for which the rewrite makes Ruby do something else?** Write
it down and run it. If there is none, the rule ships plain, and running it needs
no judgement from anyone. `style/return-nil`, `style/inverse-any`,
`rspec/redundant-stub-return`.

**If there is, does the rewrite buy something measured?** A performance win can
carry a caveat, because the caveat is checkable and the gain is real: `sum` has a
fast path for numerics, `exists?` stops a query returning rows nobody counts.
That is what `unsafe:` is for, and `--unsafe` is a seam rather than a setting —
the intended exit is to narrow the rule with a `type:` receiver until it is safe,
not to keep passing the flag.

**A preference about how code reads buys nothing measured**, so it cannot carry a
caveat. There is no amount of tidier that pays for a `NoMethodError`, and a style
rewrite that can change behaviour has no version of itself that belongs in a pack
other people run over code they have not read.

The pack is therefore smaller than the set of rewrites that are usually right,
deliberately. Usually-right is what your own rules directory is for, where you
know the receivers and you are choosing to run it — a directory *is* a pack, so
a hand-rolled rule is a first-class one.

## The ActiveRecord rules

`exists`, `find-by`, `pluck` and `relation-count` all assume their receiver is
an ActiveRecord relation, and say so. The pattern carries a *tell* rather than a
proof — `where(...)` and `to_a` are rare on anything else — which is precisely
what `unsafe:` is for.

Turn the tell into a proof where the receiver resolves:

```yaml
where:
  $R: { type: ApplicationRecord, subclasses: true, kind: class }
```

That narrows to constants rwr can trace to your model base class. It also means
the rule stops firing on `company.employees.where(...)`, whose receiver rwr
cannot resolve — under-matching, reported as nothing, which is the safe
direction.

The narrowing that makes several of these safe is a `where:` receiver type — an
`ActiveRecord::Relation` is the whole problem with the `select` rules, and rwr
can rule it out where it can resolve the receiver. Add a `type:` constraint when
running these over a Rails app, and read the residue report for the sites it
could not resolve.

## The rspec rules

Spec idioms, and the one family whose name is also a *scope*: a rule constrains
the tree, never the path, so point it at your specs.

```sh
rwr check rspec spec/
```

Both shapes it looks for are effectively spec-only — `and_return` reached from a
`receive`, and `expect(...).to eq([])` — so a whole-tree run is unlikely to find
anything in `app/`. Naming the directory is still the honest way to say what you
meant, and it is faster.

`redundant-stub-return` matches through the chain rather than at a fixed
position, so `receive(:x).with(1).and_return(nil)` is covered and
`and_return(nil, 1)` — a *queue* of return values, a different node — is not.

## Excluding a type, and why it is not the mirror of `name_not:`

`type_not:` takes a list of classes and means *resolves, and to none of these*.

```yaml
where:
  $X: { type_not: [TrueClass, FalseClass, Boolean] }
```

The second half of that sentence is the whole design. `name_not:` **passes** when
the capture has no identifier at all, because nothing that is not an identifier
can be one of the excluded ones. A type exclusion cannot inherit that: `type:`
under-matches when it cannot resolve a receiver, which is the safe direction, and
a negation that passed on missing data would *widen* — every receiver rwr cannot
see would sail straight through a guard written to stop it. So an unresolved
receiver **fails** an exclusion, and narrowing still only ever narrows.

Descent is always honoured and there is no flag for it. "Not an
`ActiveRecord::Base`" plainly means not an `Account` either, and there is no
reading of an exclusion where admitting the subclass is what the author wanted.

`Boolean` sits beside the two real classes because `T::Boolean` is a constant
path and resolves by its last segment, so a Sorbet signature returning one
arrives under that name rather than as the pair it aliases.

**It fires only where the type resolves**, which today means a signature. `-e`
distinguishes the two failures, and the difference decides what you do next:

```
$X bound `label`    -- resolved to Boolean, excluded by `type_not: [...]`
$X bound `account`  -- receiver did not resolve; `type_not: [...]` needs a receiver rwr can resolve
```

The first is the constraint working. The second is a gap in what rwr can see,
and is closed by writing a signature — not by loosening the rule.

## Writing a constraint: block or flow

Both of these mean the same thing, and the inline one usually reads better:

```yaml
where:
  $SEL: { name: [select, find_all] }        # flow — YAML's inline mapping
  $R:
    type: Array                              # block — indented lines
```

**The flow form has one trap.** Inside `{ ... }`, the characters `,` `{` `}` `[`
and `]` belong to YAML, so a pattern containing any of them gets cut short:

```yaml
$A: { contains: log($X, $Y) }     # YAML sees `log($X` — broken
$A: { contains: "log($X, $Y)" }   # quoted — fine
$A:
  contains: log($X, $Y)           # block — fine
```

rwr refuses loudly when a pattern arrives truncated, rather than running a rule
that quietly matches nothing. `$` and `.` are ordinary characters to YAML, so
`{ contains: $X.$ASSOC.$FIELD }` needs no quotes at all.

## Reordering a captured run: sequence transforms

`*$ITEMS` captures a *run* of elements rather than one node, and a suffix in the
template reorders that run:

```yaml
match: $C = [*$ITEMS]
where:
  $C: { is: constant }
rewrite: $C = [*$ITEMS.sort]
```

```ruby
PERMS = [:zebra, :apple]    ->    PERMS = [:apple, :zebra]
```

Three exist, and the set is closed:

| suffix | |
|---|---|
| `.sort` | alphabetise, by each element's own source text |
| `.uniq` | drop repeats, keeping the first |
| `.reverse` | reverse the run |

**A suffix rwr does not recognise is refused, not emitted.** `*$ITEMS.srot` would
otherwise write `items.srot` into your source, which parses and means something
else — the silent wrong rewrite this tool exists to avoid. The refusal names the
suffix and the run makes no edits.

Comments travel with the element on their line, so reordering carries them along.
An element sharing a line with a comment that could describe either neighbour is
**refused** rather than guessed at, because there is no way to tell which element
the comment is about.

The pack ships no rule that uses this. The obvious one — alphabetising a constant
array — is a rewrite that risks behaviour for tidiness, and by the standard above
that belongs in your own directory rather than in a pack run unattended. It is
four lines, and they are the four above.

## Lints: rules that flag without rewriting

A rule with no `rewrite:` is a **finding**. It reports its matches with its
`description` and proposes nothing:

```yaml
description: '`.size` on a relation queries when unloaded and counts in memory when loaded.'
match: $R.where($C).size
```

```
1 finding(s) for review, no edit proposed:

  performance/relation-size — `.size` on a relation queries when unloaded…
    app/models/company.rb:14:5: total = Company.where(active: true).size
```

Findings make `check` exit 1, like edits do — a lint that exits 0 gates nothing.
`rewrite` reports them and writes nothing for them.

This is for shapes where the right answer depends on something rwr cannot see.
`.size` on a relation is `count` unloaded and `length` loaded, and only the
caller knows which was meant; proposing either would be guessing.

## What is not here, and why

**`if !x` → `unless x`.** An `IfNode` and an `UnlessNode` are different kinds,
so the structural diff cannot align them and falls back to re-rendering the
whole statement — which collapses a multiline body onto one line and inserts a
`then`. A rewrite that produces a worse diff than its input does not ship, even
though it matches correctly. It becomes available if the diff learns
correspondence across node kinds.

**Layout.** Indentation, alignment and trailing commas are presentation, and
rwr does not own style (D34, D53).

**N+1 detection.** Association access inside an `each`/`map` block with no
`includes` upstream is a real and valuable thing to flag, and rwr cannot express
it. A pattern matches a *shape*; there is no way to say "this block contains
that call somewhere inside it", and the `includes` it would need to look for is
usually in another method entirely. A version narrow enough to write would miss
most real N+1s and a version broad enough to catch them would flag every block.
The lint mechanism above is now in place for when the matcher can express
containment.
