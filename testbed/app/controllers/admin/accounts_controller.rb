# frozen_string_literal: true

# The ordinary shapes of a decade-old controller, none of which is an edge case
# and all of which the matcher has to walk: a namespaced class, `before_action`
# with options, a `respond_to` block, a multi-line chain, a trailing-comma
# argument list, keyword arguments, adjacent-string line continuation, and
# blocks nested three deep.
#
# The hash key is the one to watch. `display_name: name` names a payload field,
# not a method to dispatch on, and counting keys as reaches is what turns a
# report into a wall.
module Admin
  class AccountsController < ApplicationController
    before_action :set_account, only: %i[show update]
    rescue_from ActiveRecord::RecordNotFound, with: :not_found

    def index
      @accounts = Account
                  .where(active: true)
                  .order(:display_name) # GT:residue -- a symbol to a query builder
                  .limit(params.fetch(:limit, 50))
    end

    def show
      respond_to do |format|
        format.html
        format.json { render json: payload(@account), status: :ok }
      end
    end

    def update
      if @account.update(account_params) &&
         @account.display_name.present? # GT:residue -- an ivar with no resolvable assignment
        redirect_to admin_account_path(@account)
      else
        render :edit, status: :unprocessable_entity
      end
    end

    def bulk_label
      Account.find_each do |account|
        account.memberships.each do |membership|
          membership.entries.map do |entry|
            entry.merge(label: account.display_name) # GT:residue -- a block parameter has no type
          end
        end
      end
    end

    private

    def payload(account)
      # GT:residue -- the receiver is a method parameter, which carries no type
      name = account.display_name
      {
        display_name: name, # GT:ignore -- a hash key names a field, not a method
        email: account.email,
      }
    end

    def title
      "Account: " \
        "#{@account.display_name}" # GT:residue -- across an adjacent-string continuation
    end

    def set_account
      @account = Account.find(params[:id])
    end

    def account_params
      params.require(:account).permit(:email, :locale)
    end

    def not_found
      head :not_found
    end
  end
end
