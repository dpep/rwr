# frozen_string_literal: true

class Account
  attr_reader :display_name

  def display_name
    "#{first} #{last}"
  end

  def greeting
    "Hello #{display_name}"
  end
end

class Presenter
  delegate :display_name, to: :account

  def initialize
    @account = Account.new
  end

  def label
    @account.display_name
  end

  def dynamic
    @account.send(:display_name)
  end
end
