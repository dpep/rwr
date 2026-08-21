# Changelog

## 0.4.0 — 2026-08-21

**The shipped pack runs 42% faster** — 970 ms to 565 ms over discourse's 11,006 files — from
deleting redundant work rather than anything clever. Every candidate file used to be reparsed
once per rule even when no rule had changed anything (~85 ms per added rule); every rule
walked every file the *set's* literals wanted, rather than checking its own; and each file
was copied once too often and resolved with a syscall needed only under `--diff` (D63).

**Residue reporting had two defects that a purpose-built testbed found immediately.** It was
computed only for files rwr had already *changed*, so a file that is nothing but dynamic
reaches — a serializer full of `delegate` and `validates` — was never looked at; and the
report was scoped to the target class, which discards exactly those reaches, since a
delegation lives in a different class from the method it names. Recall on the testbed went
from 2 of 7 to **7 of 7** (Q1).

**Residue now appears in `-j`/`-J` output.** It was text-only, so an agent — which the skill
tells to use `-j` — got the edits with no account of what they missed.

**Breaking: `-j` emits a document, not a list.** `check`/`rewrite` produce
`{schema, rwr_version, changed, residue, templates_skipped}` and `find` produces
`{schema, rwr_version, matches}`, where both previously produced a bare array. `schema` is
2; 1 was the array. `-J` is unchanged — a row per line, since a stream and a document are
different things.

**Less noise in the report.** A symbol that is a hash key is not a method reach (57% of a
15,587-entry report on discourse was keyword-argument keys), and neither is `attr_reader
:name` in an unrelated class, which defines that class's own method.

**Honest degradation on a common identifier.** A report too long to read says so and says
where to start, instead of advising you to narrow a rule whose whole point is completeness.

## 0.3.0 — 2026-08-21

**A rename across two classes now warns.** `Account#display_name` and `Company#display_name`
are different methods; a rule with no `type:` constraint renamed both at exit 0, and nothing
in the tool noticed — there is no conflict to detect, so the refusal contract never applied.
A warning rather than a refusal, because a repo-wide rename is legitimate and refusing it
would teach people to disable the check. Saying which class you meant silences it (Q10).

**`.rb` is not the whole language.** `.rake`, `.ru`, `.gemspec`, `.jbuilder`, `Rakefile`,
`Gemfile`, `Vagrantfile` and friends are searched now. Discourse keeps 11,854 lines of Ruby
in files rwr walked past — a rename silently skipped them, and the residue report claimed
completeness without having read them.

**Reports say what they did not read.** ERB and Haml embed Ruby that rwr does not parse, and
a Rails app keeps a large share of its call sites there. Any report making a completeness
claim now names the count of template files it skipped (Q11).

**Sorbet signatures resolve chained receivers.** Where a repository has `sig { returns(X) }`,
rwr reads it as a return type, so `parser.document.name` narrows by `type:` — the case D61
measured as unreachable from syntax alone. It needs no Sorbet, no RBI parser and no new file
format: a signature is ordinary Ruby, already in the tree rwr parses. `T.untyped`, `T.any`
and `void` yield nothing rather than a guess; `T.nilable(X)` yields X; `T::Array[X]` yields
Array. A repository with no signatures is unaffected and pays nothing measurable.

**Constructor chains resolve their receiver.** `Widget.new.display_name` now narrows by
`type: Widget`, and identity methods (`freeze`, `dup`, `clone`, `itself`, `tap`) pass a type
through, so `Widget.new.dup.display_name` resolves too. Anything else chained stays
unresolved and is reported as residue — see D61 for the measurements that drew that line.

**The hold-back notices are one line each.** The count of rules held back — unsafe, or
needing a newer Ruby — is still unconditional, since a rule that did not run must never look
like a rule that found nothing. The per-rule reasons moved behind `-e/--explain`: six lines
of stderr on every pre-commit run is how a report trains people to stop reading it.

## 0.2.0 — 2026-08-21

**`--diff` scopes a run to the lines a change touched**, so `check` can gate a pull request
on a codebase that has never run it — three new sites fail, two thousand pre-existing ones do
not. Bare `--diff` is the uncommitted work; `--diff main` is `main...HEAD`, the change this
branch introduces rather than every way it differs from main's tip. Works with `find`,
`check` and `rewrite`.

**Rules declare the Ruby version their output needs, and are held back when the codebase is
older.** `{foo:}` is a syntax error before 3.1 and `filter_map` does not exist before 2.7 —
and `verify` cannot catch either, because Prism parses modern Ruby and the output is valid
*there*. The version is read from `.ruby-version`, a Gemfile `ruby` line, or a gemspec's
`required_ruby_version`; `--ruby X.Y` overrides. An undetected version holds the rules back
rather than assuming the newest (Q6, now closed).

**A Claude skill**, at `claude/rwr-skill.md`, teaching an agent to drive rwr — the three
verbs, metavariable syntax, the `where:` predicates, the built-in pack, and what each exit
code means. `claude/INSTALL.md` covers installing it. It ships through the private
`rwr@myclaude` plugin until the tool has real mileage.

**`rwr-phase0` refuses instead of reporting a clean nothing.** An unrecognised option was
taken as a path, and any path that was not a directory — a quoted `~` the shell never
expanded, a typo, a file — was filtered away in silence. All three produced a valid-looking
report with `"repos": []` and no diagnostic. Each now names what was wrong and exits 2.

The report itself accounts for what it walked: `files` counts files walked (it counted files
*read*, so an unreadable file shrank the denominator invisibly), alongside `files_measured`,
`files_unreadable`, and `hot_names_omitted`/`hot_names_min_sites` for the two caps `hot_names`
applies. `schema` is now 2. A repo given as `.` reports its own directory name rather than the
`corpus` fallback.

**A built-in rule pack — ten rules, compiled into the binary.**

```sh
rwr check all app/                 # every safe rule
rwr check performance app/         # one family
rwr check style/return-nil app/    # one rule
```

It works from any directory, since `cargo install` copies the binary and nothing else. A
directory of your own rules is selected the same way, and a real path wins over a built-in
name. A run reports which rule accounted for what: a single total across ten rules is not a
reviewable answer.

`style` covers `return-nil`, `hash-shorthand`, `redundant-self-assign` and
`sorted-constant-array`; `performance` covers `detect`, `count`, `filter-map`, `sum`,
`reverse-each` and `string-replacement` (`gsub` → `tr`).

**Rules that can change behaviour say so, and are held back by default.** A rule may carry
`unsafe: <reason>` — `inject(:+)` returns nil for an empty collection where `sum` returns 0;
`select` on an ActiveRecord relation names columns rather than filtering rows. Those need
`--unsafe`, the run reports how many it skipped and why, and the reason prints next to the
diff when the rule fires. There are no per-rule options: a rule is four lines of YAML, so
the rule *is* the option (D57).

**Two new `where:` predicates.** `is:` constrains a capture's node kind and `length:` its
literal content in characters — together they are what makes `gsub` → `tr` safe rather than
plausible, since `tr` maps character by character. `is: constant` also picks the placeholder
casing, which is what lets a pattern reach `FOO = [...]` at all: before, `$C = [...]`
silently meant a *local* assignment, since both casings parse.

**Minimal diffs now survive sequence placeholders.** Any rule spelled with `*$REST` or
`**$REST` fell through to whole-node replacement, so hash shorthand returned multiline
hashes on a single line with their trailing commas removed. It now edits the one pair that
changed and leaves the layout alone (D56).

**Residue is reported for name-anchored rules only, as D7 always said.** A rule about a
*shape* — `select { }.first` -> `detect { }` — anchored on the chain's method names and
reported every `.first` in the repo. On Discourse that was 3,752 occurrences, which buried
the output. A rename still reports; the account of blind spots was never meant to be a
concordance of common method names.

**The `edits` field in JSON output is now `sites`, and counts differently.** It
had reported edits, and a rewrite that changes shape emits several edits for one
place a reader sees in the diff — `select { }.first` → `detect { }` counted as
two. It now counts matched sites.

## 0.1.0 - 2026-08-21

First working release. `find`, `check` and `rewrite` all do real work.

**Structural matching.** Patterns are Ruby source with `$METAVARS`; comments,
strings and heredoc bodies are not code. On rails, `rwr 'return nil'` finds 22
sites where ripgrep reports 40.

**Receiver narrowing.** `method: Account#display_name` renames the definition, a
subclass override, explicit-receiver calls and implicit-self calls -- and leaves
`Company#display_name` and `Account.display_name` alone, because those are
different methods.

**Residue reporting.** Every occurrence a rule could not account for is
reported and classified, so a rename that would silently break
`attr_accessor :display_name` says so.

**Minimal diffs.** Only what changed moves; layout, block spelling and heredocs
survive.

**Refusal.** Ambiguity produces a diagnostic and zero edits. Exit codes
distinguish retryable from terminal, and `check` inverts polarity so a clean
tree does not block a commit.

`--profile` reports where the time went. `rwr-phase0` emits shareable JSON
aggregates for codebases that cannot leave their machine.

## 0.0.1 - 2026-08-20

Namespace placeholder. No functionality.
