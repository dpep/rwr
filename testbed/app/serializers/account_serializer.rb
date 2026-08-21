# frozen_string_literal: true

# A DSL that takes the method name as a symbol. Rails codebases are full of
# these, and they are exactly what a syntactic rename misses.
class AccountSerializer
  # GT:residue -- serializer attribute
  attribute :display_name

  # GT:residue -- delegation
  delegate :display_name, to: :account

  # GT:residue -- validation by symbol
  validates :display_name, presence: true
end
