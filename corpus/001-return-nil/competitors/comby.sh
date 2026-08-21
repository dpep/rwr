#!/usr/bin/env bash
exec comby 'return nil' 'return' .rb -in-place "$1"
