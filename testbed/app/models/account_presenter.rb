# frozen_string_literal: true

# What does a rename find at the two ends of the dynamic-dispatch spectrum, in
# the class legacy Rails uses for it? One end is a literal symbol sitting in a
# table, plainly visible and plainly a reach. The other is a name that only
# exists while the program runs. Both break; only one is reportable, and the
# report has to be honest about which is which.
#
# The pair worth staring at is `Field.new(:display_name, ...)` and
# `Struct.new(:display_name, ...)`: identical syntax, opposite meanings. One
# hands a method name to something that will dispatch on it; the other *defines*
# a method of that name on a different class entirely.
class AccountPresenter < SimpleDelegator
  Field = Struct.new(:key, :label)

  # Struct members define this class's own readers; they reach nothing.
  Summary = Struct.new(:display_name, :email, keyword_init: true) # GT:ignore

  SAFE_FIELDS = [
    Field.new(:display_name, "Name"), # GT:residue -- a method name in a table
    Field.new(:email, "Email"),
  ].freeze

  def initialize(account, view: nil, **options)
    super(account)
    @view = view
    @options = options
  end

  def rows
    SAFE_FIELDS
      .reject { |field| @options[:except].to_a.include?(field.key) }
      .map { |field| [field.label, __getobj__.public_send(field.key)] }
      .to_h
  end

  def nameable?
    # GT:residue -- a symbol reaching a reflective predicate
    __getobj__.respond_to?(:display_name)
  end

  def method_missing(name, *args, &block)
    # GT:blind -- the name is a value here; nothing in this file spells it
    return __getobj__.public_send(name, *args, &block) if respond_to_missing?(name)

    super
  end

  def respond_to_missing?(name, include_private = false)
    __getobj__.respond_to?(name, include_private) || super
  end

  # Hash shorthand puts the key and the value at the *same* source offsets, so
  # the key is one thing and the local read is another and they are indexed
  # alike. Neither is a reach: the key names a struct member, and the value is a
  # local variable that shadows the method for the rest of this body.
  def summary
    display_name = __getobj__.display_name # GT:residue -- the call, before the local exists

    Summary.new(display_name:, email: __getobj__.email) # GT:ignore
  end
end
