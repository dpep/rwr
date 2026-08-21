# frozen_string_literal: true

class Widget
  def find(id)
    return if id.nil?

    store[id]
  end

  # A comment mentioning `return nil` must not be rewritten.
  def fetch(id)
    value = store[id]
    return unless value

    value
  end

  def describe
    <<~TEXT
      This heredoc body says return and must survive untouched.
    TEXT
  end

  def literal
    "return nil"
  end

  def nested
    [1, 2].each do |i|
      return if i.zero?
    end
  end

  def not_a_match
    return_value
  end
end
