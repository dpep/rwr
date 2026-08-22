# The integration testbed

A small Ruby app whose only purpose is to be renamed. `Account#display_name` →
`full_name`, with every site marked for what must happen to it.

It exists because Q1 — *does residue reporting hit its pass bar?* — needs
**ground truth**, and ground truth means a repository where every reach is
known. It is scored by `tests/testbed.rs`.

## The markers

| marker | meaning |
|---|---|
| `GT:rewrite` | rwr must rewrite this site |
| `GT:residue` | this breaks and rwr cannot rewrite it, so it must be **reported** |
| `GT:blind` | this breaks and rwr cannot see it; absence is expected and honest |
| `GT:ignore` | this does not break; rewriting or reporting it is a false positive |

The markers live in the source rather than in a manifest, so editing a file
moves its ground truth with it. A line-numbered manifest goes stale the first
time anyone adds a line.

## Written from the Ruby side, deliberately

The sites were enumerated by asking *how does Ruby reach a method name* —
`send`, `respond_to?`, `alias_method`, `Symbol#to_proc`, `delegate`,
`validates`, a serializer DSL, a subclass override, interpolation, ERB, YAML —
not by asking what rwr currently classifies.

That distinction is not pedantry. Earlier in this project a corpus fixture had
recorded a **bug** as its expected output, because it was written after the
behaviour it was meant to check. A testbed written from the tool's own
capabilities would have scored 7/7 on day one and found nothing.

Written this way it scored **2 of 7** and found two real defects:

- residue was computed only for files rwr had already *changed*, so a file that
  is nothing but dynamic reaches — a serializer full of `delegate` and
  `validates`, which is the dangerous case exactly — was never looked at;
- the report was scoped to the target class, which discards those same reaches,
  since a delegation lives in a *different* class from the method it names.

## The second half of the principle, learned later

Writing from the Ruby side is necessary and **not sufficient**. This testbed
scored 7 of 7 and still missed the worst bug the project has shipped: the
flagship one-line rename silently declined every method whose body assigned a
local variable, because Prism carries a scope's local table on the node and rwr
compared it as though it were syntax (D73).

It missed it because `Account#display_name` was written as a single expression —
`"#{first} #{last}"`. Every site around it was enumerated from Ruby, correctly,
and the *definition itself* was unrepresentative. A method with a variable in it
is too ordinary to occur to anyone as an edge case, which is exactly why nobody
wrote one.

So the rule has two halves:

1. Derive expectations from **Ruby semantics**, never from what rwr does.
2. Make the Ruby **representative**. Boring, ubiquitous constructs earn their
   place here as much as gnarly ones — more, because nobody thinks to test them.

When adding a case, ask what a reviewer would say if they saw it in a real pull
request. If the answer is "nobody writes code that simple", the case is not
pulling its weight.

## What it cannot measure

**Precision at scale.** A fixture its own author wrote proves nothing about
whether a report is screen-filling on a million lines. That half of Q1 is
measured against discourse and mastodon, and recorded in
`docs/internal/phase0-results.md`.
