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

## Running it by hand

The same script works against any pull request, from anywhere:

```sh
script/pr-suggest.sh 2                                   # this repo
script/pr-suggest.sh https://github.com/dpep/rwr/pull/2  # any repo, by URL
script/pr-suggest.sh dpep/rwr#2 performance app/         # rule and path, as `rwr check` takes them
```

Given a URL or `owner/repo#N` it fetches its own clone into a temp directory, so
the repo need not be checked out. It does need the *source*: rwr matches
structurally, so it wants a parse tree, and a diff hunk is not parseable Ruby.

The clone is blobless rather than shallow — `--depth 1` has no merge base, so
`--since` would have nothing to diff against.

Run locally it posts as whoever `gh` is authenticated as; in Actions it is
`github-actions[bot]`. And it posts one *review* containing every comment, so if
a single comment falls outside the diff GitHub rejects the whole review — the
script prints GitHub's error rather than failing silently.

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

**What `--since` scopes, and what it does not.** It scopes the *sites*, by the
bytes a rule would write rather than by the span it matched — so a pull request
that edits a method body does not inherit a finding about its signature. The
exit code follows those sites, so that is what gates the build.

It does **not** scope the residue: the account of what rwr could not tie to the
rule is computed over each whole file it read, because an occurrence it could
not resolve has no reliable relationship to the lines your branch touched. That
account never fails the build on its own — residue does not move the exit code —
so a red build always points at a site, and the residue beside it is context.

And where a site spans lines the change did not touch, the run says so: one line
per site on stderr and `wrote_beyond_scope` in `-j`. A site is rewritten whole or
not at all.

## Why not Code Scanning

rwr emits no SARIF, so there is nothing for
`github/codeql-action/upload-sarif` to ingest. It did once, and that path was
tried first. Three things it does worse than a review comment:

- Every SARIF upload is attributed to **GitHub Advanced Security**, which is not
  renameable and overstates what these findings are.
- Its annotations cannot carry a suggestion, so a rule that knows the fix can
  only describe it.
- Comments it leaves **cannot be deleted**, even by a repository admin.

If you want rwr's findings in Code Scanning anyway, `rwr check -j` carries
everything the SARIF run did — file, line, column, rule and description, plus
the residue and unread-file account SARIF never modelled well — and
`script/pr-suggest.sh` is a worked example of reading it.
