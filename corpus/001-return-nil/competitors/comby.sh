#!/usr/bin/env bash
# -f targets exactly one file. Passing a bare extension instead makes comby
# recurse the working directory, which once rewrote the corpus in place.
exec comby 'return nil' 'return' -f "$1" -in-place
