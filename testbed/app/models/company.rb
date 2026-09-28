# frozen_string_literal: true

# An unrelated class that happens to share the name. Nothing here may change,
# and nothing here should be reported: this is where a bare-name rename does
# its damage, and where residue turns into noise if it is not scoped.
class Company
  # GT:ignore
  def display_name
    legal_name
  end

  # A deliberate `style/return-nil` site, so the pull-request tooling has a real
  # simplification to demonstrate against. Nothing here is broken -- which is
  # exactly the point of the framing.
  def legacy_label
    return nil unless display_name

    display_name.upcase
  end

  def banner
    # GT:ignore -- a call on a Company, not an Account
    display_name.upcase
  end
end
