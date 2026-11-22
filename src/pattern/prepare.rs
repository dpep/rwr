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

use super::generated;
use super::metavar::{self, Arity};
use ruby_prism::Node;
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
    /// Match hash entries in source order only. Set by
    /// [`super::matcher::matches_in_order`], which asks whether a match needed
    /// D127's any-order entries; never set for a rule's own matching.
    pub in_order: bool,
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
    for sigil in metavar::BARE {
        if vars
            .iter()
            .filter(|v| v.name.as_deref() == Some(sigil))
            .count()
            > 1
        {
            return Err(PrepareError::Unparseable {
                message: format!(
                    "a pattern holds at most one bare `{sigil}`, as a Ruby method declares at \
                     most one -- name the others, e.g. `{sigil}$REST`"
                ),
            });
        }
    }
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
            confirm_bare(pattern, &vars, &cases)?;
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
            return Ok(Prepared {
                source,
                bindings,
                in_order: false,
            });
        };

        last_message = message;
        // A bare wildcard's placeholder is lowercase wherever it can stand.
        let flippable: Vec<Option<(usize, usize)>> = spans
            .iter()
            .zip(&vars)
            .map(|(span, v)| match &v.name {
                Some(name) if metavar::is_bare(name) => None,
                _ => Some(*span),
            })
            .collect();
        match nearest(&flippable, at) {
            Some(i) => cases[i] = cases[i].flipped(),
            // Nothing to flip — the pattern is wrong independently of casing.
            None => break,
        }
    }

    // `$$$A` is another tool's spelling for "any argument list", and Prism's
    // report of it ("unexpected constant") describes the symptom without naming
    // the cause. rwr spells a run of nodes `*$A`, which is real Ruby -- the
    // point of the metavariable syntax -- so say so rather than leaving the
    // caller to guess from a parser error.
    let message = if pattern.contains("$$") {
        format!(
            "{last_message}\n  `$$` is not rwr syntax. A run of nodes is `*$NAME` -- `foo(*$A)` matches any argument list, and `**$NAME` does the same inside a hash."
        )
    } else {
        last_message
    };
    Err(PrepareError::Unparseable { message })
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
        // Re-emit the sigil the metavariable's span swallowed, in the spelling
        // the author used -- `**` inside a hash, `*` elsewhere.
        source.push_str(var.sigil);
        let start = source.len();
        source.push_str(&cases[i].placeholder(i));
        spans.push((start, source.len()));
        cursor = var.end;
    }
    source.push_str(&pattern[cursor..]);
    (source, spans)
}

/// The method a bare `*`, `**` or `&` is parsed inside when the pattern alone
/// does not parse. Ruby accepts anonymous forwarding only where the enclosing
/// method declares it, so this keeps "a pattern is valid Ruby" true of them.
const WRAPPER: &str = "def _(*, **, &)\n";

/// Check that each bare `*`, `**` and `&` the scanner found is what Ruby calls
/// anonymous forwarding -- a splat with no operand in an argument list, or an
/// anonymous parameter -- by parsing the pattern with them left bare: alone,
/// else inside [`WRAPPER`]. A bare sigil anywhere else, say inside a string, is
/// refused rather than read as a wildcard.
fn confirm_bare(
    pattern: &str,
    vars: &[metavar::Metavar],
    cases: &[Case],
) -> Result<(), PrepareError> {
    if !vars
        .iter()
        .any(|v| v.name.as_deref().is_some_and(metavar::is_bare))
    {
        return Ok(());
    }
    // The pattern as written, metavariables substituted and bare sigils kept,
    // with where each bare sigil landed.
    let mut source = String::with_capacity(pattern.len());
    let mut bare = Vec::new();
    let mut cursor = 0;
    for (i, var) in vars.iter().enumerate() {
        source.push_str(&pattern[cursor..var.start]);
        match var.name.as_deref() {
            Some(sigil) if metavar::is_bare(sigil) => {
                bare.push((source.len(), sigil, var.start));
                source.push_str(sigil);
            }
            _ => {
                source.push_str(var.sigil);
                source.push_str(&cases[i].placeholder(i));
            }
        }
        cursor = var.end;
    }
    source.push_str(&pattern[cursor..]);

    let plain = ruby_prism::parse(source.as_bytes());
    let wrapped_source = format!("{WRAPPER}{source}\nend\n");
    let wrapped = ruby_prism::parse(wrapped_source.as_bytes());
    let first_error = |result: &ruby_prism::ParseResult<'_>, shift: usize| {
        result.errors().next().map(|d| {
            (
                d.location().start_offset().saturating_sub(shift),
                d.message().to_string(),
            )
        })
    };
    let (tree, shift) = match (first_error(&plain, 0), first_error(&wrapped, WRAPPER.len())) {
        (None, _) => (plain.node(), 0),
        (Some(_), None) => (wrapped.node(), WRAPPER.len()),
        // Report whichever parse got further into the pattern: a `class` the
        // wrapper cannot hold fails at once, while the pattern alone fails at
        // the bare sigil it has no method to forward from.
        (Some(alone), Some(inside)) => {
            let (_, message) = if inside.0 > alone.0 { inside } else { alone };
            return Err(PrepareError::Unparseable { message });
        }
    };

    let mut forwards = Vec::new();
    anonymous_forwards(&tree, None, &mut forwards);
    for (at, sigil, column) in bare {
        if !forwards.contains(&(at + shift, sigil)) {
            return Err(PrepareError::Unparseable {
                message: format!(
                    "the bare `{sigil}` at offset {column} is not an anonymous argument or \
                     parameter, so it cannot be a wildcard"
                ),
            });
        }
    }
    Ok(())
}

/// Where each anonymous argument and parameter in a tree starts, by sigil.
fn anonymous_forwards(node: &Node<'_>, parent: Option<&Node<'_>>, out: &mut Vec<(usize, &str)>) {
    let sigil = match node {
        // A splat with no operand also writes the rest of a multiple
        // assignment and matches it in a `case/in`; neither forwards anything.
        Node::SplatNode { .. }
            if node
                .as_splat_node()
                .is_some_and(|n| n.expression().is_none())
                && matches!(
                    parent,
                    Some(Node::ArgumentsNode { .. } | Node::ArrayNode { .. })
                ) =>
        {
            Some("*")
        }
        Node::RestParameterNode { .. }
            if node
                .as_rest_parameter_node()
                .is_some_and(|n| n.name().is_none()) =>
        {
            Some("*")
        }
        Node::AssocSplatNode { .. }
            if node
                .as_assoc_splat_node()
                .is_some_and(|n| n.value().is_none()) =>
        {
            Some("**")
        }
        Node::KeywordRestParameterNode { .. }
            if node
                .as_keyword_rest_parameter_node()
                .is_some_and(|n| n.name().is_none()) =>
        {
            Some("**")
        }
        Node::BlockArgumentNode { .. }
            if node
                .as_block_argument_node()
                .is_some_and(|n| n.expression().is_none()) =>
        {
            Some("&")
        }
        Node::BlockParameterNode { .. }
            if node
                .as_block_parameter_node()
                .is_some_and(|n| n.name().is_none()) =>
        {
            Some("&")
        }
        _ => None,
    };
    if let Some(sigil) = sigil {
        out.push((node.location().start_offset(), sigil));
    }
    for child in generated::children(node) {
        anonymous_forwards(&child, Some(node), out);
    }
}

/// The placeholder containing `at`, else the one closest to it, among those
/// that may be flipped.
fn nearest(spans: &[Option<(usize, usize)>], at: usize) -> Option<usize> {
    let candidates = spans
        .iter()
        .enumerate()
        .filter_map(|(i, span)| span.map(|s| (i, s)));
    if let Some((i, _)) = candidates.clone().find(|(_, (s, e))| at >= *s && at < *e) {
        return Some(i);
    }
    candidates
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

    /// D129. A bare `*`, `**` or `&` is a wildcard bound under its own
    /// spelling. Ruby accepts it only inside a method declaring anonymous
    /// parameters, so a pattern that does not parse alone is parsed inside one.
    #[test]
    fn bare_sigils_are_wildcards_bound_under_their_spelling() {
        let p = parses("$R.to_return(status: 200, **)");
        assert!(p.source.contains("**rwr_mv_"), "{:?}", p.source);
        assert!(p.bindings.values().any(|b| b.name.as_deref() == Some("**")));

        for pattern in [
            "foo(1, *)",
            "foo(&)",
            "foo(*, a: 1, **, &)",
            "def fetch(*, legacy:, **) = _",
            "[1].each { |a, *| a }",
            // A named metavariable in a name position beside a bare one.
            "x.$M(*)",
        ] {
            let p = parses(pattern);
            assert_eq!(
                ruby_prism::parse(p.source.as_bytes()).errors().count(),
                0,
                "{pattern:?} produced {:?}",
                p.source
            );
        }
    }

    /// One wildcard of each spelling, as one method declares one of each; a
    /// second would have to mean the same nodes again, or something else.
    #[test]
    fn a_second_bare_sigil_of_one_spelling_is_refused() {
        assert!(prepare("foo(*, *)").is_err());
        assert!(prepare("def m(**) = foo(**)").is_err());
        assert!(prepare("foo(*, **, &)").is_ok());
    }

    /// A bare sigil the scanner found must be Ruby's anonymous forwarding. One
    /// inside a string is text, and reading it as a wildcard would be a guess.
    #[test]
    fn a_bare_sigil_that_is_not_forwarding_is_refused() {
        let err = prepare(r#"foo("(*)")"#).unwrap_err().to_string();
        assert!(err.contains("bare `*`"), "{err}");
        // A splat with no operand that forwards nothing stays literal Ruby.
        assert!(parses("a, * = foo").bindings.is_empty());
    }

    /// The wrapper is an implementation detail: a pattern it cannot hold is
    /// reported in terms of the pattern, not of a method the user never wrote.
    #[test]
    fn a_wrapper_failure_is_reported_against_the_pattern() {
        let err = prepare("class Foo; bar(*); end").unwrap_err().to_string();
        assert!(!err.contains("method body"), "{err}");
        assert!(err.contains("anonymous"), "{err}");
    }

    /// Genuinely broken Ruby must be reported, not silently repaired forever.
    #[test]
    fn unparseable_patterns_are_rejected() {
        assert!(prepare("def foo(").is_err());
        assert!(prepare("$A +").is_err());
    }
}
