# frozen_string_literal: true

# Modules whose instance methods answer on the module itself.
#
# `extend self` and `module_function` differ in visibility and not in the thing
# that matters here: one name lands on both method tables, so `Util.foo` and
# `Util#foo` are one method rather than two. Treating `kind:` as decisive there
# made a rename do half its job -- rewriting the definition and filing every
# call as residue, or the reverse -- and the report looked complete either way.
# The semantics are pinned in `hierarchy::tests`; what this file guards is that
# such a module's own methods are not mistaken for Account's.
#
# `module_function` had a second failure underneath, and it is the reason this
# file exists rather than a unit test alone: a module using it contains no
# `class`, no `<` and no mixin keyword, so the hierarchy's pre-filter dropped
# the whole file before parsing it. The collector was correct and never ran.

module AccountFormatting
  extend self

  # GT:ignore -- AccountFormatting's own method. It shares only the word, and
  # `extend self` putting it on both tables does not make it Account's.
  def display_name(account)
    # A real reach on an unresolved receiver, from inside a self-extending
    # module.
    # GT:residue
    account.display_name.upcase
  end
end

module AccountNaming
  module_function

  def shout(account)
    # The same reach, from a file the pre-filter used to skip entirely.
    # Reported now only because the file is read at all.
    # GT:residue
    account.display_name.upcase
  end
end
