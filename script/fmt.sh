#!/usr/bin/env bash
# Format, and say which files it reflowed.
#
# `cargo fmt` rewrites silently. An edit anchored on text read *before* it stops
# matching after it, and the failure surfaces later as an edit that mysteriously
# does not apply -- three sessions have lost a round trip to exactly that. The
# invalidation is unavoidable; being unable to see it is not.
#
# `--check` reports `Diff in <path>:<line>:` per hunk and writes nothing, so ask
# what will move before moving it.
set -euo pipefail
cd "$(dirname "$0")/.."
CARGO="${CARGO:-cargo}"

pending="$("$CARGO" fmt --check 2>/dev/null |
  sed -n 's/^Diff in \(.*\):[0-9][0-9]*:$/\1/p' | sort -u || true)"

"$CARGO" fmt

if [ -z "$pending" ]; then
  echo "fmt: already formatted"
  exit 0
fi
echo "fmt: reflowed —"
printf '%s\n' "$pending" | sed "s|^$PWD/||; s/^/  /"
echo "  anchors read from these before now are stale; re-read before editing them"
