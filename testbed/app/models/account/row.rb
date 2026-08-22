# frozen_string_literal: true

# Is a class *namespaced under* Account mistaken for Account itself? `Row` is a
# separate class that happens to live in Account's namespace and happens to have
# a method of the same name -- the single commonest shape in a Rails app after
# the model itself. Nothing here breaks when Account's method is renamed, and
# every edit or report against this file is a false positive.
#
# Written in the nested form, which is the form that puts `Account` on the
# lexical scope stack. `account/exporter.rb` is the same Ruby in the compact
# form, and the two must be classified identically.
class Account
  class Row
    HEADERS = %w[Name Email Created].freeze

    def initialize(record)
      @record = record
    end

    # GT:ignore -- Row's own method, reached only through a Row
    def display_name
      @record.fetch(:name, "")
    end

    def to_a
      # GT:ignore -- implicit self inside Row, which is not inside Account
      [display_name, @record[:email], @record[:created_at]]
    end
  end
end
