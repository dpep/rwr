#!/usr/bin/env bash
# Every user-visible defect the September 2026 testing arc fixed, as an
# executable assertion.
#
# Not unit tests -- the suite has those, and twice this month a green suite
# coexisted with a broken binary. These drive the BUILT binary the way a person
# does, and each one is reduced from a defect that actually shipped.
#
#   regress.sh [path-to-rwr]        default: target/release/rwr
#
# Exit 0 = every behaviour still holds. Exit 1 = a regression, named.
#
# Tracked in the repo on purpose. The first copy lived in the session scratchpad
# under /private/tmp, which is periodically purged -- it went, along with most of
# the arc's reports. An instrument belongs beside the code it measures.
set -uo pipefail

RWR="${1:-$(cd "$(dirname "$0")/.." && pwd)/target/release/rwr}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); }
bad() { FAIL=$((FAIL+1)); echo "  REGRESSED: $1"; echo "     expected: $2"; echo "     got:      $3"; }
check() { if [ "$2" = "$3" ]; then ok; else bad "$1" "$2" "$3"; fi; }
n() { grep -c "$1" 2>/dev/null || true; }

echo "regress: $($RWR --version) at $RWR"

# ------------------------------------------------------------------ renames
d="$WORK/singleton"; mkdir -p "$d"; cat > "$d/a.rb" <<'EOF'
class Account
  def display_name; "i"; end
  class << self
    attr_accessor :display_name
    define_method(:display_name) { "c" }
    private :display_name
  end
end
EOF
got=$("$RWR" check 'Account#display_name' -r full_name "$d" 2>/dev/null | grep -oE 'would rewrite [0-9]+' | grep -oE '[0-9]+')
check "H1 instance rename must not touch class<<self macros" "1" "${got:-none}"

d="$WORK/ns"; mkdir -p "$d"; cat > "$d/a.rb" <<'EOF'
class Account
  def display_name; "top"; end
end
module Billing
  class Account
    def display_name; "b"; end
  end
end
class C
  def r
    Account.new.display_name
    Billing::Account.new.display_name
  end
end
EOF
got=$("$RWR" check 'Account#display_name' -r full_name "$d" 2>/dev/null | grep -oE 'would rewrite [0-9]+' | grep -oE '[0-9]+')
check "H2/F2 top-level rename must not reach Billing::Account calls" "2" "${got:-none}"

d="$WORK/lex"; mkdir -p "$d"; cat > "$d/a.rb" <<'EOF'
class Account
  def display_name; "top"; end
end
module Billing
  class Account
    def display_name; "b"; end
  end
  class Invoice
    def who; Account.new.display_name; end
  end
end
EOF
got=$("$RWR" check 'Account#display_name' -r full_name "$d" 2>/dev/null | grep -oE 'would rewrite [0-9]+' | grep -oE '[0-9]+')
check "F-A1 a bare constant receiver resolves where it is written" "1" "${got:-none}"

d="$WORK/args"; mkdir -p "$d"; cat > "$d/a.rb" <<'EOF'
class Widget
  def label(s = nil); "w"; end
  def both
    label
    label("y")
  end
end
class C
  def r
    Widget.new.label
    Widget.new.label("y")
  end
end
EOF
got=$("$RWR" check 'Widget#label' -r caption "$d" 2>/dev/null | grep -oE 'would rewrite [0-9]+' | grep -oE '[0-9]+')
check "F3 a rename reaches calls that pass arguments" "5" "${got:-none}"

for arg in 'Account#==' 'Account#name='; do
  for verb in find check; do
    "$RWR" "$verb" "$arg" "$WORK/ns" >/dev/null 2>&1; got=$?
    check "B1 $verb '$arg' refuses at 5" "5" "$got"
  done
done

# ------------------------------------------------------------ residue reach
d="$WORK/dyn"; mkdir -p "$d"
printf 'class Account\n  def display_name; "x"; end\nend\n' > "$d/a.rb"
printf 'class Account\n  def d(f)\n    public_send("display_#{f}")\n    send("display_#{f}")\n  end\nend\n' > "$d/b.rb"
got=$("$RWR" find 'Account#display_name' "$d" 2>&1 >/dev/null | n 'Dynamic:')
check "B2 cross-file dynamic reach is reported" "2" "$got"

d="$WORK/macro"; mkdir -p "$d"
printf 'class Account\n  attr_reader :display_name\nend\n' > "$d/a.rb"
printf 'class R\n  def run(a); a.display_name; end\nend\n' > "$d/b.rb"
printf 'match: attr_reader :display_name\nrewrite: attr_reader :full_name\n' > "$d/r.yml"
got=$("$RWR" check "$d/r.yml" "$d" 2>&1 >/dev/null | n 'Call:')
check "H4 a macro-definer rename reports its callers" "1" "$got"

# -------------------------------------------------------------- reporting
d="$WORK/blind"; mkdir -p "$d"
printf 'class Foo\n  def show(r); r.display_name; end\nend\n' > "$d/ok.rb"
printf 'class Broken\n  def x\n    r.display_name\n' > "$d/broken.rb"
got=$("$RWR" find '$R.display_name' "$d" -j 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin).get("unparsed") or []))')
check "B3 find -j reports files that did not parse" "1" "$got"

d="$WORK/tpl"; mkdir -p "$d"
printf 'class Account\n  def display_name; "x"; end\nend\n' > "$d/a.rb"
printf '%%div\n  = account.display_name\n' > "$d/v.haml"
got=$("$RWR" find 'Account#display_name' "$d" -j 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin).get("template_residue") or []))')
check "B4 find -j carries template residue" "1" "$got"

d="$WORK/sup"; mkdir -p "$d"
printf 'description: no puts\nmatch: puts($A)\n' > "$d/no_puts.yml"
printf 'def f\n  puts "x"  # rwr:ignore style/no-puts\nend\n' > "$d/a.rb"
got=$("$RWR" check "$d/no_puts.yml" "$d/a.rb" -j 2>/dev/null | python3 -c 'import json,sys;u=json.load(sys.stdin).get("unknown_suppressions") or [];print(u[0].get("did_you_mean") if u else "none")')
check "B5 an unknown rule id is named, with a suggestion" "no_puts" "$got"

d="$WORK/unread"; mkdir -p "$d"
printf 'class Account\n  def display_name; "x"; end\nend\n' > "$d/only.rb"; chmod 000 "$d/only.rb"
got=$("$RWR" find 'Account#display_name' "$d" -j 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin).get("unreadable") or []))')
chmod 644 "$d/only.rb"
check "B6 an unreadable file is reported, not read as empty" "1" "$got"

d="$WORK/dedup"; mkdir -p "$d"
printf 'class Widget\n  def key(x); x; end\nend\n' > "$d/a.rb"
printf -- '-# p\n= key + key\n' > "$d/p.html.haml"
got=$("$RWR" check 'Widget#key' -r feed_key "$d" -j 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin).get("template_residue") or []))')
check "A3 template residue is one entry per site, not per sub-rule" "2" "$got"

# -------------------------------------------------------------- rule pack
d="$WORK/hash"; mkdir -p "$d"; echo "3.4.9" > "$d/.ruby-version"
printf 'def a = 99\nh = { a: :a }\n' > "$d/a.rb"
"$RWR" rewrite all "$d" >/dev/null 2>&1
check "R1 hash-shorthand must not eat a symbol value" "1" "$(grep -c 'a: :a' "$d/a.rb")"
printf 'def f(name)\n  { name: name }\nend\n' > "$d/b.rb"
"$RWR" rewrite all "$d/b.rb" >/dev/null 2>&1
check "R1b a legitimate shorthand site is still rewritten" "1" "$(grep -cE 'name: *\}' "$d/b.rb")"

# ------------------------------------------------------------- designators
d="$WORK/nsdot"; mkdir -p "$d"
printf 'module Foo\n  class Bar\n    def self.connection; 1; end\n  end\nend\nFoo::Bar.connection\n' > "$d/a.rb"
"$RWR" find 'Foo::Bar.connection' "$d" >/dev/null 2>&1
check "R2 a namespaced class-method designator runs" "0" "$?"

echo
echo "regress: $PASS held, $FAIL regressed"
[ "$FAIL" -eq 0 ] || exit 1
