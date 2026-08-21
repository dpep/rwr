# frozen_string_literal: true

namespace :accounts do
  desc "print names"
  task :print do
    # GT:rewrite -- a .rake file is Ruby, and was invisible before Q11
    puts Account.new.display_name
  end
end
