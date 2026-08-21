//! Name-scoped residue reporting (D7 as amended).
//!
//! The account of what rwr *could not* see. For a rule anchored on an
//! identifier -- a rename, a signature change -- enumerate every remaining
//! occurrence of that identifier the structural match did not account for, and
//! classify it by syntactic context.
//!
//! This is what a careful person does with `rg` after a rename: lexical and
//! AST context, no dataflow. Rails metaprogramming overwhelmingly flows
//! *literal* symbols through macros (`delegate`, `attr_*`, `alias_method`), so
//! the classifiable fraction is large.
//!
//! **Scope:** name-anchored rules only. `return nil -> return` has no
//! identifier to track and reports nothing, which is correct rather than a gap.

use crate::pattern::generated;
use crate::pattern::matcher;
use crate::pattern::prepare::Prepared;
use ruby_prism::Node;
use serde::Serialize;

/// Where an unaccounted-for occurrence of the anchor turned up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Context {
    /// `:foo` -- the commonest way a name reaches metaprogramming.
    Symbol,
    /// `"foo"` or `'foo'`.
    String,
    /// A call by that name the rule did not match, e.g. a different receiver.
    Call,
    /// A definition of that name.
    Definition,
}

/// One occurrence the rule did not account for.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Occurrence {
    pub context: Context,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Literal identifiers a pattern is anchored on.
///
/// A method name written out in the pattern is an anchor; a metavariable in
/// name position is not, since it stands for whatever it matched.
pub(crate) fn anchors(pattern: &Node<'_>, prepared: &Prepared) -> Vec<Vec<u8>> {
    let mut found: Vec<Vec<u8>> = Vec::new();
    let mut stack = vec![generated::dup(pattern)];
    while let Some(node) = stack.pop() {
        if matcher::placeholder_name(&node, prepared).is_none()
            && let Some(call) = node.as_call_node()
            && call.message_loc().is_some()
        {
            let name = call.name().as_slice().to_vec();
            if !found.contains(&name) {
                found.push(name);
            }
        }
        stack.extend(generated::children(&node));
    }
    found
}

/// Occurrences of `anchors` that fall outside every matched range.
pub(crate) fn find(
    root: &Node<'_>,
    anchors: &[Vec<u8>],
    matched: &[(usize, usize)],
    source: &[u8],
) -> Vec<Occurrence> {
    if anchors.is_empty() {
        return Vec::new();
    }
    let covered = |start: usize| matched.iter().any(|(s, e)| start >= *s && start < *e);

    let mut out = Vec::new();
    let mut stack = vec![generated::dup(root)];
    while let Some(node) = stack.pop() {
        let loc = node.location();
        let (start, end) = (loc.start_offset(), loc.end_offset());

        let context = match &node {
            Node::SymbolNode { .. } => node
                .as_symbol_node()
                .and_then(|s| s.unescaped().to_vec().into())
                .filter(|v: &Vec<u8>| anchors.contains(v))
                .map(|_| Context::Symbol),
            Node::StringNode { .. } => node
                .as_string_node()
                .map(|s| s.unescaped().to_vec())
                .filter(|v| anchors.contains(v))
                .map(|_| Context::String),
            Node::CallNode { .. } => node
                .as_call_node()
                .filter(|c| c.message_loc().is_some())
                .map(|c| c.name().as_slice().to_vec())
                .filter(|v| anchors.contains(v))
                .map(|_| Context::Call),
            Node::DefNode { .. } => node
                .as_def_node()
                .map(|d| d.name().as_slice().to_vec())
                .filter(|v| anchors.contains(v))
                .map(|_| Context::Definition),
            _ => None,
        };

        if let Some(context) = context
            && !covered(start)
        {
            out.push(Occurrence {
                context,
                byte_start: start,
                byte_end: end.min(source.len()),
            });
        }
        stack.extend(generated::children(&node));
    }
    out.sort_by_key(|o| o.byte_start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{matcher, prepare};

    fn residue_of(pattern: &str, source: &str) -> Vec<Context> {
        let prepared = prepare::prepare(pattern).expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let p_root = matcher::pattern_root(&p_node).expect("single expression");
        let anchors = anchors(&p_root, &prepared);

        let parsed = ruby_prism::parse(source.as_bytes());
        let hits = matcher::search(&p_root, &parsed.node(), &prepared);
        let matched: Vec<(usize, usize)> = hits
            .iter()
            .map(|m| {
                let l = m.node.location();
                (l.start_offset(), l.end_offset())
            })
            .collect();

        find(&parsed.node(), &anchors, &matched, source.as_bytes())
            .into_iter()
            .map(|o| o.context)
            .collect()
    }

    /// The point of the whole feature: a rename that matches every syntactic
    /// call still misses the symbol a macro dispatches through, and rwr says so.
    #[test]
    fn symbols_reaching_metaprogramming_are_reported() {
        let src = "a.display_name\ndelegate :display_name, to: :account\n";
        assert_eq!(residue_of("$R.display_name", src), vec![Context::Symbol]);
    }

    #[test]
    fn strings_are_reported() {
        let src = "a.display_name\nsend(\"display_name\")\n";
        let found = residue_of("$R.display_name", src);
        assert!(found.contains(&Context::String), "{found:?}");
    }

    #[test]
    fn the_definition_is_reported() {
        let src = "class A\n  def display_name\n    1\n  end\nend\na.display_name\n";
        let found = residue_of("$R.display_name", src);
        assert!(found.contains(&Context::Definition), "{found:?}");
    }

    /// A rule with no identifier to track reports nothing. That is the correct
    /// answer, not a gap -- `return nil -> return` has no name to follow.
    #[test]
    fn rules_without_an_anchor_report_nothing() {
        assert!(residue_of("return nil", "return nil\n:return\n").is_empty());
    }

    /// Matched sites are accounted for and must not be reported back as
    /// residue, or the report is noise.
    #[test]
    fn matched_sites_are_not_residue() {
        assert!(residue_of("$R.display_name", "a.display_name\n").is_empty());
    }
}
