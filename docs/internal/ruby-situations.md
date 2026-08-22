# What Ruby actually contains

A map of the situations a structural search-and-rewrite tool meets in real Ruby
and real Rails, ranked by how likely each is to break it.

The organising bias is deliberate: **the boring and ubiquitous outranks the
gnarly and rare.** The flagship rename declined every method with a local
assignment in its body for months (D73) and nobody noticed, because the corpus's
one method body happened to be a single expression. Nothing in that failure was
exotic. Most of what follows isn't either.

Each entry names which of four difficulties it is, because they have different
correct answers:

- **match** — the shape is unusual, or one meaning has several shapes
- **splice** — matching is fine; editing the bytes safely is not
- **account** — rwr cannot rewrite it and must instead *report* it
- **invisible** — the name is computed and never appears; rwr cannot even report
  the occurrence, only the fact that it is blind here

Correct behaviour is one of **rewrite** / **report** / **refuse**. Refusing beats
guessing; a loud miss beats a silent one; a wrong rewrite is the only
unrecoverable outcome.

Running example throughout: renaming `Account#display_name → full_name`.

> **Status column.** Entries marked ✅ / ❌ were checked against the binary after
> this catalogue was written. Unmarked entries are unverified — the catalogue was
> written from Ruby semantics, deliberately not from rwr's behaviour, and that
> order matters: a corpus derived from what the tool does scores full marks on
> day one and finds nothing.

## A. Method definition shapes

The dullest category and the one that has already produced a months-long silent
bug.

**A1. A body with more than one statement.** *match.* A body is not one node;
Prism gives a `StatementsNode` with N children, so a pattern whose body capture
binds a single node matches only single-expression methods. Fails silently and
selectively: works on the toy, declines the real thing, exits 0. **Rewrite.**
Ubiquitous. ✅ fixed in D73.

**A2. A body that is implicitly a `begin` — `rescue`/`ensure`/`else`.** *match.*
Same failure as A1 wearing a different hat, which is why it survives the fix for
A1: a `def` with a `rescue` has a *single* child wrapping the statements, so a
matcher that special-cased `StatementsNode` misses it entirely. **Rewrite.**
Common — any method touching I/O. ❌ **confirmed broken.**

**A3. Method arity** — positional, default, keyword, splat, block. *match.* A
rename must ignore the parameter list entirely. **Rewrite.** Ubiquitous.

**A4. `def self.display_name`.** *match.* Requires the receiver distinction to be
load-bearing. **Rewrite exactly one per the rule's `#`/`.` sigil; report the
other** so the reader sees the near-miss. Ubiquitous.

**A5. `class << self`.** ✅ fixed. *match.* The `def` node is indistinguishable from an
instance method's; only the enclosing `SingletonClassNode` says otherwise. Wrong
in *both* directions — a `#`-rename that rewrites it corrupts the class method, a
`.`-rename that skips it misses the definition. **Rewrite under `.`, skip-and-
report under `#`.** Common, near-universal in library code. Was broken in
*both* directions — the `#` rename rewrote it and the `.` rename missed it.

**A6. `private def`.** *match.* The statement is a `CallNode` whose argument is a
`DefNode`. **Rewrite.** Common and rising. ✅ works.

**A7. Endless methods** (`def display_name = ...`). *match, splice.* No `end`
token; deletion-unit logic derived from `def…end` needs re-checking.
**Rewrite.** Occasional and climbing. ✅ works.

**A8. `?`, `!`, `=` suffixed siblings.** *account.* Different methods, but a
rename of the reader usually intends the writer too. **Rewrite only the exact
name; report the siblings labelled as siblings** — they are the next three
commands the user should run, not a miss. Ubiquitous.

**A9. Operator method names** (`==`, `<=>`, `[]`, `[]=`, `to_s`). *match.* Every
one has two spellings at the call site: `a == b` is `a.==(b)`. **Canonicalize at
match time; preserve the original spelling on rewrite** — a rewrite that turns
`a == b` into `a.==(b)` gets the tool uninstalled. Ubiquitous.

**A10. The same method defined twice in one class.** *account.* Ruby takes the
last. **Rewrite both, and say there were two** — the duplicate is itself a
finding. Occasional, but near-guaranteed above ~50k lines.

**A11. Conditional definition** (`if defined?(...)`). *match, account.*
**Rewrite the definitions, report the conditionality.** Occasional.

**A12. `alias` and `alias_method`.** *account.* `alias` takes bare identifiers,
not symbols, and parses to its own node kind — so a symbol-oriented residue scan
can miss it entirely, the dangerous direction. **Report both.** The alias target
may deliberately be the *old public name*, so rewriting can be exactly backwards.
Common in legacy — it is how Ruby did deprecation before deprecation gems.

## B. Call-site shapes

**B1. Implicit self, same class.** **Rewrite.** Ubiquitous.

**B2. Implicit self, subclass.** *match.* Requires the hierarchy, which requires
finding every reopening. **Rewrite.** Common.

**B3. Implicit self, from an included module.** *match.* Lexically a call on
nothing in particular; an `Account` method only by virtue of an `include`
elsewhere. **Rewrite when included into exactly the target class and its
subclasses; report when included into several unrelated classes.** Ubiquitous in
Rails — concerns are the default answer to "where should this live".

**B4. Explicit `self`.** **Rewrite.** Common.

**B5. Explicit receiver, resolvable type.** **Rewrite** where it resolves;
**report** where it doesn't. `None` means "not known", never "assume yes".

**B6. Explicit receiver, unresolvable.** *account.* Ivars, block params, hash
values — the majority of call sites in controllers, views and jobs. **Report as
`Call`.** Ubiquitous.

**B7. Multi-line method chains.** *match, splice.* The call's extent begins at
the receiver, so replacing "the call" replaces the whole chain; only the message
range may be touched. **Rewrite, touching only the identifier.** Ubiquitous.

**B8. Safe navigation** (`account&.display_name`). *match, splice.* A rewrite
that normalises `&.` to `.` introduces a `NoMethodError` on nil visible only in
production. **Rewrite, preserving `&.` exactly.** Common.

**B9. Symbol-to-proc** (`map(&:display_name)`). *account.* A call by that name on
each element, spelled as a symbol; the element type is usually unknowable. One of
the most common ways a method is reached in idiomatic Ruby. **Report as
`Symbol`.** Ubiquitous — if only one metaprogramming-adjacent shape is tested,
test this one.

**B10. Dynamic dispatch with a literal name** — `send`, `public_send`, `try`,
`respond_to?`, `method(:x)`, `instance_method(:x)`. *account.* **Report as
`Symbol`; never rewrite** — `try(:display_name)` on an unknown receiver may reach
a different class entirely. Common; `try` alone makes it ubiquitous in Rails.

**B11. `super` and `super()`.** *match, and hard to notice.* `super` names no
method; it dispatches on the enclosing method's name. Rename both definitions and
`super` stays correct with no edit. Reach only one and the override becomes an
orphan, `super` raises, and nothing in the diff shows it. **Rewrite nothing at the
`super`; treat every `super` inside a renamed method as a constraint that the
rename must reach the whole override chain, and refuse if it cannot.** Common.

**B12. Keyword argument labels that look like the method.** *account.*
`Account.new(display_name: "x")` is a hash key — except ActiveRecord really does
dispatch to `display_name=`. Genuinely both. **Report, classified as a label
rather than a call**; mislabelling makes the whole report look noisy. Ubiquitous.

**B13. Hash shorthand** (`{ display_name:, email: }`). *match.* The value is
elided. **Report.** Occasional, rising.

**B14. A local variable with the same name as the method.** *match.* After
assignment it is a `LocalVariableReadNode`, not a call. **Do not rewrite; report
separately from calls.** Common.

**B15. The rename target collides with an existing local.** *account — and the
one that produces working, wrong code.* Renaming `display_name → full_name` where
`full_name` is already a local yields `full_name = full_name if …`: a
self-assignment that quietly evaluates to the local's current value. No
exception, no failing parse, `verify` passes. **Refuse the rename in any scope
where the new name is already bound as a local, parameter or block parameter.**
Occasional — but the cost of missing it is total, and the probability rises
sharply for short natural names (`name`, `id`, `value`, `total`). ❌ **confirmed
broken — produces the self-assignment.**

## C. Strings, literals, and splice hazards

**C1. Heredocs.** *splice — the canonical hazard.* The node's location ends at
`<<~BODY`; the body sits on later lines. A splice using the location and not
`effective_range` detaches the body — and the result *still parses*, so no
reparse check catches it. The interpolated call inside **should** be rewritten in
place. **Rewrite the interpolated call; never splice a range that starts before
the opener and ends before the body.** Common — SQL, email bodies, GraphQL,
`class_eval` payloads.

**C2. Heredoc arguments mid-call.** *splice, severely.* Two bodies stack below in
opener order, after the closing paren; the call's textual extent and its
`effective_range` diverge by several lines. **Refuse any edit whose range would
cross an opener without its body.** Occasional — the shape most likely to corrupt
a file if the range discipline has a hole.

**C3. Squiggly heredoc indentation is semantic.** *splice.* `<<~` strips the
*common* leading whitespace, so changing one line's indentation changes the string
for every line; `<<-` preserves it and the same edit is inert. Two flavours,
opposite sensitivities. **Never re-indent inside a heredoc body.** Occasional,
invisible when wrong.

**C4. `%w` and `%i` literals.** *account.* `%i[display_name]` *is* the symbol —
decode element by element, not as one opaque blob. And `%i` is the idiomatic
spelling precisely in the macros that generate methods. **Report each element.**
Common.

**C5. String interpolation.** **Rewrite** the embedded call, touching only the
identifier. Ubiquitous.

**C6. A name inside an ordinary string.** *account.* Three things in one costume:
`send("display_name")` is a dispatch, `where("display_name ILIKE ?")` is a SQL
column, and a `raise` message is prose. **Report as `String`** — rewriting the SQL
one breaks a query that currently works. Ubiquitous.

**C7. A name that is also an English word** (`name`, `value`, `type`, `status`).
*account, at scale.* The report finds every occurrence everywhere; 900 lines is a
report nobody reads, and an unread report is a silent miss with extra steps.
**Report, but scope hard**, and be explicit about how many were suppressed and
why. The honest degradation is a *ranked* report, not a truncated one.
Ubiquitous — short generic names are the most common rename targets.

**C8. Comments.** *account, permanently.* Prose cannot be disambiguated.
**Report, never rewrite.** Ubiquitous.

**C9. `=begin`/`=end` block comments.** *account.* Not a `#` comment; a scanner
keyed on `#` classifies the contents as code. **Report as `Comment`.** Rare — but
it appears in exactly the vendored, ancient files where a mis-classification is
least noticed.

**C10. `__END__` and `DATA`.** *account.* Everything after `__END__` is not Ruby;
a byte-level search finds names there and calls them code. **Stop code scanning
at `__END__`.** Rare.

**C11. Magic comments and the first line.** *splice.* Magic comments are only
magic at the top of the file. Any insertion at byte 0 displaces them and silently
changes string mutability or encoding for the whole file. **No rewrite ever
precedes the magic-comment block.** Ubiquitous (the pragmas).

## D. Control flow and modern syntax

**D1. Block spellings** — `{ |a| }`, `do |a| end`, `_1`, `it`, `&:sym`. *match.*
Five spellings, one meaning; a pattern matching `$R.map { |$X| $B }` sees at most
two. `&:sym` carries the most volume and matches the fewest patterns. **Match all
block-bodied forms; preserve the original spelling** — normalising brace style is
a style change the tool has disclaimed owning. Ubiquitous.

**D2. Explicit block parameter vs `yield`**, and anonymous forwarding (`&`, `*`,
`**`, `...`). *match.* A rule reasoning about "does this iterate" is wrong in the
under-matching direction. Common.

**D3. `case`/`in` pattern matching.** *match, account.* `display_name:` in
`in { account: { display_name: } }` is a hash key pattern; in
`in Account(display_name:)` it calls `deconstruct_keys`. Same token, three
meanings. **Report; never rewrite** — the arm may destructure a JSON payload whose
key is fixed by an external contract. Occasional, rising.

**D4. Modifier `rescue`/`if`/`unless`, and `and`/`or`.** *match.* Modifier forms
nest the opposite way from their block forms. **Rewrite** the inner call; any rule
reasoning about *statements* must handle both nestings. Ubiquitous.

**D5. `retry`, `redo`, `ensure`-with-`return`, `begin…end while`.** Bodies shaped
unlike ordinary statement lists (see A2). Occasional.

**D6. `defined?` on a bare method name.** *account.* Takes an expression, not a
symbol. **Report.** Occasional.

## E. Class and module structure, constant resolution

**E1. A class reopened across many files.** *match.* The method set is the union
over every file; a hierarchy built from one file finds no subclasses and then
declines everything — silently, in the conservative direction the design already
tolerates, so nothing flags it. **Union all reopenings.** Ubiquitous in Rails:
model, concern, decorator, spec support, initializer patch, minimum.

**E2. Compact nesting vs nested modules.** *match.* `class Billing::Account` and
`module Billing; class Account` are the same class in two lexical shapes — and
*not* equivalent for constant lookup inside the body. **Normalise to the fully-
qualified name.** Ubiquitous in any namespaced app or engine.

**E3. `::Account` vs `Account`.** *match.* **Normalise; never remove a leading
`::` in a rewrite**, since it may disambiguate against `Billing::Account`. Common.

**E4. A class created by assignment** (`Class.new`, `Struct.new(:display_name)`).
*match, account.* No `ClassNode`, so a hierarchy walk keyed on `class` never sees
it; and `Struct.new(:display_name)` *defines* the reader. **Report**, and for
`Struct` report as a probable definition site. Rare / occasional.

**E5. `include` / `extend` / `prepend`.** ✅ reported. *account, silently.* `prepend` puts the
module's method *ahead* of the class's. Rename only the class's own definition and
the prepended one no longer overrides anything: the module's behaviour stops
happening, `super` in it raises, callers skip it. Everything parses; most suites
pass. **The rename must reach every definition in the ancestry chain that
participates in the override, or refuse** — a partially-renamed override chain is
a behaviour change, not a stale reference. Occasional for `prepend`, common for
the general shape.

**E6. `ActiveSupport::Concern` — `included do` and `class_methods do`.** ✅ reported. *match.*
`included do` executes in the including class's singleton context; defs inside
`class_methods do` are class methods, with the only signal being the enclosing
block's method name. **Rewrite the instance def; skip-and-report the
`class_methods` def under `#`; report the macro symbols inside `included do`.**
Common — the standard shape of a modern Rails concern.

**E7. `extend self` / `module_function`.** *match.* The def is written as an
instance method and callable as a module method; both `#` and `.` are arguably
correct. **Rewrite, and note the dual nature** rather than picking one silently.
Occasional.

**E8. Refinements.** ✅ reported, and a file that *activates* one with `using` is now refused rather than rewritten. *account.* The method exists only in files with a matching
`using`. **Report; do not rewrite** as part of a class-scoped rename. Rare — but
wrong handling is a wrong rewrite, not a miss.

**E9. Monkey patches on core classes.** *account, both directions.* Renaming
`Account#display_name` must not touch `String#display_name`; and once
`String#display_name` exists, "unresolved receiver" is no longer a safe
conservative decline — it is genuinely ambiguous. **Rewrite nothing outside the
target class; report the core-class definition** so the reader can judge the
residue. Occasional.

**E10. A top-level `def`.** *account — and it poisons everything.* A top-level def
becomes a private instance method on `Object`, callable with an implicit receiver
from every class in the process, so every implicit-self occurrence in the repo is
ambiguous. **When one exists, downgrade implicit-self rewrites to reports, or
refuse and say why.** Rare — high on the list anyway, because it converts a
confident rename into a wrong one repo-wide and is cheap to detect.

## F. Rails macros that define methods

The unifying fact: **in Rails, most methods are not defined by `def`.** A tool
whose model is "find the definition, rewrite call sites" is working with a
minority of the actual definitions.

**F1. `attr_accessor` and friends.** *account.* The symbol *is* the definition.
**Report** — rewriting is nearly safe, but `attr_reader :display_name` beside a
hand-written `def display_name` (A10) means rewriting produces two methods where
there was one. Ubiquitous.

**F2. `delegate`.** *account, in two opposite directions at once.*
`delegate :display_name, to: :profile` both *defines* `Account#display_name` and
*calls* `Profile#display_name`. And `prefix: true` defines `owner_display_name`,
a name that appears nowhere. **Always report, never rewrite.** The densest
ambiguity in Rails and the clearest illustration of why report beats guess.
Ubiquitous.

**F3. Association macros.** *account.* `has_many :widgets` defines roughly a dozen
methods from one symbol; `inverse_of:`, `class_name:`, `foreign_key:`, `source:`
name things on other classes. **Report; refuse to rewrite** — an association
rename is a schema change, not a text edit. Ubiquitous.

**F4. `scope`.** *account.* The defining symbol makes a class method; the lambda
body contains column names, a different namespace entirely. **Report both,
separately.** Common.

**F5. Validations.** *account.* The symbol reaches the reader method *and* the
i18n key *and* the `errors[:display_name]` key that views and JSON clients read.
**Report, and name the non-Ruby reaches it implies.** Ubiquitous.

**F6. Callbacks** (`before_save`, `before_action`, `only:`/`except:`). *account.*
Every symbol names a method, usually private, reached only by that symbol.
`only:` names controller actions, which are also routes, view filenames and spec
descriptions. **Report; refuse a controller-action rename** — it spans four
places and rwr sees one. Ubiquitous.

**F7. `enum`.** *account, verging on invisible.* Defines `active?`, `active!`,
`Account.active` and, with `prefix: true`, composed names appearing nowhere. Two
macro spellings; a rule matching one misses every Rails 7 model. **Report; refuse
to rewrite.** Common.

**F8. `alias_attribute`, `class_attribute`, `store_accessor`, `serialize`.** ✅ recognised as definers.
*account.* `class_attribute` defines five methods from one symbol;
`store_accessor` is backed by a JSON column key, so renaming means migrating
stored data. **Report**, and flag `store_accessor` as data-backed. Occasional
each, common in aggregate.

**F9. `has_secure_password`, `acts_as_*`, gem macros.** *invisible.* **Nothing to
report per occurrence; state that completeness is not claimed** in a class using
macros rwr does not model. Common.

**F10. Columns — methods with no definition anywhere.** *account, and the most
important entry in this section.* For a database-backed attribute there is no
Ruby definition. **Refuse, and say why**: "Found 47 call sites and no definition;
`display_name` appears in `db/schema.rb` as a column on `accounts`" beats 47
rewritten call sites against an unrenamed column, which is a repo-wide
`NoMethodError` on deploy. Ubiquitous — in Rails, most methods anyone wants to
rename are columns.

**F11. Query DSL symbols** (`where`, `order`, `group`, `pluck`, `select`,
`find_by`, `merge`). *account.* Column names, not method calls, spelled
identically. **Report; never rewrite** — the column has not moved. Ubiquitous.

## G. Views, templates, and non-Ruby reaches

**G1. ERB.** *match, splice.* Ruby stitched out of fragments; an `if` and its
`end` live in different tags. **Rewrite within a tag; refuse an edit spanning
tags.** The receiver is usually an ivar, so most of these are B6 reports rather
than rewrites — worth stating plainly, because a user told "ERB is parsed" expects
their views rewritten. Ubiquitous.

**G2. HAML and Slim.** *match — not parsed at all.* **Text-search at identifier
boundaries and report as `Text`, labelled as weaker evidence.** The count of
text-searched templates must be reported unconditionally. Common — plenty of real
apps are majority-HAML.

**G3. Form and view helpers taking attribute symbols.** *account.* The symbol
reaches the reader, the writer, and the HTML `name=` attribute, which determines
the params key, which determines what `permit` must allow. **Report** — rewriting
without the rest produces a form that silently drops the field on submit.
Ubiquitous.

**G4. Strong parameters.** *account.* A wire-format key joined to the form field,
the column and the writer. Unpermitted parameters are dropped, not raised.
**Report, and say what it is joined to.** Ubiquitous.

**G5. i18n keys.** *account, partly out of reach.* The YAML is not Ruby; lazy
`t(".display_name")` resolves from the *template's path*, so renaming a view file
changes key resolution with no textual change anywhere. **Report the Ruby side;
state explicitly that locale files were not scanned.** Ubiquitous.

**G6. Serializers and JSON builders.** *account, match.* `attributes
:display_name` both calls the method and names the output key; `json.display_name`
is `method_missing` on a builder where the name *is* the key. **Report loudly** —
renaming changes a public API response shape. Common.

**G7. Routes.** *account.* An action named by string, by symbol, and implied by
`resources`, each generating a URL helper under another name. **Report; refuse a
controller-action rename.** Common.

**G8. Factories and fixtures.** *account / out of reach.* The factory parses as
Ruby with an unresolvable receiver; the fixture YAML does not. **Report the
factory; state that fixture YAML was not scanned.** Common.

## H. Legacy-codebase realities

**H1. Files that do not parse.** ✅ named, for files that could have contributed. *invisible by construction.* Generator templates
under `lib/templates/` with `.rb` extensions, `.rb.tt`, syntax from a newer Ruby,
genuinely broken files. A file that fails to parse contributes zero matches and
zero residue, and looks exactly like a file with nothing in it. **Count and
report every unparseable file, unconditionally, on both output planes.** The
purest instance of the failure class: a filter that over-fires is
indistinguishable from a quiet corpus. Common — near-guaranteed with generators
or vendored code.

**H2. Encoding, BOM, and line endings.** *splice.* A UTF-8 BOM shifts every offset
by three; a lossy decode of Latin-1 produces offsets that do not match the bytes
on disk, so the rewrite lands in the wrong place while still parsing. CRLF breaks
blank-line and whole-line logic in the deletion unit. **Refuse the file loudly and
count it** — silently lossy decoding corrupts rather than declines. Rare /
occasional, and corrupting rather than missing, which earns the place.

**H3. Vendored and generated code.** *account.* Must not be rewritten — but
`db/schema.rb` names every column (F10) and `sorbet/rbi/` names every method, so
they are exactly the evidence a rename needs in files a rewrite must never touch.
**Exclude by whole path component**: a substring rule excluding `tmp` also
excludes `app/models/attempt.rb`. Ubiquitous.

**H4. Huge or pathological files.** **Process or skip, but never skip silently.**
Occasional.

**H5. Zeitwerk — the file path is part of the name.** Renaming a class means
renaming its file or the app will not boot. **Out of scope; say so** for class
renames rather than leaving the user to discover it at boot. Ubiquitous for class
renames.

## I. Genuinely invisible

Where rwr cannot report an occurrence because there is no occurrence. The only
honest product is **a statement that the completeness claim does not hold here.**

**I1. A computed method name.** `define_method("formatted_#{attr}")`,
`send("get_#{attr}")`. *invisible, half* — the `%i[...]` source list is findable
and should be reported; the composed name is not. **Report the visible half; flag
the `define_method`-with-interpolation as a definition site whose names cannot be
enumerated.** Common in older models and any "DRY the accessors" refactor.

**I2. `method_missing`.** *invisible, and it invalidates the account.* Once a
class in scope defines it, any name may be handled dynamically, so "every reach
has been accounted for" is not a claim rwr can make about that class. **Report its
presence and downgrade the completeness claim explicitly** — a caveat on the whole
report, not a residue line. Occasional; its cost is that it silently falsifies the
tool's headline promise.

**I3. `eval` / `class_eval` with a heredoc.** *invisible and a splice hazard at
once.* The text is Ruby and defines methods, but rwr sees a string; the body is
also detached from its opener (C1); and `__LINE__ + 1` means any edit changing the
file's line count shifts error line numbers inside it. **Report as `String`; note
it is an unparsed definition site.** Occasional, concentrated in the oldest files.

**I4. `OpenStruct`, `Hashie`, delegating wrappers.** *invisible.* Any name
responds. **Report the call as unresolved.** Occasional.

**I5. Reaches that leave the process.** A Sidekiq argument serialized into Redis
before a deploy; a GraphQL field; a database column; an API key a JavaScript
client reads; a cached fragment; a `Marshal` dump. *Invisible, permanently.*
**Out of reach — and the report should say which file classes it scanned**, so the
reader knows the boundary of the claim rather than inferring a larger one.

## Open, with repros

Two entries were confirmed against the binary and are **not yet fixed**. Written
down rather than left in a transcript, because a known bug that nobody recorded
is indistinguishable from an unknown one.

**A2 — a `rescue`/`ensure` body is declined.** A `def` carrying a `rescue` has a
`BeginNode` body rather than a `StatementsNode`, so the D73 body fix does not
reach it. The rename declines the definition and *reports* it as residue, so the
run is honest rather than silent — but it declines every method that touches I/O.

```ruby
class Account
  def display_name
    remote.name
  rescue Timeout::Error
    email
  end
end
```
→ `rewrites: 0`, residue `[(2, 'definition')]`.

The obvious fix — hoisting the body check above the discriminant comparison in
`matcher::match_node` — makes the *match* succeed and then the rewrite path
claims edits it does not make and never converges (exit 4 on every rerun, file
unchanged). The splice side needs understanding first; a reported miss is
strictly better than a retry loop that lies about its work.

**B15 — the rename target collides with an existing local.** The worst outcome in
this document: working code with changed behaviour.

```ruby
def summary
  full_name = "unknown"
  full_name = display_name if profile?
  full_name
end
```
→ renaming `display_name → full_name` yields `full_name = full_name if profile?`,
a self-assignment that quietly evaluates to `"unknown"`. It parses, it runs,
`verify`'s reparse passes, and nothing anywhere reports it.

The fix is a refusal, not a smarter rewrite: before rewriting a call site, check
whether the new name is already bound as a local, parameter or block parameter in
that scope, and refuse the whole rename if so.

**Still open, from the metaprogramming audit:**

- **Interpolated dynamic dispatch gets no blind-spot notice.** `send("display_#{x}")` inside the
  target class is invisible, which the design accepts — but the design also says rwr should
  *state* that completeness is not claimed. Needs a new `Context` variant, and the trigger set is
  a judgement call: keying on `send`/`try`/`define_method` with a non-literal argument is
  defensible; prefix-matching the anchor against an interpolation is not.
- **A module included into a class rwr never parses.** The worklist parses files naming an
  already-known class, so `Account.prepend(A)` in one file and `module A; include B` in another
  works, but a third hop to `module B` does not — B's file names no known class. Fixing it means
  seeding the fixpoint with discovered *module* names, which changes the shape of the worklist.
- **`validates`, callbacks and `scope` over-report in unrelated classes.** Deliberate: they
  *refer* rather than define, and a serializer's `validates :display_name` is a genuine two-hop
  reach. Collapsing them into the definer list would conflate two mechanisms that happen to
  agree in the common case.

## The ten to test first

Ranked by (likelihood of being wrong) × (cost of being wrong *silently*). A wrong
rewrite outranks a missed one; a silent miss outranks a loud refusal.

1. **A1/A2 — multi-statement and `rescue`-bearing bodies.** The bug that already
   happened, and its twin that survives the fix. Ubiquitous, silently selective,
   exits 0.
2. **B15 — the rename target collides with an existing local.** `full_name =
   full_name if …` parses, runs, returns the wrong value, and passes `verify`. The
   only entry that produces working code with changed behaviour.
3. **F10/F1/F2 — a method with no `def`: a column, an `attr_accessor`, a
   `delegate`.** In Rails the common case, not the exception, and the failure mode
   is call sites renamed with the definition untouched.
4. **B9/B10 — `&:display_name`, `send(:...)`, `try(:...)`.** The highest-volume
   unconvertible reach in idiomatic Ruby. If they are missing from the report, the
   tool has told the user they are finished when they are not.
5. **C1/C2 — heredoc splice.** The canonical wrong-but-still-parses edit, with no
   downstream check that can catch it. Interpolation, two-heredocs-in-one-call,
   and squiggly indentation fail independently.
6. **A4/A5 — `def self.x` and `class << self`.** Receiver polarity is the tool's
   headline differentiator, and `class << self` gets it wrong in *both* directions
   from an identical-looking node.
7. **E5/E6/B3 — concerns and `prepend`.** A partially-renamed override chain
   silently unhooks a module: no error, no failing parse, behaviour quietly gone.
   And concerns are where most Rails methods live.
8. **H1 — files that don't parse, and templates that aren't parsed.** A skipped
   file is indistinguishable from an empty one at every later stage.
9. **E10/I2 — a top-level `def`, or a `method_missing` in scope.** Each converts
   "this rename is complete" from a fact into a falsehood. Both cheap to detect,
   expensive to leave unsaid.
10. **E1/E2/E3 — reopened classes, compact nesting, `::Foo`.** Class identity
    underlies every receiver decision, and getting it wrong under-reaches quietly.

Just off the list, named so they are not mistaken for absent: **B12** (keyword
labels, the largest single source of residue noise), **D1** (block spellings,
where under-matching is the default failure of every performance rule), and
**G5** (locale YAML, the clearest case for stating which file classes went
unscanned).

## The shape of the whole thing

**In Rails, the Ruby is a minority of the reach.** A single attribute rename
touches a column, a form field, a param key, an i18n key, a serializer key, a
factory, a fixture and a locale file — of which rwr can see two or three. The
tool's credibility rests less on how many of those it rewrites than on whether
the report names the ones it cannot.
