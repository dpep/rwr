# frozen_string_literal: true

class Report
  def first_active(accounts)
    accounts.detect { |a| a.active? }
  end

  def first_matching(accounts, term)
    accounts
      .detect { |account| account.name.include?(term) }
  end

  def with_do_end(accounts)
    accounts.detect do |account|
      account.active? && account.balance.positive?
    end
  end

  # `.select { ... }.first` here is prose, not code.
  def already_correct(accounts)
    accounts.detect { |a| a.active? }
  end

  def not_a_match(accounts)
    accounts.select { |a| a.active? }.last
  end

  def also_not(accounts)
    accounts.select { |a| a.active? }
  end
end
