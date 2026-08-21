# frozen_string_literal: true

RSpec.describe Account do
  it "has a display name" do
    # GT:rewrite -- specs are Ruby like anything else
    expect(Account.new.display_name).to be_a(String)
  end

  it "responds to it" do
    # GT:residue -- a symbol in a matcher
    expect(Account.new).to respond_to(:display_name)
  end
end
