# frozen_string_literal: true

class Router
  def label = thing.modern_name

  def dispatch(payload)
    case payload
    in { kind: "widget", id: Integer => id }
      lookup(id).modern_name
    in [first, *rest]
      first&.modern_name
    else
      payload => { fallback: }
      fallback.modern_name
    end
  end

  def safe(thing)
    thing&.modern_name
  end
end
