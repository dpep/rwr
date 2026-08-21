# frozen_string_literal: true

# A subclass. Its override is the same method and must move with the parent.
class PremiumAccount < Account
  # GT:rewrite -- an override left behind is a NoMethodError
  def display_name
    "#{super} (premium)"
  end
end
