#!/usr/bin/env bash
# Gate-gated commit: runs the full gate and commits only if it passes.
#
# This exists because the obvious one-liner is wrong in two ways that both look
# right. `check.sh; git commit` ignores the gate entirely, and
# `check.sh | grep ... && git commit` masks it behind grep's exit status --
# which is how two commits went out red before this script existed.
#
#   script/commit.sh -m "subject"        # message inline
#   script/commit.sh -F -                # message on stdin
set -euo pipefail
cd "$(dirname "$0")/.."

log=$(mktemp)
if ! CARGO="${CARGO:-cargo}" ./script/check.sh >"$log" 2>&1; then
  echo "gate is red — not committing" >&2
  grep -E 'panicked|^error|FAILED' -A4 "$log" | head -30 >&2
  rm -f "$log"
  exit 1
fi
rm -f "$log"

git add -A
git -c user.name="Daniel Pepper" -c user.email="pepper.daniel@gmail.com" commit -q "$@"
echo "committed on $(git rev-parse --abbrev-ref HEAD): $(git log -1 --format=%s)"
