# Phase 0 corpus

The gate that decides whether rwr should exist. Seeded from the author's actual wanted-rules
list (`docs/rule-corpus.md`) rather than invented transformations — real rules produced a
different predicate set than imagined ones did, which is the whole argument for this file.

## Format

One directory per transformation:

```
corpus/
  001-return-nil/
    meta.yml            name, family, competitor invocations, notes
    rule.yml            the rwr rule
    in/basic.rb         input fixture
    out/basic.rb        expected output after the rewrite
```

**Scoring is output equality.** A tool that produces `out/x.rb` from `in/x.rb` found exactly
the right sites; one that doesn't shows precisely where it went wrong. Precision and recall
collapse into a single unambiguous check, and no hand-maintained list of line numbers can
drift out of sync with the fixtures.

A tool that cannot express the rule at all is recorded as `inexpressible` in `meta.yml` — which
is a result, not a gap. Three of those in the semantic partition is a large part of the
argument for building anything.

## Partitions

`meta.yml` declares `family: syntactic | semantic`, and **they are scored separately** — this
is not cosmetic. ast-grep definitionally cannot pass the semantic partition, so counting those
wins toward rwr's survival would be scoring a race against an absent runner. The syntactic
partition judges whether Phase 1 deserves to exist as a new engine; the semantic partition
judges Phase 2.

## Fixtures must be gnarly

A fixture that only covers the easy shape measures nothing. Every transformation carries at
least one of: a heredoc, a multiline call, a block, string interpolation, a comment adjacent
to a moved node, or a nested instance of the same pattern.

## Provenance and scrubbing

Rules are drawn from real work, fixtures are **written fresh**. Use neutral placeholders
(`Widget`, `Account`, `Foo`) — never class names, domain terms, or identifiers from a private
codebase. This is a public repo and the corpus is its most-read artifact.

## Refusal fixtures

A fixture named `in/refuses-<name>.rb` has **no `out/` counterpart** and must produce a
refusal (exit 3) with the source untouched. Refusing correctly is a behaviour worth pinning:
the design's value rests as much on declining ambiguous work as on doing unambiguous work.
