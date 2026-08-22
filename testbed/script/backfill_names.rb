#!/usr/bin/env ruby
# encoding: utf-8
# frozen_string_literal: true

# The one-off script every monolith accumulates, and the four things that make
# one awkward to read mechanically: a shebang, a redundant encoding comment,
# multibyte characters ahead of the first site -- which put every byte offset
# out of step with every character offset -- and a `__END__` marker, after which
# the file stops being Ruby at all.
#
# `END { }` is here because it is the one construct whose body runs after the
# program does, and it is still ordinary Ruby the parser has to place.

require_relative "../app/models/account"

# The “name” column was renamed in the 2018 migration — twice, in fact — so the
# CSV below still carries the old header. None of that prose is code.
FIELDS = %w[id name email].freeze

def report(account)
  # GT:residue -- a parameter receiver, well past the multibyte prose above
  $stdout.puts(account.display_name)
end

count = FIELDS.size \
        + DATA.each_line.count

account = Account.new
# GT:rewrite -- a local assigned from a constructor, well past the prose above
$stdout.puts(account.display_name)

report(account)

END { $stdout.puts("backfilled #{count} rows") }

__END__
# GT:ignore -- everything past __END__ is data, and data is not code
id,display_name,email
1,Widget,widget@example.test
