# frozen_string_literal: true

# The class the rename is about. Everything here either changes or breaks.
class Account
  # GT:rewrite -- the definition itself.
  #
  # Deliberately more than one statement, and it assigns a local. Prism carries
  # a scope's local table on the node, and treating that as syntax meant a
  # pattern matched only bodies whose locals were identical to its own -- so
  # this definition was renamed while it was a one-liner and silently declined
  # the moment anyone gave it a variable.
  def display_name
    given = first
    family = last
    "#{given} #{family}"
  end

  def greeting
    # GT:rewrite -- implicit self, 43.5% of call sites in rails
    "Hello #{display_name}"
  end

  def formal
    # GT:rewrite -- explicit self
    self.display_name.upcase
  end

  def label
    # GT:blind -- Symbol#to_proc; the symbol is the method name
    [self].map(&:display_name).first
  end

  def dynamic(suffix)
    # GT:blind -- interpolated, so the name never appears whole. Nothing can
    # resolve this without running the program, and rwr must not pretend to.
    send("display_#{suffix}")
  end

  def guarded
    # GT:residue -- a symbol reaching a reflective call
    respond_to?(:display_name) ? send(:display_name) : nil
  end

  # GT:residue -- the classic metaprogramming reach
  alias_method :name_for_display, :display_name

  def documented
    # A comment mentioning display_name is not code, and must never be reported.
    thing = "display_name"                   # GT:ignore -- a string is not code
    <<~TEXT                                  # GT:ignore -- a heredoc body is not code
      display_name
    TEXT
  end
end
