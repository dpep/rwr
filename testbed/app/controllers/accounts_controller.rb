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
    # GT:blind -- a chained receiver rwr cannot resolve (D61)
    current_scope.accounts.first.display_name
  end
end
