# The integration testbed

A small Ruby app whose only purpose is to be renamed. `Account#display_name` →
`full_name`, with every site marked for what must happen to it.

It exists because Q1 — *does residue reporting hit its pass bar?* — needs
**ground truth**, and ground truth means a repository where every reach is
known. It is scored by `tests/testbed.rs`.

## The markers

| marker | meaning |
|---|---|
| `GT:rewrite` | rwr must rewrite this site |
| `GT:residue` | this breaks and rwr cannot rewrite it, so it must be **reported** |
| `GT:blind` | this breaks and rwr cannot see it; absence is expected and honest |
| `GT:ignore` | this does not break; rewriting or reporting it is a false positive |
| `GT:notice` | this does not break, and must be reported anyway — a near-miss worth showing |

`notice` exists because `ignore` and `residue` could not express a deliberate
near-miss. `Account.display_name` beside `Account#display_name` does not break
when the instance method is renamed — so calling it `residue` is a lie — but
saying nothing lets a reader wonder whether it was considered. Reporting it is
the intended behaviour, so scoring it as a false positive would penalise the
tool for being helpful.

The markers live in the source rather than in a manifest, so editing a file
moves its ground truth with it. A line-numbered manifest goes stale the first
time anyone adds a line.

## Written from the Ruby side, deliberately

The sites were enumerated by asking *how does Ruby reach a method name* —
`send`, `respond_to?`, `alias_method`, `Symbol#to_proc`, `delegate`,
`validates`, a serializer DSL, a subclass override, interpolation, ERB, YAML —
not by asking what rwr currently classifies.

That distinction is not pedantry. Earlier in this project a corpus fixture had
recorded a **bug** as its expected output, because it was written after the
behaviour it was meant to check. A testbed written from the tool's own
capabilities would have scored 7/7 on day one and found nothing.

Written this way it scored **2 of 7** and found two real defects:

- residue was computed only for files rwr had already *changed*, so a file that
  is nothing but dynamic reaches — a serializer full of `delegate` and
  `validates`, which is the dangerous case exactly — was never looked at;
- the report was scoped to the target class, which discards those same reaches,
  since a delegation lives in a *different* class from the method it names.

## The second half of the principle, learned later

Writing from the Ruby side is necessary and **not sufficient**. This testbed
scored 7 of 7 and still missed the worst bug the project has shipped: the
flagship one-line rename silently declined every method whose body assigned a
local variable, because Prism carries a scope's local table on the node and rwr
compared it as though it were syntax (D73).

It missed it because `Account#display_name` was written as a single expression —
`"#{first} #{last}"`. Every site around it was enumerated from Ruby, correctly,
and the *definition itself* was unrepresentative. A method with a variable in it
is too ordinary to occur to anyone as an edge case, which is exactly why nobody
wrote one.

So the rule has two halves:

1. Derive expectations from **Ruby semantics**, never from what rwr does.
2. Make the Ruby **representative**. Boring, ubiquitous constructs earn their
   place here as much as gnarly ones — more, because nobody thinks to test them.

When adding a case, ask what a reviewer would say if they saw it in a real pull
request. If the answer is "nobody writes code that simple", the case is not
pulling its weight.

## What is in it

Every file starts with the question it exists to answer, because a corpus file
with no stated purpose rots. The material is deliberately **ordinary**: the
seven bugs found on the day this corpus was extended were a method body that
assigned a local, a comment above the first statement, a reason after a
directive, the second rule in a list, and a template that parsed. Not one was
exotic. So what is here is what a ten-year-old Rails monolith actually holds.

| area | files |
|---|---|
| the rename itself | `app/models/account.rb`, `premium_account.rb`, `archived_account.rb` |
| an unrelated class sharing the name | `app/models/company.rb` |
| namespaced classes, nested and compact | `app/models/account/row.rb`, `account/exporter.rb` |
| a concern's contribution to its includer | `app/models/concerns/nameable.rb` |
| dynamic dispatch at both ends | `app/models/account_presenter.rb` |
| Rails DSLs taking symbols | `app/serializers/account_serializer.rb` |
| ordinary controller and job control flow | `app/controllers/`, `app/jobs/` |
| heredocs, all four flavours | `lib/reports/account_report.rb` |
| reopened classes, `class << self`, `class_eval` | `lib/account_ext.rb` |
| owners that are not the enclosing class: `class ::Account`, `class << Account`, `def Account.x` | `lib/account_owners.rb` |
| `prepend` and `refine` | `lib/account_patches.rb` |
| a legacy script: shebang, encoding, multibyte, `__END__` | `script/backfill_names.rb` |
| templates, stitched and not | `app/views/accounts/` |
| the spec suite | `spec/` |
| a `.rake` file and a YAML config | `lib/tasks/`, `config/` |

Two pairs are worth reading side by side, because each is one piece of Ruby
written two ways with the same meaning, or two pieces written the same way with
opposite meanings:

- `account/row.rb` and `account/exporter.rb` declare the same kind of class in
  the nested and the compact form. Ruby cannot tell them apart and neither may
  a rename.
- `Field.new(:display_name, ...)` and `Struct.new(:display_name, ...)` in
  `account_presenter.rb` are identical syntax. The first hands a method name to
  something that will dispatch on it; the second *defines* a method of that name
  on a different class.

Where a site lives inside a heredoc or a template body, the marker sits on the
opening line instead -- a heredoc body has nowhere to put a Ruby comment. One
marker per line: the scorer reads the first `GT:` on a line and stops.

## What it cannot measure

**Precision at scale.** A fixture its own author wrote proves nothing about
whether a report is screen-filling on a million lines. That half of Q1 is
measured against discourse and mastodon, and recorded in
`docs/internal/phase0-results.md`.
