# frozen_string_literal: true

class Report
  def totals(db)
    db.run_query(<<~SQL)
      SELECT count(*)
      FROM widgets
      WHERE kind = 'end'
    SQL
  end

  def inline(db)
    db.run_query("SELECT 1")
  end

  def nested(db)
    db.run_query(<<~SQL.strip)
      SELECT 2
    SQL
  end

  def untouched
    <<~TEXT
      db.run_query(:not_code)
    TEXT
  end
end
