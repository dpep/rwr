# rwr in GitHub Actions

`rwr` emits SARIF, and `github/codeql-action/upload-sarif` turns SARIF into
annotations on the pull request. That is the entire integration — no app to
install, no token beyond the one Actions already gives you.

## Annotate a pull request

```yaml
name: rwr
on: pull_request

permissions:
  contents: read
  security-events: write   # required by upload-sarif

jobs:
  rwr:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0   # --since needs the base branch, not a shallow clone

      - run: cargo install rwr

      - name: Check what this branch introduces
        run: rwr check all --since "origin/$GITHUB_BASE_REF" --sarif > rwr.sarif
        continue-on-error: true   # findings are results, not a failed step

      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: rwr.sarif
```

Three details that decide whether this works:

**`fetch-depth: 0`.** `actions/checkout` clones one branch by default, so
`origin/main` does not exist and `--since` has nothing to diff against.

**`--since "$GITHUB_BASE_REF"`, not a hardcoded branch.** A pull request
targeting a release branch has a base that is not `main`, and Actions already
knows which. This is also why rwr does not try to infer a default branch.

**`continue-on-error: true`.** `check` exits 1 when there is work to do — that
is the polarity that makes it usable as a gate — so without this the step fails
before the upload runs and no annotation ever appears.

## Fail the build instead

If you want findings to block rather than annotate, drop the SARIF upload and
let the exit code do its job:

```yaml
      - run: rwr check all --since "origin/$GITHUB_BASE_REF"
```

Exit 1 means there is work to do. The two are not exclusive — upload the SARIF
for annotations *and* keep a second step that fails — but decide which one is
the gate, because a build that is red for advisory findings gets ignored.

## What the levels mean

| level | what it is |
|---|---|
| `warning` | a site a rule would rewrite, or a lint finding — actionable |
| `note` | residue: an occurrence rwr could **not** account for, which needs a human to judge |

Residue is deliberately not a `warning`. It is not a defect in your code; it is
rwr saying what it could not reach. A rename that reports `delegate :old_name`
has told you the one thing that will break, and grading that the same as "this
line can be auto-fixed" would train people to skim past both.

Blind spots with no line to point at — a file that would not parse, a template
that could only be text-searched — arrive as SARIF
`toolExecutionNotifications` rather than results, because giving them an
invented location would be inventing evidence.

## Fixing rather than reporting

`rwr rewrite` writes to disk, so a job can open a pull request with the fixes
instead of annotating:

```yaml
      - run: rwr rewrite all --since "origin/$GITHUB_BASE_REF"
      - uses: peter-evans/create-pull-request@v6
```

Read the residue report before trusting an automated rewrite PR: the account of
what rwr could *not* convert is the part that decides whether the change is
safe, and it is the part a diff does not show you.
