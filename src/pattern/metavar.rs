//! Metavariable recognition in pattern source.
//!
//! Pure lexical scanning, ahead of any parsing. Per D18 each metavariable is
//! substituted with a syntactically valid placeholder before Prism sees the
//! pattern, so this scanner runs first and its output drives that rewrite.
//!
//! Syntax (D32). Two orthogonal axes rather than four forms to memorise:
//! `*` means many (Ruby's splat), `_` means don't care (Ruby's throwaway),
//! `$NAME` binds a capture.
//!
//! |            | one node  | zero or more |
//! |------------|-----------|--------------|
//! | anonymous  | `_`       | `*_`         |
//! | captured   | `$NAME`   | `*$NAME`     |
//!
//! All four are valid Ruby, so Ruby's own grammar validates where a sequence
//! may appear - splats are legal exactly where sequence metavariables are
//! wanted. `*_` is already idiomatic Ruby for "rest, ignored".
//!
//! A bare `*`, `**` or `&` -- Ruby's anonymous forwarding -- is a wildcard that
//! a template may forward (D129). It binds under its own spelling, which no
//! `$NAME` can collide with, and `prepare` confirms with Prism that each one
//! the scanner found really is an anonymous argument or parameter.

/// How many nodes a metavariable stands for, before any `where:` refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arity {
    /// `$NAME` / `_` — exactly one node.
    One,
    /// `*$NAME` / `*_` — zero or more nodes.
    Many,
}

/// One metavariable occurrence in a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Metavar {
    /// `None` for the anonymous forms `_` and `*_`.
    pub name: Option<String>,
    pub arity: Arity,
    /// The sigil the span swallowed: `*`, `**`, `&`, or empty for a single node.
    ///
    /// The whole `**$NAME` is one token: treating the second asterisk as the
    /// metavariable would leave the first behind as literal template text, and
    /// an empty sequence would render `{*, a:}`.
    pub sigil: &'static str,
    /// Byte range in the pattern source, for substitution and diagnostics.
    pub start: usize,
    pub end: usize,
}

/// Scan a pattern for metavariable occurrences, in source order.
///
/// A capture name is an uppercase letter followed by uppercase letters, digits
/// or underscores. That case rule keeps ordinary Ruby globals (`$stdout`, `$_`,
/// `$1`, `$:`) matchable as literals with no escape — they are the common case.
/// Uppercase globals (`$LOAD_PATH`) are genuinely ambiguous and escape as
/// `\$LOAD_PATH`.
pub(crate) fn scan(pattern: &str) -> Vec<Metavar> {
    let b = pattern.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < b.len() {
        // `\$` escapes a literal Ruby global; skip both bytes.
        if b[i] == b'\\' && i + 1 < b.len() && b[i + 1] == b'$' {
            i += 2;
            continue;
        }

        let start = i;
        let sigil = if b[i] == b'*' && b.get(i + 1) == Some(&b'*') {
            "**"
        } else if b[i] == b'*' {
            "*"
        } else {
            ""
        };
        let arity = if sigil.is_empty() {
            Arity::One
        } else {
            Arity::Many
        };
        let j = i + sigil.len();

        match parse_binder(b, j) {
            Some((name, end)) => {
                out.push(Metavar {
                    name,
                    arity,
                    sigil,
                    start,
                    end,
                });
                i = end;
            }
            None => {
                if let Some(sigil) = bare_sigil(b, i) {
                    out.push(Metavar {
                        name: Some(sigil.to_string()),
                        arity: Arity::Many,
                        sigil,
                        start,
                        end: start + sigil.len(),
                    });
                    i = start + sigil.len();
                    continue;
                }
                // Not a metavariable. A rejected `$` is a literal Ruby global,
                // and the *whole* token must be consumed: stepping one byte past
                // the `$` in `$_` would leave a bare `_` to be misread as a
                // wildcard.
                i += 1;
                if b[start] == b'$' {
                    while is_ident_byte(b.get(i)) {
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

/// The spellings a bare forwarding wildcard binds under.
pub(crate) const BARE: [&str; 3] = ["*", "**", "&"];

/// Whether a capture name is a bare `*`, `**` or `&` rather than a `$NAME`.
pub(crate) fn is_bare(name: &str) -> bool {
    BARE.contains(&name)
}

/// A bare `*`, `**` or `&` at `i`: the sigil standing alone, followed by what
/// may close an argument or parameter. Multiplication, `&&`, `&.` and a splat
/// of an expression all have an operand after them instead.
fn bare_sigil(b: &[u8], i: usize) -> Option<&'static str> {
    let sigil = match (b[i], b.get(i + 1)) {
        (b'*', Some(b'*')) => "**",
        (b'*', _) => "*",
        (b'&', _) => "&",
        _ => return None,
    };
    if i > 0 && matches!(b[i - 1], b'*' | b'&') {
        return None;
    }
    let mut k = i + sigil.len();
    while matches!(b.get(k), Some(c) if c.is_ascii_whitespace()) {
        k += 1;
    }
    matches!(b.get(k), None | Some(b',' | b')' | b']' | b'}' | b'|')).then_some(sigil)
}

/// Parse `_` or `$NAME` at `j`, returning the capture name and end offset.
fn parse_binder(b: &[u8], j: usize) -> Option<(Option<String>, usize)> {
    match b.get(j) {
        // Anonymous: a lone `_`, not part of a longer identifier such as
        // `_unused` or `foo_bar`.
        Some(b'_')
            if !is_ident_byte(b.get(j + 1))
                && !is_ident_byte(j.checked_sub(1).and_then(|k| b.get(k))) =>
        {
            Some((None, j + 1))
        }
        Some(b'$') => {
            let name_start = j + 1;
            if !matches!(b.get(name_start), Some(c) if c.is_ascii_uppercase()) {
                return None; // a literal global: `$stdout`, `$_`, `$1`
            }
            let mut k = name_start;
            while is_name_byte(b.get(k)) {
                k += 1;
            }
            Some((
                Some(String::from_utf8_lossy(&b[name_start..k]).into_owned()),
                k,
            ))
        }
        _ => None,
    }
}

fn is_name_byte(c: Option<&u8>) -> bool {
    matches!(c, Some(c) if c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

fn is_ident_byte(c: Option<&u8>) -> bool {
    matches!(c, Some(c) if c.is_ascii_alphanumeric() || *c == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(p: &str) -> Vec<(Option<String>, Arity)> {
        scan(p).into_iter().map(|m| (m.name, m.arity)).collect()
    }

    /// The 2x2: `*` is many, `_` is anonymous, `$NAME` captures.
    #[test]
    fn covers_all_four_forms() {
        assert_eq!(
            found("foo($A, *$REST, _, *_)"),
            vec![
                (Some("A".into()), Arity::One),
                (Some("REST".into()), Arity::Many),
                (None, Arity::One),
                (None, Arity::Many),
            ]
        );
    }

    /// Ruby needs a double splat inside a hash, and the whole `**$NAME` is one
    /// token -- treating the second asterisk as the metavariable leaves the
    /// first behind as literal text, so an empty sequence renders `{*, a:}`.
    #[test]
    fn a_double_splat_is_one_token() {
        let found = scan("{**$REST}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sigil, "**");
        assert_eq!(&"{**$REST}"[found[0].start..found[0].end], "**$REST");
        assert_eq!(scan("foo(*$R)")[0].sigil, "*");
    }

    /// The case rule means ordinary Ruby globals need no escape - including
    /// `$_`, which an earlier design had to carve out as an exception.
    #[test]
    fn lowercase_globals_are_literals() {
        assert_eq!(found("$stdout.puts($1)"), vec![]);
        assert_eq!(found("$_.strip"), vec![]);
    }

    /// Uppercase globals genuinely collide, so they escape.
    #[test]
    fn escaped_dollar_is_a_literal_global() {
        assert_eq!(
            found("\\$LOAD_PATH.unshift($A)"),
            vec![(Some("A".into()), Arity::One)]
        );
        assert_eq!(
            found("$LOAD_PATH"),
            vec![(Some("LOAD_PATH".into()), Arity::One)]
        );
    }

    /// A lone `_` is the wildcard; `_foo` and `foo_bar` are ordinary
    /// identifiers and must not be mistaken for one.
    #[test]
    fn underscore_wildcard_requires_word_boundaries() {
        assert_eq!(found("foo(_)"), vec![(None, Arity::One)]);
        assert_eq!(found("foo(_unused)"), vec![]);
        assert_eq!(found("foo(bar_baz)"), vec![]);
    }

    /// A `*` with an operand is multiplication or a plain splat, and must not
    /// swallow what follows it.
    #[test]
    fn an_asterisk_with_an_operand_is_not_a_metavariable() {
        assert_eq!(found("a * $B"), vec![(Some("B".into()), Arity::One)]);
        assert_eq!(found("foo(*args)"), vec![]);
        assert_eq!(found("a *\n  b"), vec![]);
        assert_eq!(found("a && b&.c(&blk)"), vec![]);
        assert_eq!(found("a ** 2"), vec![]);
    }

    /// Ruby's anonymous forwarding: the sigil alone, closed by what ends an
    /// argument or parameter. Each binds under its own spelling.
    #[test]
    fn a_bare_sigil_is_a_wildcard() {
        for (pattern, sigil) in [
            ("foo(1, *)", "*"),
            ("foo(a: 1, **)", "**"),
            ("foo(&)", "&"),
            ("def m(*, **) = _", "*"),
            ("foo(*\n)", "*"),
            ("proc { |a, *| a }", "*"),
            ("**", "**"),
        ] {
            let vars = scan(pattern);
            let bare = vars.iter().find(|v| v.name.as_deref() == Some(sigil));
            let bare = bare.unwrap_or_else(|| panic!("{pattern:?}: no bare {sigil}"));
            assert_eq!(&pattern[bare.start..bare.end], sigil);
        }
        let vars = scan("foo(*, **, &)");
        let names: Vec<_> = vars.iter().filter_map(|v| v.name.as_deref()).collect();
        assert_eq!(names, ["*", "**", "&"]);
    }

    /// Repeated names bind once and are checked for AST equality at match time
    /// (D16); the scanner reports each occurrence.
    #[test]
    fn repeated_name_reports_each_occurrence() {
        assert_eq!(scan("$A.foo($A)").len(), 2);
    }

    #[test]
    fn ranges_cover_the_whole_token() {
        let m = &scan("foo(*$REST)")[0];
        assert_eq!(&"foo(*$REST)"[m.start..m.end], "*$REST");
    }
}
