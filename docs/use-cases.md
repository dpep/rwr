# Use cases — design stress-test by worked example

Seven case studies of an experienced Rails engineer using rwr for real work in a large
monolith, written **before any code exists** to pressure-test `DESIGN.md` §5's pattern
syntax, §4's safety contract, and the CLI conventions against work that actually occurs.
The case studies are the vehicle; the payload is **What these expose** and **Cannot do**
at the end. Several cases end with "rwr has no answer" — that is the point of the exercise.

Written against DESIGN.md revision 3 (D1–D17). Does not modify the design; findings are
input to it.

## Conventions invented by this document

The design does not yet specify how a rule reaches the CLI. This doc assumes, and
**flags as invented** (⚠):

- ⚠ `-f FILE` passes a YAML rule file; a bare quoted pattern is an ad-hoc inline rule
  (`rwr find '$X.present? ? $X : nil' app/`).
- ⚠ Rule files may hold multiple rules separated by `---`, applied as one transaction.
- ⚠ Rule metadata fields `name:`, `message:`, `severity:` for CI reporting.
- ⚠ `where:` predicates beyond the two DESIGN.md shows (`keywords:`, `receiver_type:`)
  are invented where a case needs one, and each invention is a finding.

D16 adopts min/max occurrence counts as the metavariable *semantics* but no *surface
syntax* exists for them. Where a case needs a sequence or optional metavariable, the gap
is called out rather than papered over.

---

# 1. Refactoring / migration

## 1.1 Renaming a domain method: `User#full_name` → `User#display_name`

**Situation.** Product renames the concept; `full_name` is defined on `User`, delegated
from `AdminUser`, called from ~200 sites in models, services, serializers, mailers, specs
— and views. `rg full_name` returns 600+ hits including `Company#full_name` (a different
method that must not change), i18n keys, and a `full_name` column on an unrelated table.
`sed` cannot tell a `User#full_name` call from any of those; this is the canonical rwr job.

**Rules** (⚠ multi-rule file):

```yaml
# rules/rename-full-name.yml
name: rename-full-name
match: $RECV.full_name
rewrite: $RECV.display_name
---
# implicit-self call sites inside User and its concerns
match: full_name
rewrite: display_name
---
# the definition itself
match: |
  def full_name
    $BODY
  end
rewrite: |
  def display_name
    $BODY
  end
```

**Invocation:**

```
rwr rewrite -f rules/rename-full-name.yml app/ lib/ spec/ --dry-run
rwr rewrite -f rules/rename-full-name.yml app/ lib/ spec/ --json
```

**Before/after**, including the case Prism gets right that a lexical tool cannot:

```ruby
def greeting_for(user)
  full_name = user.full_name          # LHS is a local; RHS is the call
  I18n.t("mailers.greeting", name: full_name)
end
```

After:

```ruby
def greeting_for(user)
  full_name = user.display_name
  I18n.t("mailers.greeting", name: full_name)
end
```

Prism resolves `full_name` on lines 2–3 as `LocalVariableRead`, not `CallNode`, so rule 2
correctly leaves the local alone. This is a genuine D1 fidelity win — a bare identifier is
call-or-local depending on scope history, which a grammar without local tracking cannot
distinguish.

**Output** (trimmed):

```json
{
  "rule": "rename-full-name",
  "matched": 214, "rewritten": 214, "conflicts": 0,
  "skipped_files": [],
  "residue": {
    "identifier": "full_name",
    "occurrences": [
      {"file": "app/models/admin_user.rb", "line": 9,  "context": "symbol_in_delegate",
       "snippet": "delegate :full_name, to: :user"},
      {"file": "app/serializers/user_serializer.rb", "line": 4, "context": "symbol_in_macro",
       "snippet": "attributes :id, :full_name, :email"},
      {"file": "app/models/concerns/auditable.rb", "line": 21, "context": "send_argument",
       "snippet": "record.send(field)  # field ∈ AUDITED_FIELDS = [:full_name, ...]"},
      {"file": "spec/factories/users.rb", "line": 7, "context": "string_literal",
       "snippet": "full_name { \"Ada Lovelace\" }"},
      {"file": "config/locales/en.yml", "line": null, "context": "unparsed_file",
       "note": "not Ruby; not scanned"}
    ]
  }
}
```

The residue report is doing exactly its §4 job: the `delegate` and `attributes` macro
sites are *definition-adjacent* sites the structural rules cannot reach, and the engineer
responds by adding rules — `delegate :full_name, to: $T` → `delegate :display_name, to: $T`
is a perfectly matchable pattern because the symbol is a literal. Residue-driven rule
iteration is the agent loop working as designed. Two loops later, residue is empty in `.rb`.

**What goes wrong:**

1. **Rule 2 (`match: full_name`) has no way to say "only inside `User`."** Implicit-self
   call sites need an *enclosing-scope* constraint. `receiver_type:` (Phase 2) narrows
   explicit receivers; the implicit-self case needs something like `inside: class User` —
   which is exactly the relational rule family D12 defers "until the corpus demonstrates
   need." The first realistic case demonstrates the need. Without it, rule 2 must be
   path-scoped by hand (`-p app/models/user.rb -p app/models/concerns/...`), which is
   fragile and silently wrong the day someone adds a new concern.
2. **`Company#full_name` call sites match rule 1 and are rewritten, silently, at exit 0.**
   Not a refusal — a confident wrong match. Pre-Phase-2 there is no constraint that can
   express "receiver is a User." The safety contract (§4) guards *edit mechanics*
   (overlap, parse errors); it never sees a structurally-valid-but-semantically-wrong
   match. This is D6's premise made concrete, and it is the dominant risk in this case —
   not refusal noise.
3. **The views are invisible.** `app/views/users/show.html.erb` contains
   `<%= @user.full_name %>`; jbuilder has `json.full_name`. rwr parses Ruby files; ERB,
   Haml, and jbuilder are where a huge fraction of a Rails rename actually lives. Neither
   the match nor the residue report touches them, so the report's implicit claim
   ("here is everything left") is false for the repo even when true for `.rb`. The
   engineer is back to `rg` for the largest remaining surface.
4. `$BODY` in rule 3 must bind a multi-statement body — a sequence metavariable with no
   surface syntax (D16 gap, see 1.2).

## 1.2 Positional → keyword arguments on a service entry point

**Situation.** `Payments::Charge.create(amount_cents, currency, idempotency_key = nil)`
has ~340 call sites; after one too many transposed-argument incidents, the team moves to
keywords. Text tools are hopeless: call sites are multiline, arguments are arbitrary
expressions, and `create` is one of the most common tokens in the codebase.

**Rules.** The third positional is optional, so one rule per arity — because occurrence
counts have no syntax yet:

```yaml
name: charge-kwargs
match: Payments::Charge.create($AMOUNT, $CURRENCY, $KEY)
where:
  keywords: none
rewrite: Payments::Charge.create(amount_cents: $AMOUNT, currency: $CURRENCY, idempotency_key: $KEY)
---
match: Payments::Charge.create($AMOUNT, $CURRENCY)
where:
  keywords: none
rewrite: Payments::Charge.create(amount_cents: $AMOUNT, currency: $CURRENCY)
```

D16's semantics (`$KEY` with min 0, max 1) would collapse these into one rule; nothing in
D2's "Ruby source with $METAVARS" can *write* that, because `create($A, $B, $C?)` is not
Ruby. This is the D2 reversal clause ("anything about arity, ordering, or absence")
firing on a bread-and-butter migration.

**Invocation and human-facing dry run:**

```
$ rwr rewrite -f rules/charge-kwargs.yml app/ lib/ spec/ --dry-run
app/services/orders/checkout.rb:118
-      Payments::Charge.create(order.total_cents, order.currency, idem_key)
+      Payments::Charge.create(amount_cents: order.total_cents, currency: order.currency, idempotency_key: idem_key)
337 sites in 121 files would be rewritten
```

**The gnarly site** — multiline call with a heredoc argument and a nested call:

```ruby
Payments::Charge.create(
  order.total_cents + Fees.for(order).cents,
  order.currency,
  Digest::SHA1.hexdigest(<<~SEED)
    #{order.id}-#{order.updated_at.to_i}
  SEED
)
```

After:

```ruby
Payments::Charge.create(
  amount_cents: order.total_cents + Fees.for(order).cents,
  currency: order.currency,
  idempotency_key: Digest::SHA1.hexdigest(<<~SEED)
    #{order.id}-#{order.updated_at.to_i}
  SEED
)
```

This works *because* the template preserves argument order, so every edit is a pure
insertion of `label: ` before an existing argument and the heredoc body never moves. Had
the template **reordered** arguments (`idempotency_key:` first), the heredoc body must
travel with its call — the exact D14 `effective_range()` case, and the reason raw
`.location` must stay unexposed. Worth making this pair an explicit fixture: same rule
shape, order-preserving vs order-changing template, both must be byte-correct.

**What goes wrong:**

1. **Splat sites escape silently.** `Payments::Charge.create(*charge_args)` matches
   neither rule — correctly, since rwr cannot know the arity. But then the safety net is
   residue reporting, and here §4's honest degradation becomes a real hole:

   ```json
   "residue": {
     "identifier": "create",
     "note": "identifier too common; 4,812 occurrences across corpus, not enumerated"
   }
   ```

   `create`, `update`, `call`, `perform`, `run` — Rails naming conventions concentrate
   migrations onto exactly the identifiers residue reporting must give up on. The
   name-commonality degradation is honest, but it zeroes out the differentiator on the
   *most common* migration shape. The fix suggests itself: the residue key for this rule
   is not `create` but `(Payments::Charge, create)` — a receiver-qualified residue scan
   (lexically: `create` occurrences within sites that also mention `Charge`) would be
   low-noise where the bare identifier is hopeless. §4 as written keys residue on the
   identifier alone.
2. **Does `create($A, $B, $C) { ... }` match?** Sites passing a block are unspecified —
   the pattern says nothing about a block, and DESIGN.md never says whether an
   unmentioned block is "don't care" (Semgrep-ish) or "must be absent." Either answer is
   defensible; not choosing one is a contract hole (D2/D16).
3. Sites that already migrated by hand (`create(amount_cents: x, ...)`) are excluded by
   `keywords: none` — this constraint earns its place and the rule is safely re-runnable.

## 1.3 Dynamic finders: `find_by_email(x)` → `find_by(email: x)`

**Situation.** A legacy corner still uses `method_missing`-era dynamic finders:
`find_by_email`, `find_by_slug`, `find_by_account_id_and_state`, ~90 distinct names.
The engineer wants one rule:

> match `$RECV.find_by_$ATTR($VAL)` → rewrite `$RECV.find_by($ATTR: $VAL)`

**rwr has no answer.** Metavariables bind AST nodes; `find_by_$ATTR` needs a metavariable
bound to a *fragment of an identifier*, and the rewrite needs a *name transform* (splice
the fragment into a keyword key, or for `_and_` finders, split it into two keys). "Ruby
source with $METAVARS" cannot lex `find_by_$ATTR` at all — it isn't Ruby. Comby does this
trivially (textually, with all of Comby's problems); RuboCop does it with a regex on the
method name inside a cop. rwr's honest options are ~90 generated per-attribute rules
(machine-writable by an agent, admittedly, but a workaround) or declaring sub-identifier
matching out of scope. Either way the design should say which.

The same wall blocks every migration of the form "rename by convention":
`*_filter` → `*_action`, `test_*` → `it "..."`, `has_many :old_*` renames. This is a
class of real work, not an edge case.

---

# 2. Simplification / modernization

## 2.1 `x.present? ? x : nil` → `x.presence`

**Situation.** Hundreds of pre-`presence` ternaries survive in a codebase older than
Rails 3. `rg` cannot verify the two `x`s are the same expression across arbitrary
whitespace and multiline forms.

**Rule** — this is D16's repeated-metavariable AST equality earning its keep:

```yaml
name: use-presence
match: $X.present? ? $X : nil
rewrite: $X.presence
---
match: $X.blank? ? nil : $X
rewrite: $X.presence
```

**Before/after:**

```ruby
subject = params[:subject].to_s.strip.present? ? params[:subject].to_s.strip : nil
```

```ruby
subject = params[:subject].to_s.strip.presence
```

AST equality (not Comby's textual equality) means the second occurrence matches even when
formatted differently across a line break. Good.

**What goes wrong:**

1. **The purity trap — a confident wrong match.** Repeated-metavariable equality is
   *structural*; the rewrite assumes the expression is *pure*:

   ```ruby
   header = rows.shift.present? ? rows.shift : nil
   ```

   The two `rows.shift` are AST-equal, the rule matches, and the rewrite changes two
   destructive calls into one. (The original was almost certainly already a bug — but the
   tool changed observable behavior at exit 0 with no flag.) rwr cannot decide purity and
   should not pretend to; but the design should decide whether repeated-metavar rules get
   a standing caveat in output, because every idiom-collapsing rule in this family
   (`||=`-ification, memoization, `presence`) carries the same trap.
2. **Isomorphism pressure is immediate, not hypothetical (Q8).** The same idea appears as
   `if x.present? then x end`, `x if x.present?`, `unless x.blank? ...`, and the
   `.blank?` mirror already forced a second rule. Covering one idiom took 2 rules and
   honestly needs 4–5; each is a separate match/rewrite pair that can drift. This is
   precisely the pull toward Coccinelle-style isomorphisms — and precisely the thing
   Semgrep shipped and withdrew. Q8's "spend 30 minutes finding out why" should happen
   before this family of rules is a headline use case.

## 2.2 `map { |u| u.email }` → `map(&:email)`

**Situation.** The engineer wants the general modernization: any single-param block whose
body is exactly one no-arg call on the param.

**The rule they want to write:**

```yaml
match: $RECV.map { |$X| $X.$M }        # ← not Ruby; does not lex
rewrite: $RECV.map(&:$M)               # ← :$M also not Ruby
```

**rwr has no answer**, and the reason generalizes into the single most important syntax
finding of this exercise: **a `$METAVAR` can only appear where a global variable is
grammatically legal Ruby**, because D2 parses the pattern with Prism. Expression
positions work (`foo($A)`, `$X.present?`). Method-name position (`$X.$M`), `def` name
position, symbol content (`:$M`), keyword-argument keys (`$K: v`), and constant paths
(`Foo::$C`) all fail to parse — the pattern language's reach is bounded by Ruby's grammar
for gvars, and nothing in DESIGN.md acknowledges the bound. ast-grep hits a milder form
of this and leans on tree-sitter's error tolerance; Prism's strictness — the product's
core bet — is what closes the door. Options: a pre-lex step that substitutes valid
placeholder identifiers before Prism sees the pattern (how several tools actually do it),
or documenting the bound as a contract limit. This needs a decision entry either way.

The fallback — one rule per method name (`map { |$X| $X.email }` → `map(&:email)`) — is
expressible and useless at modernization scale.

Two smaller observations from this case, one good, one unspecified:

- `{ |u| u.email }` vs `do |u| u.email end` produce the same AST, so structural matching
  unifies them for free — an isomorphism rwr never has to build.
- Whether a block and its params are even matchable *positions* (can a pattern constrain
  "block has exactly one param"?) is unspecified; see 3.2.

---

# 3. Enforcing standards — codemod as policy

## 3.1 Guardrail: no string-interpolated SQL (report-only, CI)

**Situation.** After an injection near-miss, the team bans interpolation in relation
methods. RuboCop has `Rails/OutputSafety`-style cops but nothing precise here without
writing a custom cop; the team wants a declarative rule in-repo. This is rwr as a
*standing* check, not a migration: the rule lives in `rules/`, runs in CI forever.

**Rule:**

```yaml
name: no-sql-interpolation
severity: error
message: interpolated SQL — use bound parameters ("state = ?", state) or Arel
match: $RECV.where($SQL)
where:
  $SQL:
    kind: interpolated_string    # ⚠ invented predicate
```

The `where:` block needs a **node-kind predicate** — "this capture is a string literal
containing interpolation" — which DESIGN.md's two shown constraints cannot express. It
also immediately needs **method alternation**: the same policy applies to `where`,
`having`, `order`, `group`, `joins`. With no `method: [where, having, ...]` form, that is
five copies of the rule (any bang-variant pair — `update_attributes`/`update_attributes!` — hits the
same wall). Alternation over *names* is not the rule algebra D12 defers; it is a flat
constraint and cheap.

**Invocation and the CI problem:**

```
rwr find -f rules/no-sql-interpolation.yml -J app/ lib/
```

Per conventions, exit 0 = matched. For a guardrail, *matched means the build must fail* —
the CI script wants the inversion. The naive `! rwr find ...` is actively dangerous: `!`
maps **every** nonzero to success, so a usage error (4) or internal error passes CI
silently. Every team will write this wrapper and some will write it wrong. The
three-audience story diverges here: agents want find's semantics; CI wants a `check` verb
(or `--expect-none`) where clean = 0, violations = nonzero-with-report, errors = a third
thing. One flag, or one verb, closes it.

**Output** (NDJSON row, stable location shape per conventions):

```json
{"rule": "no-sql-interpolation", "severity": "error", "file": "app/models/report.rb",
 "line": 44, "col": 12, "byte_start": 1180, "byte_end": 1236,
 "snippet": "scope.where(\"created_at >= '#{cutoff}'\")",
 "message": "interpolated SQL — use bound parameters (\"state = ?\", state) or Arel"}
```

**What goes wrong:**

1. **Legitimate matches need a suppression story.** `where("#{table_name}.deleted_at IS
   NULL")` in a shared concern is safe and idiomatic; so is interpolating
   `connection.quote(...)`. A standing CI rule without an inline escape hatch
   (`# rwr:disable no-sql-interpolation`, à la rubocop) gets deleted within a month —
   guardrail tools live or die on suppression ergonomics. rwr has no designed mechanism,
   and comments are exactly the thing not in the AST; §7's comment-attachment hazard work
   has to serve reads (policy suppression) as well as writes (deletion/movement).
2. `--explain` is what makes a refused-or-flagged site actionable for the human whose PR
   just went red — "matched `where`, `$SQL` kind `interpolated_string`, interpolation at
   col 31." The conventions doc has this right; it should be required output in `check`
   mode, not optional.

## 3.2 Autofix hook: `Timecop.freeze` → `travel_to`

**Situation.** The team standardized on ActiveSupport time helpers; Timecop lingers and
new uses keep arriving by copy-paste. Policy: block-form `Timecop.freeze` is auto-fixed
in a lefthook pre-commit hook; everything else is reported.

**Rule:**

```yaml
name: timecop-to-travel-to
match: Timecop.freeze($T) { $BODY }
where:
  block_params: none            # ⚠ invented — see below
rewrite: travel_to($T) { $BODY }
```

**Gnarly before** — nested freezes, multiline body:

```ruby
Timecop.freeze(Time.zone.parse("2024-01-01")) do
  order = create(:order)
  Timecop.freeze(3.days.from_now) do
    expect(order.reload).to be_overdue
  end
end
```

`rewrite` is outermost-only (D15): first run rewrites the outer call, reports the inner
match as skipped-inside-rewritten-range, **exits 2**. Rerun rewrites the inner one, exits 0.

```
$ rwr rewrite -f rules/timecop.yml spec/
rewrote 61 sites in 48 files; 1 skipped inside a rewritten range — rerun to make progress
$ rwr rewrite -f rules/timecop.yml spec/
rewrote 1 site in 1 file
```

**The hook**, which is where the exit contract meets reality:

```yaml
# lefthook.yml
pre-commit:
  commands:
    timecop:
      glob: "spec/**/*.rb"
      run: rwr rewrite -f rules/timecop.yml {staged_files} && git add {staged_files}
```

A hook is a one-shot, non-interactive caller: exit 2 fails the commit with work half-done
unless the team wraps rwr in a retry loop — which every adopter will write, badly.
DESIGN.md's no-auto-fixpoint rule (D15) is aimed at divergent self-matching rules, but
the exit-2 class is convergent *by construction* (the skipped set shrinks every pass). A
bounded internal retry for exactly that class — never for fresh matches of rewritten
output — would not reintroduce divergence, and it is what the hook audience needs. Agents
can loop; hooks and CI cannot, cheaply.

**What goes wrong:**

1. **Sites that use the yielded time cannot be blindly rewritten:**
   `Timecop.freeze(t) { |now| ... }` — `travel_to` yields nothing. Hence the invented
   `block_params: none` constraint: the rule needs to say "block binds no parameters,"
   which is an occurrence-count-on-block-params question (D16 semantics, no syntax, and
   blocks aren't even confirmed matchable positions — same hole as 1.2's unmentioned-block
   question, now load-bearing).
2. **Blockless `Timecop.freeze` in a `before` hook is a coordinated two-site change** —
   the freeze and its paired `Timecop.return` in `after` must convert together
   (`travel_to` + `travel_back`, or a block restructure). rwr's unit of rewrite is one
   match; "this edit is only valid if that other edit also happens" has no representation.
   Correctly left to the human — but the rule can't even *express* "match blockless form,
   report, never fix"; report-vs-fix is per-invocation, not per-rule. A `severity:`/
   `action:` per rule (fix vs flag) inside one rule file is the natural shape for policy
   packs.
3. Residue works nicely here (`Timecop` is a rare, well-anchored constant): stubbed
   references in shared contexts and a `Timecop.safe_mode!` in `rails_helper.rb` surface
   as classified leftovers for the human. This is §4 at its best — rare identifier, high
   signal.

---

# What these expose

Ranked. Each item names the case that surfaced it.

1. **Metavariables only fit where a gvar parses (2.2, 1.3).** D2's "pattern is Ruby
   parsed by Prism" silently bounds `$X` to expression positions. Method names, `def`
   names, symbol contents, keyword keys, and constant paths are unwritable — and those
   positions are where modernization rules live. Needs a decision: pre-lex placeholder
   substitution before Prism sees the pattern, or a documented contract limit. This is
   the largest unacknowledged hole in the v0.1 public contract.
2. **Residue degradation collides head-on with Rails naming (1.2).** `create`, `update`,
   `call`, `perform` are simultaneously the likeliest migration targets and the
   identifiers the "too common; not enumerated" clause abandons. The differentiator
   zeroes out exactly where it is most needed. Receiver-qualified residue keys
   (`Payments::Charge` + `create` co-occurrence, lexically) would restore signal; §4 keys
   on the bare identifier.
3. **D16 has semantics but no syntax (1.1, 1.2, 3.2).** Multi-statement `$BODY`, optional
   trailing argument, "block with no params," must-not-appear — every case needed an
   occurrence count and none could write one. The surface syntax is not a detail; it is
   most of what D2's public contract *is*.
4. **The refusal contract guards the wrong risk for match quality (1.1, 2.1).** In seven
   realistic cases, no natural exit-3 refusal ever fired — but two confident wrong
   rewrites sailed through at exit 0 (`Company#full_name`, the impure repeated-metavar
   `rows.shift`). Refusal protects edit mechanics; nothing protects match semantics
   before Phase 2. Phase 0 measurement (b) is therefore measuring the right thing, and
   the README's safety story should not imply otherwise.
5. **Implicit-self call sites demand an enclosing-scope constraint (1.1).** The first
   realistic rename needs `inside: class User` for its bare-identifier rule. D12 defers
   relational rules "until the corpus demonstrates need"; the corpus's very first entry
   demonstrates it. At minimum, `inside:` with a class/module target belongs in the
   Phase-2 conversation alongside `receiver_type:`.
6. **`where:` vocabulary is two predicates short of usable (3.1, 3.2, 1.3).** Concretely
   needed by these cases: node-kind of a capture (`interpolated_string`), method-name
   alternation (`where`/`having`/...; `update_attributes`/`!`), block-params arity.
   Alternation over names is flat and cheap — it is not the deferred rule algebra.
7. **Non-Ruby Rails surfaces are invisible to match *and* residue (1.1).** ERB/Haml/
   jbuilder hold much of any real rename. Full parsing is out of scope (rightly — see
   Cannot do), but residue's implicit "this is everything left" claim is false repo-wide.
   A cheap lexical fallback scan of non-Ruby files, reported as its own residue class
   (`unparsed_file_lexical_hit`), keeps the report honest for one extra grep's cost.
8. **The rule-input surface is unspecified (all cases).** Inline pattern vs `-f` file vs
   a `rules/` policy pack; multi-rule files and whether they apply as one transaction
   (1.1's four coordinated rules must); `name`/`severity`/`message` for CI; per-rule
   fix-vs-flag (3.2). This is CLI contract, D17-adjacent, and currently invented by this
   document.
9. **Three audiences, two missing affordances (3.1, 3.2).** CI needs a `check` verb or
   `--expect-none` (the `!`-inversion idiom swallows internal errors); hooks need a
   bounded internal retry for the convergent exit-2 class (distinct from the divergent
   fixpoint D15 rightly bans). Agents are served well by the current contract.
10. **Guardrail mode requires a suppression mechanism (3.1).** `# rwr:disable <rule>`
    or equivalent. Comments are outside the AST but policy tools are unusable without
    inline escapes; this intersects §7's comment-attachment work.
11. **Block-matching semantics are unspecified (1.2, 3.2).** Does an arg-only pattern
    match a call with a block? Can a metavariable bind a block, or block params?
    Load-bearing in both migration and policy cases. (Positive: `{}` vs `do…end`
    unification falls out of AST matching for free.)
12. **Isomorphism pressure arrives immediately (2.1).** One idiom = 4–5 rule variants
    today. Q8's "find out why Semgrep withdrew equivalences" should be answered before
    modernization is a marketed use case.

# Cannot do

Transformations these cases wanted that the design as written cannot express — listed
plainly rather than bent to fit.

- **Sub-identifier matching and name transforms (1.3).** `find_by_$ATTR` →
  `find_by($ATTR:)`, `*_filter` → `*_action`, any snake/camel case transform in a
  rewrite. Metavariables bind nodes, never name fragments.
- **General idiom rules needing metavars in non-expression positions (2.2).**
  `map { |$X| $X.$M }` → `map(&:$M)` and its whole family, pending finding 1.
- **Concrete-syntax modernization.** Hash rockets → `key:`, `and` → `&&`, quote-style
  normalization: the variants parse to identical (or same-typed) nodes, so structural
  matching cannot even *see* the distinction, and matching would produce self-rewrites.
  This is RuboCop's territory; the README should say so explicitly.
- **Non-anchored insertions.** "Add `include SoftDeletable` to every model matching X,"
  "add a magic comment / `require` at file top." Rewrite is anchored to a match and
  replaces it; inserting a new child into a class body or file head has no
  representation.
- **Coordinated multi-site edits (3.2).** Blockless `Timecop.freeze` + its paired
  `after { Timecop.return }`; any change valid only if a sibling change lands too. One
  match, one edit is the model — correctly, but state the limit.
- **Semantic guards.** Purity (2.1's `rows.shift`), truthiness (`x.nil? ? y : x` →
  `x || y` is wrong when `x` is `false`). No static tool at this layer can decide these;
  the design's honest move is a documented hazard class, possibly a standing caveat on
  repeated-metavariable rewrites.
- **Templates that are not Ruby (1.1).** ERB/Haml/jbuilder. Rightly out of scope for
  matching (principle 10 — depth over breadth), but see finding 7 for keeping residue
  honest about it.
