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
is printed next to the diff. RuboCop carries the same information as
`SafeAutoCorrect: false`, in a config file nobody reads at the moment of the
edit.

**Held back by default:** `performance/detect`, `performance/count`,
`performance/filter-map`, `performance/sum`,
`performance/string-replacement`, `style/sorted-constant-array`.

The narrowing that makes several of these safe is a `where:` receiver type — an
`ActiveRecord::Relation` is the whole problem with the `select` rules, and rwr
can rule it out where it can resolve the receiver. Add a `type:` constraint when
running these over a Rails app, and read the residue report for the sites it
could not resolve.

## What is not here, and why

**`if !x` → `unless x`.** An `IfNode` and an `UnlessNode` are different kinds,
so the structural diff cannot align them and falls back to re-rendering the
whole statement — which collapses a multiline body onto one line and inserts a
`then`. A rewrite that produces a worse diff than its input does not ship, even
though it matches correctly. It becomes available if the diff learns
correspondence across node kinds.

**Layout.** Indentation, alignment and trailing commas are presentation, and
rwr does not own style (D34, D53).
