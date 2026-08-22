# frozen_string_literal: true

# Does a rename survive an override whose signature drifted from the parent's?
# The testbed's first override was a one-line `super`; a decade-old one has
# picked up an optional argument nobody ever added to the parent. Arity is the
# only variable here on purpose -- the rescue/ensure body, which fails for its
# own reason, is isolated in `lib/account_ext.rb`.
#
# Around it: the ordinary siblings a rename must leave alone. A writer sharing
# the stem, a class method sharing the name, and a local that shadows the method
# from the point Ruby parses the assignment.
class ArchivedAccount < Account
  DEFAULT_FORMAT = :long

  # GT:rewrite -- an override left behind is a NoMethodError, arity or no arity
  def display_name(format = DEFAULT_FORMAT)
    return archived_label unless archived_at

    case format
    when :short then super().split.first
    when :long then "#{super()} (archived)"
    else archived_label
    end
  end

  # The class-level label. `ArchivedAccount.display_name` and
  # `ArchivedAccount#display_name` are two methods with one name, in a class the
  # rename does reach -- so the instance/singleton distinction is the only thing
  # keeping this one still.
  def self.display_name # GT:notice -- a different method; reporting the near-miss is the point
    "Archived accounts"
  end

  # A writer shares the stem and nothing else: `display_name=` is its own
  # method, and renaming the reader neither moves it nor breaks it.
  def display_name=(value) # GT:ignore
    @archived_label = value.presence
  end

  # Sorting reaches the method twice, on two different receivers.
  def <=>(other)
    # GT:rewrite -- implicit self, in a subclass of the renamed class
    mine = display_name
    # GT:residue -- a parameter has no type, so the call is left where it is
    mine <=> other.display_name
  end

  # The keyword form of aliasing. `alias_method` takes symbols as arguments;
  # `alias` takes them as syntax, and both leave a second name pointing at the
  # method that is about to move.
  alias legacy_name display_name # GT:residue

  def to_s
    # A local shadows the method from the point Ruby *parses* the assignment,
    # even inside a branch that never runs. Both lines below read the local.
    display_name = short_code if short_code # GT:ignore
    display_name || "archived" # GT:ignore
  end

  private

  def archived_label
    "Archived"
  end

  def short_code
    @short_code
  end
end
