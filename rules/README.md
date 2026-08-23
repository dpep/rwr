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

Which rules are held back is not listed here, because that list changes every
time one is added. The run says so itself, with a count and — under `-e` — the
reason for each:

```
rwr: N rule(s) held back as unsafe; --unsafe to include them, -e for why
```

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

## The `!` rules

`style/inverse-any` is safe and `style/not-empty` is not, which is easier to see
in Ruby than in prose:

```ruby
![nil, false].any? { |x| x }   # => true
[nil, false].none? { |x| x }   # => true   -- a real inverse

![nil, false].empty?           # => true
[nil, false].any?              # => false  -- not one
"abc".empty?                   # => false
"abc".any?                     # => NoMethodError
```

With a block, `any?` and `none?` disagree about the *predicate*, so no element
value can split them. Without one, `any?` asks about truthiness while `empty?`
asks about presence, and `empty?` is common on strings, where `any?` does not
exist. Hence one ships plain and the other ships held back.

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
