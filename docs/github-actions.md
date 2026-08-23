# rwr on a pull request

The default is **inline review comments**: an applicable suggestion where a rule
can fix what it found, a plain comment where it cannot. Posted by
`github-actions[bot]` through the ordinary reviews API — no Code Scanning, no
security alerts, no entry in the Security tab.

That is deliberate. Nothing rwr finds is a vulnerability, and filing a
`return nil` simplification as a security event earns a reviewer's scepticism
rather than their attention.

## Suggest simplifications

```yaml
name: rwr
on: pull_request

permissions:
  contents: read
  pull-requests: write

jobs:
  rwr:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0   # --since needs the base branch, not a shallow clone
          ref: ${{ github.head_ref }}

      - run: cargo install rwr

      - env:
          GH_TOKEN: ${{ github.token }}
        run: script/pr-suggest.sh ${{ github.event.pull_request.number }}
```

Two details decide whether this works, and both fail quietly:

**`fetch-depth: 0`.** `actions/checkout` clones one branch, so `origin/main` does
not exist and `--since` has nothing to diff against.

**`pull-requests: write`.** Without it the review POST is rejected and the step
fails after rwr has already done its work.

## What gets commented

| what rwr found | comment |
|---|---|
| a site a rule can rewrite | the rule's description, plus an applicable ` ```suggestion ` block |
| a finding rule's match | the description, and a note that it needs a decision rather than a fix |
| residue inside the diff | that a rename could not account for it, so it may still name the old method |

**Inline only, and scoped to the diff.** A review is about what *this* change
introduced. Residue on lines nobody touched is pre-existing, and GitHub rejects
review comments outside the diff anyway — the full account of a rename lives in
the terminal and in `-j`, which is where a refactor reads it.

There is deliberately no summary comment. A preamble restating what is already
visible inline is what makes a bot easy to mute.

## Applying instead of suggesting

`rwr rewrite` writes to disk, so a job can push the fixes:

```yaml
      - run: rwr rewrite all --since "origin/$GITHUB_BASE_REF"
      - uses: peter-evans/create-pull-request@v6
```

Worth keeping opt-in rather than automatic. An applied rewrite is one rwr could
*prove* — but the residue report is the part that says where it could **not**
reach, and a push nobody reads is exactly how that gets skipped.

## Failing the build

If findings should block rather than annotate, the exit code already says so:

```yaml
      - run: rwr check all --since "origin/$GITHUB_BASE_REF"
```

Exit 1 means there is work to do. Decide which of the two is the gate, though —
a build that goes red for advisory findings gets ignored.

## SARIF, if you want Code Scanning

`--sarif` emits SARIF 2.1.0, which `github/codeql-action/upload-sarif` ingests:

```yaml
      - run: rwr check all --since "origin/$GITHUB_BASE_REF" --sarif > rwr.sarif
        continue-on-error: true   # check exits 1 when there is work to do
      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: rwr.sarif
          category: rwr
```

This is **not** the recommended path for pull-request review, and it was tried
first. Three things it does worse:

- Every SARIF upload is attributed to **GitHub Advanced Security**, which is not
  renameable and overstates what these findings are.
- Its annotations cannot carry a suggestion, so a rule that knows the fix can
  only describe it.
- Comments it leaves **cannot be deleted**, even by a repository admin.

Where it does earn its place is outside pull requests: it is a standard other
tools ingest, and Code Scanning tracks alerts, dismissals and branch state over
time in a way an ephemeral review comment cannot.

Levels, if you use it: a rewritable site or a finding is `warning`; residue is
`note`, because it is not a defect in your code but a thing rwr could not reach.
Blind spots with no line to point at — a file that would not parse — arrive as
`toolExecutionNotifications` rather than results, since giving them an invented
location would be inventing evidence.
