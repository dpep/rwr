# frozen_string_literal: true

# Can a rename splice around heredocs? A heredoc's body sits far past the end of
# the node that opens it, so an edit computed from that node -- or a capture
# spliced out of it -- detaches the body, and the result still parses. All four
# flavours are here, plus the three cases that matter:
#
#   * a body that *contains* the method name and does not break, because it is a
#     column and a column is not a method,
#   * a body whose interpolation holds a real call, which does break, and
#   * two heredocs opened on one line, which is what catches an implementation
#     assuming a body starts on the line after its opener.
#
# Markers sit on the opening line where the site is in a body, since a body has
# nowhere to put a Ruby comment.
module Reports
  class AccountReport
    NOTICE = <<-'TEXT' # GT:ignore -- a format key, in a heredoc that cannot interpolate
      Hello %{display_name}, your account has been archived.
    TEXT

    def initialize(account)
      @account = account
    end

    def query
      <<~SQL
        SELECT accounts.display_name -- GT:ignore, a column is not a method
          FROM accounts
         WHERE accounts.id = :id
      SQL
    end

    def html
      account = Account.new
      <<-HTML.strip # GT:rewrite -- a call inside a body, on a local that resolves
        <span class="name">#{account.display_name}</span>
      HTML
    end

    # A plain heredoc, whose terminator has to sit in column zero.
    def footer
      <<HTML # GT:residue -- an ivar receiver with no assignment to resolve from
<p class="footer">#{@account.display_name}</p>
HTML
    end

    def notify(mailer)
      mailer.deliver(<<~SUBJECT, <<~BODY) # GT:residue -- first of two bodies, one line
        Account #{@account.display_name} updated
      SUBJECT
        The name on this account changed. No action is needed.
      BODY
    end
  end
end
