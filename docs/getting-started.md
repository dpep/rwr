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

`$NAME` captures one node, `*$NAME` a run of them, `_` and `*_` match without
capturing. All four are valid Ruby, so a pattern stays copy-pasteable from real
code.

## Find a method

Where a method is used is a different question from where a shape appears. Ask it
in Ruby's own notation — `#` for an instance method, `.` for a class method:

```sh
rwr find 'Account#display_name' app/
```

That reports the definition, calls on a receiver that resolves to `Account` or a
subclass, `send(:display_name)`, the `attr_*` and visibility macros, and
implicit-self calls inside the class — and leaves `Company#display_name` alone.
Anything it could not tie to the method is reported as residue rather than
claimed as a match.

The enclosing body decides which method a macro configures: `attr_accessor
:display_name` in the class body is `Account#display_name`, and the identical
line inside `class << self` is `Account.display_name`. Same bytes, different
method, so each designator reaches only its own.

A pattern is Ruby and `#` starts a comment, so the notation is the only way to
say this. Going the other way, the two-part form always means the method, so
write `Account.display_name()` when you want the literal call shape.

Operator and writer methods — `==`, `<=>`, `[]`, `display_name=` — are not
supported through the notation yet. rwr names the offending method and refuses at
exit 5, rather than reading `Account#==` as a pattern, where it would silently
have meant the bare constant `Account`. A plain pattern still finds the
definition — `rwr find 'def ==(*$A); $B; end' app/` — but that is a shape match,
without the receiver narrowing or the residue account the notation gives you.

### Namespaced classes

An unqualified class name means the class of exactly that name if the run can
see one, and otherwise the single class whose qualified name ends with it. So
`Account#display_name` finds `Billing::Account` in a codebase that has only
that one — and means the top-level `Account`, and not `Billing::Account`, in one
that has both.

When several classes share a last segment and none is top-level — nine
`…::LogSubscriber`s and no plain one — the short name matches none of them.
Picking one would be a rename applied to the wrong class, which is the failure
the whole tool exists to prevent, so rwr picks none and you write the qualified
name: `ActiveSupport::LogSubscriber#logger`.

**An ambiguous name looks exactly like a missing one** — zero sites, exit 1, and
nothing saying why. So when a designator you expected to hit comes back empty,
qualify it before concluding the method isn't there.

Constants written in your source resolve the way Ruby resolves them, against the
enclosing module nesting.

## Change something

```sh
rwr check 'foo($A)' -r 'bar($A)' app/     # preview
rwr rewrite 'foo($A)' -r 'bar($A)' app/   # apply
```

`check` names each file and counts the sites in it. It does not print a diff.

## Delete something

```sh
rwr rewrite 'def legacy_total(*$A); $B; end' -d app/
```

Deletion takes the whole unit — the definition, the comment written directly
above it, and one of the blank lines that separated it, so the survivors keep
their spacing. `-r ''` means the same thing. A match that does not occupy whole
lines is refused: deleting `a.name` out of `x = a.name` would leave `x = `,
which swallows the line below and still parses.

**Write the parameter list as `(*$A)`.** A pattern matches arity exactly, so
`def legacy_total($A)` finds only the one-parameter version and `def legacy_total`
only the zero-parameter one. `*$A` captures a run, so it covers every arity.
Get it wrong and the method comes back as residue, the file keeps every byte,
and — because `rewrite` never exits 1 — the run still exits 0. A deletion that
deleted nothing and a deletion that worked have the same exit code, so read the
output, not the status.

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

The `method:` line expands to the whole rename — the definition, subclass
overrides, explicit-receiver calls, and implicit-self calls inside the class. It
leaves `Company#display_name` and `Account.display_name` alone, because those are
different methods.

### A definition a concern contributes

A method defined in a module the class includes is the class's method, written in
another file, so the rename moves it:

```ruby
module Suspensions          # def suspended? here is Account#suspended?
  def suspended?; true; end
end
class Account
  include Suspensions
end
```

**Unless another class includes the same module.** Then the definition is
`Invoice`'s too, and moving it would rename a method you did not ask about —
while `invoice.suspended?` stays behind, because the rename narrows receivers to
`Account`. rwr leaves the definition alone, reports it as residue, and exits 1
rather than completing a rename that is quietly wider than the one you typed. To
rename it for everybody, edit the module and run the rename once per includer.

`extend` is not `include`: it puts the module's methods on the class's *singleton*
table, so `Account.find_it` comes from `def find_it` in an extended module. That
definition is still reported rather than moved.

### `attr_accessor` and the writer

`attr_accessor :label` defines `label` **and** `label=` from one symbol, so
renaming it to `caption` renames both — and the rename carries the `w.label = 1`
call sites with it. A hand-written `def label=` is a separate method: the rename
leaves it, and its callers, exactly where they are.

`w.label += 1` and the other operator-assignment forms are not reached yet.

## Read the residue report

Anything rwr could not account for — a symbol reaching `delegate`, a
`send("display_name")`, a call whose receiver it could not resolve, a doc comment
still naming the old method — is listed with its file, line, and classification.
Those are the sites that will break or go stale, and handling them is part of
finishing the rename.

It prints unconditionally. The account of what rwr could not see is the product,
not a diagnostic, so it is never behind a verbosity flag.

**It prints on stderr.** The matches and the per-file counts go to stdout; the
residue report, the blind-spot warnings and the `read … as` line go to stderr.
A script that captures stdout alone sees `rewrote 3 site(s)` and exit 0, and
never learns that four sites need a human. Capture both, or use `-j`, which puts
everything in one document on stdout.

Each entry carries a `context` saying what kind of occurrence it is, which is
what to triage on:

| context | what it is | does it break? |
|---|---|---|
| `call` | a call by that name whose receiver rwr could not resolve | maybe — it may be a different class's method |
| `symbol` | a symbol handed to something that dispatches (`delegate`, `send`, a serializer) | usually |
| `definition` | another definition of the name | depends — an override breaks, an unrelated class's method does not |
| `string` | a string that *is* the name | maybe — `send("x")` breaks, a SQL column does not |
| `prose` | the name inside a longer string or regexp — `raise ArgumentError, "display_name needs a Router"` | no, but it is now stale |
| `comment` | the name in a Ruby comment | no, but it is now stale |
| `text` | found by text search in a template rwr cannot parse | weaker evidence than anything above |
| `dynamic` | a dispatch on a *computed* name, in this class | unknowable — this is rwr saying it is blind here |

**The class you name sets how far prose and comments are searched.** Those two
are kept inside that class, its contributors and its subclasses — unscoped,
renaming a name as ordinary as `name` would report every sentence in the
repository containing the word. So `Account#display_name` reports the error
message *inside* `Account` and not the spec description outside it. Calls are not
scoped this way: a call is a reach wherever it lives.

The classless form has no class to scope by, which makes it the wide net:

```sh
rwr find '#display_name'          # every mention, spec descriptions included
```

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

## The rest of the blind spots

Residue is what rwr read and could not attribute. Four more kinds of gap say what
it never read at all, on every report from every verb:

| Field | What it means |
|---|---|
| `unparsed` | a Ruby file with a syntax error — nothing in it was searched |
| `unreadable` | a file rwr could not open, usually permissions — likewise |
| `templates_skipped` / `template_residue` | a template rwr cannot parse (Haml), text-searched instead; grep-grade evidence |
| `unknown_suppressions` | a `# rwr:ignore` naming a rule this run does not have, so it suppressed nothing — see [suppressing findings](suppressing.md) |

`unparsed` and `unreadable` are the same blind spot with opposite fixes — one is
the file's problem, the other your permissions' — which is why they are separate
lists. Neither changes the exit code: eighty matches plus one unreadable vendored
file is a successful run with a gap declared in it.

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
rwr check all --since "origin/$GITHUB_BASE_REF"   # what this branch introduces
rwr check all --since main --diff             # both
rwr check all app/x.rb:3-15                   # or name the lines yourself
```

A scope names lines, and a site is in it when the **bytes the rule would write**
fall on one of them — not when the code it matched happens to span one. A rename
matches a whole `def … end` and writes only the signature, so editing line 40 of
a method does not put its signature on line 12 in scope.

The other direction is not free. A site is rewritten whole or not at all, so
naming one line of an expression that spans three writes all three:

```ruby
result = things
  .select { |t| t.active? }    # name this line
  .first                       # and this one is rewritten too
```

rwr says so rather than leaving it to the diff — one line per site on stderr,
and `wrote_beyond_scope` in `-j`, present only when it happened.

**Residue is not scoped.** The sites are; the account of what rwr could not tie
to the rule is computed over each whole file it read. An occurrence it could not
resolve — a `public_send`, a name in prose, a template it cannot parse — has no
reliable relationship to the lines your change touched, and hiding one because
it sits ten lines away would defeat the point of reporting it. Residue does not
move the exit code, so it never fails the gate on its own.

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
It reports falling short with 4 or 5 instead. So a rewrite that matched nothing
and a rewrite that changed the whole repository both exit 0 — branch on the
output, not the status. Only `rewrite` exits 4; `check` writes nothing, so it has
nothing to defer.

Add `-j` whenever something will parse the output. `-J` is the same document
printed on a single line, for a consumer that reads line by line.
