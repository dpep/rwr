# Rule corpus — Phase 0 seed

The author's actual wanted-rules list, checked against the design boundary. This replaces
invented case studies as the Phase 0 corpus: rules someone wants beat transformations someone
imagined.

Goal framing (Q12): not displacing RuboCop, but a faster and more ergonomic engine for a
*personal* corpus — ported favourites plus rules RuboCop cannot express.

---

## Verdicts

| Rule | Verdict | Needs |
|---|---|---|
| `return nil` -> `return` | Ports clean | nothing |
| Performance/* | Ports clean | method-name alternation; some want Phase 2 |
| Hash shorthand | Reachable | cross-capture name equality |
| Trailing commas | Reachable | inter-node source inspection, multiline test |
| Sorted arrays | Reachable, needs a decision | sequence transforms in templates |
| Correct indenting | Out of scope | shell out to a formatter |

## `return nil` -> `return`
Pure AST: a `ReturnNode` whose argument is a `NilNode`, argument removed. The design's own
canonical example. No new machinery.

## Performance rules
The best fit in the list. `$X.select { ... }.first` -> `$X.detect { ... }`,
`select.size` -> `count`, `map.compact` -> `filter_map`, `reverse.each` -> `reverse_each` are
all pure method-chain rewrites.

Two notes:
- **Confirms method-name alternation** as a needed `where:` predicate — `select`/`find_all`
  are synonyms and a rule must match both. The case studies flagged this independently.
- **Natural Phase 2 demonstrators.** `Performance/StringReplacement` (`gsub` -> `tr`) is only
  valid when the receiver is a String. That is receiver narrowing, applied to a rule someone
  actually wants — a better Phase 2 justification than an invented example.

## Hash shorthand
Splits in two.

**Rocket -> colon** is straightforward: `AssocNode::operator_loc()` is `Some` for `:a => 1`
and `None` for `a: 1` (pinned by `source::tests::hash_rocket_is_distinguishable_from_shorthand`).
Guard that the key is a plain symbol.

**`{foo: foo}` -> `{foo:}`** (Ruby 3.1+) needs comparing a *symbol key* against a
*local-variable or method-call value* — same name, different node kinds. D16's AST equality
does not apply, since the nodes are not equal. **New predicate: name equality across capture
kinds.**

## Trailing commas -- out of scope (D53)

| Cop | Verdict | Note |
|---|---|---|
| `Style/TrailingCommaInArguments` | **out** | |
| `Style/TrailingCommaInArrayLiteral` | **out** | |
| `Style/TrailingCommaInHashLiteral` | **out** | |

An earlier revision listed these as reachable pending an inter-node-source predicate. They are
not, and the reason is measurable rather than aesthetic: **a trailing comma is invisible to
rwr's equality**. `[a, b]` and `[a, b,]` are the same program by the tool's own definition, so
adding one is presentation, not structure -- the same class as indentation, and the
formatter's job (principle 7, D34).

The predicate would have worked; building it would have made rwr a formatter through the back
door.

## Sorted literals

| Rule | Verdict | Note |
|---|---|---|
| alphabetise array / list / hash-key literals | machinery | No standard cop; this is a custom rule, which is part of the point. Needs D33 sequence transforms, D35 comment attachment, and D14 `effective_range` under movement. Corpus 004 — the hardest entry, deliberately. |

## Layout — out of scope

| Cop family | Verdict | Note |
|---|---|---|
| `Layout/*` (indentation, alignment) | **out** | Principle 7: rwr does not own style. D34 draws the line — rwr *repairs* indentation of regions it rewrote, because otherwise the minimal-diff promise is broken by damage rwr caused, and shells out for anything more. syntax_tree is the natural target, sharing Prism lineage. |

---

## What this backlog implies

**Build order for `where:` predicates**, ranked by how many target rules each unblocks:

1. ~~**Method-name alternation**~~ — **built.** `where: { $SEL: { name: [select, find_all] } }`.
   One rule covers both synonyms; ast-grep needs a separate pass per name. Unblocks
   `Performance/Detect` and `Performance/Count`.
2. ~~Inter-node source inspection~~ — **dropped.** Its only consumer was the trailing-comma
   cops, which D53 puts out of scope.
3. ~~Multiline test~~ — dropped with (2).
4. ~~**Cross-capture name equality**~~ — **built.** `where: { $K: { same_name_as: $V } }`
   relates two captures by identifier across node kinds, which D16's AST equality cannot
   express. Hash shorthand works for a single-pair hash and covers **both spellings with one
   pattern**, since the rocket and the label parse to the same node.

## Hash shorthand, including multi-pair hashes

```yaml
match: '{**$BEFORE, $K: $V, **$AFTER}'
where:
  $K: { same_name_as: $V }
rewrite: '{**$BEFORE, $K:, **$AFTER}'
```

```ruby
{ foo: foo }                  ->  {foo:}
{ :bar => bar }               ->  {bar:}      # one pattern, both spellings
{ baz: qux }                  ->  unchanged
{ name: name, other: thing }  ->  {name:, other: thing}
```

Three things this needed, and the first was not a code problem at all:

1. **`AssocSplatNode` as a sequence placeholder.** Ruby spells "the remaining entries" with a
   double splat inside a hash. This was recorded as an unexplained bug -- `splat_placeholder`
   returned `None` for a node that *was* an `AssocSplatNode` -- and the explanation was that
   the branch had never been added: a string-replacement edit had silently failed to match
   and the code being debugged did not exist. Edits now assert that they applied.
2. **`**$NAME` is one token.** Treating only the second asterisk as the metavariable left the
   first as literal template text, so an empty sequence rendered `{*, a:}`.
3. **Empty sequences drop a separator on either side.** The cleanup only removed a *preceding*
   comma, so `{**$B, $K:}` rendered `{, k:}`.

**A note on direction.** Several of these cops are configurable and RuboCop's default may be
the *opposite* of what is wanted — `Style/ReturnNil` is the clearest case. rwr rules encode a
direction explicitly, which is a small ergonomic win worth noting: there is no `EnforcedStyle`
to misread, because the rule *is* the direction.
