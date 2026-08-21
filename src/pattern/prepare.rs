//! Substituting metavariables so a pattern parses as ordinary Ruby (D18).
//!
//! `$M` is a *global variable* token, which lexes only where a global is legal.
//! `foo($A)` parses; `x.$M`, `def $M`, `:$M`, `$K: v` and `Foo::$C` do not. So
//! each metavariable is replaced with a placeholder identifier before Prism
//! sees the pattern, and mapped back afterwards.
//!
//! **Which case the placeholder needs is not knowable before parsing.** A
//! constant position demands `RwrMv0`; a block parameter demands `rwr_mv_0` and
//! rejects the capitalised form. D18 proposed parsing lowercase and retrying
//! capitalised, which cannot serve a pattern needing both — `Foo::$C.each { |$P| }`
//! fails either way. So the retry is per-placeholder instead: on a parse error,
//! flip the case of the placeholder nearest the error and try again. Patterns
//! are tiny, so the loop is cheap, and it stays deterministic.

use super::metavar::{self, Arity};
use std::collections::HashMap;
use std::fmt;

/// What a placeholder identifier stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    /// `None` for the anonymous forms `_` and `*_`.
    pub name: Option<String>,
    pub arity: Arity,
}

/// A pattern rewritten into parseable Ruby, with the mapping back.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    /// Ruby source in which every metavariable is a placeholder identifier.
    pub source: String,
    /// Placeholder identifier -> what it stands for.
    pub bindings: HashMap<String, Binding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrepareError {
    /// The pattern does not parse as Ruby under any placeholder casing.
    Unparseable { message: String },
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrepareError::Unparseable { message } => {
                write!(f, "pattern is not valid Ruby: {message}")
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Lower,
    Upper,
}

impl Case {
    fn placeholder(self, index: usize) -> String {
        match self {
            Case::Lower => format!("rwr_mv_{index}"),
            Case::Upper => format!("RwrMv{index}"),
        }
    }

    fn flipped(self) -> Self {
        match self {
            Case::Lower => Case::Upper,
            Case::Upper => Case::Lower,
        }
    }
}

/// Rewrite `pattern` into parseable Ruby, replacing metavariables with
/// placeholder identifiers.
pub(crate) fn prepare(pattern: &str) -> Result<Prepared, PrepareError> {
    prepare_with(pattern, &[])
}

/// As [`prepare`], seeding the named metavariables with constant casing.
///
/// The case-repair loop flips a placeholder only when the parse *fails*, which
/// cannot help where both casings parse and mean different things:
/// `rwr_mv_1 = [1]` is a local-variable write and `RwrMv1 = [1]` a constant
/// write, so a pattern for the latter silently became one for the former. A rule
/// that says `where: { $C: { is: constant } }` has already answered the
/// question, so the declaration seeds the substitution rather than a second
/// mechanism being invented for it.
pub(crate) fn prepare_with(pattern: &str, constants: &[String]) -> Result<Prepared, PrepareError> {
    let vars = metavar::scan(pattern);
    let mut cases: Vec<Case> = vars
        .iter()
        .map(|v| match &v.name {
            Some(name) if constants.iter().any(|c| c.trim_start_matches('$') == name) => {
                Case::Upper
            }
            _ => Case::Lower,
        })
        .collect();

    // One flip per placeholder is enough to reach any reachable assignment,
    // plus a final attempt to observe the result.
    let attempts = vars.len() + 1;
    let mut last_message = String::from("no metavariables and the source does not parse");

    for _ in 0..attempts {
        let (source, spans) = render(pattern, &vars, &cases);
        // The parse result borrows `source`, so the diagnostic is copied out and
        // the result dropped before `source` can be moved into `Prepared`.
        let failure = {
            let result = ruby_prism::parse(source.as_bytes());
            result
                .errors()
                .next()
                .map(|d| (d.message().to_string(), d.location().start_offset()))
        };

        let Some((message, at)) = failure else {
            let bindings = vars
                .iter()
                .zip(&cases)
                .enumerate()
                .map(|(i, (v, c))| {
                    (
                        c.placeholder(i),
                        Binding {
                            name: v.name.clone(),
                            arity: v.arity,
                        },
                    )
                })
                .collect();
            return Ok(Prepared { source, bindings });
        };

        last_message = message;
        match nearest(&spans, at) {
            Some(i) => cases[i] = cases[i].flipped(),
            // Nothing to flip — the pattern is wrong independently of casing.
            None => break,
        }
    }

    Err(PrepareError::Unparseable {
        message: last_message,
    })
}

/// Build the substituted source, returning each placeholder's span within it.
fn render(
    pattern: &str,
    vars: &[metavar::Metavar],
    cases: &[Case],
) -> (String, Vec<(usize, usize)>) {
    let mut source = String::with_capacity(pattern.len());
    let mut spans = Vec::with_capacity(vars.len());
    let mut cursor = 0;

    for (i, var) in vars.iter().enumerate() {
        source.push_str(&pattern[cursor..var.start]);
        // The metavariable's span includes the leading `*` for sequences, so the
        // splat has to be re-emitted around the placeholder.
        if var.arity == Arity::Many {
            // Re-emit the splat the metavariable's span swallowed, in the
            // spelling the author used -- `**` inside a hash, `*` elsewhere.
            source.push_str(if var.double { "**" } else { "*" });
        }
        let start = source.len();
        source.push_str(&cases[i].placeholder(i));
        spans.push((start, source.len()));
        cursor = var.end;
    }
    source.push_str(&pattern[cursor..]);
    (source, spans)
}

/// The placeholder containing `at`, else the one closest to it.
fn nearest(spans: &[(usize, usize)], at: usize) -> Option<usize> {
    if spans.is_empty() {
        return None;
    }
    if let Some(i) = spans.iter().position(|(s, e)| at >= *s && at < *e) {
        return Some(i);
    }
    spans
        .iter()
        .enumerate()
        .min_by_key(|(_, (s, e))| {
            if at < *s {
                s - at
            } else {
                at.saturating_sub(*e)
            }
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parses(pattern: &str) -> Prepared {
        prepare(pattern).unwrap_or_else(|e| panic!("{pattern:?} should prepare: {e}"))
    }

    #[test]
    fn expression_position_needs_no_repair() {
        let p = parses("foo($A, $B)");
        assert_eq!(p.bindings.len(), 2);
        assert!(ruby_prism::parse(p.source.as_bytes()).errors().count() == 0);
    }

    /// The whole reason D18 exists: none of these lex with a `$` sigil.
    #[test]
    fn positions_that_a_global_cannot_occupy() {
        for pattern in [
            "x.$M(1)",     // method name
            "def $M; end", // definition name
            ":$M",         // symbol
            "foo($K: 1)",  // keyword argument name
            "Foo::$C",     // constant path
        ] {
            let p = parses(pattern);
            assert_eq!(
                ruby_prism::parse(p.source.as_bytes()).errors().count(),
                0,
                "{pattern:?} produced unparseable {:?}",
                p.source
            );
        }
    }

    /// A class name demands an uppercase placeholder and a block parameter
    /// rejects one, so this pattern is unparseable under any *uniform* casing.
    /// It is the case that forced per-placeholder repair rather than D18's
    /// original whole-pattern retry.
    #[test]
    fn mixed_case_requirements_are_repaired_independently() {
        let p = parses("class $C; def go; [1].each { |$P| $P }; end; end");
        assert_eq!(ruby_prism::parse(p.source.as_bytes()).errors().count(), 0);
        assert!(
            p.source.contains("class RwrMv"),
            "class name lowercase: {:?}",
            p.source
        );
        assert!(
            p.source.contains("|rwr_mv_"),
            "block param capitalised: {:?}",
            p.source
        );
    }

    /// `Foo::bar` is a *method call* and `Foo::Bar` is a constant, so `Foo::$C`
    /// parses either way and the placeholder's case silently decides which the
    /// pattern means. Repair cannot detect this - there is no parse error to
    /// react to - so the matcher must treat a placeholder as a wildcard rather
    /// than trusting the node type it happened to land on.
    #[test]
    fn scope_resolution_parses_under_either_casing() {
        let p = parses("Foo::$C");
        assert_eq!(ruby_prism::parse(p.source.as_bytes()).errors().count(), 0);
        assert!(
            p.source.contains("rwr_mv_0"),
            "unexpected repair: {:?}",
            p.source
        );
    }

    #[test]
    fn sequence_metavariables_keep_their_splat() {
        let p = parses("[*$ITEMS]");
        assert!(p.source.starts_with("[*"), "splat lost: {:?}", p.source);
        assert_eq!(p.bindings.values().next().unwrap().arity, Arity::Many);
    }

    #[test]
    fn anonymous_metavariables_bind_no_name() {
        let p = parses("foo(_, *_)");
        assert_eq!(p.bindings.len(), 2);
        assert!(p.bindings.values().all(|b| b.name.is_none()));
    }

    /// Genuinely broken Ruby must be reported, not silently repaired forever.
    #[test]
    fn unparseable_patterns_are_rejected() {
        assert!(prepare("def foo(").is_err());
        assert!(prepare("$A +").is_err());
    }
}
