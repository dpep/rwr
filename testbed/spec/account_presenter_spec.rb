# frozen_string_literal: true

# Does a rename reach the spec suite? It is where the last references to a
# method live, and where they are least likely to be typed as a call: a stub
# names it as a symbol, a shared example takes it as an argument, and a
# description merely quotes it. The quoted one is the interesting negative --
# `"#display_name"` is not the identifier, and a report that cannot tell the
# difference will list every `describe` block in the suite.
RSpec.describe AccountPresenter do
  subject(:presenter) { described_class.new(account) }

  let(:account) { Account.new }

  describe "#display_name" do # GT:ignore -- a description is prose, not a name
    it "delegates to the account" do
      # GT:residue -- a stubbed method name
      allow(account).to receive(:display_name).and_return("Widget")

      expect(presenter.display_name).to eq("Widget") # GT:residue -- a let is untyped
    end

    it "reads through the model" do
      # GT:rewrite -- explicit receiver through a constructor
      expect(Account.new.display_name).to be_a(String)
    end
  end

  # GT:residue -- a method name handed to a shared example
  it_behaves_like "a named record", :display_name

  it "exposes the safe fields" do
    expect(described_class::SAFE_FIELDS.map(&:key)).to include(:display_name) # GT:residue
  end
end
