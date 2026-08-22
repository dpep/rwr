# frozen_string_literal: true

# The same question as `account/row.rb`, asked in the compact form. `class
# Account::Exporter` and `class Account; class Exporter` declare the same class
# and mean the same thing to Ruby, so a rename must treat them the same way --
# the difference is only in what the parser hands you for the class name.
#
# Also here: the `%i[]` list of method names that a legacy exporter drives
# through `public_send`, which is the most ordinary dynamic reach in the app and
# is stated entirely in literals.
class Account::Exporter
  # GT:residue -- a list of method names, dispatched below
  COLUMNS = %i[display_name email created_at].freeze

  def initialize(accounts, **options)
    @accounts = accounts
    @separator = options.fetch(:separator, ",")
  end

  def each_row
    return to_enum(:each_row) unless block_given?

    @accounts.each do |account|
      yield COLUMNS.map { |column| account.public_send(column) }.join(@separator)
    end
  end

  # GT:ignore -- the exporter's own header, reached only through an Exporter
  def display_name
    "Account export"
  end
end
