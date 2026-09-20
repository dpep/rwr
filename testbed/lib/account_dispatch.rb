# frozen_string_literal: true

# A dispatcher in a file of its own.
#
# The definition lives in `app/models/account.rb`, and these bytes hold no part
# of the name it defines -- deliberately, comment included. So nothing derived
# from that name can admit this file to a run; only the dispatcher can. The
# same code written inside `account.rb` was reported all along, which is what
# made a one-file fixture useless for the question: splitting a dispatcher from
# the definition it reaches is ordinary app structure.
class Account
  def dynamic_attribute(suffix)
    # GT:residue -- a computed reach the rename cannot follow
    public_send("display_#{suffix}")
  end
end
