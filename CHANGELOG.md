# Changelog

## 0.6.5 — 2026-08-24

**If a rewrite that used to work now refuses**, that is these two new checks, and it is worth
reporting rather than working around — they are built to skip whatever they cannot judge, so a
refusal means one of them is wrong. Both name what they saw and what they expected, so the message
is the bug report. There is no flag to disable them: a check you can turn off stops being a
guarantee, and the honest fix for a false refusal is to narrow the check.

**Bindings are verified too, not just shape.** A splice can produce the right shape around the
wrong code — `foo($A, $B)` emitted with its arguments swapped matches its own template perfectly —
and shape-checking alone cannot see it. A capture is spliced verbatim, so a metavariable the
template carries over from the pattern must hold byte-identical text afterwards. It doesn't, the
run refuses:

```
rwr: refused app.rb: the rewrite moved $A: it captured `a` before and `b` after,
so the result has the right shape around the wrong code
```

Same conservatism as the shape check: a capture the template drops, or one that cannot be rendered
contiguously (a heredoc), is not compared rather than compared wrongly.

And a capture another site rewrote is not compared either, because that rule is working rather than
failing. `$R.freeze` over `x.freeze.freeze` matches twice — the outer match captures `x.freeze` and
the inner one rewrites exactly that — so the text legitimately differs afterwards. A nested site
inside the capture's span is what tells the two cases apart; a site's own edits never land inside
its own captures, since a capture is carried over verbatim, so a corrupted splice is still caught.

**A rewrite is checked against the template it came from, not just reparsed.** `verify` catches a
splice that produces invalid Ruby and cannot catch one that produces *valid* Ruby meaning something
else — its own comment said so, and `!$X.empty?` → `$X.any?` writing `any?xs` proved it. Every site
is now re-matched against its template after the edits are applied, and a mismatch refuses the
whole transformation:

```
rwr: refused app.rb: the rewrite produced `any?xs`, which is not what `$X.any?`
describes -- valid Ruby, and not the transformation the rule asked for
```

Conservative by construction: anything it cannot check it skips, and a skip is never a failure —
refusing a correct rewrite breaks a working run, where missing an incorrect one only leaves things
as they were. It declines to judge a deletion, a template carrying a sequence transform, one that
is not a single expression, and any site whose node it cannot locate again. Shape only, not
bindings.

Both checks run **in memory, before anything is written**, as they always have — there is no
partially-written file to undo.

**`$X.foo` no longer matches `foo(bar)`.** A call's optional children are dropped from the child
list when absent, so `receiver` and `arguments` both arrived as *the lone child* when the other was
missing — and the positional comparison lined them up. `$X.foo` matched a receiverless `foo(bar)`,
binding `$X` to the argument list: `CALL` with a receiver and `FCALL` with an argument reported as
the same site. The same collision let `$X.foo` match `foo { 1 }`.

Receiver presence is now compared before the children are. Only the receiver: a splat must still
absorb nothing, so `foo(*$REST)` matches `foo()`, and `foo` still equals `foo()`.

Renames were protected by accident — a `type:` constraint cannot resolve an argument list, so it
declined — which is why this survived. `rwr find` was not protected, and find is observation, where
over-reporting is as much a lie as a miss.

## 0.6.4 — 2026-08-23

**`extend self` and `module_function` no longer split a rename in half.** A module that extends
itself puts one method on both tables — `Util.foo` and `Util#foo` are the same method — and rwr
treated `kind:` as decisive, so a rename rewrote the definition and filed every call as residue, or
the reverse. The report looked complete either way, because the half it missed was reported rather
than dropped. `extend Other` is untouched: an ordinary mixin does not collapse the extending
module's own tables.

Underneath was a pre-filter bug that hid the feature working at all. The hierarchy reads a file
only if it contains `class` and `<`, or a mixin keyword — and a module using `module_function` has
none of those, so the file was dropped before parsing and the module never recorded. The collector
was correct and never ran. The pre-filter and the collector now read from one list. `extend self`
worked throughout because `extend` is itself a mixin keyword.

**`def Account.foo` is rewritten by a rename, not just reported.** It was never a matcher
limitation — the class-method expansion emitted `def self.foo` and a `def foo` inside
`class << self`, and simply not the third spelling. It needs no scope: the receiver written in the
source pins the class, which matters because such a definition usually sits inside a *different*
class, and that is exactly why it was missed.

**Bare `attr` counts as defining a method.** It was missing from the macros whose symbols name
methods on the enclosing class, so an unrelated class's own `attr :display_name` read as a reach
where the `attr_reader` beside it did not.

## 0.6.3 — 2026-08-23

**A definition's owner is its receiver, not its lexical nesting.** Ruby decides which class a
definition attaches to from the receiver; nesting only supplies a namespace. rwr read the owner off
the nesting, which was wrong three ways and silent every time:

| written | owner | rwr said |
|---|---|---|
| `class ::Bar` inside `module Foo` | `Bar` — `::` resets to top level | `Foo::Bar` |
| `class << Foo` inside `class Bar` | `Foo`'s singleton | `Bar` |
| `def Foo.bar` inside `class Bar` | `Foo`'s singleton | `Bar` |

The middle row was the worst: a rename of `Bar.bar` rewrote a method living on `Foo` at exit 0, and
a rename of `Foo.bar` reported **no residue** for the definition it had missed. An owner rwr cannot
name — `class << obj` — now gets a name no constant can spell, so a rule naming a real class matches
nothing inside rather than borrowing the enclosing class's. `class << self` is unchanged: `self` *is*
the enclosing class.

**Residue files a definition under its owner.** Every occurrence was recorded against the scope it
was written in — right for a reference, wrong for a definition. `def Foo.bar` was filed under the
class containing it, so the rename that cared could not see it.

`def Constant.name` is reported rather than rewritten: the definition pattern cannot match an
explicit constant receiver, and reporting what it cannot convert is the contract working.


## 0.6.2 — 2026-08-23

**A `type:` that cannot be a Ruby constant is refused at load.** `is:` has a closed set and rejects
an unknown value outright; `type:` takes a class name and would accept any string, so `type: string`
resolved to nothing and matched nothing without a word — silent, and in the direction that looks
like a clean run. A value not starting with a capital now refuses, and when the value is one of
`is:`'s own words it says so: *did you mean `is: constant`?* Every class in a `type_not:` list is
checked, not just the first.

The two predicates ask different questions and both are needed. `is:` is syntactic — which kind of
node this is. `type:` is semantic — which class the receiver resolves to. `type: constant` is
meaningless because a constant read is not a class.

**`style/sorted-constant-array` is no longer in the pack.** Sorting a constant whose order is
*meaning* — `PRIORITY_ORDER`, a migration sequence, anything positional — is a bug, and nothing in
the source tells the two apart. It bought tidiness and risked behaviour, which by the pack's own
standard belongs in your own rules directory rather than in a set run unattended. Nothing in the
shipped pack is held back as `unsafe:` for a style reason any more.

**Sequence transforms are documented for the first time.** `*$ITEMS` captures a run of elements and
a suffix reorders it — `.sort`, `.uniq`, `.reverse`, and the set is closed. The removed rule was
their only user in the pack, so the capability would otherwise have gone dark; it is now in
[rules/README.md](rules/README.md), in the skill, and pinned by end-to-end tests rather than unit
tests alone. An unrecognised suffix is still refused rather than written into your source.

**Refusals read as sentences.** They were printed with `{:?}`, so a typo'd transform arrived as
`UnknownTransform { name: "srot" }`. A refusal is the product of refusing rather than guessing, and
it costs the caller a round trip, so it now says what happened and what was expected — for every
refusal, not just that one.

**`cargo publish` no longer sweeps up files from outside the package.** `include` globs are
gitignore-style, so a bare `README.md` matches one at *any* depth — which pulled a gitignored
scratch file into the 0.6.1 tarball and blocked the publish. Anchored with a leading `/`.

## 0.6.1 — 2026-08-23

**`-e` now says when nothing could have resolved.** "Receiver did not resolve" meant two different
things — this receiver is hard, or there was nothing in scope to resolve it against — and they
have opposite fixes: write a signature, or stop editing the rule. The second now says so. The
check is whether the index read *anything*, not whether it read a return type; a repository whose
signatures are all `params(..).void` has no return types and is emphatically not empty.

**Computed dispatch that provably cannot reach the name is no longer reported.** `send("get_#{x}")`
produces names beginning `get_`, so no value of `x` yields `display_name` — that is a proof, and
the entry goes. The opposite inference, that `send("display_#{x}")` *does* reach it, stays a guess
and is never made: everything that cannot be disproved is kept, including a bare `send(x)` with no
static text at all. Only the outer literal run each side is used, since matching the middle is a
longer argument for a smaller gain.

**Sorbet signatures now yield parameter types, not just return types.** A return type answers
"what does this chain evaluate to"; a parameter type answers "what is this bare local", which is
what most code actually asks — `return if x.nil?` guards an argument far more often than a chain.
Reading them also meant not gating on the return: `sig { params(x: String).void }` states nothing
usable about the result and everything about the argument, and it is the commonest shape on a
command or a setter, so gating dropped every one. A type that names no single class is still
absent rather than guessed, so the local stays unresolved and a constraint declines.

**`type_not:` — exclude a receiver by class, safely.** Takes a list and means *resolves, and to
none of these*. Not the mirror of `name_not:`, deliberately: `name_not:` passes when there is no
identifier, but a type exclusion that passed on an unresolved receiver would turn a narrowing
predicate into a widening one, so an unresolved receiver **fails** it. Descent is always honoured
— "not an `ActiveRecord::Base`" means not an `Account` either — with no flag to set.

**`style/return-unless-nil` ships, narrowed by it.** `return if x.nil?` → `return unless x` is
wrong for a nilable boolean, and that is now expressible: `type_not: [TrueClass, FalseClass,
Boolean]`. `Boolean` is listed because `T::Boolean` is a constant path and resolves by its last
segment. The rule fires only where the type resolves, so a codebase with no signatures gets no
rewrites — pinned as a fixture, and `-e` says so per site rather than leaving you to wonder.

**A prefix operator is no longer treated as a method rename.** `!x` and `x.any?` both keep their
name in Prism's `message_loc`, and the structural diff took any two differing names for a rename —
so `!$X.empty?` → `$X.any?` wrote `any?` over the `!` and left the receiver's call standing:
`any?xs`. Ruby parses that as `any?(xs)`, so `verify` passed it. The same mismatch the other way
round (`$X.foo` → `foo($X)`) silently produced no edit at all while the run reported a rewritten
site. Names now correspond only slot for slot; anything else unwraps or re-renders the span.

**Four new rules, and a third family.** All four ship plain — none is held back, because a rule
that needs a flag before it is safe to run unattended is one the pack should not carry.

`style/inverse-any` — `!x.any? { }` → `x.none? { }`, and the `&:sym` block-pass spelling. A true
inverse: both ask about the predicate, so no element value splits them.

`rspec/redundant-stub-return` — drops `.and_return(nil)` from a stub, which returns nil already.
Matches through the chain, so `receive(:x).with(1).and_return(nil)` is covered; `and_return(nil, 1)`
is a different node and is not.

`rspec/be-empty` — `expect(x).to eq([])` / `eq({})` / `eq('')` → `be_empty`. What it drops is
assertion strength, not runtime behaviour: `eq([])` pins the type as well as the emptiness, so a
subject that regressed from `[]` to `{}` fails the original and passes the rewrite. That caveat
lives in each rule's `description:` — which reaches `-j`, SARIF and the pull-request comment —
rather than in `unsafe:`, which is reserved for rewrites that change what the program *does*.

`rspec` is the first family whose name is also a scope. A rule constrains the tree, never the
path — point it at the specs: `rwr check rspec spec/`.

**`rules/README.md` now states what the pack is designed to guarantee**: `rwr rewrite all app/`
is meant to be run unattended, on code you have not read. Safety comes from a rule not being in
the default set, never from a warning attached to one — a warning asks for vigilance, and
vigilance does not scale to ten thousand call sites. A rewrite that buys a measured win can carry
a caveat; a rewrite that buys only a reading preference cannot, because there is no amount of
tidier that pays for a `NoMethodError`.

**Inline review comments on a pull request, with an Apply button.**
`script/pr-suggest.sh` turns `rwr check -j` into review comments — an applicable ` ```suggestion `
block where a rule can fix what it found, a plain comment where it cannot (a finding, or an
occurrence a rename could not convert). Posted through the ordinary reviews API, so no Code
Scanning, no security alerts, and no `github-advanced-security` attribution, which is not
renameable and whose comments cannot be deleted.

Inline only and scoped to the diff: a review is about what *this* change introduced, and the full
account of a rename lives in the terminal and in `-j`. No summary comment — a preamble restating
what is visible inline is what makes a bot easy to mute.

The report carries what a suggestion needs: `end_line`, the rule's own `description`, and the
`replacement` text for the lines a site occupies.

**`script/pr-suggest.sh` runs against any pull request, from anywhere.** A URL or `owner/repo#N`
makes it fetch its own blobless clone, so the repo need not be checked out. It still needs the
source — rwr matches structurally, so it wants a parse tree and a diff hunk is not parseable
Ruby — but it no longer needs *your* checkout.

**`--sarif` is still there, for Code Scanning and other SARIF consumers**, but it is no longer the
recommended path for pull-request review. See [docs/github-actions.md](docs/github-actions.md)
for why.

**Findings are framed as simplifications, not violations.** A rewritable site now reads
"🎯 An explicit `return nil` says nothing `return` does not." rather than "a rule would rewrite
this". Nothing rwr flags here is broken, and calling it a violation earns a reviewer's scepticism
rather than their attention.

**`--sarif` emits SARIF 2.1.0, which is the whole GitHub integration.**
`github/codeql-action/upload-sarif` turns it into annotations on a pull request — a serializer
rather than an app: no hosting, no OAuth, no webhook. See
[docs/github-actions.md](docs/github-actions.md) for the workflow, including the three settings
that decide whether it works (`fetch-depth: 0`, `--since "$GITHUB_BASE_REF"`, and
`continue-on-error` so `check`'s exit 1 does not kill the upload step).

Levels are a judgement, not a formality: a rewritable site or a lint finding is `warning`,
residue is `note`. Residue is not a defect in your code — it is rwr saying what it could not
reach — and grading it the same as "this can be auto-fixed" would train people to skim past
both. Blind spots with no line to point at arrive as `toolExecutionNotifications` rather than
results, since inventing a location would be inventing evidence.

**`check -j` now says where each rewritable site is.** `changed` carried a per-file count and no
line, which answers "how much" and not "where" — enough for a human reading a terminal, not
enough for an annotation. Each entry now carries the line and column of every site.

## 0.6.0 — 2026-08-22

**A fixture can assert what a rule *reports*, not only what it rewrites.** `residue: N` pins how
many occurrences a rule should be unable to account for — the half of a rename that decides
whether the change is safe to ship, and the half a fixture could previously say nothing about.
Refused on a rule that moves no name, since it would pass at zero forever. A case may assert
several things at once, and all of them are now checked: as an `else if` chain, a case carrying
both `output:` and `residue:` checked only the first, so it looked like it asserted two things
and asserted one.

**`residue` is absent when the question does not apply.** Residue exists only where a rule moves
a name (D7), so an empty list meant two opposite things — "I looked and found nothing left" and
"this rule has no leftovers by construction" — and nothing distinguished them: a count meaning
*not run* reading exactly like a count meaning *clean*. It now has three states, present-with-
entries, present-and-empty, and absent. Reading absent as empty gives "nothing to review", which
is correct; only a consumer asking whether a *rename* is complete needs the difference.

**A rename rewrites `send` when the name is literal, and notices it when it is not.** Two halves
of one shape. `account.send(:display_name)` is as provable as `account.display_name` once the
receiver resolves — the same narrowing decides both — so reporting it was declining work rwr had
already shown it could do safely. `send`, `public_send`, `__send__`, `try` and `try!` are covered,
with symbol or string. A receiver that does not resolve is still reported, never rewritten.

`send("display_#{attr}")` is genuinely invisible, and a report that says nothing about it looks
complete while a class dispatches on names nobody can enumerate. Those sites are now reported as
a new `dynamic` residue context, scoped to the class the rule is about — unscoped they were 12%
of a real report on discourse, scoped they are three entries. Schema is now **5**.

**A rename reaches an override whose parameter list has drifted.** `def display_name(format =
:long)` overriding a zero-arity parent is the ordinary shape of legacy inheritance, and no
pattern could express it — a definition pattern with no parameter list matched only a definition
that had none, and `def foo(*$P)` matched nothing at all, because in a parameter position `*$P`
is a real Ruby rest parameter rather than a sequence placeholder. It now means "with any
parameters", including none, and the rename uses it.

**A constant holding a list of symbols is a reach.** `COLUMNS = %i[display_name email].freeze`,
read back through `public_send`, is the most ordinary dynamic dispatch a legacy exporter has —
and it was dropped, because the symbols are an argument to nothing: the array is `freeze`'s
receiver. Measured on discourse: 8 more residue entries out of 832, and it closes the last
recall gap in the testbed.

**`inside:` and `method:` name one class, by its qualified name.** Lexical nesting is
*namespacing*, not membership — `class Account; class Row` declares `Account::Row`, a different
class that does not inherit from `Account`. Matching any enclosing name meant a rule scoped to
`Account` rewrote code inside `Account::Row` and inside `Billing::Account`; the compact spelling
`class Account::Exporter` had the opposite fault and was seen as plain `Exporter`. Both are
fixed by the same change, and `inside: Billing::Account` now reaches the class it names. A
`class << self` body stays transparent: it opens a context, not a class.

**A rename reaches a method whose body carries `rescue` or `ensure`.** Such a `def` has a
`BeginNode` body rather than a `StatementsNode`, so it never met the body rule and the rename
declined every method that touches I/O — reported as residue, so honest, but declined. Both
halves needed fixing: the matcher binds a body-position metavariable whatever shape the body
has, and the structural diff now recognises the same thing. Without the second, the diff called
the body diverged and re-rendered the whole `def`, and that wider edit swallowed the correct
one — leaving the file unchanged while the run claimed a rewrite and asked to be run again,
forever.

**A rewrite that would collide with an existing local refuses.** Renaming `display_name` to
`full_name` where `full_name` was already a local produced `full_name = full_name if profile?`
— a self-assignment that parses, runs, quietly evaluates to the local's old value, and passes
`verify`'s reparse. It was the only defect here that produced *working* code with changed
behaviour. Scoped per Ruby scope rather than per file: a local in one method does not block a
rewrite in another.

**A rename refuses a file where a refinement of the target is active.** Renaming
`Account#display_name` in a file that says `using AccountRefinements` rewrote the call site;
afterwards `Account#full_name` existed, the refinement still defined `display_name`, and the
call quietly stopped going through the refinement — no error, no failing parse, the refined
behaviour just stopped happening. The refusal is scoped to `using`, not to the refinement's
existence: one nobody activates is inert, so a call really does reach the class and renaming it
is correct.

**Residue reports overrides written with `include`, `prepend`, `extend` and `refine`.** A method
redefined in a module — a concern, a patch, a refinement — was dropped from the account
entirely, because the hierarchy recorded `class X < Y` links and nothing else, and the report
compared scope names literally. In Rails a large share of a model's methods live in concerns, so
this was a whole category rwr was silent about being silent on. Testbed recall went from 37 of
45 to 44.

**A Ruby file that does not parse is now named.** Templates had `templates_skipped`; Ruby that
failed to parse had nothing at all, so a generator template with a `.rb` extension — or any
broken file — vanished with the run still exiting 0. The same blind spot, surfaced in one case
and hidden in the other. Only files that could have contributed are listed: one with no mention
of the name is declined by the prefilter before parsing is attempted. `-j` gains `unparsed` and
the schema is now **4**.

**`class_attribute`, `store_accessor`, `alias_attribute` and `enum` are recognised as defining
methods**, so a symbol handed to one in an unrelated class is no longer reported as a reach into
this one.

**`class << self` no longer confuses instance and class methods.** The definition rule left
singleton context unconstrained and the walk cleared the flag on entry to every method inside a
singleton class, so an instance rename **rewrote a class method's call sites** — injecting a
`NoMethodError` into code the rule had correctly declined to rename — while a class rename
missed the definition entirely. Both directions now behave, and the near-miss is reported rather
than passed over in silence.

**An override in a subclass is reported when it cannot be rewritten.** `subclasses: true` was
honoured by the matcher and ignored by the residue report, so an override whose parameter list
had drifted from its parent's was neither rewritten nor mentioned — exit 0, with the work half
done. It is now named.

**A rename now reaches methods that have a body.** Prism carries a scope's local-variable table
on the node, and rwr compared it as though it were syntax — so `def foo; $B; end` matched only
methods whose locals were identical to the pattern's, which in practice meant methods with no
locals at all. The one-line `method:`/`rename:` form renamed one-liners and silently declined
every method that assigned a variable, reporting its own definition as residue. Nobody writes a
local table, so it is no longer part of equality (D36).

The same fault reached block bodies, where it was quietly costing matches: `xs.reverse.each do
|post| … end` was skipped by `performance/reverse-each` whenever the block assigned anything.
Measured across ~/code/lib/ruby: **+3 sites of 1051, none lost** — strictly more matching, which
is what a correctness fix should look like.

Also: a lone metavariable in a body position now binds the whole body, since a Ruby body holds a
statements sequence and that sequence is one node.

**An ERB edit that cannot be made is refused, not dropped.** The template pass skipped past both
a `plan` refusal and a cross-tag `splice` refusal — no count, no report, no exit code — while
the identical refusal on a `.rb` file was reported and exited 5. "Never silently drop an edit"
is the second first principle. A refused template now keeps its bytes, rather than getting part
of a rule set that could not finish.

**`templates_skipped` means the same thing in text and JSON.** The JSON counted *every* template
as skipped, including ones rwr had parsed structurally — over-claiming a blind spot in the plane
an agent acts on, while the human report had it right.

**`# rwr:ignore <rule-id>` accepts a finding at the site.** Trailing on a line, or leading above
one. It covers the **outermost statement starting on the attached line**, so a directive above a
`def` covers the whole method rather than its signature — rwr has the parse tree, and a
line-scoped directive would leave that on the table. No `disable`/`enable` block form: a
forgotten terminator silently suppresses the rest of a file, which is the blind spot this tool
exists to refuse.

A reason may follow `--`: `# rwr:ignore style/no-sleep -- flaky in CI`. Rule ids are required. A
bare `# rwr:ignore` is reported as malformed and suppresses nothing,
because a blanket ignore is one no staleness check can audit. A directive naming a rule outside
the current run is left alone.

**Suppressions report themselves, unconditionally.** How many findings were accepted, and which
directives have nothing left to accept, print on every run and appear in `-j` — a mechanism that
can silence a run must never be able to silence itself. Stale directives are reported but do not
set the exit code: their finding is already gone, so what remains is tidying.

`rewrite` honours directives identically to `check`, since `check` is its preview.

**`name_not:` excludes identifiers.** `where:` could say `name: [a, b]` but had no negation, so
a rule that over-matched on common tokens had nowhere to say so. Asymmetric with `name:` on
purpose: `name:` fails a capture with no identifier, `name_not:` passes one -- nothing that is
not an identifier can be one of the excluded ones, and widening on missing data would be a
guess. `name:` and `name_not:` on the same capture is refused, since an allowlist already says
which names count.

**`-e` says which constraint declined a site.** The flag's own help had promised this since it
shipped and it produced silence: a site rejected by `type: Widget` printed nothing at all. The
matcher had the answer and discarded it once rebinding was exhausted. Scoped to a line, this is
the rule-authoring loop:

```
$ rwr check rule.yml app.rb:5 -e
app.rb:5:1: t/widget: matched, then declined
  $R bound `g` -- resolved to Gadget, not Widget
```

Three cases that were one silence are now three answers: a receiver that **resolved to the
wrong class**, one that **did not resolve at all** (receiver narrowing is conservative, and this
may not be fixable by editing the rule), and one that resolved correctly but as the other of
instance/class than the rule means.

A rule *bug* — a constraint naming a capture the pattern never binds, a `contains:` that failed
to prepare — no longer reports as a scope miss. It said `WrongScope`, which sent an author
looking at their `scope:` for a typo'd `where:` key.

Report schema is **3**: `check -j -e` carries a `rejections` array with `capture`, `constraint`,
`detail` and `bound`. Absent without `-e`, since nobody asking is not the same as nothing being
declined. Rejections stay behind the flag deliberately — they describe sites a rule *correctly*
refused, which is debugging detail, not a blind spot; the account of what rwr could not see
stays unconditional.

**`rwr test` runs a rule's own fixtures.** A rule file may carry `tests:` -- an input snippet
plus `output:`, `unchanged: true`, or `finds: N` -- and `rwr test rule.yml` (or a directory, or
a built-in name) checks them. It exits 1 on a failure with a diff, so a custom rule can be
pinned in CI and upgrading rwr cannot quietly change what it does to real code.

A case that asserts nothing is refused rather than passing: `input:` alone, `output:` with
`unchanged:`, and `finds:` on a rule that rewrites are all exit 3. A snippet that does not
parse **fails** rather than being skipped the way `check` skips an unparseable file -- the
commonest fixture bug is a typo'd snippet, and skipping it would pass every negative assertion
vacuously. A rule set declaring no fixtures at all exits 2 rather than reporting a green
nothing.

**A `contains:` rule now reaches templates from any position in a set.** The ERB pass built
every rule's criteria from the *first* rule's sub-pattern map, so a set whose second rule used
`contains:` silently matched nothing in `.erb` files -- while the identical rule matched the
identical code in a `.rb` file, and matched fine as a set of one. Exit 0, no diagnostic.

**`--diff` no longer eats a path.** It took an optional revision, so `rwr check all --diff app/`
built the range `app/...` and failed inside git. `--diff` now takes **no value** and the new
`--since REV` requires one, so no token's role depends on what it looks like. `--diff main`
becomes an error naming `--since main`. Together — `--since main --diff` — they mean the merge
base against the working tree, which is what this branch introduces *including* what you have
not committed; neither flag said that before.

**`--diff` sees brand-new files.** `git diff` cannot see a file git is not tracking, so an
untracked file full of violations reported as a clean tree — the pre-commit case failing exactly
when the change is largest. Untracked files are now in scope.

**A path that does not exist is an error.** `rwr check all app/typo` exited 0 and reported a
clean tree; in CI that is a green gate that checked nothing. It is now exit 2.

**`PATH:N` and `PATH:N-M` scope a run to those lines**, the `file:line` rwr already prints, so
an output line pastes back in as an input.

**Shell completions**: `rwr --completions` prints a script for the shell you are in, or
`rwr --completions zsh` names one. `clap_complete` had been a dependency since the first
commit without ever being called.

## 0.5.0 — 2026-08-21

**A `contains:` pattern YAML truncated now refuses loudly.** Inside a flow mapping,
`{ contains: log($A, $B) }` arrives as `log($A` — the comma belongs to YAML. That pattern
failed to prepare and the failure was being swallowed, leaving a rule that ran clean and
matched nothing. It exits 3 and names the cause. A constraint that cannot be built must not
degrade into one that is never satisfied.

**Stale comments are reported.** A rename left `# Returns the display_name` behind and rwr
said nothing — comments are not in Prism's tree, so neither the matcher nor the residue pass
saw them. They now have a pass of their own, scoped by position so an unrelated class's
prose stays out of the report.

They are still never *matched* (that is the whole thesis: `rwr 'return nil'` finds 22 sites
on rails where ripgrep reports 40) and never *rewritten*, because `# See also #display_name
on Company` is about a different class and nothing in the prose says so (D67).

**Deletion.** `rwr rewrite 'def legacy; $B; end' -d` removes a definition — along with the
doc comment written above it, its line, and one of the blank lines that separated it, so the
survivors keep their spacing. `-r ''` and `rewrite: ''` mean the same thing.

A deletion whose match does not occupy whole lines of its own is **refused**: removing
`a.name` from `x = a.name` would leave `x = `, which swallows the line below and still
parses (D66).

**ERB templates are parsed, matched and rewritten.** Their tags are stitched into one Ruby
program — 95% of real templates parse that way — so a rename reaches inside a view and leaves
every byte of HTML where it was. An edit spanning two tags is refused, since those bytes are
not Ruby. A template that does not stitch falls back to the text search, which says it is
weaker. On discourse 114 of 124 templates parse; a rename of `User#name` finds 53 occurrences
by parsing against 49 by text (D65).

**`contains:` — a constraint that relates a sub-pattern to the outer match.** A pattern
matches a shape; this says "and somewhere inside it, this", with shared metavariables
required to refer to the same thing:

```yaml
match: $R.each { |$X| $B }
where:
  $B: { contains: $X.$ASSOC.$FIELD }
```

It ships `performance/possible-n-plus-one`, a lint that narrows discourse's 637 `each`/`map`
blocks to 51 candidates. It flags the N+1 *shape* and cannot see whether the relation was
eager-loaded, which its description says (D64).

**Templates are searched, not just counted.** rwr cannot parse `.erb`/`.haml`, so a rename
used to say "356 template file(s) were not searched" and stop. It now searches them for the
name at whole-identifier boundaries and reports what it finds as its own class — grep-grade
evidence, labelled as weaker than anything parsed. On mastodon a rename of `User#name` finds
145 parsed occurrences and 194 more in templates, so over half the account was invisible.

**Rules can lint without rewriting.** A rule with no `rewrite:` is a *finding*: it reports
its matches with its `description` and proposes no edit. Findings make `check` exit 1 like
edits do, since a lint that exits 0 gates nothing. Ships as `performance/relation-size`,
because `.size` on a relation is `count` unloaded and `length` loaded and only the caller
knows which was meant.

**Fixed: residue reported against rules that move no name.** `gsub` → `tr` is shaped exactly
like a rename — a literal name applied to metavariables — but `String#gsub` still exists
afterwards, so every `.gsub` the rule declined to rewrite is fine. They were being listed as
"could not account for", which is a false claim. Residue now applies only where the rule set
rewrites a *definition*, because that is the only way a name moves.

**Fixed: residue now names the rule it belongs to**, and each rule scopes by its own class.
An unlabelled block after several rules fired left the reader guessing, and a pack of two
renames reported everything against the first one's class and dropped the second's entirely.

**The template gap is a warning, not a footnote.** rwr cannot read `.erb`/`.haml`, so a
rename *under-reports* there — a call site in a view is missing from the account rather than
listed. That is the dangerous direction, and the message now says so.

**Five ActiveRecord performance rules**: `exists` (`where(...).count > 0` → `exists?`),
`find-by`, `pluck`, and `relation-count` (`to_a.size` → `count`). All held back as unsafe
with their caveats, since the pattern carries a tell rather than a proof.

**`T::Struct` field declarations feed receiver narrowing.** `const :name, String` states a
type with no `sig` anywhere, and rwr was not reading it — 45,068 sites on a Sorbet monolith,
a third again as many as its `sig` blocks. The signature prefilter now looks for `T::` as
well, since a struct of pure field declarations contains no `sig` at all; it costs about
10 ms on a codebase with no Sorbet. graph_weaver went from 79 signatures to 122.

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
