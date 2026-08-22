# The suppression corpus

Ground truth for `# rwr:ignore`, kept apart from the rename testbed next door
because it answers a different question. That one asks *what did a rename fail
to see*; this one asks *what did a directive accept, and did it accept exactly
that*.

Scored by `tests/suppression.rs`, against the shipped pack.

| marker | meaning |
|---|---|
| `GT:accepted` | a directive covers this finding; it must be suppressed and counted |
| `GT:flagged` | this violation must survive — a directive nearby must not reach it |
| `GT:stale` | this directive has nothing left to accept and must be reported |
| `GT:malformed` | this directive names no rule; it must be reported and suppress nothing |

**Written from the failure side.** Every case here is one where a plausible
implementation is silently wrong rather than visibly broken: a directive that
reaches too far, one that stops too soon, one that suppresses a rule it does not
name. The first implementation of node scoping passed every happy-path case in
this file and still swallowed an entire file, because nothing here asked what
happened *after* the covered statement ended. `GT:flagged` is that question.
