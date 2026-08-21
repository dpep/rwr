# frozen_string_literal: true

class Router
  def label = thing.legacy_name

  def dispatch(payload)
    case payload
    in { kind: "widget", id: Integer => id }
      lookup(id).legacy_name
    in [first, *rest]
      first&.legacy_name
    else
      payload => { fallback: }
      fallback.legacy_name
    end
  end

  def safe(thing)
    thing&.legacy_name
  end
end
