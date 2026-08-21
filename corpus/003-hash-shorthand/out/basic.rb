# frozen_string_literal: true

class Widget
  def payload(name)
    {name:, kind: "widget"}
  end

  def rocket(total)
    {total:}
  end

  def unrelated(count)
    { total: count }
  end
end
