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
    /// The name appears in a comment.
    ///
    /// Reported, never rewritten. A name in prose may be a reference, an
    /// example, or an ordinary English word, and rwr cannot tell which --
    /// rewriting it would be a guess where reporting it is a fact. A rename
    /// that leaves `# returns the display_name` behind has left something
    /// stale, and saying so is the whole job of this report.
    Comment,
    /// Found by text search in a file rwr cannot parse -- a template, where
    /// Ruby is embedded rather than written.
    ///
    /// Deliberately its own class rather than mixed in with the rest: every
    /// other context is a fact about the parse tree, and this one is a string
    /// that looked right. Labelling it keeps the difference visible to whoever
    /// reads the report.
    Text,
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
    /// Whether this is a call with no explicit receiver.
    ///
    /// An implicit-self call dispatches on its *enclosing* class, which lexical
    /// scope already tells us -- so `Company#banner` calling its own `name` is
    /// not a reach for a rename of `User#name`, and reporting it is noise a
    /// scoped report can drop outright.
    #[serde(skip)]
    pub implicit: bool,
    /// The call this occurrence is an argument to, if any.
    ///
    /// What decides whether a symbol is a *reach*. `delegate :display_name` and
    /// `validates :display_name` hand a method name to something that will
    /// dispatch on it; a bare `:display_name` in an unrelated array does not.
    #[serde(skip)]
    pub via: Option<String>,
}

/// The class or module a node introduces, if any.
fn scope_name(node: &Node<'_>) -> Option<String> {
    // Shared with the matcher, so a rule and the report it produces agree about
    // which class a site sits in -- `class Account::Exporter` is
    // `Account::Exporter`, not `Exporter`.
    matcher::scope_name_of(node)
}

/// Calls whose symbol argument *defines* a method on the enclosing class rather
/// than referring to one defined elsewhere.
///
/// The distinction decides whether a symbol in some other class is a reach.
/// `delegate :display_name, to: :account` forwards to an Account and breaks
/// when Account's method is renamed; `attr_reader :display_name` in a Widget
/// makes Widget's own method and is untouched by it.
///
/// The bar for entry is strict, because `via` labels *every* argument of a call
/// with that call's name: a macro belongs here only if each name it takes lands
/// on the enclosing class. That is what keeps these out --
///
/// - `attribute :display_name` defines on an ActiveRecord model and *reaches*
///   on a serializer, identical syntax either way (testbed marks the serializer
///   one a reach, correctly);
/// - `has_many` / `belongs_to` define from the first symbol but take
///   `inverse_of:`, `foreign_key:` and `source:`, which name methods on
///   *another* class;
/// - `scope` defines the class method and carries a lambda whose body is full of
///   column names;
/// - `validates` and the callbacks refer rather than define -- to the enclosing
///   class, so the conclusion matches, but by a different mechanism that a
///   serializer's two-hop `validates` does not share.
const DEFINERS: &[&[u8]] = &[
    b"attr_reader",
    b"attr_accessor",
    b"attr_writer",
    b"define_method",
    b"alias_method",
    // Every name these take is the enclosing class's own. `class_attribute`
    // makes five methods from one symbol, `store_accessor` names the store
    // column and then its keys, `alias_attribute` names both sides locally, and
    // an `enum` defines predicates and bangs -- none of them reaches outward.
    b"class_attribute",
    b"store_accessor",
    b"alias_attribute",
    b"enum",
];

/// Narrow a report to what a class-anchored rule could plausibly be about.
///
/// This is the payoff of receiver narrowing: the reason an unscoped report
/// reaches thousands of entries is that it counts every unrelated class that
/// happens to share an identifier. Given the class the rule is about, two
/// things remain interesting -- anything inside that class, and any call whose
/// receiver could not be resolved, since those are exactly the sites narrowing
/// silently declined to rewrite.
pub(crate) fn scoped_to(
    occurrences: Vec<Occurrence>,
    class: &str,
    hierarchy: &crate::hierarchy::Hierarchy,
) -> Vec<Occurrence> {
    occurrences
        .into_iter()
        .filter(|o| {
            // An implicit-self call inside a class that is neither the target
            // nor one of its descendants cannot be the target method: lexical
            // scope has already named the receiver.
            if o.context == Context::Call
                && o.implicit
                && let Some(enclosing) = o.scope.last()
                && enclosing != class
                && !hierarchy.descends_from(enclosing, class)
                // A module mixed into the class dispatches on the class, so an
                // implicit call in its body reaches the target after all. This
                // guard runs before the keep-rules below, so without the check
                // here a concern's own methods were rejected early and never
                // reconsidered.
                && !hierarchy.contributes_to(enclosing, class)
            {
                return false;
            }
            matcher::enclosing_class(&o.scope).as_deref() == Some(class)
                // A concern's contribution is the class's own code, written
                // elsewhere. `included do`, an instance method in the module
                // body, a `prepend`ed override, a `refine` block -- all have an
                // enclosing scope that never equals the anchor class, so
                // comparing names literally dropped the whole category and said
                // nothing about having dropped it. In Rails that is where a
                // large share of a model's methods live.
                || o.scope
                    .iter()
                    .any(|s| hierarchy.contributes_to(s, class))
                // A definition in a *subclass* is the rule's business too: an
                // override the rename failed to reach is the one occurrence
                // guaranteed to break. `subclasses: true` was honoured by the
                // matcher and ignored here, so an override whose arity had
                // drifted from its parent's was neither rewritten nor reported
                // -- exit 0, with the work half done.
                // Anything written in a descendant is the rule's business: an
                // override the rename failed to reach is the one occurrence
                // guaranteed to break, and an `alias` or a symbol table in a
                // subclass names the same method the rule is moving.
                || matcher::enclosing_class(&o.scope)
                    .is_some_and(|enclosing| hierarchy.descends_from(&enclosing, class))
                || o.context == Context::Call
                // A symbol handed to a call is a reach wherever it lives, and
                // scoping it away lost the whole Rails metaprogramming
                // category: `delegate`, `validates` and `attribute` sit in a
                // *different* class from the one they name, by construction.
                // Measured on the testbed: recall went from 2 of 7 to 7 of 7.
                //
                // Except where the call *defines* a method rather than
                // referring to one. `attr_reader :name` in an unrelated class
                // creates that class's own `name`; it does not reach this one.
                || o.via
                    .as_deref()
                    .is_some_and(|call| !DEFINERS.contains(&call.as_bytes()))
        })
        .collect()
}

/// Whether a pattern rewrites a *definition* of a method.
///
/// This is what makes residue meaningful, and the name-shape test alone is not
/// enough. Residue answers "what breaks because this name moved", and a name
/// only moves when its definition does. `$R.gsub($F, $T)` -> `$R.tr($F, $T)`
/// looks exactly like a rename -- a literal name applied to metavariables --
/// but `String#gsub` still exists afterwards, so every `.gsub` the rule
/// declined to rewrite is perfectly fine. Reporting those as unaccounted-for
/// was a false claim, and it is what a real run hit first.
pub(crate) fn defines_a_method(pattern: &Node<'_>, prepared: &Prepared) -> bool {
    if matches!(pattern, Node::DefNode { .. }) {
        return true;
    }
    let Some(call) = pattern.as_call_node() else {
        return false;
    };
    // A macro that defines a method counts too: renaming `attr_reader :old` to
    // `attr_reader :new` moves the name just as `def` does.
    DEFINERS.contains(&call.name().as_slice())
        && matcher::placeholder_name(pattern, prepared).is_none()
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
        || matcher::splat_placeholder_name(node, prepared).is_some()
}

/// Occurrences of `anchors` inside comments.
///
/// Comments are not in the tree -- Prism carries them alongside it -- so they
/// need their own pass. Without one, a rename silently leaves every doc comment
/// that named the method stale, and the account that claims to list what was
/// left over does not mention them at all.
pub(crate) fn in_comments(
    parsed: &ruby_prism::ParseResult<'_>,
    anchors: &[Vec<u8>],
    source: &[u8],
) -> Vec<Occurrence> {
    // A comment is not in the tree, so its lexical scope has to come from its
    // position. Without it every comment would escape the class scoping that
    // keeps the rest of the report from filling the screen.
    let mut enclosing: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut stack: Vec<(Node<'_>, Vec<String>)> =
        vec![(generated::dup(&parsed.node()), Vec::new())];
    while let Some((node, here)) = stack.pop() {
        let mut inner = here.clone();
        if let Some(name) = scope_name(&node) {
            inner.push(name);
            let location = node.location();
            enclosing.push((
                location.start_offset(),
                location.end_offset(),
                inner.clone(),
            ));
        }
        for child in generated::children(&node) {
            stack.push((child, inner.clone()));
        }
    }

    let mut out = Vec::new();
    for comment in parsed.comments() {
        let location = comment.location();
        let (start, end) = (location.start_offset(), location.end_offset());
        let Some(text) = source.get(start..end) else {
            continue;
        };
        // The innermost class or module whose body contains the comment.
        let scope = enclosing
            .iter()
            .filter(|(from, to, _)| start >= *from && end <= *to)
            .min_by_key(|(from, to, _)| to - from)
            .map(|(_, _, names)| names.clone())
            .unwrap_or_default();

        for anchor in anchors {
            for at in crate::source::identifier_offsets(text, anchor) {
                out.push(Occurrence {
                    context: Context::Comment,
                    byte_start: start + at,
                    byte_end: start + at + anchor.len(),
                    scope: scope.clone(),
                    implicit: false,
                    via: None,
                });
            }
        }
    }
    out.sort_by_key(|o| o.byte_start);
    out
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
    let mut stack: Vec<(Node<'_>, Vec<String>, Option<String>)> =
        vec![(generated::dup(root), Vec::new(), None)];
    while let Some((node, here, via)) = stack.pop() {
        let loc = node.location();
        let (start, end) = (loc.start_offset(), loc.end_offset());

        let mut implicit_self = false;
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
                .map(|c| (c.name().as_slice().to_vec(), c.receiver().is_none()))
                .filter(|(v, _)| anchors.contains(v))
                .map(|(_, implicit)| {
                    implicit_self = implicit;
                    Context::Call
                }),
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
                implicit: implicit_self,
                via: via.clone(),
            });
        }

        let mut inner = here;
        if let Some(name) = scope_name(&node) {
            inner.push(name);
        }
        // A call re-labels its argument subtrees with its own name, and clears
        // the label everywhere else -- a receiver or a block body is not an
        // argument. Identified by span, since the children accessor is generic.
        let arguments = node.as_call_node().and_then(|c| c.arguments()).map(|a| {
            let name = String::from_utf8_lossy(
                node.as_call_node().map_or(&[][..], |c| c.name().as_slice()),
            )
            .into_owned();
            (a.location().start_offset(), a.location().end_offset(), name)
        });
        // A hash key names a parameter, not a method to dispatch on. Without
        // this, every `render json: { name: x }` in the corpus counted as a
        // reach for a rename of `name` -- 57% of a 15,587-entry report on
        // discourse was keyword keys.
        let key_span = node
            .as_assoc_node()
            .map(|a| a.key().location())
            .map(|l| (l.start_offset(), l.end_offset()));

        for child in generated::children(&node) {
            if let Some((start, end)) = key_span {
                let loc = child.location();
                if loc.start_offset() == start && loc.end_offset() == end {
                    stack.push((child, inner.clone(), None));
                    continue;
                }
            }
            let child_via = match &arguments {
                Some((start, end, name)) => {
                    let loc = child.location();
                    (loc.start_offset() >= *start && loc.end_offset() <= *end).then(|| name.clone())
                }
                // Not a call: whatever label we arrived with still applies, so a
                // symbol nested in an array inside `delegate(...)` keeps it.
                None => via.clone(),
            };
            stack.push((child, inner.clone(), child_via));
        }
    }
    out.sort_by_key(|o| o.byte_start);
    out
}

#[cfg(test)]
mod tests {

    /// Residue survives the prefilter even though the engine wires in no
    /// anchors.
    ///
    /// `Filter::may_contribute` checks required literals conjunctively OR the
    /// anchors, because residue is reported from files a rule does *not* match.
    /// `Engine::new` passes `&[]` for anchors, which reads like a silent loss of
    /// exactly that report -- and is not: `anchors` only returns a name when the
    /// pattern is that name applied to metavariables, so the anchor is the
    /// pattern's only literal identifier and the required check already covers
    /// it.
    ///
    /// Pinned because the invariant holds by coincidence rather than by
    /// construction. The day `anchors` returns something `required` does not
    /// extract, the engine silently stops reporting the blind spots it exists
    /// for, and this is the test that says so.
    #[test]
    fn an_anchor_is_always_one_of_the_required_literals() {
        for pattern in ["$R.display_name", "display_name", "$R.display_name($A)"] {
            let prepared = crate::pattern::prepare::prepare(pattern).expect("prepares");
            let parsed = ruby_prism::parse(prepared.source.as_bytes());
            let root = matcher::pattern_root(&parsed.node()).expect("one expression");
            let found = anchors(&root, &prepared);
            assert!(!found.is_empty(), "{pattern} should anchor");

            let required = crate::pattern::prefilter::required(pattern);
            for a in &found {
                let a = String::from_utf8_lossy(a).into_owned();
                assert!(
                    required.contains(&a),
                    "{pattern}: anchor {a:?} missing from required {required:?} -- \
                     Engine::new must start passing anchors to Filter::new"
                );
            }
        }
    }
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

        let scoped = scoped_to(all, "Account", &crate::hierarchy::Hierarchy::default());
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

    /// A rename leaves every doc comment that named the method stale, and
    /// comments are not in the tree -- so without a pass of their own the
    /// report that claims to list what was left over never mentions them.
    #[test]
    fn comments_that_name_the_method_are_reported() {
        let src = "class Account\n  # Returns the display_name.\n                     def display_name; 1; end\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let found = in_comments(&parsed, &[b"display_name".to_vec()], src.as_bytes());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].context, Context::Comment);
        // Scoped by position, since a comment has no place in the tree to read
        // its scope from -- without this every comment escapes class scoping.
        assert_eq!(found[0].scope, vec!["Account".to_string()]);
    }

    /// Whole identifiers only. `display_names` is a different word, and a
    /// report that cannot tell them apart is one people stop reading.
    #[test]
    fn a_longer_word_is_not_the_name() {
        let src = "# display_names and display_name_for\nx = 1\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        assert!(in_comments(&parsed, &[b"display_name".to_vec()], src.as_bytes()).is_empty());
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
