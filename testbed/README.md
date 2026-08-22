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

## What it cannot measure

**Precision at scale.** A fixture its own author wrote proves nothing about
whether a report is screen-filling on a million lines. That half of Q1 is
measured against discourse and mastodon, and recorded in
`docs/internal/phase0-results.md`.
