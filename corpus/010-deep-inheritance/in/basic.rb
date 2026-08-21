# frozen_string_literal: true

class Account
  def display_name
    "base"
  end
end

class Premium < Account
end

class Gold < Premium
  def display_name
    "gold"
  end
end

class Unrelated
  def display_name
    "not an account"
  end
end

gold = Gold.new
gold.display_name

other = Unrelated.new
other.display_name
