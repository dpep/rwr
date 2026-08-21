# frozen_string_literal: true

class Widget
  def payload(name, size)
    { name:, size:, kind: "widget" }
  end

  def rocket(total)
    { total: }
  end

  def nested(type_filters)
    render(
      extras: {
        type_filters:,
      },
    )
  end

  def unrelated(count)
    { total: count }
  end
end
