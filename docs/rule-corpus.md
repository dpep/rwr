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

## Trailing commas
The comma **is not a node**. It lives in the gap between the last element's end and the
closing bracket's start, so this needs a capability rwr has no concept of yet: **a predicate
that inspects source between nodes.**

Also needs a **multiline test** — RuboCop's `comma`/`consistent_comma` styles apply only to
multiline literals. Derivable from locations (start line != end line), so cheap.

The edit itself is anchored (insert before the closing bracket), so it is not the
"non-anchored insertion" case that is genuinely out of reach.

## Sorted arrays
**Confirmed scope:** alphabetising literal elements in arrays, lists, and hash keys.

**Resolved by D33:** `match: '[*$ITEMS]'` / `rewrite: '[*$ITEMS.sort]'`. Transforms are
recognised only on *sequence* captures, since `$X.sort` on a single capture is legitimate
literal output. The closed set is defined rather than listed - zero-argument, deterministic,
total, sequence-to-sequence - which excludes `sort_by` by construction rather than by taste.
`sort` compares effective source text.

**It is also the best stress test in the list.** Sorting *moves* nodes, which hits:
- **D14's heredoc hazard** — a moved element carrying a heredoc must move its body.
  `effective_range` is built for exactly this.
- **Comment attachment under movement** — flagged as a missing design area by the
  staff-engineer review. Which comment travels with which element? This rule makes that gap
  load-bearing rather than theoretical.

Strong Phase 0 corpus candidate for those reasons alone.

## Correct indenting — out of scope
Not a capability limit: **principle 7** says rwr does not own style, and DESIGN.md keeps
formatting separate deliberately.

Indentation is not a node property. It is whitespace whose correctness depends on full
nesting context plus a style policy — a formatter's job. Every rwr rewrite will disturb
indentation, so the answer is the already-planned "format changed code if necessary" step:
rewrite, then pipe to `rubocop -a Layout`, `standardrb`, or syntax_tree.

**Resolved by D34:** rwr repairs indentation of the regions *it* rewrote - without that the
minimal-diff promise is broken by whitespace damage rwr caused - and shells out for anything
more. syntax_tree is the natural target, sharing Prism lineage.

---

## Derived work items

New `where:` predicates, in the order this list justifies them:
1. **Method-name alternation** — `select`/`find_all` (perf rules; also flagged by case studies)
2. **Cross-capture name equality** — symbol key vs identifier value (hash shorthand)
3. **Inter-node source inspection** — the gap between two nodes (trailing commas)
4. **Multiline test** — cheap, derived from locations (trailing commas)

New design decisions needed:
- ~~Closed set of sequence transforms~~ - decided, D33.
- ~~Comment attachment under movement~~ - decided at principle level, D35.
- ~~Formatter integration~~ - decided, D34 (repair, not formatting).

Note none of the case studies' other flagged predicates (capture node-kind, block-param
arity) appear here. Real wanted-rules produced a different predicate set than invented ones —
which is the argument for seeding Phase 0 from this file.
