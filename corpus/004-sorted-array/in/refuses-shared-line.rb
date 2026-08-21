# frozen_string_literal: true

# The comment shares a line with two elements, so it has no unambiguous owner.
# D35 says refuse rather than reattach it to a neighbour.
PERMISSIONS = [
  :zebra, :apple, # which of these does this describe?
  :mango,
].freeze
