---
name: rwr
description: Search and rewrite Ruby by structure with the `rwr` CLI — find code shapes, rename methods across a codebase, and apply modernization rules. Use for "find every place that calls X", "rename this method everywhere", "apply hash shorthand / return nil / performance fixes", or any Ruby refactor where a regex would hit comments and strings. Prefer over sed/rg for changing Ruby; it matches the parse tree, never the text.
---

# rwr — structural search and rewrite for Ruby

`rwr` is `rg`/`sed` for Ruby *programs* rather than Ruby *text*. It parses with
Prism, so a comment, a string literal, or a heredoc body that happens to contain
your pattern is not a match. Reach for it whenever the goal is **"change this
shape of Ruby everywhere"**.

Its contract, which is what makes it usable unattended:

- **It refuses rather than guesses.** Ambiguity produces a diagnostic and zero
  edits. A refusal costs a round trip; a wrong rewrite costs trust.
- **It never silently drops an edit.** If something didn't apply, it says so and
  the exit code reflects it.
- **It reports what it could not see**, unconditionally — the `attr_reader
  :display_name` a rename would break is listed, not hidden behind `--verbose`.
- **Diffs are minimal.** Untouched code keeps its bytes, including layout and
  heredocs.

## Three verbs, and the verb carries the mode

```sh
rwr 'foo($A)' app/          # find    — read-only, the bare-pattern shorthand
rwr check <rule> app/       # preview — what would change; never writes
rwr rewrite <rule> app/     # apply   — writes to disk
```

Writing always requires typing `rewrite`, so the terse form can never surprise
you. Trailing arguments are paths, rg-style.

Always add `-j` (JSON) or `-J` (NDJSON) when you'll parse the output. `-j` is one
document — `{schema, rwr_version, changed, residue, templates_skipped}` for
`check`/`rewrite`, `{schema, rwr_version, matches}` for `find` — so the account
of what a rule *missed* is machine-readable, not just printed. `-J` streams a
row per line and carries no metadata.

## Finding

```sh
rwr 'Account.find($ID)' -j
rwr '$R.select { |$X| $B }.first' app/services -j
```

Patterns are Ruby source with metavariables:

| Form | Means |
|---|---|
| `$NAME` | capture one node |
| `*$NAME` | capture a run of nodes (`foo($A, *$REST)`); `**$NAME` inside a hash |
| `_` | match one node, don't capture |
| `*_` | match a run, don't capture |

All four are valid Ruby, so a pattern stays copy-pasteable from real code.

## Renaming a method

The common case has a one-line spelling, in Ruby's own notation:

```yaml
# rename.yml
method: Account#display_name    # `#` instance, `.` class — they are different methods
rename: full_name
```

```sh
rwr check rename.yml app/ && rwr rewrite rename.yml app/
```

That one line expands to the whole rename: the definition, subclass overrides,
explicit-receiver calls, and implicit-self calls inside the class. It leaves
`Company#display_name` and `Account.display_name` alone, because those are
different methods — the receiver narrowing no other Ruby structural tool does.

**Read the residue report.** Anything it could not account for — a symbol
reaching `delegate`, a `send("display_name")`, a call whose receiver it could not
resolve — is listed with its file, line, and classification. Those are the sites
that will break, and handling them is part of finishing the rename.

## Applying the built-in rules

The pack is compiled into the binary and works from any directory:

```sh
rwr check all app/                 # every safe rule, read-only
rwr check performance app/         # one family
rwr check style/return-nil app/    # one rule
rwr rewrite all app/               # apply
```

Families are `style` and `performance`. A real path wins over a built-in name,
so your own rules directory is selected the same way — a pack *is* a directory.

**Rules that can change behaviour are held back** and the run says which and
why. Pass `--unsafe` to include them; when one fires, its caveat prints next to
the diff. Don't pass `--unsafe` blind — read the reasons first, and prefer
narrowing the rule with a receiver type.

## Writing a rule

A rule is a YAML file. `match` and `rewrite` are Ruby with metavariables;
`where` adds what syntax cannot say.

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

`where:` predicates:

| Key | Constrains |
|---|---|
| `name: [a, b]` | the capture is one of these identifiers |
| `type: Klass` | the receiver resolves to that class (conservative — unresolved never matches) |
| `kind: instance\|class` | which method table `type:` means |
| `subclasses: true` | admit descendants of `type:` |
| `same_name_as: $V` | two captures name the same identifier, across node kinds |
| `is: constant\|symbol\|string\|integer\|array\|hash` | the capture's node kind |
| `length: N` | a string/symbol literal's content, in characters |

`type:` resolves a receiver from a constructor (`Widget.new.foo`), a local or
ivar assigned from one, a constant, `self`, and — where the repo has Sorbet
signatures — whatever `sig { returns(X) }` says. A receiver it cannot resolve
does **not** match, so `type:` only ever narrows; the misses show up as residue.

And `scope:` constrains the match as a whole — `inside: Account`,
`singleton: true`, `subclasses: true`.

A file may hold one rule or a list; a list applies in order, each rule seeing
the last one's output. Point `rwr` at a directory to run all of them.

There are **no per-rule options** — a rule is four lines of YAML, so the rule
*is* the option. Want the opposite direction? Copy the file and swap `match`
with `rewrite`.

## Scoping to a change

For a pre-commit hook or a pull-request gate, restrict the run to lines the
change touched — otherwise a rule with two thousand pre-existing sites fails a
change that added three:

```sh
rwr check all --diff          # uncommitted work
rwr check all --diff main     # what this branch introduces (main...HEAD)
```

Works with `find`, `check` and `rewrite`.

## Safety signals to actually read

Three things rwr says that are worth acting on rather than skimming:

**`warning: rewrote receivers of N different classes`.** `Account#display_name`
and `Company#display_name` are different methods. The rule renamed both. Add a
`type:` constraint unless that was really meant.

**`N rule(s) held back as unsafe`.** Those rewrites can change behaviour. `-e`
prints why for each; `--unsafe` runs them anyway. Read the reasons before
passing it.

**`N template file(s) were not searched`.** ERB and Haml embed Ruby that rwr
does not parse, so the completeness claim covers `.rb` and friends only. In a
Rails app a real share of call sites lives there and needs checking by hand.

Rules whose output needs a newer Ruby than the codebase targets are also held
back — detected from `.ruby-version`, a Gemfile `ruby` line, or a gemspec's
`required_ruby_version`, and overridable with `--ruby X.Y`.

## Exit codes

| Code | Means |
|---|---|
| 0 | matched (`find`) / nothing to change (`check`) / applied (`rewrite`) |
| 1 | no match (`find`) / there is work to do (`check`) |
| 2 | usage error |
| 3 | the pattern or rule is wrong |
| 4 | retryable — an edit sat inside a wider one; **run again** |
| 5 | refused — ambiguity, and zero edits were made |

`check` inverts polarity deliberately so a clean tree does not block a commit,
which is what makes it usable in a pre-commit hook or CI.

**Exit 4 means run the same command again.** Two redundant pairs in one hash, or
nested matches, need more than one pass; rwr reports the remainder rather than
looping internally, because a rule like `foo($A)` → `foo(bar($A))` would never
converge if it did.

## Notes

- `--profile` reports where the time went. A whole Rails app is ~1.5s.
- Generated and vendored code is skipped; `--include-vendored` overrides.
- Ruby means more than `.rb`: `.rake`, `.ru`, `.gemspec`, `.jbuilder`,
  `Rakefile`, `Gemfile` and friends are searched too.
- `-e/--explain` says which constraint rejected a candidate and how a residue
  occurrence was classified — reach for it when a rule matches less than you
  expected.
- rwr does **not** format. Indentation, alignment and trailing commas are
  presentation; it only repairs layout it disturbed itself.

## Installing / updating the binary

If `rwr` isn't on PATH, install it, then retry:

```sh
cargo install rwr        # needs the Rust toolchain; no Homebrew formula yet
```

Re-run the same line to update. Nothing else to do afterwards — there is no
index, cache, or config to set up, and the rule pack is compiled into the
binary.

**If rwr rejects something this skill describes** — `check all`, `--unsafe`,
an `is:`/`length:` constraint — the installed binary predates the skill rather
than lacking the feature. Check `rwr --version`, update, and retry before
concluding the tool can't do it.

Source and issues: <https://github.com/dpep/rwr>.
