# frozen_string_literal: true

class Account
  def full_name
    "#{first} #{last}"
  end
end

class Company
  # Same method name, different class. Must NOT be rewritten - this is the
  # confident-wrong-match failure the case studies found shipping at exit 0.
  def display_name
    legal_name
  end
end

class Greeter
  def greet_account(account)
    account = Account.new
    "Hello #{account.full_name}"
  end

  def greet_company(company)
    company = Company.new
    "Regards, #{company.display_name}"
  end

  def unknown_receiver(thing)
    thing.display_name
  end
end
