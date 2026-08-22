# frozen_string_literal: true

# Two ways to redefine a method without ever writing `class Account`, both of
# which a monolith reaches for once it stops being willing to reopen the model.
#
# `prepend` is the ordinary one: the module sits *in front of* Account in the
# ancestor chain, so its `display_name` is the one that runs and its `super` is
# Account's. Rename Account's method and this override stops overriding -- it
# quietly becomes a new method, and `super` raises. `refine` is the rare one and
# fails the same way. Neither module names Account in a `class X < Y` line, and
# neither definition is lexically inside a class at all.
module AccountAudit
  # GT:residue -- an override that a rename turns into a stray new method
  def display_name
    Audit.record(:read, self)
    super
  end
end

module AccountRefinements
  refine Account do
    # GT:residue -- refined, which is an override with a smaller blast radius
    def display_name
      super.upcase
    end
  end
end

Account.prepend(AccountAudit)
