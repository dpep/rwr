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
    /// Enclosing class and module names, outermost first.
    #[serde(skip)]
    pub scope: Vec<String>,
}

/// The class or module a node introduces, if any.
fn scope_name(node: &Node<'_>) -> Option<String> {
    let name = match node {
        Node::ClassNode { .. } => node.as_class_node()?.name().as_slice().to_vec(),
        Node::ModuleNode { .. } => node.as_module_node()?.name().as_slice().to_vec(),
        _ => return None,
    };
    String::from_utf8(name).ok()
}

/// Narrow a report to what a class-anchored rule could plausibly be about.
///
/// This is the payoff of receiver narrowing: the reason an unscoped report
/// reaches thousands of entries is that it counts every unrelated class that
/// happens to share an identifier. Given the class the rule is about, two
/// things remain interesting -- anything inside that class, and any call whose
/// receiver could not be resolved, since those are exactly the sites narrowing
/// silently declined to rewrite.
pub(crate) fn scoped_to(occurrences: Vec<Occurrence>, class: &str) -> Vec<Occurrence> {
    occurrences
        .into_iter()
        .filter(|o| o.scope.iter().any(|s| s == class) || o.context == Context::Call)
        .collect()
}

/// The identifier a rule is anchored on, if it is anchored on one at all.
///
/// Residue applies to **name-anchored rules only** (D7 amended): a rename has a
/// target identifier, and every occurrence of that identifier the rule did not
/// convert is a site that will break. `return nil` -> `return` has no such
/// target, and neither does a rule about a *shape*.
///
/// The distinction is whether the pattern is that name applied to
/// metavariables, or an expression that merely contains it. `$R.display_name`
/// is about `display_name`; `$R.select { |$P| $B }.first` is about a chain, and
/// treating `first` as its anchor reported every `.first` in the repo -- 3,752
/// of them on Discourse, which buries the account it exists to give.
pub(crate) fn anchors(pattern: &Node<'_>, prepared: &Prepared) -> Vec<Vec<u8>> {
    // Only the root: a literal name deeper in the pattern is part of a shape,
    // not the thing the rule is about.
    let Some(call) = pattern.as_call_node() else {
        return Vec::new();
    };
    if call.message_loc().is_none() || matcher::placeholder_name(pattern, prepared).is_some() {
        return Vec::new();
    }
    // A real receiver or argument makes the rule about a shape. Blocks are
    // structure too: `$R.each { |$P| $B }` is not a rule about `each`.
    if call.block().is_some() {
        return Vec::new();
    }
    if let Some(receiver) = call.receiver()
        && !is_metavariable(&receiver, prepared)
    {
        return Vec::new();
    }
    if let Some(arguments) = call.arguments()
        && !arguments
            .arguments()
            .iter()
            .all(|a| is_metavariable(&a, prepared))
    {
        return Vec::new();
    }
    vec![call.name().as_slice().to_vec()]
}

/// Whether a node stands for whatever it matched, rather than for itself.
fn is_metavariable(node: &Node<'_>, prepared: &Prepared) -> bool {
    matcher::placeholder_name(node, prepared).is_some()
        || matcher::is_splat_placeholder(node, prepared)
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
    // Depth-paired stack so each occurrence carries the lexical scope it sits
    // in, which is what lets a class-anchored rule scope its own report.
    let mut stack: Vec<(Node<'_>, Vec<String>)> = vec![(generated::dup(root), Vec::new())];
    while let Some((node, here)) = stack.pop() {
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
                scope: here.clone(),
            });
        }

        let mut inner = here;
        if let Some(name) = scope_name(&node) {
            inner.push(name);
        }
        stack.extend(
            generated::children(&node)
                .into_iter()
                .map(|c| (c, inner.clone())),
        );
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
        let hits = matcher::search(
            &p_root,
            &parsed.node(),
            &prepared,
            &matcher::Criteria::none(),
        );
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

    /// The payoff of receiver narrowing: a class-anchored rule scopes its own
    /// report, so the unrelated classes that make an unscoped report reach
    /// thousands of entries fall away -- while unresolved calls, the sites
    /// narrowing silently declined to rewrite, are kept.
    #[test]
    fn a_class_anchor_scopes_the_report() {
        let src = "class Account\n  def display_name; 1; end\nend\nclass Widget\n  attr_reader :display_name\nend\nthing.display_name\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let anchors = vec![b"display_name".to_vec()];
        let all = find(&parsed.node(), &anchors, &[], src.as_bytes());

        let widget_symbol = all.iter().filter(|o| o.context == Context::Symbol).count();
        assert_eq!(widget_symbol, 1, "unscoped report includes Widget's symbol");

        let scoped = scoped_to(all, "Account");
        assert!(
            scoped.iter().all(|o| o.context != Context::Symbol),
            "Widget's symbol should fall away"
        );
        assert!(
            scoped.iter().any(|o| o.context == Context::Definition),
            "Account's own definition must survive"
        );
        assert!(
            scoped.iter().any(|o| o.context == Context::Call),
            "an unresolved call is a blind spot and must survive"
        );
    }

    /// A rule with no identifier to track reports nothing. That is the correct
    /// answer, not a gap -- `return nil -> return` has no name to follow.
    #[test]
    fn rules_without_an_anchor_report_nothing() {
        assert!(residue_of("return nil", "return nil\n:return\n").is_empty());
    }

    /// A rule about a *shape* is not name-anchored either.
    ///
    /// `$R.select { |$P| $B }.first` rewrites a chain; a bare `.first` elsewhere
    /// is a different program, not a site the rule failed to convert. Treating
    /// the chain's method names as anchors reported 3,752 occurrences on
    /// Discourse, burying the account residue exists to give.
    #[test]
    fn a_shape_rule_is_not_name_anchored() {
        let src = "xs.select { |i| i.ok? }.first\nys.first\nzs.select { |i| i.ok? }\n";
        assert!(residue_of("$R.select { |$P| $B }.first", src).is_empty());
    }

    /// The narrowing must not go so far that a rename stops reporting: the
    /// pattern is the name applied to a metavariable, which is the anchored case.
    #[test]
    fn a_rename_with_arguments_is_still_name_anchored() {
        let src = "a.set_size(1)\ndelegate :set_size, to: :account\n";
        assert_eq!(residue_of("$R.set_size($A)", src), vec![Context::Symbol]);
    }

    /// Matched sites are accounted for and must not be reported back as
    /// residue, or the report is noise.
    #[test]
    fn matched_sites_are_not_residue() {
        assert!(residue_of("$R.display_name", "a.display_name\n").is_empty());
    }
}
