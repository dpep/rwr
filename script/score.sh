#!/usr/bin/env bash
# Score each corpus entry against every competitor that can express it.
#
# Scoring is output equality: a tool that turns in/x.rb into out/x.rb found
# exactly the right sites. A tool with no competitors/<tool>.sh for an entry is
# recorded as inexpressible, which is a result rather than a gap.
set -uo pipefail
cd "$(dirname "$0")/.."

TOOLS=(ast-grep comby)
printf '%-22s %-16s %-12s %s\n' ENTRY FIXTURE TOOL RESULT
printf '%.0s-' {1..64}; echo

for entry in corpus/[0-9]*/; do
  name=$(basename "$entry")
  for input in "$entry"in/*.rb; do
    [ -e "$input" ] || continue
    fixture=$(basename "$input")
    expected="$entry/out/$fixture"
    [ -e "$expected" ] || continue   # refusal fixtures are scored by rwr only

    for tool in "${TOOLS[@]}"; do
      runner="$entry/competitors/$tool.sh"
      if [ ! -x "$runner" ]; then
        printf '%-22s %-16s %-12s %s\n' "$name" "$fixture" "$tool" "inexpressible"
        continue
      fi
      if ! command -v "$tool" >/dev/null 2>&1; then
        printf '%-22s %-16s %-12s %s\n' "$name" "$fixture" "$tool" "not installed"
        continue
      fi
      tmp=$(mktemp -d)
      cp "$input" "$tmp/$fixture"
      "$runner" "$tmp/$fixture" >/dev/null 2>&1
      if diff -q "$tmp/$fixture" "$expected" >/dev/null 2>&1; then
        result=match
      elif diff -q "$tmp/$fixture" "$input" >/dev/null 2>&1; then
        result="no-change"
      else
        result=WRONG
      fi
      printf '%-22s %-16s %-12s %s\n' "$name" "$fixture" "$tool" "$result"
      rm -rf "$tmp"
    done
  done
done
