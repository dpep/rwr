# frozen_string_literal: true

# Is a concern still the class's code when the mixin is written *relatively*?
#
# `include Naming` inside `module Reporting` names `Reporting::Naming`, exactly
# as the absolute spelling does. But the report asked the hierarchy about the
# bare scope segment `Naming`, which is not a class name at all -- and that was
# right only while the segment had exactly one candidate to widen to. Writing
# the mixin relatively puts its written spelling in the index beside the
# declared one, the widening stops, and the concern's contribution leaves the
# account with nothing said: same exit code, nothing on either stream.
#
# The absolute spelling is the one that worked, which is why this is written
# the short way. On rails the same shape hid `ActiveModel::Type::Helpers::
# Numeric#cast`, an override on three numeric types, from every report.
module Reporting
  module Naming
    # An override of Account#display_name, living in a module. A rename that
    # leaves it behind ships a class whose old name still answers.
    def display_name # GT:residue -- a concern's override, not lexically in Account
      "#{super} (reporting)"
    end

    def heading
      display_name.upcase # GT:residue -- implicit self, dispatching on an Account
    end
  end

  # The mixin is named without its namespace, which is how Ruby inside a module
  # is ordinarily written, and what the report could not see.
  class Summary < ::Account
    include Naming
  end
end
