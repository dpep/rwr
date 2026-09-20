---
name: rwr
description: Find or change Ruby by structure with the `rwr` CLI. Use for "find every place that calls X", "where is this shape of code used", "rename this method everywhere", "apply hash shorthand / return nil / performance fixes", linting or modernizing a codebase in bulk, or any Ruby refactor where a regex would hit comments and strings. Scales to a whole codebase without giving up precision — it matches the parse tree, never the text, and refuses rather than guessing. Prefer over sed/rg for reading or editing Ruby by shape.
---

# rwr — structural search and rewrite for Ruby

`rwr` is a power tool for refactoring Ruby codebases — a scalpel at scale. It's
`rg`/`sed` for Ruby *programs* rather than Ruby *text*: it parses with Prism, so
a comment, a string literal, or a heredoc body that happens to contain your
pattern is not a match. Reach for it whenever the goal is **"change this shape of
Ruby everywhere"** — the bigger the codebase, the more it beats doing it by hand
or by regex.

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

**Always run with `-j`.** In text mode the matches and the per-file counts go to
**stdout** and everything rwr could *not* account for goes to **stderr** — the
residue report, the unparsed and unreadable file lists, the suppression audit and
the `read … as` line. Capture stdout alone and you read `rewrote 3 site(s)`,
exit 0, and never learn that four sites need a human. `-j` puts all of it in one
document on stdout. `-J` is that same document on a single line, for every verb;
there is no event stream and no terminator record.

The document is one shape across the verbs:

```
{schema, rwr_version, residue, template_residue, templates_skipped,
 unparsed, unreadable, suppressed, stale_suppressions, unknown_suppressions,
 malformed_directives}
  + {matches, interpreted}   on find
  + {changed, findings}      on check and rewrite
```

Current `schema` is `6`. What each blind-spot field means:

| Field | Means | What to do |
|---|---|---|
| `residue` | read, but could not be tied to the method | triage each by its `context` |
| `unparsed` | a Ruby file with a syntax error | fix the file and rerun; the run never saw it |
| `unreadable` | a file rwr could not open | fix permissions and rerun; the run never saw it |
| `templates_skipped`, `template_residue` | a template rwr cannot parse (Haml), text-searched | check by hand — grep-grade evidence |
| `stale_suppressions` | rwr ran that rule and it fired nowhere | delete the comment |
| `unknown_suppressions` | rwr never ran that rule — the id is wrong | the finding is probably still live; fix the id, which `did_you_mean` often names |

`unparsed` and `unreadable` are the same blind spot with opposite fixes. Neither
changes the exit code: eighty matches plus one unreadable vendored file is a
successful run with a gap declared in it.

**`residue` has three states.** Present with entries: these need a human. Present
and empty: rwr moved a name and found nothing left over. **Absent**: the rule
moves no name, so it has no leftovers by construction. Reading absent as empty
gives "nothing to review", which is right; only ask the difference when you need
to know whether a *rename* is complete.

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

## Finding a *method*, not a shape

"Where is this method called" is a different question from "where does this shape
appear", and it has its own spelling: Ruby's method notation.

```sh
rwr find 'Account#display_name' app/     # the instance method
rwr find 'Account.display_name' app/     # the class method — different method
rwr find '#display_name' app/            # the method on any class
```

This is receiver narrowing, read-only. It reports the definition in all three
spellings, explicit-receiver calls narrowed by class *and* subclasses,
`send`/`try` with a literal name, the `attr_*` and visibility macros,
`define_method`/`alias_method`, and implicit-self calls inside the class — and
leaves `Company#display_name` and `Account.display_name` alone, because those are
different methods. A call whose receiver it cannot resolve is reported as residue
rather than claimed as a match.

**A macro belongs to the body it sits in.** `attr_accessor :display_name` in the
class body configures `Account#display_name`; the identical line inside
`class << self` configures `Account.display_name`. Same bytes, different method,
and each designator reaches only its own.

**An unqualified class name is resolved against the classes the run can see.** It
means the class of exactly that name if there is one, and otherwise the single
class whose qualified name ends with it — so `Account#display_name` finds
`Billing::Account` in a codebase that has only that one. Where several classes
share a last segment and none is top-level, it means *none of them*: choosing
between `ActiveSupport::LogSubscriber` and `ActionView::LogSubscriber` would be a
rename applied to the wrong class. Write the qualified name to say which you
meant — `rwr find 'ActiveSupport::LogSubscriber#logger' .`.

That case looks exactly like a method that isn't there: zero sites, exit 1,
nothing said about the ambiguity. **When a designator you expected to hit comes
back empty, qualify it before reporting that the method does not exist.**

**Operator and writer methods are refused, at exit 5 from all three verbs.**
`Account#==`, `User#name=`, `Account#[]`. Renaming them is not supported yet;
rwr names the method and stops rather than reading `Account#==` as a pattern,
where `#` opens a comment and it would silently have meant the bare constant
`Account`. A plain pattern still finds the definition —
`rwr find 'def ==(*$A); $B; end' app/` — but it is a shape match with no receiver
narrowing and no residue account, so the call sites are yours to chase.

Exit 0 when there are sites, 1 when there are none — find's usual polarity.
Residue does not make it 0: an occurrence rwr could not tie to *this* method is
not a site of it. `-j` carries the full document, `interpreted` included: that
object names the reading rwr took, and is worth checking when the answer
surprises you.

**A pattern is Ruby, so `#` starts a comment.** That is why the notation exists:
without it `Account#display_name` would parse as the constant `Account` followed
by a comment. Going the other way, the two-part form always means the method, so
write `Account.display_name()` when you want the literal call shape and nothing
else.

The same notation works wherever a rule is named, and means the same sites:

```sh
rwr check 'Account#display_name' app/                    # same, enforcement polarity
rwr rewrite 'Account#display_name' -r full_name app/     # the whole rename, no YAML
```

## Renaming a method

The common case has a one-line spelling, in Ruby's own notation:

```yaml
# rename.yml
method: Account#display_name    # `#` instance, `.` class — they are different methods
rename: full_name
```

```sh
rwr check rename.yml app/      # read the site counts and the residue report
rwr rewrite rename.yml app/    # then apply
```

A file is worth it when the rename needs `where:` or fixtures. For a plain one,
`rwr rewrite 'Account#display_name' -r full_name app/` expands to the same rule
set with nothing to save. Naming a method with no new name for it refuses
(exit 5) rather than reporting its sites and exiting 0 having written nothing.

Two commands, not one chained with `&&`: `check` exits **1** when there is work
to do, so `check && rewrite` would apply the rename only when there was nothing
to rename. The polarity is deliberate — it is what makes `check` usable as a
gate — and it is the opposite of what a shell pipeline reads like.

The `method:` line expands to the whole rename: the definition, subclass
overrides, explicit-receiver calls, and implicit-self calls inside the class. It
leaves `Company#display_name` and `Account.display_name` alone, because those
are different methods — the receiver narrowing no other Ruby structural tool
does.

**Read the residue report.** Anything it could not account for — a symbol
reaching `delegate`, a `send("display_name")`, a call whose receiver it could not
resolve, a doc comment still naming the old method — is listed with its file,
line, and classification. Those are the sites that will break or go stale, and
handling them is part of finishing the rename. In text mode it is on **stderr**;
under `-j` it is the `residue` array. Either way, read it before telling anyone
the rename is done.

**A rename has a prose half, and rwr reports it but never rewrites it.** The
places a parser must ignore are exactly where a public method documents itself:
its doc comment, its spec descriptions, and the error message it raises at its
own caller. Those come back as `comment` and `prose`, and on a well-documented
method they can outnumber the code sites. `prose` is a mention inside a longer
string or a regexp — `raise ArgumentError, "display_name needs a Router"`,
`/display_name/` — as against `string`, which is a string that *is* the name and
may be a live dispatch.

**The class you name sets how far prose and comments are searched.** Both are
kept inside that class, its contributors and its subclasses — unscoped, renaming
a name as ordinary as `name` would report every sentence in the repository
containing the word. So `Account#display_name` reports the error message *inside*
`Account` and not a spec description outside it. Calls are not scoped this way; a
call is a reach wherever it lives.

The classless form has no class to scope by, which makes it the wide net:

```sh
rwr find '#display_name'          # every mention, spec descriptions included
```

If you want every stale mention of a name across a repo, that is the command —
not the class-scoped one.

## Applying the built-in rules

The pack is compiled into the binary and works from any directory:

```sh
rwr check all app/                 # every safe rule, read-only
rwr check performance app/         # one family
rwr check style/return-nil app/    # one rule
rwr rewrite all app/               # apply
```

Families are `style`, `performance` and `rspec`. A real path wins over a
built-in name, so your own rules directory is selected the same way — a pack
*is* a directory.

`rspec` is the one family whose name is also a scope. A rule constrains the
tree, never the path, so point it at the specs: `rwr check rspec spec/`.

**Rules that can change behaviour are held back** and the run says which and
why. Pass `--unsafe` to include them; when one fires, its caveat prints next to
the counts. Don't pass `--unsafe` blind — read the reasons first, and prefer
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
| `name_not: [a, b]` | the capture is none of these — narrow a rule that over-matches |
| `type: Klass` | the receiver resolves to that class (conservative — unresolved never matches) |
| `type_not: [A, B]` | the receiver **resolves, and to none of these** — unresolved fails it |
| `kind: instance\|class` | which method table `type:` means |
| `subclasses: true` | admit descendants of `type:` |
| `same_name_as: $V` | two captures name the same identifier, across node kinds |
| `is: constant\|symbol\|string\|integer\|array\|hash` | the capture's node kind |
| `length: N` | a string/symbol literal's content, in characters |

`type:` resolves a receiver from a constructor (`Widget.new.foo`), a local or
ivar assigned from one, a constant, `self`, and — where the repo has Sorbet
signatures — whatever `sig { returns(X) }` says, plus the parameter types a
`sig { params(x: X) }` gives the body it describes. A receiver it cannot resolve
does **not** match, so `type:` only ever narrows; the misses show up as residue.

`type_not:` is **not** the mirror of `name_not:`. `name_not:` passes when the
capture has no identifier at all; a type exclusion that passed on an unresolved
receiver would widen instead of narrow, letting every receiver rwr cannot see
past a guard written to stop it. So unresolved *fails* an exclusion. Descent is
always honoured with no flag — "not an `ActiveRecord::Base`" means not an
`Account` either. Note `T::Boolean` resolves by its constant path's last segment,
so excluding a boolean means `[TrueClass, FalseClass, Boolean]`.

**When a `type:`/`type_not:` rule matches nothing, run `-e` before editing the
rule.** It separates the two failures, and they have opposite fixes:

```
$X bound `flag`    -- resolved to Boolean, excluded by `type_not: [...]`
$X bound `account` -- receiver did not resolve; `type_not: [...]` needs a receiver rwr can resolve
```

The first is the constraint working. The second is a gap in what rwr can see —
closed by writing a signature, not by loosening the rule. A repo with no
signatures at all resolves almost nothing, and such a rule correctly rewrites
nothing.

`*$ITEMS` captures a *run* of elements, and a suffix reorders it in the template
— `.sort`, `.uniq`, `.reverse`, and nothing else:

```yaml
match: $C = [*$ITEMS]
where:
  $C: { is: constant }
rewrite: $C = [*$ITEMS.sort]        # PERMS = [:zebra, :apple] -> [:apple, :zebra]
```

An unrecognised suffix is **refused**, never emitted as text — `*$ITEMS.srot`
would otherwise write `items.srot` into the source and parse fine. Comments
travel with the element on their line; one that could describe either neighbour
refuses rather than guessing.

And `scope:` constrains the match as a whole — `inside: Account`,
`singleton: true`, `subclasses: true`.

A file may hold one rule or a list; a list applies in order, each rule seeing
the last one's output. Point `rwr` at a directory to run all of them.

There are **no per-rule options** — a rule is four lines of YAML, so the rule
*is* the option. Want the opposite direction? Copy the file and swap `match`
with `rewrite`.

## Pinning a rule with fixtures

A rule can carry its own tests, so an rwr upgrade cannot quietly change what it
does to real code:

```yaml
match: $R.$SEL { |$P| $B }.first
where:
  $SEL: { name: [select, find_all] }
rewrite: $R.detect { |$P| $B }
tests:
  - input: "a = xs.select { |x| x.ok? }.first\n"
    output: "a = xs.detect { |x| x.ok? }\n"
  - input: "a = xs.select { |x| x.ok? }.last\n"
    unchanged: true          # the negative case
```

```sh
rwr test rule.yml       # exits 1 with a diff on a failure
rwr test my-rules/ -j   # a directory, for CI
```

A rule with no `rewrite:` asserts `finds: N` instead of `output:`. A rule that
moves a *name* can also assert `residue: N` — how many occurrences it should be
unable to account for. That is the half of a rename that decides whether the
change is safe to ship, and without it a fixture pins only the easy half. A case
may assert several of these at once; all of them are checked. **Write a case
when you write a rule** — and write the expectation from what the rule is *for*,
never by pasting what it currently produces, which records bugs as expectations.

`output:` is compared byte for byte, the trailing newline included. The snippet is
evaluated as a whole file, so a rule needing a class or a `type:` receiver writes
that into its own input (`a = Account.new` then `a.foo`).

Refused rather than run: a case that asserts nothing, `output:` with
`unchanged:`, and `finds:` on a rule that rewrites. A snippet that does not parse
**fails** — it would otherwise pass every negative assertion vacuously.

## Scoping to a change

For a pre-commit hook or a pull-request gate, restrict the run to lines the
change touched — otherwise a rule with two thousand pre-existing sites fails a
change that added three:

```sh
rwr check all --diff                # uncommitted work, untracked files included
rwr check all --since main          # what this branch introduces (main...HEAD)
rwr check all --since main --diff   # both: merge base against the working tree
rwr check all app/x.rb:3-15         # those lines, named directly
```

`--diff` takes **no value** — `--diff main` reads `main` as a path. Use `--since`
for a revision. In CI that is `--since "origin/$GITHUB_BASE_REF"`, which is more correct
than the default branch for a PR targeting a release branch.

`--since main` alone is commit-to-commit, so your uncommitted work sits outside
it; add `--diff` to include the working tree.

`PATH:N` and `PATH:N-M` are the `file:line` rwr itself prints, so an output line
pastes back in. Every path must name lines or none may, and they cannot be
combined with `--diff`/`--since`: two answers to which lines to check is a usage
error at **exit 2**, not a precedence rule. (Exit 2 is a malformed command — fix
the arguments and rerun. It is not exit 5, which means rwr understood you and
declined to guess.)

Works with `find`, `check` and `rewrite`.

## Accepting a finding you are not going to fix

```ruby
sleep 0.1  # rwr:ignore style/no-sleep

# rwr:ignore style/no-sleep, performance/detect
def wait_for_worker
  sleep 0.1        # covered — the directive takes the whole method
end
```

Trailing on a line, or leading above one. It covers the **outermost node
starting on the attached line**, so above a `def` it means the method. There is
no `disable`/`enable` block form — a forgotten terminator would silently
suppress the rest of a file.

Rule ids are required: a bare `# rwr:ignore` is reported as malformed and
suppresses nothing. A reason may follow `--`:
`# rwr:ignore style/no-sleep -- flaky in CI, see PIE-4`. `style/no-sleep` is a rule of your own, not one the pack ships — a directive names whatever rule id fired. `rewrite` honours directives exactly as `check` does.

**The id to write is the one the run prints.** A rule file's id is its stem
(`rename.yml` → `rename`), a rule in a pack is its path (`style/return-nil`), and
a designator on the command line is itself (`Account#display_name`). So the same
rename suppresses under a different id depending on which form ran it. If unsure,
run it and read the id out of `-j`.

Every run says how many findings were accepted, which directives have nothing
left to accept (`stale_suppressions` — delete the comment), and which name a rule
it never ran (`unknown_suppressions` — the finding is still live). Neither fails
the build; both are always printed.

**Before reaching for it, ask which of these you mean** — they are not
interchangeable:

| You believe | Use |
|---|---|
| The finding is wrong; the rule over-matches | a `where:` predicate — `name_not:`, `type:` |
| Code this change touched must be clean | `--diff` / `--since` |
| This one site is a deliberate exception | `# rwr:ignore` |

Narrow before you suppress. If you would have to explain why the finding is
*wrong*, fix the rule instead — a suppression records debt, and recording debt
that is not debt leaves the rule broken for the next repo.

## Safety signals to actually read

**All of these are on stderr.** They are absent from a stdout-only capture and
from the exit code. Run with `-j`, or capture both streams.

**`warning: rewrote receivers of N different classes`.** `Account#display_name`
and `Company#display_name` are different methods. The rule renamed both. Add a
`type:` constraint unless that was really meant.

**`N rule(s) held back as unsafe`.** Those rewrites can change behaviour. `-e`
prints why for each; `--unsafe` runs them anyway. Read the reasons before
passing it.

**`N occurrence(s) ... found by text search rather than parsed`.** ERB is parsed
like Ruby, so a rename reaches inside a view. Haml is not: those files are
text-searched and reported as their own, weaker class. Treat anything in that
block as a lead to check by hand, not a fact.

**`rwr: N rule(s) declare a Ruby version and none was detected; pass --ruby X.Y
or add a .ruby-version, -e for which`.** Common on `check all` in a repo without
a `.ruby-version` — a gem, a script directory, a fresh checkout. Rules whose
output needs a newer Ruby are held back rather than assumed safe, so the run is
quietly doing less than you asked. Pass `--ruby 3.3` (or whatever the project
targets) and rerun; `{foo:}` is a syntax error before 3.1 and nothing downstream
would catch it.

**`rwr: N rwr:ignore directive(s) name a rule this run does not have`.** The
comment suppressed nothing and the finding it meant to accept is still live —
different from a *stale* directive, where the rule ran and fired nowhere. The
message names the id you probably meant when it differs only by namespace, case
or separator.

## Deleting

```sh
rwr rewrite 'def legacy_total(*$A); $B; end' -d
```

Removes the definition *and* the doc comment above it, plus one of the blank
lines that separated it, so the survivors keep their spacing. `-r ''` means the
same thing. A match that does not occupy whole lines of its own is refused —
deleting `a.name` out of `x = a.name` would leave `x = ` and swallow the line
below.

**Write the parameter list as `(*$A)`.** A pattern matches arity exactly:
`def legacy_total($A)` matches only the one-parameter version and
`def legacy_total` only the zero-parameter one. `*$A` captures a run, so it
covers every arity.

Get it wrong and the failure is silent by design. The method comes back as one
`definition` residue row, the file keeps every byte, and the run exits **0** —
because `rewrite` never exits 1, a deletion that deleted nothing and one that
worked have the same status. **Check the file changed, or run `check` first and
look for `would rewrite N site(s)`; never conclude from the exit code alone.**

## Rules that only report

A rule with no `rewrite:` is a **finding**: it prints its matches with its
`description` and proposes no edit, and still exits 1 so a gate can act on it.
Use it for shapes where the right answer depends on something rwr cannot see.

`contains:` puts a whole sub-pattern inside a constraint, with shared
metavariables required to refer to the same thing:

```yaml
match: $R.each { |$X| $B }
where:
  $B: { contains: $X.$ASSOC.$FIELD }
```

Inside `{ ... }`, a comma, brace or bracket belongs to YAML — quote such a
pattern or use indented keys. rwr refuses loudly rather than running a rule that
silently matches nothing.

## Reporting into a pull request

```sh
rwr check all --since "origin/$GITHUB_BASE_REF" --sarif > rwr.sarif
```

SARIF 2.1.0, which `github/codeql-action/upload-sarif` turns into annotations.
A rewritable site or a lint finding is `warning`; residue is `note`, because it
is not a defect in the code but a thing rwr could not reach and a human must
judge. Blind spots with no line — a file that would not parse — arrive as
`toolExecutionNotifications` rather than results.

The workflow needs `fetch-depth: 0` (a shallow clone has no base branch to diff
against) and `continue-on-error: true` on the rwr step, since `check` exits 1
when there is work to do and would otherwise kill the upload.

## Exit codes

| Code | Means |
|---|---|
| 0 | matched (`find`) / nothing to change (`check`) / applied (`rewrite`) |
| 1 | no match (`find`) / there is work to do (`check`) |
| 2 | usage error |
| 3 | the pattern or rule is wrong — including an unknown field, a constraint on a capture the pattern never binds, or a template metavariable that was never captured |
| 4 | retryable — an edit sat inside a wider one; **run again** |
| 5 | refused — ambiguity, and zero edits were made. All three verbs |

`check` inverts polarity deliberately so a clean tree does not block a commit,
which is what makes it usable in a pre-commit hook or CI.

**`rewrite` never exits 1.** Having applied whatever there was to apply is
success, so a rewrite that matched nothing and one that changed a thousand files
both exit 0; falling short is 4 or 5. Never report a rewrite as done on the exit
code alone — read the counts, or the diff.

Distinguish **2** from **5**: 2 is a malformed command, so fix the arguments and
rerun. 5 is rwr understanding you and declining to guess, so rerunning the same
command changes nothing — read the message and ask a different question.

**Exit 4 means run the same command again.** Two redundant pairs in one hash, or
nested matches, need more than one pass; rwr reports the remainder rather than
looping internally, because a rule like `foo($A)` → `foo(bar($A))` would never
converge if it did.

## Notes

- `--profile` reports where the time went, as a phase table on stderr, with the
  file counts each phase handled — on a pattern search the scan row breaks out
  `parsed` / `skipped` / `unparsed` / `unreadable`, so it doubles as a check on
  how much of the tree the run actually read. A whole Rails app is ~1.5s.
  `RWR_PROFILE=1` in the environment does the same, for a command you cannot add
  a flag to.
- Generated and vendored code is skipped; `--include-vendored` overrides.
- Ruby means more than `.rb`: `.rake`, `.ru`, `.gemspec`, `.jbuilder`,
  `Rakefile`, `Gemfile` and friends are searched too.
- `-e/--explain` says which constraint declined a candidate. Point it at one
  site while writing a rule: `rwr check r.yml app/x.rb:42 -e` prints the capture,
  what it bound, and what the constraint wanted. Under `-j` it is a `rejections`
  array. A `type:` miss distinguishes "resolved to the wrong class" from
  "receiver did not resolve at all" — the second may not be fixable by changing
  the rule, since narrowing is conservative by design.
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

Shell completions, for a human at a terminal: `rwr --completions` (uses
`$SHELL`) or `rwr --completions zsh`.

Source and issues: <https://github.com/dpep/rwr>.
