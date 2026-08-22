# frozen_string_literal: true

# Every violation here is `style/return-nil`, so what varies is the directive
# rather than the rule. Markers sit on the line they describe.
class Directives
  def trailing
    return nil # rwr:ignore style/return-nil -- GT:accepted
  end

  def undirected
    return nil # GT:flagged
  end

  # A leading directive covers the whole method, nested blocks included, and
  # stops at its `end`.
  # rwr:ignore style/return-nil
  def leading
    return nil if early? # GT:accepted

    [1, 2].each do
      return nil # GT:accepted
    end
  end

  # The method *after* a covered one is the case that catches a directive scoped
  # to a node rather than a statement: the widest node starting on a line is the
  # whole program when a comment sits above the first statement, so a plausible
  # implementation swallows everything below.
  def after_leading
    return nil # GT:flagged
  end

  # rwr:ignore style/return-nil -- GT:stale
  def nothing_to_accept
    1
  end

  def other_rule
    return nil # rwr:ignore performance/detect -- GT:flagged
  end

  def blanket
    return nil # rwr:ignore -- GT:malformed, and the finding survives
  end

  # Attachment does not cross a blank line (D35), so this reaches nothing.
  # rwr:ignore style/return-nil -- GT:stale

  def past_a_blank_line
    return nil # GT:flagged
  end
end
