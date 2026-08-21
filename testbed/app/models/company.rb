# frozen_string_literal: true

# An unrelated class that happens to share the name. Nothing here may change,
# and nothing here should be reported: this is where a bare-name rename does
# its damage, and where residue turns into noise if it is not scoped.
class Company
  # GT:ignore
  def display_name
    legal_name
  end

  def banner
    # GT:ignore -- a call on a Company, not an Account
    display_name.upcase
  end
end
