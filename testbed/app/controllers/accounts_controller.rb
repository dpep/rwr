# frozen_string_literal: true

class AccountsController
  def show
    account = Account.new
    # GT:rewrite -- receiver resolves through a constructor
    render json: { name: account.display_name }
  end

  def index
    # GT:residue -- a symbol passed to a query builder
    Account.order(:display_name)
  end

  def export
    # GT:residue -- cannot *resolve* is not cannot *see*; reported as a call (D61)
    current_scope.accounts.first.display_name
  end
end
