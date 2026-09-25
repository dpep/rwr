# frozen_string_literal: true

# Does a rename reach an override whose body is *empty*?
#
# Prism gives `def display_name; end` a nil body, so the definition pattern's
# `$B` had nothing to bind and the rename matched every call site and not the
# definition (D121). The empty body is the only variable here: arity drift lives
# in `archived_account.rb` and a `rescue` body in `lib/account_ext.rb`, both of
# which fail for their own reasons.
#
# An override stubbed out this way is the shape that hides the defect best. It
# does not raise when it is left behind -- it simply stops overriding, because
# callers now ask the parent for `full_name` and get the implementation this
# class exists to suppress. A silent revert is worse than a `NoMethodError`,
# which at least announces itself.
class StubAccount < Account
  # GT:rewrite -- one line, no parameters: the reported shape
  def display_name; end
end

# The same method spelled over two lines, with a parameter list. Prism gives
# both a nil body, and the parameter list is irrelevant -- a zero-arity `def`
# and `def x(a); end` failed identically -- so a fix that handled only the
# one-liner would look right here.
class QuietAccount < Account
  # GT:rewrite -- two lines, and an argument nobody passes
  def display_name(_format = nil)
  end
end
