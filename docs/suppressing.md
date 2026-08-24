# Suppressing findings

There are three ways to stop a finding failing a run, and they are not
interchangeable. Pick by what you actually believe:

| You believe | Use | Lifetime |
|---|---|---|
| The finding is **wrong** — the rule over-matches | a `where:` predicate | permanent, and portable to every repo |
| Code this change **touched** must be clean | `--diff` / `--since` | per run, no state |
| This one site is a **deliberate exception** | `# rwr:ignore` | permanent, visible at the site, reviewed with the code |

**Narrow before you suppress.** If you would have to explain why the finding is
*wrong*, fix the rule instead. A suppression records debt; recording debt that is
not debt leaves the rule broken for the next repo that runs it.

## Fixing the rule

If a rule flags names that are genuinely unrelated, it over-matches. Say so in
the rule:

```yaml
where:
  $C: { name_not: [ALL, TYPES, STATUSES] }
```

The finding then does not exist — it is not counted, not reported, and not debt.

## Scoping to a change

```sh
rwr check all --diff                       # not committed yet
rwr check all --since "$GITHUB_BASE_REF"   # what this branch introduces
```

Nothing is recorded anywhere. Pre-existing sites are out of scope this run and
back in scope the moment someone touches their lines.

## `# rwr:ignore`

```ruby
sleep 0.1  # rwr:ignore style/no-sleep

# rwr:ignore style/no-sleep, performance/detect -- flaky in CI, see PIE-4
def wait_for_worker
  sleep 0.1        # covered — the directive takes the whole method
end
```

`style/no-sleep` is a rule of your own, not one the pack ships — a directive names whatever rule id fired. Trailing on a line, or leading above one. It covers the **outermost statement
starting on the attached line**, so above a `def` it means the whole method,
nested blocks included, and it stops at that method's `end`.

A reason may follow `--`. Rule ids are required: a bare `# rwr:ignore` is
reported as malformed and suppresses nothing, because a blanket ignore is one no
staleness check can audit.

There is deliberately **no `disable`/`enable` block form**. A forgotten
terminator silently suppresses the rest of a file, which is the invisible blind
spot rwr exists to refuse. If you need a wider exception than a statement, the
rule is probably mis-scoped — narrow it instead.

`rewrite` honours directives exactly as `check` does, because `check` is its
preview.

## What a suppression can never do

**Silence itself.** Every run says how many findings were accepted and which
directives have nothing left to accept, in text and in `-j`:

```
rwr: 2 finding(s) accepted by rwr:ignore directive(s)
rwr: 1 stale rwr:ignore directive(s) -- nothing left to accept there:
  app/models/order.rb:14: style/return-nil -- delete the comment
```

A stale directive does not fail the build — its finding is already gone, so what
remains is tidying — but it is never invisible. This is the mechanism that stops
a suppression list becoming a permanent monument to work nobody did.

**Touch the residue report.** Directives suppress findings and edits. The account
of what rwr could not see is the product, and nothing here can quiet it.
