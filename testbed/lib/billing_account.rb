# frozen_string_literal: true

# A second class called Account, in a namespace of its own.
#
# The testbed had namespaced *neighbours* -- `Admin::AccountsController`,
# `Account::Row` -- but no namespaced class sharing the target's name, so the
# one question it could not ask was the one that matters most: 84% of rails
# classes are namespaced, and `Billing::Account` is a different class from
# `Account` in every way except the last word of its name.
#
# Nothing here may move when `Account#display_name` is renamed. Its own
# `display_name` is a different method; so is the one on the subclass below,
# which descends from *this* Account and not from the top-level one. Before
# D100 the hierarchy kept only the last segment of every class name, so the
# subclass answered to both and the rename rewrote it.
module Billing
  class Account
    # GT:ignore -- Billing::Account is not Account
    def display_name
      "#{number} (billing)"
    end

    def number
      @number
    end
  end

  # A subclass of the *namespaced* Account. The superclass is written out, which
  # is what made the two indistinguishable: `Billing::Account` and `Account`
  # have the same last segment and nothing else in common.
  class Statement < Account
    # GT:ignore -- an override of Billing::Account#display_name
    def display_name
      "statement for #{super}"
    end

    def header
      display_name.upcase # GT:ignore -- implicit self on Billing::Statement
    end
  end
end
