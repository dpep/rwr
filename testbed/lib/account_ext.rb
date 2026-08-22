# frozen_string_literal: true

# Four shapes a decade-old monolith grows, all of them written as `class
# Account`, and a rename has to tell them apart:
#
#   * a reopened class body, which is a second definition of the same method in
#     a file the model does not know about -- and whose body is a `begin`,
#     because it carries a `rescue`, which is what any method touching I/O
#     looks like,
#   * the same method defined *twice in one body*, which Ruby resolves by taking
#     the last one -- so a rename that moves only one of them resurrects the
#     other under the old name,
#   * `class << self`, whose `def display_name` is `Account.display_name`, a
#     different method that no rename of `Account#display_name` may touch, and
#   * `class_eval` over a non-interpolating heredoc, which is Ruby to Ruby and a
#     string to everyone else.
class Account
  # GT:rewrite -- a redefinition, in a file the model does not know about
  def display_name
    return @cached_name if defined?(@cached_name)

    @cached_name = [first, last].compact.join(" ").squeeze(" ")
  rescue NoMethodError
    ""
  ensure
    @looked_up = true
  end

  def initials
    display_name.split.map { |part| part[0] }.join # GT:rewrite -- implicit self
  end

  # GT:rewrite -- and the same method again, which is what a bad merge leaves
  def display_name
    [first, last].compact.join(" ")
  end

  class << self
    # `Account.display_name` is the label the admin menu prints. It shares a
    # name with the instance method and shares nothing else: renaming the
    # instance method leaves this one exactly where it is.
    def display_name # GT:notice -- `class << self` makes this Account.display_name
      "Accounts"
    end

    def label_for(kind)
      # GT:ignore -- `self` is the class here, so both reads are the singleton
      kind == :plural ? display_name : display_name.singularize
    end
  end
end

# GT:blind -- a method body written as a string; rwr does not evaluate Ruby
Account.class_eval <<~'RUBY', __FILE__, __LINE__ + 1
  def display_name_with_id
    "#{display_name} (#{id})"
  end
RUBY
