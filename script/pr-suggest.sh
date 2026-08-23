#!/usr/bin/env bash
# Turn `rwr check -j` into GitHub review comments carrying applicable suggestions.
#
# Deliberately a script rather than a subcommand. Fetching a pull request and
# posting review comments is network, auth, and GitHub-specific shapes; rwr is a
# single static binary with no daemon and no state, and it should stay one. It
# emits facts -- where a site is and what those lines become -- and this turns
# them into something a reviewer can click.
#
#   script/pr-suggest.sh <pr-number> [rule] [path]
#
# Needs `gh` authenticated, and to be run inside the repo.
set -euo pipefail

PR="${1:?usage: pr-suggest.sh <pr-number> [rule] [path]}"
RULE="${2:-all}"
TARGET="${3:-.}"
RWR="${RWR:-rwr}"

repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
head_sha=$(gh pr view "$PR" --json headRefOid --jq .headRefOid)

# Only lines this pull request touched. A suggestion on a line the author did
# not write is noise, and GitHub rejects a comment outside the diff anyway.
base=$(gh pr view "$PR" --json baseRefName --jq .baseRefName)

report=$("$RWR" check "$RULE" "$TARGET" --since "origin/$base" -j || true)

echo "$report" | python3 -c '
import json, os, subprocess, sys

report = json.load(sys.stdin)
repo, pr, sha = sys.argv[1], sys.argv[2], sys.argv[3]

comments = []
for changed in report.get("changed", []):
    for site in changed.get("at", []):
        # Framed as a simplification, because that is what it is. None of this
        # is broken, and calling it a violation earns the reviewer\'s
        # scepticism rather than their attention.
        body = "🎯 " + (site.get("note") or "This can be simplified.")
        rule = site.get("rule")
        if rule:
            body += f"\n\n<sub>`{rule}` · apply here, or run `rwr rewrite {rule}` locally</sub>"
        body += "\n\n```suggestion\n" + site["replacement"] + "\n```"
        comment = {
            "path": site["file"].removeprefix("./"),
            "line": site["end_line"],
            "side": "RIGHT",
            "body": body,
        }
        # A multi-line suggestion replaces a range, and GitHub wants both ends.
        if site["end_line"] > site["line"]:
            comment["start_line"] = site["line"]
            comment["start_side"] = "RIGHT"
        comments.append(comment)

if not comments:
    print("nothing to suggest")
    sys.exit(0)

review = {
    "commit_id": sha,
    "event": "COMMENT",
    "body": (
        f"🎯 **rwr found {len(comments)} simplification(s).**\n\n"
        "Each is applicable in place — nothing here is broken, so take them or "
        "leave them. They are the kind of change that is easy to make now and "
        "annoying to make later."
    ),
    "comments": comments,
}
proc = subprocess.run(
    ["gh", "api", f"repos/{repo}/pulls/{pr}/reviews", "--method", "POST", "--input", "-"],
    input=json.dumps(review), text=True, capture_output=True,
)
if proc.returncode != 0:
    # The commonest cause is a comment outside the diff, which GitHub rejects
    # for the whole review rather than per comment. Say so rather than failing
    # opaquely.
    sys.stderr.write(proc.stderr)
    sys.exit(1)
print(f"posted {len(comments)} suggestion(s)")
' "$repo" "$PR" "$head_sha"
