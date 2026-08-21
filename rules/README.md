# The shipped rule pack

Point `rwr` at this directory, or at one subdirectory of it:

```sh
rwr check rules app/            # everything, read-only
rwr check rules/performance     # one family
rwr rewrite rules/style app/    # apply
```

A pack is a directory because that is how a subset gets turned on. Rules apply
in path order, each seeing the previous one's output, and the run reports which
rule accounted for what — "27 sites changed" across five rules is not a
reviewable answer.

Every rule's id defaults to its path within the pack, so
`rules/performance/detect.yml` reports as `performance/detect`. A rule may
override that with an `id:` key.

## What is not here, and why

**Nothing that changes behaviour silently.** `map { }.compact` →
`filter_map { }` is the clearest omission: `compact` drops `nil`, `filter_map`
drops `nil` *and* `false`, so the rewrite is wrong for any block that can return
`false`. RuboCop ships it as an unsafe autocorrect. rwr's contract is that a
match is a fact, so it stays out until a `where:` predicate can rule the false
case out.

**Nothing about layout.** Indentation, alignment and trailing commas are
presentation, and rwr does not own style (D34, D53).

## The ActiveRecord caveat on `performance/*`

`select` on an `ActiveRecord::Relation` names columns; on an Array it filters
elements. The perf rules assume the second. This is the case receiver narrowing
exists for — add a `type:` constraint on `$R` when running these over a Rails
app, and read the residue report for the sites rwr could not resolve.
