#!/usr/bin/env bash
# Pre-push gate: format, lint, test. Stops at the first failure.
set -euo pipefail
CARGO="${CARGO:-cargo}"
"$CARGO" fmt --check
"$CARGO" clippy --all-targets -- -D warnings
"$CARGO" test
echo "check: ok"
