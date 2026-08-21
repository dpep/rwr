# frozen_string_literal: true

class Widget
  def payload(name, size)
    { name: name, size: size, kind: "widget" }
  end

  def rocket(total)
    { :total => total }
  end

  def unrelated(count)
    { total: count }
  end
end
