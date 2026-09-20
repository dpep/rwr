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
    /// `"foo"` or `'foo'` -- a string that *is* the name, which is how a name
    /// most often reaches `send`.
    String,
    /// The name appears *inside* a longer string: an error message, a log line,
    /// a spec description.
    ///
    /// A different thing from [`Context::String`], and the difference is what
    /// the reader does about it. A string that is the name may be a live
    /// dispatch and will break; a name mentioned in one is documentation and
    /// will go stale. `raise ArgumentError, "have_fetched reads a Router's
    /// trace"` names a method that a rename has moved, and it is the text a
    /// developer sees when they get it wrong.
    ///
    /// Reported, never rewritten, for [`Context::Comment`]'s reason: prose that
    /// contains an identifier may be naming it or may be using an ordinary
    /// word, and rwr cannot tell. Reporting is a fact; rewriting would be a
    /// guess.
    Prose,
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
    /// A dispatch whose method name is *computed* -- `send("display_#{x}")`,
    /// `define_method("formatted_#{attr}")`.
    ///
    /// Not an occurrence of the name: the name is not there, and rwr cannot say
    /// whether this reaches the renamed method. Reported anyway, because the
    /// alternative is a report that looks complete while a class dispatches on
    /// names nobody can enumerate. The account of blind spots is the product,
    /// and this is a blind spot with a location.
    Dynamic,
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
    // `attr` is the same thing as `attr_reader` for this question: every symbol
    // it takes names a method on the enclosing class. Its second argument
    // decides whether a writer comes too (`attr :x, true`), and that argument is
    // not a name, so it is never labelled either way. Left out, an unrelated
    // class's own attribute read as a reach.
    b"attr",
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

/// Every class a scope stack names, outermost first and fully qualified.
///
/// `["App", "Helpers", "Numeric"]` names `App`, `App::Helpers` and
/// `App::Helpers::Numeric`. Asking the hierarchy about the bare segments
/// instead happened to work while the module's only spelling left one candidate
/// to widen to, and stopped the moment the same module was also written
/// relatively somewhere -- a written constant read literally where the rest of
/// the run reads it lexically (D100).
fn enclosing_classes(scope: &[String]) -> Vec<String> {
    (1..=scope.len())
        .filter_map(|n| matcher::enclosing_class(&scope[..n]))
        .collect()
}

/// The class an implicit-self call in this scope dispatches on, qualified.
///
/// A `class << self` body keeps the marker instead of a class name: `self` is
/// the class object there, so a bare call is a singleton method and never the
/// instance one a `#` rule names, and the marker matches no class.
fn implicit_receiver(scope: &[String]) -> Option<String> {
    match scope.last() {
        Some(last) if last == matcher::SINGLETON => Some(last.clone()),
        _ => matcher::enclosing_class(scope),
    }
}

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
                && let Some(enclosing) = implicit_receiver(&o.scope)
                && enclosing != class
                && !hierarchy.descends_from(&enclosing, class)
                // A module mixed into the class dispatches on the class, so an
                // implicit call in its body reaches the target after all. This
                // guard runs before the keep-rules below, so without the check
                // here a concern's own methods were rejected early and never
                // reconsidered.
                && !hierarchy.contributes_to(&enclosing, class)
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
                || enclosing_classes(&o.scope)
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
                // A dispatcher's own name is not a reason to keep it: `send`
                // appears in every class, and a computed name in an unrelated
                // one says nothing about this rename. Only the scope rules above
                // keep a `Dynamic`, which puts it in the target class, a
                // descendant, or a module mixed into it -- where it is a caveat
                // worth reading. Unscoped it was 12% of a real report.
                || (o.context != Context::Dynamic
                    && o.via
                        .as_deref()
                        .is_some_and(|call| !DEFINERS.contains(&call.as_bytes())))
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

/// Calls that dispatch on a method name given as a value.
///
/// A literal argument is already handled: it either names the anchor and is
/// reported (or rewritten), or it names some other method and is irrelevant. It
/// is the *non*-literal argument that nothing can see.
const DISPATCHERS: &[&[u8]] = &[
    b"send",
    b"public_send",
    b"__send__",
    b"try",
    b"try!",
    b"method",
    b"define_method",
    b"instance_method",
    b"method_defined?",
    b"respond_to?",
];

/// Whether a computed name could be one of `anchors`, at all.
///
/// The direction matters and only one of the two is sound. Concluding that
/// `send("display_#{x}")` *does* reach `display_name` would be a guess -- `x`
/// may only ever be `title`. Concluding that `send("get_#{x}")` *cannot* reach
/// it is a proof: every name that expression produces begins `get_`, and
/// `display_name` does not, so no value of `x` yields it.
///
/// So this rules entries out and never in. Anything with no static text to
/// reason from -- a bare `send(x)`, a computed prefix -- is kept, because
/// unknown is not the same as impossible and a blind spot dropped in silence is
/// the failure this whole report exists to prevent.
fn may_produce(node: &Node<'_>, anchors: &[Vec<u8>]) -> bool {
    let Some(interpolated) = node.as_interpolated_string_node() else {
        return true;
    };
    let parts: Vec<Node<'_>> = interpolated.parts().iter().collect();
    // Only the outermost literal run each side is load-bearing: what sits
    // between interpolations constrains the middle of the name, and matching
    // that is a longer argument for a smaller gain.
    let literal = |part: &Node<'_>| {
        part.as_string_node()
            .map(|s| s.unescaped().to_vec())
            .filter(|bytes| !bytes.is_empty())
    };
    let prefix = parts.first().and_then(literal).unwrap_or_default();
    let suffix = match parts.len() {
        0 | 1 => Vec::new(),
        _ => parts.last().and_then(literal).unwrap_or_default(),
    };
    if prefix.is_empty() && suffix.is_empty() {
        return true;
    }
    anchors.iter().any(|name| {
        // An interpolation can be the empty string, so the two ends may meet but
        // never overlap -- which is what the length check enforces.
        name.len() >= prefix.len() + suffix.len()
            && name.starts_with(&prefix)
            && name.ends_with(&suffix)
    })
}

/// Mentions of the anchors inside one literal's content span.
///
/// Scanned over the raw source slice rather than the unescaped bytes, so an
/// offset here is an offset in the file; `content_loc` is also what keeps a
/// heredoc pointing at its body rather than at its opening tag (D14).
fn mentions(
    source: &[u8],
    span: (usize, usize),
    anchors: &[Vec<u8>],
    matched: &[(usize, usize)],
    scope: &[String],
) -> Vec<Occurrence> {
    let (from, to) = span;
    let Some(text) = source.get(from..to) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for anchor in anchors {
        for at in crate::source::identifier_offsets(text, anchor) {
            let start = from + at;
            if matched.iter().any(|(s, e)| start >= *s && start < *e)
                || !standalone(text, at, anchor.len())
            {
                continue;
            }
            out.push(Occurrence {
                context: Context::Prose,
                byte_start: start,
                byte_end: start + anchor.len(),
                scope: scope.to_vec(),
                implicit: false,
                via: None,
            });
        }
    }
    out
}

/// The method name a literal argument spells, if it spells one.
///
/// `attr_reader :display_name` and `alias_method "new", "old"` both name
/// methods; `attr :x, true` carries an argument that is not a name at all, and
/// `enum status: { ... }` a hash whose keys are names this does not reach.
fn literal_name(node: &Node<'_>) -> Option<Vec<u8>> {
    let name = match node {
        Node::SymbolNode { .. } => node.as_symbol_node()?.unescaped().to_vec(),
        Node::StringNode { .. } => node.as_string_node()?.unescaped().to_vec(),
        _ => return None,
    };
    (!name.is_empty()).then_some(name)
}

/// Whether a name is one of the pattern's placeholders rather than a name.
fn is_placeholder_text(name: &[u8], prepared: &Prepared) -> bool {
    std::str::from_utf8(name).is_ok_and(|text| prepared.bindings.contains_key(text))
}

/// Whether a node is a literal the anchor scan can already see.
fn is_literal_name(node: &Node<'_>) -> bool {
    match node {
        Node::SymbolNode { .. } => true,
        // An interpolated string is not a literal: `"display_#{x}"` parses as an
        // InterpolatedStringNode, which is exactly the case worth reporting.
        Node::StringNode { .. } => true,
        _ => false,
    }
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
    // A `def` names exactly what it renames, and is the shape a hand-written
    // rename most obviously takes. Without this the rule claimed completeness --
    // `defines_a_method` says a DefNode does -- and then had nothing to search
    // for, so `residue: []` meant "nothing was looked for" while reading as
    // "nothing was left over". A rename with no account of what it missed is
    // the one case the account exists for.
    if let Some(def) = pattern.as_def_node() {
        let name = def.name().as_slice().to_vec();
        // `def $M(...)` renames whatever it matched, so there is no one name to
        // anchor on.
        return match std::str::from_utf8(&name) {
            Ok(text) if prepared.bindings.contains_key(text) => Vec::new(),
            _ => vec![name],
        };
    }

    // Only the root: a literal name deeper in the pattern is part of a shape,
    // not the thing the rule is about.
    let Some(call) = pattern.as_call_node() else {
        return Vec::new();
    };
    if call.message_loc().is_none() || matcher::placeholder_name(pattern, prepared).is_some() {
        return Vec::new();
    }

    // A macro definer names what it creates in its arguments, and
    // `defines_a_method` already counts it -- so without this the rule claimed
    // completeness with nothing to search for, and `residue: []` meant "nothing
    // was looked for" while reading as "nothing was left over". D97 fixed that
    // for `def` and left `attr_reader`, `define_method` and `alias_method`
    // behind; the shape rules below reject all three, on a block or on an
    // argument that is a literal rather than a metavariable.
    if DEFINERS.contains(&call.name().as_slice()) {
        let Some(arguments) = call.arguments() else {
            return Vec::new();
        };
        return arguments
            .arguments()
            .iter()
            .filter_map(|a| literal_name(&a))
            // A placeholder stands for whatever it matched, so it fixes no name
            // -- the answer `def $M` gives.
            .filter(|name| !is_placeholder_text(name, prepared))
            .collect();
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

/// Every literal whose presence in a file means that file may hold residue.
///
/// The anchor, and -- where the pattern moves a definition -- the dispatchers
/// too. A computed name is a blind spot with a location (D85), and the bytes
/// that locate it belong to the dispatcher, never to the anchor:
/// `public_send("display_#{attr}")` contains no part of `display_name`. So a
/// prefilter built from the anchor alone drops the file, and with it the one
/// report it could have produced. Splitting a dispatcher from the definition it
/// reaches -- a decorator, a serializer, a form object -- is ordinary app
/// structure, and the same code in one file was reported.
///
/// Read off `DISPATCHERS`, the list [`find`] scans with, so the filter in front
/// of the collector cannot fall behind it.
pub(crate) fn reach(pattern: &Node<'_>, prepared: &Prepared) -> Vec<Vec<u8>> {
    let mut out = anchors(pattern, prepared);
    // With no anchor there is no report, so there is nothing to admit a file
    // for; and a rule that moves no definition claims no completeness (D7), so
    // its residue never runs.
    if !out.is_empty() && defines_a_method(pattern, prepared) {
        out.extend(DISPATCHERS.iter().map(|d| d.to_vec()));
    }
    out
}

/// Whether an identifier inside a string is a mention of a name, rather than one
/// segment of a qualified one.
///
/// `t("accounts.display_name")` is an i18n key and `"admin/display_name"` a
/// path; neither is prose naming the method, and the testbed marks both
/// `GT:ignore`. A dotted or slashed neighbour is what separates them from
/// `"display_name reads a Router trace"`, which is exactly the sentence a rename
/// leaves stale.
fn standalone(text: &[u8], at: usize, len: usize) -> bool {
    let qualifier = |b: u8| b == b'.' || b == b'/' || b == b':';
    let before = at.checked_sub(1).and_then(|i| text.get(i));
    let after = text.get(at + len);
    !before.is_some_and(|b| qualifier(*b)) && !after.is_some_and(|b| qualifier(*b))
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
        // A directive is an instruction addressed to rwr, not prose about the
        // code, so it is not a blind spot (D72). Skipped here rather than
        // filtered later, because a directive names the id it suppresses --
        // so it reported *itself* as residue a human should review, and the
        // only way to drain it was to delete the suppression.
        if crate::suppress::is_directive(&String::from_utf8_lossy(text)) {
            continue;
        }
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

        // A dispatcher handed a name it computes. Emitted regardless of the
        // anchors, because there is no anchor to match -- the point is that
        // *something* here dispatches on a name rwr cannot enumerate, and a
        // report that stays silent about it claims a completeness it does not
        // have.
        if let Some(call) = node.as_call_node()
            && DISPATCHERS.contains(&call.name().as_slice())
            && let Some(arguments) = call.arguments()
            && let Some(first) = arguments.arguments().iter().next()
            && !is_literal_name(&first)
            && may_produce(&first, anchors)
        {
            let at = first.location();
            out.push(Occurrence {
                context: Context::Dynamic,
                byte_start: at.start_offset(),
                byte_end: at.end_offset(),
                scope: here.clone(),
                implicit: false,
                via: Some(String::from_utf8_lossy(call.name().as_slice()).into_owned()),
            });
        }

        // A name mentioned inside a longer string. Scanned over the raw source
        // slice rather than the unescaped bytes, so an offset here is an offset
        // in the file; `content_loc` is also what keeps a heredoc pointing at
        // its body rather than at its opening tag (D14).
        if let Some(string) = node.as_string_node() {
            let content = string.content_loc();
            let (from, to) = (content.start_offset(), content.end_offset());
            // Trimmed, so a heredoc body that is just the name counts as being
            // the name rather than as prose containing it -- the trailing
            // newline is layout, not content.
            let whole = string.unescaped();
            let whole = whole.trim_ascii().to_vec();
            // A string that *is* the name is a dispatch, reported below as
            // `String`. Only the rest are mentions.
            if !anchors.contains(&whole) {
                out.extend(mentions(source, (from, to), anchors, matched, &here));
            }
        }

        // A regexp is a literal too, and a name written into one goes stale the
        // way one in a string does. Its own arm because a regexp is not a
        // `StringNode`, which is why nothing scanned it at all.
        //
        // The word boundaries are prose's, so a metacharacter that looks like an
        // identifier hides the name beside it: `/\Adisplay_name\z/` reads as the
        // word `Adisplay_name`. Escape-aware boundaries are a regexp-syntax
        // problem, not a prose one, and are not solved here.
        if let Some(regexp) = node.as_regular_expression_node() {
            let content = regexp.content_loc();
            let span = (content.start_offset(), content.end_offset());
            out.extend(mentions(source, span, anchors, matched, &here));
        }

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

        let mut inner = here.clone();
        if let Some(name) = scope_name(&node) {
            inner.push(name);
        }

        if let Some(context) = context
            && !covered(start)
        {
            out.push(Occurrence {
                context,
                byte_start: start,
                byte_end: end.min(source.len()),
                // A *definition* belongs to the class it defines on, which is
                // not always the one it is written inside: `def Foo.bar` in
                // `class Bar` defines on Foo. Recording it against the enclosing
                // scope filed it under Bar, so a rename of `Foo.bar` reported no
                // residue for a definition it had not rewritten -- a report
                // claiming a completeness it did not have. Every other context
                // is a *reference*, which does sit where it is written.
                scope: match node {
                    Node::DefNode { .. } => inner.clone(),
                    _ => here.clone(),
                },
                implicit: implicit_self,
                via: via.clone(),
            });
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
        // A constant assigned a list of symbols is a name table -- `COLUMNS =
        // %i[display_name email]`, read back through `public_send`. It is the
        // most ordinary dynamic reach a legacy exporter has, stated entirely in
        // literals, and it was dropped because the symbols are an argument to
        // nothing: the array is `freeze`'s receiver, or the write's value.
        let constant = match &node {
            Node::ConstantWriteNode { .. } => node
                .as_constant_write_node()
                .map(|c| String::from_utf8_lossy(c.name().as_slice()).into_owned()),
            _ => None,
        };

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
                // symbol nested in an array inside `delegate(...)` keeps it --
                // or the constant this subtree is being assigned to.
                None => constant.clone().or_else(|| via.clone()),
            };
            stack.push((child, inner.clone(), child_via));
        }
    }
    out.sort_by_key(|o| o.byte_start);
    out
}

#[cfg(test)]
mod tests {

    /// The narrowing is sound in one direction only, and this pins which.
    ///
    /// Ruling an entry *out* is a proof: every name `send("get_#{x}")` produces
    /// begins `get_`, so no value of `x` yields `display_name`. Ruling one *in*
    /// would be a guess, and this never does that -- everything it cannot
    /// disprove is kept, because a blind spot dropped in silence is the failure
    /// the report exists to prevent.
    #[test]
    fn a_computed_name_is_ruled_out_only_when_it_provably_cannot_match() {
        let anchors = vec![b"display_name".to_vec()];
        let reaches = |src: &str| {
            let owned = format!("send({src})\n");
            let parsed = ruby_prism::parse(owned.as_bytes());
            let node = parsed.node();
            let program = node.as_program_node().unwrap();
            let statements = program.statements();
            let call = statements.body().iter().next().unwrap();
            let argument = call
                .as_call_node()
                .unwrap()
                .arguments()
                .unwrap()
                .arguments()
                .iter()
                .next()
                .unwrap();
            may_produce(&argument, &anchors)
        };

        // Provably unreachable: the static text contradicts the name.
        assert!(!reaches("\"get_#{x}\""), "prefix rules it out");
        assert!(!reaches("\"#{x}_at\""), "suffix rules it out");
        assert!(!reaches("\"display_#{x}_at\""), "both ends together");
        // Too long to be produced however the ends meet.
        assert!(!reaches("\"display_name_#{x}_suffix\""));

        // Consistent, so kept -- `x` could be `name`.
        assert!(reaches("\"display_#{x}\""));
        assert!(reaches("\"#{x}_name\""));
        // An interpolation can be the empty string, so the ends may meet.
        assert!(reaches("\"display_#{x}name\""));
        // Nothing static to reason from: unknown is not impossible.
        assert!(reaches("x"));
        assert!(reaches("\"#{x}\""));
    }

    /// A regexp is a literal, and nothing scanned it -- so a rename left every
    /// pattern written against the old name behind without a word.
    #[test]
    fn a_name_written_into_a_regexp_is_reported() {
        assert_eq!(
            residue_of("$R.display_name", "/display_name/.match?(x)\n"),
            [Context::Prose]
        );
        // The same rule as a string: a neighbouring qualifier means this is one
        // segment of a longer name, not a mention of the method.
        assert!(residue_of("$R.display_name", "/admin\\/display_name/ =~ x\n").is_empty());
    }

    /// A macro definer names what it creates in its arguments, so a rename
    /// written that way has the same account to give as the `def` spelling.
    ///
    /// D97 gave `def` an anchor and left the macros without one, though
    /// `defines_a_method` counts them: the rule claimed completeness, had
    /// nothing to search for, and printed `residue: []` -- which reads as
    /// "nothing was left over" and meant "nothing was looked for".
    #[test]
    fn a_macro_definer_anchors_on_the_names_it_defines() {
        let names = |pattern: &str| {
            let prepared = prepare::prepare(pattern).expect("prepares");
            let parsed = ruby_prism::parse(prepared.source.as_bytes());
            let node = parsed.node();
            let root = matcher::pattern_root(&node).expect("one expression");
            anchors(&root, &prepared)
                .into_iter()
                .map(|a| String::from_utf8_lossy(&a).into_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(names("attr_reader :display_name"), ["display_name"]);
        assert_eq!(
            names("attr_accessor :display_name, :label"),
            ["display_name", "label"]
        );
        // A block is structure here, not a reason to give up: the name is the
        // argument, exactly as it is without one.
        assert_eq!(
            names("define_method(:display_name) { $B }"),
            ["display_name"]
        );
        assert_eq!(names("alias_method :$A, :display_name"), ["display_name"]);
        // Nothing fixed to anchor on -- the answer `def $M` gives.
        assert!(names("attr_reader :$A").is_empty());
        // Not a definer, so the shape rules apply and `delegate` is not a name.
        assert!(names("delegate :display_name, to: :owner").is_empty());
    }

    /// A computed name is located by the dispatcher's bytes, never the
    /// anchor's, so a file may hold residue without spelling the anchor at all.
    #[test]
    fn a_definition_reaches_past_its_own_name() {
        let reach_of = |pattern: &str| {
            let prepared = prepare::prepare(pattern).expect("prepares");
            let parsed = ruby_prism::parse(prepared.source.as_bytes());
            let node = parsed.node();
            let root = matcher::pattern_root(&node).expect("one expression");
            reach(&root, &prepared)
                .into_iter()
                .map(|a| String::from_utf8_lossy(&a).into_owned())
                .collect::<Vec<_>>()
        };

        let moving = reach_of("def display_name; $B; end");
        assert!(moving.contains(&"display_name".to_string()));
        assert!(moving.contains(&"public_send".to_string()));

        // No definition moves, so nothing claims completeness and no residue
        // runs -- widening the filter would only cost parses.
        assert_eq!(reach_of("$R.display_name"), vec!["display_name"]);
        // No anchor, so nothing to report and nothing to admit a file for.
        assert!(reach_of("$R.select { |$P| $B }.first").is_empty());
    }

    /// The prefilter may never hide a file this report would have named.
    ///
    /// A file is parsed only if its bytes pass `Filter::may_contribute`, so the
    /// filter's reach bounds residue's reach -- and residue is reported from
    /// files a rule does *not* match, which the required literals alone cannot
    /// be relied on to admit.
    ///
    /// This replaces a test that asserted the anchor was always one of the
    /// required literals. That was true, by coincidence, and it is why the
    /// engine could pass `&[]` for the residue side of every filter for a whole
    /// release without a red test: the coincidence held for the patterns the
    /// suite had, and said nothing about whether the engine wired anything in.
    /// This asserts the property the filter is *for*, over patterns and sources
    /// together, so a pattern whose anchor is not a required literal is covered
    /// by the same words.
    #[test]
    fn the_prefilter_admits_every_file_residue_would_report_from() {
        let sources = [
            "class Account\n  def display_name; 1; end\nend\n",
            "class Account\n  attr_reader :display_name\nend\n",
            "class Account\n  # display_name is the label\nend\n",
            "class Account\n  delegate :display_name, to: :owner\nend\n",
            "class Account\n  def go; raise \"display_name moved\"; end\nend\n",
            "class Widget\n  def unrelated; 1; end\nend\n",
            // Nowhere in these bytes is the anchor, whole or in part -- the
            // dispatcher is the only thing that can admit the file.
            "class Account\n  def go(a); public_send(\"display_#{a}\"); end\nend\n",
            "class Account\n  def go(a); send(a); end\nend\n",
        ];
        for pattern in [
            "$R.display_name",
            "display_name",
            "$R.display_name($A)",
            "def display_name; $B; end",
        ] {
            let prepared = prepare::prepare(pattern).expect("prepares");
            let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
            let p_node = p_parsed.node();
            let p_root = matcher::pattern_root(&p_node).expect("one expression");
            let filter = crate::pattern::prefilter::Filter::for_pattern(&p_root, &prepared);
            let anchors = anchors(&p_root, &prepared);

            for source in sources {
                let parsed = ruby_prism::parse(source.as_bytes());
                let mut would = find(&parsed.node(), &anchors, &[], source.as_bytes());
                would.extend(in_comments(&parsed, &anchors, source.as_bytes()));
                // A rule that moves no definition claims no completeness (D7),
                // so the engine never runs its residue -- and a dispatcher in
                // some other rule's file is not its business.
                if !defines_a_method(&p_root, &prepared) {
                    would.retain(|o| o.context != Context::Dynamic);
                }
                if would.is_empty() {
                    continue;
                }
                assert!(
                    filter.may_contribute(source.as_bytes()),
                    "{pattern} reports {} occurrence(s) in {source:?}, \
                     but the prefilter would never let the file be parsed",
                    would.len()
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

    /// The sharp edge: a method's own error message names it, and after a rename
    /// it points at a method that no longer exists. Reported as prose rather
    /// than as a string, because a string that *is* the name may be a live
    /// dispatch while a name inside one is documentation -- different problems,
    /// different fixes.
    #[test]
    fn a_name_mentioned_inside_a_string_is_reported_as_prose() {
        let src = "a.display_name\nraise ArgumentError, \"display_name needs a Router\"\n";
        let found = residue_of("$R.display_name", src);
        assert!(found.contains(&Context::Prose), "{found:?}");
        assert!(!found.contains(&Context::String), "{found:?}");
    }

    /// The two do not collapse into each other: `send("x")` stays a string, and
    /// only the leftover text becomes prose.
    #[test]
    fn a_string_that_is_the_name_stays_a_string() {
        let src = "a.display_name\nsend(\"display_name\")\nlog(\"display_name moved\")\n";
        let found = residue_of("$R.display_name", src);
        assert!(found.contains(&Context::String), "{found:?}");
        assert!(found.contains(&Context::Prose), "{found:?}");
    }

    /// Prose matching is on identifiers, not substrings — otherwise renaming a
    /// short name would report every English word containing it.
    #[test]
    fn prose_matches_whole_identifiers_only() {
        let src = "a.display_name\nlog(\"display_names and display_nameify\")\n";
        let found = residue_of("$R.display_name", src);
        assert!(!found.contains(&Context::Prose), "{found:?}");
    }

    /// A dotted or slashed neighbour means the identifier is one segment of a
    /// qualified name, not a mention of the method. The testbed proved this is
    /// load-bearing: without it an i18n key and a heredoc body both became
    /// residue, and both are marked `GT:ignore` there.
    #[test]
    fn a_qualified_name_inside_a_string_is_not_a_mention() {
        for src in [
            "a.display_name\nt(\"accounts.display_name\")\n",
            "a.display_name\nrender(\"admin/display_name\")\n",
            "a.display_name\nfetch(\"user:display_name\")\n",
            // A heredoc whose body is just the name is the name, not prose
            // about it -- the trailing newline is layout.
            "a.display_name\nx = <<~T\n  display_name\nT\n",
        ] {
            let found = residue_of("$R.display_name", src);
            assert!(!found.contains(&Context::Prose), "{src:?} -> {found:?}");
        }
    }

    /// A `def` pattern is the shape a hand-written rename most obviously takes.
    /// It claimed completeness and had no anchor, so it searched for nothing and
    /// reported `residue: []` — which reads as "nothing left over".
    #[test]
    fn a_def_pattern_anchors_on_the_name_it_renames() {
        let prepared = prepare::prepare("def display_name($A); $B; end").expect("prepares");
        let parsed = ruby_prism::parse(prepared.source.as_bytes());
        let root = matcher::pattern_root(&parsed.node()).expect("single expression");
        assert_eq!(anchors(&root, &prepared), vec![b"display_name".to_vec()]);
    }

    /// Unless the name is itself a metavariable, where there is no one name the
    /// rule is about.
    #[test]
    fn a_def_pattern_with_a_metavariable_name_anchors_on_nothing() {
        let prepared = prepare::prepare("def $M($A); $B; end").expect("prepares");
        let parsed = ruby_prism::parse(prepared.source.as_bytes());
        let root = matcher::pattern_root(&parsed.node()).expect("single expression");
        assert!(anchors(&root, &prepared).is_empty());
    }

    #[test]
    fn the_definition_is_reported() {
        let src = "class A\n  def display_name\n    1\n  end\nend\na.display_name\n";
        let found = residue_of("$R.display_name", src);
        assert!(found.contains(&Context::Definition), "{found:?}");
    }

    /// A concern's contribution is reported however the mixin was spelled.
    ///
    /// The scope stack holds *segments*; a class is the join of them. Asking
    /// the hierarchy about the bare segment `Numeric` worked only while that
    /// segment had exactly one candidate to widen to -- so writing the same
    /// module relatively, which puts its written spelling in the index beside
    /// its declared one, silently dropped the concern's `def` and the
    /// implicit-self calls in its body. Two files, one word apart, one
    /// reported.
    #[test]
    fn a_relative_mixin_path_keeps_the_concern_in_the_report() {
        for spelling in ["App::Helpers::Numeric", "Helpers::Numeric"] {
            let src = format!(
                "module App\n  module Helpers\n    module Numeric\n      def cast(v)\n        \
                 cast(v)\n      end\n    end\n  end\n  class Value\n    def cast(v); v; end\n  \
                 end\n  class Integer < Value\n    include {spelling}\n  end\nend\n"
            );
            let parsed = ruby_prism::parse(src.as_bytes());
            let anchors = vec![b"cast".to_vec()];
            let all = find(&parsed.node(), &anchors, &[], src.as_bytes());
            let hierarchy = crate::hierarchy::Hierarchy::from_source(&src);
            let scoped = scoped_to(all, "App::Value", &hierarchy);
            // In the concern, not in `App::Value` -- whose own `def cast` is
            // kept by the plain name comparison and would pass this for the
            // wrong reason.
            let in_concern = |want| {
                scoped.iter().any(|o| {
                    o.context == want
                        && matcher::enclosing_class(&o.scope).as_deref()
                            == Some("App::Helpers::Numeric")
                })
            };
            assert!(
                in_concern(Context::Definition),
                "`include {spelling}` loses the concern's def: {scoped:?}"
            );
            assert!(
                in_concern(Context::Call),
                "`include {spelling}` loses the implicit call in its body: {scoped:?}"
            );
        }
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

    /// A directive is an instruction addressed to rwr, not prose about the
    /// code, so it is not a blind spot (D72). It was counted as one -- and
    /// since a directive names the very id it suppresses, it raised the
    /// headline blind-spot number by one and could not be drained without
    /// deleting the suppression it documents.
    #[test]
    fn a_directive_is_not_residue() {
        let src =
            "class Account\n  # rwr:ignore Account#display_name\n  def display_name; 1; end\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let found = in_comments(&parsed, &[b"display_name".to_vec()], src.as_bytes());
        assert!(found.is_empty(), "{found:?}");
    }

    /// A malformed one is still an instruction: it names no rule, so it is
    /// reported as malformed, but it is no more prose than a well-formed one.
    #[test]
    fn a_malformed_directive_is_not_residue_either() {
        let src = "# rwr:ignore -- display_name is fine here\nx = 1\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        assert!(in_comments(&parsed, &[b"display_name".to_vec()], src.as_bytes()).is_empty());
    }

    /// ...but prose that merely mentions the marker is prose, and still a
    /// blind spot. The narrowest way this fix could have gone wrong is by
    /// excluding every comment that says `rwr:ignore` anywhere in it.
    #[test]
    fn prose_mentioning_the_marker_is_still_residue() {
        let src = "# we never write rwr:ignore for display_name here\nx = 1\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        assert_eq!(
            in_comments(&parsed, &[b"display_name".to_vec()], src.as_bytes()).len(),
            1
        );
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
