# frozen_string_literal: true

class Report
  def first_active(accounts)
    accounts.select { |a| a.active? }.first
  end

  def first_matching(accounts, term)
    accounts
      .select { |account| account.name.include?(term) }
      .first
  end

  def with_do_end(accounts)
    accounts.select do |account|
      account.active? && account.balance.positive?
    end.first
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
