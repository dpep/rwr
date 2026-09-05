# Getting started

## Install

```sh
cargo install rwr
```

Re-run the same line to update. There is no index, cache, or config to set up,
and the built-in rules are compiled into the binary.

## Three verbs

```sh
rwr 'foo($A)' app/          # find    — read-only
rwr check <rule> app/       # preview — what would change; never writes
rwr rewrite <rule> app/     # apply   — writes to disk
```

Writing always requires typing `rewrite`, so the terse form can never surprise
you. Trailing arguments are paths, rg-style.

## Find a shape

```sh
rwr 'Account.find($ID)'
rwr '$R.select { |$X| $B }.first' app/services
```

## Find a method

A shape is not the same question as a method. For a method, use Ruby's own
notation — `#` for an instance method, `.` for a class method:

```sh
rwr find 'Account#display_name' app/
```

That reports the definition, calls on a receiver that resolves to `Account` or a
subclass, `send(:display_name)`, the `attr_*` and visibility macros, and
implicit-self calls inside the class — and leaves `Company#display_name` alone.
Anything it could not tie to the method is reported as residue rather than
claimed as a match.

Since a pattern is Ruby and `#` starts a comment, the notation is the only way to
say this; and because the two-part form always means the method, write
`Account.display_name()` if you want the literal call shape instead.

`$NAME` captures one node, `*$NAME` a run of them, `_` and `*_` match without
capturing. All four are valid Ruby, so a pattern stays copy-pasteable from real
code.

## Change something

```sh
rwr check 'foo($A)' -r 'bar($A)' app/     # preview
rwr rewrite 'foo($A)' -r 'bar($A)' app/   # apply
```

`check` names each file and counts the sites in it. It does not print a diff.

## Delete something

```sh
rwr rewrite 'def legacy_total; $B; end' -d app/
```

Deletion takes the whole unit — the definition, the comment written directly
above it, and one of the blank lines that separated it, so the survivors keep
their spacing. `-r ''` means the same thing. A match that does not occupy whole
lines is refused: deleting `a.name` out of `x = a.name` would leave `x = `,
which swallows the line below and still parses.

## Rename a method

The common case has a one-line spelling, in Ruby's own notation:

```yaml
# rename.yml
method: Account#display_name    # `#` instance, `.` class — different methods
rename: full_name
```

```sh
rwr check rename.yml app/      # what it would touch, and what it could not
rwr rewrite rename.yml app/    # then apply
```

Two commands, not one chained with `&&`. `check` exits **1** when there is work
to do, so `check && rewrite` would rename only when there was nothing to rename.
The polarity is what makes `check` usable as a CI gate, and it is the opposite of
what a shell pipeline reads like.

That one line expands to the whole rename — the definition, subclass overrides,
explicit-receiver calls, and implicit-self calls inside the class. It leaves
`Company#display_name` and `Account.display_name` alone, because those are
different methods.

## Read the residue report

Anything rwr could not account for — a symbol reaching `delegate`, a
`send("display_name")`, a call whose receiver it could not resolve, a doc comment
still naming the old method — is listed with its file, line, and classification.
Those are the sites that will break or go stale, and handling them is part of
finishing the rename.

It prints unconditionally. The account of what rwr could not see is the product,
not a diagnostic, so it is never behind a verbosity flag.

Each entry carries a `context` saying what kind of occurrence it is, which is
what to triage on:

| context | what it is | does it break? |
|---|---|---|
| `call` | a call by that name whose receiver rwr could not resolve | maybe — it may be a different class's method |
| `symbol` | a symbol handed to something that dispatches (`delegate`, `send`, a serializer) | usually |
| `definition` | another definition of the name | depends — an override breaks, an unrelated class's method does not |
| `string` | the name as a string literal | maybe — `send("x")` breaks, a SQL column or a message does not |
| `comment` | the name in prose | no, but it is now stale |
| `text` | found by text search in a template rwr cannot parse | weaker evidence than anything above |
| `dynamic` | a dispatch on a *computed* name, in this class | unknowable — this is rwr saying it is blind here |

rwr deliberately does not put a confidence number on these. Measured against the
testbed's ground truth, `definition` splits evenly between breaking and not, and
`string`, `text` and `dynamic` have too few samples to support a figure — a score
derived from that would read like a measurement and be a guess.

In `-j`, `residue` has three states rather than two. Present with entries:
these need a human. Present and empty: rwr moved a name and found nothing left
over. **Absent**: this rule moves no name, so there is nothing it could be
incomplete about — a `return nil` → `return` rule has no leftovers by
construction. Reading absent as empty gives "nothing to review", which is right;
only a consumer asking whether a *rename* is complete needs the difference.

## Apply the built-in rules

```sh
rwr check all app/                 # every safe rule, read-only
rwr check performance app/         # one family
rwr check style/return-nil app/    # one rule
rwr rewrite all app/               # apply
```

Rules that can change behaviour are held back, and the run says how many; `-e`
prints the reason for each. `--unsafe` includes them; read the reasons first.
The pack and its safety notes: [rules/README.md](../rules/README.md).

## Gate a change in CI

Restrict the run to lines the change touched, so a rule with two thousand
pre-existing sites does not fail a pull request that added three:

```sh
rwr check all --diff                          # not committed yet
rwr check all --since "$GITHUB_BASE_REF"      # what this branch introduces
rwr check all --since main --diff             # both
rwr check all app/x.rb:3-15                   # or name the lines yourself
```

## Exit codes

| Code | Means |
|---|---|
| 0 | matched (`find`) / clean (`check`) / ran (`rewrite`) |
| 1 | no match (`find`) / there is work to do (`check`) |
| 2 | usage or I/O error, a path that does not exist included |
| 3 | the pattern or rule is wrong |
| 4 | retryable — an edit sat inside a wider one; **run again** |
| 5 | refused — ambiguity, and zero edits were made |

`rewrite` never exits 1: having applied whatever there was to apply is success.
It reports falling short with 4 or 5 instead. Only `rewrite` exits 4 — `check`
writes nothing, so it has nothing to defer.

Add `-j` (JSON) or `-J` (NDJSON) whenever something will parse the output.
