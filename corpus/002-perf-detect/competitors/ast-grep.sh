#!/usr/bin/env bash
# ast-grep has no method-name alternation in a bare pattern, so select and
# find_all need separate passes -- which is itself a finding worth recording.
ast-grep run --lang ruby --pattern '$R.select { $$$B }.first' --rewrite '$R.detect { $$$B }' --update-all "$1"
exec ast-grep run --lang ruby --pattern '$R.select do $$$B end.first' --rewrite '$R.detect do $$$B end' --update-all "$1"
