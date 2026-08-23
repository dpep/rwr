#!/usr/bin/env bash
# Turn `rwr check -j` into inline pull-request review comments.
#
# Deliberately a script rather than a subcommand. Fetching a pull request and
# posting comments is network, auth, and GitHub-specific shapes; rwr is a single
# static binary with no daemon and no state, and it should stay one. It emits
# facts -- where a site is and what those lines become -- and this turns them
# into something a reviewer can act on.
#
# Inline only, and scoped to the diff. A review is about what *this* change
# introduced; the full account of a rename lives in the terminal and in `-j`,
# where a refactor actually reads it. And a summary comment restating what is
# already visible inline is what makes a bot easy to mute.
#
#   script/pr-suggest.sh <pr-number> [rule] [path]      # this repo
#   script/pr-suggest.sh <pr-url>     [rule] [path]      # any repo
#   script/pr-suggest.sh owner/repo#7 [rule] [path]      # any repo
#
# Needs `gh` authenticated. Given a URL or `owner/repo#N` it fetches its own
# blobless clone of that pull request, so you do not need the repo checked out
# -- but it does need the *source*, because rwr parses whole files and cannot
# work from a diff hunk.
set -euo pipefail

TARGET_PR="${1:?usage: pr-suggest.sh <pr-number|pr-url> [rule] [path]}"
RULE="${2:-all}"
TARGET="${3:-.}"
RWR="${RWR:-rwr}"

# A bare number means the repo you are standing in; a URL means any repo, and
# this fetches its own copy.
#
# rwr parses whole files -- structural matching needs an AST, not a diff hunk --
# so a checkout is not an implementation detail that could be avoided by asking
# the API harder. What *is* avoidable is making you clone by hand.
case "$TARGET_PR" in
  *github.com*)
    repo=$(printf '%s' "$TARGET_PR" | sed -E 's#.*github\.com/([^/]+/[^/]+)/pull/[0-9]+.*#\1#')
    PR=$(printf '%s' "$TARGET_PR" | sed -E 's#.*/pull/([0-9]+).*#\1#')
    ;;
  */*\#*)
    repo="${TARGET_PR%%\#*}"
    PR="${TARGET_PR##*\#}"
    ;;
  *)
    repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
    PR="$TARGET_PR"
    ;;
esac

base=$(gh pr view "$PR" --repo "$repo" --json baseRefName --jq .baseRefName)
head_sha=$(gh pr view "$PR" --repo "$repo" --json headRefOid --jq .headRefOid)

work=""
if [ "$repo" != "$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null || true)" ]; then
  # Blobless rather than shallow: `--since` needs the merge base, which a
  # `--depth 1` clone does not have, and blobless still skips most of the
  # download.
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  echo "fetching $repo#$PR into $work" >&2
  git clone --quiet --filter=blob:none "https://github.com/$repo.git" "$work"
  git -C "$work" fetch --quiet origin "pull/$PR/head"
  git -C "$work" checkout --quiet FETCH_HEAD
  cd "$work"
fi

# `check` exits 1 when there is work to do, which is the whole point here.
"$RWR" check "$RULE" "$TARGET" --since "origin/$base" -j > /tmp/rwr-report.json || true

python3 - "$repo" "$PR" "$head_sha" <<'PY'
import json, subprocess, sys

repo, pr, sha = sys.argv[1], sys.argv[2], sys.argv[3]
report = json.load(open("/tmp/rwr-report.json"))
DART = "\U0001f3af"

comments = []


def at(entry, body):
    return {
        "path": entry["file"].removeprefix("./"),
        "line": entry["line"],
        "side": "RIGHT",
        "body": body,
    }


# A rule with a fix: an applicable suggestion, framed as what it is. None of
# this is broken, and calling it a violation earns a reviewer's scepticism
# rather than their attention.
for changed in report.get("changed", []):
    for site in changed.get("at", []):
        body = f"{DART} " + (site.get("note") or "This can be simplified.")
        rule = site.get("rule")
        if rule:
            body += f"\n\n<sub>`{rule}` · apply here, or run `rwr rewrite {rule}` locally</sub>"
        body += "\n\n```suggestion\n" + site["replacement"] + "\n```"
        comment = at(site, body)
        # A multi-line suggestion replaces a range, and GitHub wants both ends.
        if site["end_line"] > site["line"]:
            comment["start_line"] = site["line"]
            comment["start_side"] = "RIGHT"
        comments.append(comment)

# A rule with no fix: a finding, or an occurrence a rename could not convert.
# Nothing to suggest, so it is a plain comment saying what it is -- and these
# are often the ones a reviewer most needs, which no suggestion block can carry.
for f in report.get("findings", []):
    body = f"{DART} " + (f.get("note") or "Worth a look.")
    if f.get("rule"):
        body += f"\n\n<sub>`{f['rule']}` · no automatic fix; this one needs a decision</sub>"
    comments.append(at(f, body))

for r in report.get("residue") or []:
    body = (
        f"{DART} A rename could not account for this "
        f"(`{r.get('context', 'occurrence')}`), so it may still name the old method."
    )
    comments.append(at(r, body))

if not comments:
    print("nothing to suggest")
    raise SystemExit(0)

review = {
    "commit_id": sha,
    "event": "COMMENT",
    # No summary body: the comments are the message.
    "body": "",
    "comments": comments,
}
proc = subprocess.run(
    ["gh", "api", f"repos/{repo}/pulls/{pr}/reviews", "--method", "POST", "--input", "-"],
    input=json.dumps(review),
    text=True,
    capture_output=True,
)
if proc.returncode != 0:
    # The commonest cause is a comment outside the diff, which GitHub rejects
    # for the whole review rather than per comment. Say so rather than failing
    # opaquely.
    sys.stderr.write(proc.stderr)
    raise SystemExit(1)
print(f"posted {len(comments)} comment(s)")
PY
