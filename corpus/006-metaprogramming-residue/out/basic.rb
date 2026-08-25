# frozen_string_literal: true

class Account
  attr_reader :full_name

  def full_name
    "#{first} #{last}"
  end

  def greeting
    "Hello #{full_name}"
  end
end

class Presenter
  delegate :display_name, to: :account

  def initialize
    @account = Account.new
  end

  def label
    @account.full_name
  end

  def dynamic
    @account.send(:full_name)
  end
end
