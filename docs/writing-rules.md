# Writing rules

A rule is a YAML file. `match:` and `rewrite:` are Ruby with metavariables;
`where:` adds what syntax cannot say.

```yaml
description: What this is for.
match: $R.$SEL { |$P| $B }.first
where:
  $SEL:
    name: [select, find_all]     # one rule covers both synonyms
  $R:
    type: Array                  # receiver must resolve to this class
rewrite: $R.detect { |$P| $B }
```

Point `rwr` at the file, a directory of them, or a built-in name:

```sh
rwr check my-rules/ app/
rwr check my-rules/detect.yml app/
```

A real path wins over a built-in name, so your own pack is selected the same way
the shipped one is — a pack *is* a directory.

## `where:` predicates

| Key | Constrains |
|---|---|
| `name: [a, b]` | the capture is one of these identifiers |
| `name_not: [a, b]` | the capture is none of these — narrow a rule that over-matches |
| `type: Klass` | the receiver resolves to that class |
| `kind: instance\|class` | which method table `type:` means |
| `subclasses: true` | admit descendants of `type:` |
| `same_name_as: $V` | two captures name the same identifier, across node kinds |
| `is: constant\|symbol\|string\|integer\|array\|hash` | the capture's node kind |
| `length: N` | a string/symbol literal's content, in characters |
| `contains: <pattern>` | a sub-pattern holds somewhere inside the capture |

`scope:` constrains the match as a whole — `inside: Account`, `singleton: true`,
`subclasses: true`.

**`type:` only ever narrows.** It resolves a receiver from a constructor
(`Widget.new.foo`), a local or ivar assigned from one, a constant, `self`, and —
where the repo has Sorbet signatures — whatever `sig { returns(X) }` says. A
receiver it *cannot* resolve does **not** match. That is deliberate, and it means
a `type:` rule quietly does less than you might expect; `-e` tells you which
happened:

```
$ rwr check rule.yml app/x.rb:42 -e
app/x.rb:42:3: rule: matched, then declined
  $R bound `whatever` -- receiver did not resolve; `type: Widget` only matches receivers rwr can resolve
```

"Did not resolve" and "resolved to the wrong class" are different problems. The
first may not be fixable by changing the rule.

## `contains:`

A whole sub-pattern inside a constraint, with shared metavariables required to
refer to the same thing:

```yaml
match: $R.each { |$X| $B }
where:
  $B: { contains: $X.$ASSOC.$FIELD }
```

Inside `{ ... }` a comma, brace or bracket belongs to YAML — quote such a pattern
or use indented keys. rwr refuses loudly rather than running a rule that silently
matches nothing.

## The other keys

| Key | Means |
|---|---|
| `id:` | what the rule is called in reports; defaults to the file's name, or its path within a pack |
| `ruby:` | the lowest Ruby version this rule's *output* parses on |
| `unsafe:` | why the rewrite can change behaviour (below) |
| `tests:` | fixtures (below) |

An unknown key is refused rather than ignored: `wher:` for `where:` would
otherwise run the rule *without its constraint*.

`ruby: "3.1"` holds the rule back on an older codebase. Nothing else can catch
that — `{foo:}` is a syntax error before 3.1, and Prism parses the output
happily. The codebase's version comes from `.ruby-version`, a Gemfile `ruby`
line, or a gemspec's `required_ruby_version`; `--ruby X.Y` overrides it, and an
undetected version holds the rule back rather than assuming the newest.

## Rules that only report

A rule with no `rewrite:` is a **finding**: it prints its matches with its
`description`, proposes no edit, and still exits 1 so a gate can act on it. Use
it where the right answer depends on something rwr cannot see.

## Sets

A file may hold one rule or a list. A list applies in order, each rule seeing the
last one's output — which is what makes a rename work, since a definition and its
call sites are different shapes.

## Unsafe rules

```yaml
unsafe: >-
  Assumes an ActiveRecord relation. `where` on anything else is a different
  method entirely.
```

Present means unsafe, and the value is the reason — there is no boolean to set
without saying what for. Held back unless `--unsafe`, and the run prints the
reason of every unsafe rule that fired.

## Fixtures

A rule can carry its own tests, so an rwr upgrade cannot quietly change what it
does to real code:

```yaml
tests:
  - input: "a = xs.select { |x| x.ok? }.first\n"
    output: "a = xs.detect { |x| x.ok? }\n"
  - input: "a = xs.select { |x| x.ok? }.last\n"
    unchanged: true          # the negative case
```

```sh
rwr test my-rules/        # exits 1 with a diff on a failure
rwr test my-rules/ -j     # for CI
```

A finding rule asserts `finds: N` instead of `output:`. A pack that declares no
fixtures at all is an error, not a pass.

Refused rather than run, because each would let a fixture pass without claiming
anything: a case that asserts nothing, `output:` together with `unchanged:`,
`unchanged: false`, `finds:` on a rule that rewrites, and `output:`/`unchanged:`
on a rule that only reports. A snippet that does not parse **fails** — otherwise
a typo'd fixture passes every negative assertion.

`output:` is compared byte for byte, the trailing newline included. The snippet
is evaluated as a whole file, so a rule needing a class or a `type:` receiver
writes that into its own input:

```yaml
tests:
  - input: |
      a = Account.new
      a.legacy_total
    output: |
      a = Account.new
      a.total
```

**Write the expectation from what the rule is for**, never by pasting what it
currently produces. A fixture written after the behaviour records bugs as
expectations — which has happened in this repo, and is why the testbed is written
from the Ruby side instead.

## There are no per-rule options

A rule is four lines of YAML, so the rule *is* the option. Want the opposite
direction? Copy the file and swap `match:` with `rewrite:`.
