# frozen_string_literal: true

# Definitions whose owner is *not* the class they are written inside.
#
# Ruby decides the owner from the receiver; lexical nesting only supplies a
# namespace. Reading the owner off the nesting is wrong in three separate ways
# and each one is silent -- the run completes, the count looks right, and the
# wrong methods moved. Shopify's rubydex `docs/ruby-behaviors.md` catalogues the
# family; this testbed had `class << self` and none of the rest.

module Billing
  # `class ::Account` is the **top-level** Account. `::` resets the namespace,
  # so this reopens the class the rename is about. It is not
  # `Billing::Account`, which does not exist -- and calling it that lost the
  # call below entirely.
  class ::Account
    def owner_reset_reach
      # GT:rewrite -- implicit self on the real Account, through a rooted reopening.
      display_name
    end
  end

  # An ordinary nested class for contrast: this one really is
  # `Billing::Statement`, and its `display_name` is its own.
  class Statement
    def display_name
      # GT:ignore -- a different class that merely shares the word.
      "statement"
    end
  end
end

# A singleton class opened on *another* object, from inside an unrelated class.
class Ledger
  class << Account
    # This body belongs to Account's singleton, so it defines
    # `Account.display_name` -- a class method, which the *instance* rename does
    # not break. Attributing the body to `Ledger` was the older bug: under a
    # Ledger rename it was a wrong rewrite, and under this one it vanished.
    #
    # GT:notice -- same class, other method table; reported as the near-miss it is.
    def display_name
      "class method on Account"
    end
  end
end

# `def Foo.bar` written inside another class: same rule, different syntax.
class Reconciler
  # Defines `Account.display_name`, not `Reconciler`'s. rwr cannot rewrite a
  # definition with an explicit constant receiver, so the most it can do is say
  # so -- and it must file it under Account, or this rename never sees it.
  #
  # GT:notice -- same class, other method table.
  def Account.display_name
    "another class method"
  end
end
