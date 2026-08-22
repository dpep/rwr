# frozen_string_literal: true

# Does a rename reach the code a concern *contributes* to the class it is mixed
# into? A concern's body is Account's code living in another file: `included do`
# runs in Account, `ClassMethods` lands on Account, and an implicit-self call in
# an instance method here dispatches on an Account. None of it is lexically
# inside `class Account`, and no `class X < Y` line ever names the connection.
module Nameable
  extend ActiveSupport::Concern

  included do
    # GT:residue -- a symbol handed to a macro, in the including class
    validates :display_name, presence: true

    # GT:residue -- a symbol reaching a query builder through a lambda
    scope :by_name, -> { order(:display_name) }

    # Metaprogramming that *defines* rather than calls. In an unrelated class a
    # definer makes that class's own method and is no reach at all; mixed into
    # Account it makes Account's, and a rename that leaves the block behind
    # quietly does nothing -- the old name still answers.
    define_method(:display_name_prefix) { display_name.to_s[0, 3] } # GT:residue
  end

  module ClassMethods
    # The attribute the admin index sorts by. Callers `public_send` it.
    def label_method
      :display_name # GT:residue -- a bare symbol, returned for later dispatch
    end
  end

  # An ordinary instance method on the including class, calling Account's method
  # with no receiver at all.
  def name_html
    # GT:residue -- implicit self, but the enclosing lexical scope is a module
    return "" if display_name.blank?

    ERB::Util.html_escape(display_name) # GT:residue -- same, one line later
  end

  # A method whose name merely *starts* with the one being renamed. Legacy
  # models are full of these, and not one of them breaks.
  def display_name_length # GT:ignore
    name_html.length
  end
end
