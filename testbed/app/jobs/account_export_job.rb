# frozen_string_literal: true

# Does a rename survive the control flow legacy background jobs are made of?
# `begin`/`rescue`/`else`/`ensure` with a `retry`, a method that yields to an
# explicit block, and a `case/in` pattern match. The bodies here are `begin`
# nodes and `case` nodes rather than expressions, which is the ordinary shape a
# definition pattern has to bind -- the flagship rename already broke once on a
# body that was merely two statements instead of one.
class AccountExportJob
  MAX_ATTEMPTS = 3

  def perform(account_id, format: :csv, **options)
    account = Account.new
    attempts = 0

    begin
      attempts += 1
      # GT:rewrite -- a local assigned from a constructor, inside a begin
      write(account.display_name, format:, **options)
    rescue Timeout::Error
      retry if attempts < MAX_ATTEMPTS
      raise
    else
      Rails.logger.info("exported #{account_id}")
    ensure
      cleanup(account_id)
    end
  end

  def each_label(accounts)
    return to_enum(:each_label, accounts) unless block_given?

    accounts.each do |account|
      # GT:residue -- a block parameter, yielded onward
      yield account.display_name
    end
  end

  def describe(payload)
    case payload
    in { name: String => name }
      name
    in { account: Account => account }
      account.display_name # GT:residue -- a pattern-match binding is not narrowed
    else
      "unknown"
    end
  end

  private

  def write(*)
    nil
  end

  def cleanup(_id)
    nil
  end
end
