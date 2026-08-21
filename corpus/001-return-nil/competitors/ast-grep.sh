#!/usr/bin/env bash
# $1 is a writable copy of the input; rewrite it in place.
exec ast-grep run --lang ruby --pattern 'return nil' --rewrite 'return' --update-all "$1"
