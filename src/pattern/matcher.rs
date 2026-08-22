//! Matching a prepared pattern against a target tree.
//!
//! Built on [`compare::node_eq`] so that matching, D16's repeated-metavariable
//! equality, and §7's reparse-verify all share one notion of equality rather
//! than drifting apart.
//!
//! Metavariables reach the pattern tree as placeholder identifiers (D18), so
//! they are recognised **by name** rather than by the node type they happened
//! to parse into — `Foo::$C` parses as a constant or a method call depending on
//! the placeholder's case, and neither reading should change what it matches.

use super::compare::{self, Atom};
use super::generated;
use super::prepare::{Binding, Prepared};
use crate::hierarchy::Hierarchy;
use crate::rule::{Constraint, NodeKind, Scope};
use ruby_prism::Node;
use std::collections::HashMap;

/// What a metavariable captured.
#[derive(Debug)]
pub(crate) enum Bound<'pr> {
    /// A single node, from `$NAME`.
    One(Node<'pr>),
    /// Zero or more consecutive siblings, from `*$NAME`.
    Many(Vec<Node<'pr>>),
    /// An identifier, when the metavariable stood in a name position.
    Name(Vec<u8>),
}

// `Node` derives neither `Clone` nor `Copy`, so backtracking -- which snapshots
// the environment before each trial -- needs an explicit handle copy.
impl<'pr> Clone for Bound<'pr> {
    fn clone(&self) -> Self {
        match self {
            Bound::One(n) => Bound::One(generated::dup(n)),
            Bound::Many(v) => Bound::Many(v.iter().map(generated::dup).collect()),
            Bound::Name(b) => Bound::Name(b.clone()),
        }
    }
}

/// Captures accumulated during a match.
pub(crate) type Env<'pr> = HashMap<String, Bound<'pr>>;

/// One match: the node that matched, what it captured, and where it sits.
#[derive(Debug)]
pub(crate) struct Match<'pr> {
    pub node: Node<'pr>,
    pub env: Env<'pr>,
    /// Enclosing class and module names, outermost first.
    ///
    /// Lexical scope is what resolves the largest receiver bucket: measurement
    /// (b) found 43.5% of rails call sites are implicit self, and knowing which
    /// class you are inside needs no type inference at all.
    pub scope: Vec<String>,
    /// Whether the match sits in a singleton context -- inside `def self.x` or
    /// `class << self` -- which decides what `self` denotes.
    pub singleton: bool,
    /// Local variables whose class is known from an assignment in scope.
    ///
    /// Locals are 17.9% of rails call receivers -- the second-largest bucket --
    /// and `x = Foo.new` pins most of them without any type inference.
    pub locals: HashMap<String, String>,
}

/// The identifier a node reads, when it is a bare name reference.
///
/// Prism parses an unassigned lowercase identifier as a `CallNode` with no
/// receiver and no arguments rather than a local-variable read, so all three
/// shapes have to be recognised.
fn bare_name<'a>(node: &Node<'a>) -> Option<Vec<u8>> {
    match node {
        Node::CallNode { .. } => {
            let call = node.as_call_node()?;
            (call.receiver().is_none() && call.arguments().is_none() && call.block().is_none())
                .then(|| call.name().as_slice().to_vec())
        }
        Node::ConstantReadNode { .. } => {
            Some(node.as_constant_read_node()?.name().as_slice().to_vec())
        }
        Node::LocalVariableReadNode { .. } => Some(
            node.as_local_variable_read_node()?
                .name()
                .as_slice()
                .to_vec(),
        ),
        _ => None,
    }
}

/// The *metavariable* a node stands for, if it is a placeholder reference.
///
/// Keyed on the metavariable rather than the placeholder, since one
/// metavariable used twice substitutes to two distinct placeholders.
pub(crate) fn placeholder_name(node: &Node<'_>, prepared: &Prepared) -> Option<String> {
    let key = placeholder(node, &prepared.bindings)?;
    prepared.bindings.get(key).and_then(|b| b.name.clone())
}

/// If `node` is a placeholder reference, the metavariable it stands for.
fn placeholder<'a>(node: &Node<'_>, bindings: &'a HashMap<String, Binding>) -> Option<&'a str> {
    let name = bare_name(node)?;
    let name = std::str::from_utf8(&name).ok()?;
    bindings.get_key_value(name).map(|(k, _)| k.as_str())
}

/// The metavariable a sequence placeholder stands for, i.e. `*$NAME` -> `$NAME`.
pub(crate) fn splat_placeholder_name(node: &Node<'_>, prepared: &Prepared) -> Option<String> {
    let key = splat_placeholder(node, &prepared.bindings)?;
    prepared.bindings.get(key).and_then(|b| b.name.clone())
}

/// A sequence placeholder, i.e. `*$NAME` — a splat wrapping a placeholder.
fn splat_placeholder<'a>(
    node: &Node<'_>,
    bindings: &'a HashMap<String, Binding>,
) -> Option<&'a str> {
    let inner = match node {
        Node::SplatNode { .. } => node.as_splat_node()?.expression()?,
        // Ruby spells "the remaining entries" with a double splat inside a
        // hash, which is a different node. Without this a pattern cannot reach
        // one pair among several.
        Node::AssocSplatNode { .. } => node.as_assoc_splat_node()?.value()?,
        _ => return None,
    };
    placeholder(&inner, bindings)
}

/// How many times one node may be re-matched after a constraint rejection.
///
/// A backstop, not a budget: each retry forbids one more finite binding, so the
/// loop terminates on its own. Real patterns rebind once or twice.
const MAX_REBINDS: usize = 32;

/// Bindings already rejected by a constraint, keyed by metavariable.
///
/// Constraints are checked after a structural match, deliberately: a constraint
/// may not change *what* matched, only whether it counts. The cost is that a
/// node admitting several bindings, where only a later one satisfies the
/// constraint, was under-matched -- `{name: name, size: size}` bound `$K` to
/// `name`, was rejected, and the `size` binding was never tried (Q13).
///
/// Rather than thread constraints through matching, a rejected binding is
/// *forbidden* and the match retried, which forces backtracking to a different
/// one. Terminating, because each retry forbids one more finite binding.
pub(crate) type Forbidden = HashMap<String, Vec<String>>;

/// A stable identity for a binding, so it can be excluded on retry.
fn fingerprint(bound: &Bound<'_>) -> String {
    match bound {
        Bound::Name(bytes) => format!("n:{}", String::from_utf8_lossy(bytes)),
        Bound::One(node) => {
            let l = node.location();
            format!("o:{}..{}", l.start_offset(), l.end_offset())
        }
        Bound::Many(nodes) => {
            let spans: Vec<String> = nodes
                .iter()
                .map(|n| {
                    let l = n.location();
                    format!("{}..{}", l.start_offset(), l.end_offset())
                })
                .collect();
            format!("m:{}", spans.join(","))
        }
    }
}

/// Bind `key`, enforcing D16: a repeated metavariable must match AST-equal
/// nodes, never merely equal source text.
fn bind<'pr>(env: &mut Env<'pr>, key: &str, value: Bound<'pr>, forbidden: &Forbidden) -> bool {
    if let Some(rejected) = forbidden.get(key)
        && rejected.contains(&fingerprint(&value))
    {
        return false;
    }
    match (env.get(key), &value) {
        (None, _) => {
            env.insert(key.to_string(), value);
            true
        }
        (Some(Bound::One(a)), Bound::One(b)) => compare::node_eq(a, b),
        (Some(Bound::Name(a)), Bound::Name(b)) => a == b,
        (Some(Bound::Many(a)), Bound::Many(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| compare::node_eq(x, y))
        }
        // A metavariable can land in a name position and a node position in
        // one pattern -- `{ |$P| $P.active? }` binds `$P` as a parameter name,
        // then meets it again as an expression. Equate the two by identifier,
        // which is what rename rules want.
        (Some(Bound::Name(a)), Bound::One(n)) => bare_name(n).as_deref() == Some(a.as_slice()),
        (Some(Bound::One(n)), Bound::Name(b)) => bare_name(n).as_deref() == Some(b.as_slice()),
        _ => false,
    }
}

/// Match one pattern node against one target node.
pub(crate) fn match_node<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
    env: &mut Env<'pr>,
    forbidden: &Forbidden,
) -> bool {
    // A bare placeholder is a wildcard: it matches any node, and binds unless
    // it is anonymous.
    if let Some(key) = placeholder(pattern, &prepared.bindings) {
        // Key on the *metavariable* name: `foo($A, $A)` substitutes to two
        // distinct placeholders, so keying on those would never share a
        // binding and D16's equality check would never fire.
        return match prepared.bindings.get(key).and_then(|b| b.name.clone()) {
            None => true,
            Some(name) => bind(env, &name, Bound::One(generated::dup(target)), forbidden),
        };
    }

    // `def foo(*$P)` means "with any parameters". In a parameter position `*$P`
    // is real Ruby -- a rest parameter -- so the splat machinery that handles
    // argument runs does not apply, and the pattern only matched a target whose
    // parameter list was itself a lone rest. An override whose arity had drifted
    // from its parent's, which is the ordinary shape of legacy inheritance, was
    // therefore unmatchable by any spelling.
    if matches!(pattern, Node::ParametersNode { .. })
        // Only against another parameter list. Binding it to whatever sat in
        // that position let `def foo(*$P)` swallow the `self` receiver of
        // `def self.foo` and match it -- an instance rename renaming a class
        // method, which is the one thing receiver narrowing exists to prevent.
        && matches!(target, Node::ParametersNode { .. })
        && let Some(name) = lone_rest_placeholder(pattern, prepared)
    {
        return bind(env, &name, Bound::One(generated::dup(target)), forbidden);
    }

    // A lone metavariable standing for a body binds the *whole* body, whatever
    // shape that body has.
    //
    // Not an exception to D32's "one node" -- a Ruby body position holds a
    // statements sequence, and that sequence is one node. Comparing the two
    // sequences child by child instead made `def foo; $B; end` match only
    // single-statement methods, so the flagship rename declined every real
    // method (D73).
    //
    // Checked *before* the discriminant, because a `def` carrying `rescue` or
    // `ensure` has a `BeginNode` body rather than a `StatementsNode` -- so a
    // version of this that ran after the discriminant fixed the plain case and
    // left its twin broken.
    //
    // `rewrite::structural_diff` carries the matching half of this rule. Without
    // it the diff calls such a body diverged, re-renders the whole `def`, and
    // that wider edit swallows the correct one.
    if matches!(pattern, Node::StatementsNode { .. }) {
        let statements = generated::children(pattern);
        if let [only] = statements.as_slice()
            && let Some(key) = placeholder(only, &prepared.bindings)
        {
            return match prepared.bindings.get(key).and_then(|b| b.name.clone()) {
                None => true,
                Some(name) => bind(env, &name, Bound::One(generated::dup(target)), forbidden),
            };
        }
    }

    if std::mem::discriminant(pattern) != std::mem::discriminant(target) {
        return false;
    }

    if !match_atoms(pattern, target, prepared, env, forbidden) {
        return false;
    }

    match_children(
        &generated::children(pattern),
        &generated::children(target),
        prepared,
        env,
        forbidden,
    )
}

/// Atoms match pairwise, except that a name atom which *is* a placeholder acts
/// as a wildcard over the corresponding target name (`x.$M`, `def $M`).
fn match_atoms<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
    env: &mut Env<'pr>,
    forbidden: &Forbidden,
) -> bool {
    let (pa, ta) = (generated::atoms(pattern), generated::atoms(target));
    if pa.len() != ta.len() {
        return false;
    }
    pa.iter().zip(&ta).all(|(p, t)| match (p, t) {
        // A symbol's name is a *value* atom, not a constant one, so `:$M` and a
        // label key `$K:` reach here rather than the Name arm below. Without
        // this a metavariable in label position silently matched nothing.
        (Atom::Value(pn), Atom::Value(tn)) => match std::str::from_utf8(pn)
            .ok()
            .and_then(|s| prepared.bindings.get_key_value(s))
        {
            Some((_, binding)) => match &binding.name {
                None => true,
                Some(name) => bind(env, name, Bound::Name(tn.clone()), forbidden),
            },
            None => pn == tn,
        },
        (Atom::Name(pn), Atom::Name(tn)) => match std::str::from_utf8(pn)
            .ok()
            .and_then(|s| prepared.bindings.get_key_value(s))
        {
            Some((_, binding)) => match &binding.name {
                None => true,
                Some(name) => bind(env, name, Bound::Name(tn.clone()), forbidden),
            },
            None => pn == tn,
        },
        _ => p == t,
    })
}

/// Match child sequences, with `*$NAME` absorbing zero or more siblings.
///
/// Naive leftmost backtracking, shortest-first: deterministic, and adequate
/// because real patterns carry at most one or two sequence metavariables per
/// list while argument lists run to tens of elements.
fn match_children<'pr>(
    pattern: &[Node<'_>],
    target: &[Node<'pr>],
    prepared: &Prepared,
    env: &mut Env<'pr>,
    forbidden: &Forbidden,
) -> bool {
    let Some((head, rest)) = pattern.split_first() else {
        return target.is_empty();
    };

    if let Some(key) = splat_placeholder(head, &prepared.bindings) {
        let name = prepared.bindings.get(key).and_then(|b| b.name.clone());
        for take in 0..=target.len() {
            let mut trial = env.clone();
            let absorbed: Vec<Node> = target[..take].iter().map(generated::dup).collect();
            if let Some(name) = &name
                && !bind(&mut trial, name, Bound::Many(absorbed), forbidden)
            {
                continue;
            }
            if match_children(rest, &target[take..], prepared, &mut trial, forbidden) {
                *env = trial;
                return true;
            }
        }
        return false;
    }

    // `def foo(*$P)` against a `def` that has no parameter list: Prism gives a
    // zero-arity definition no `ParametersNode` at all, so the pattern carries a
    // child the target lacks and positional alignment pairs the parameters
    // against the body. "Any parameters" has to include none, so let it absorb
    // nothing and carry on.
    if let Some(name) = lone_rest_placeholder(head, prepared)
        && !target
            .first()
            .is_some_and(|t| matches!(t, Node::ParametersNode { .. }))
    {
        let mut trial = env.clone();
        if bind(&mut trial, &name, Bound::Many(Vec::new()), forbidden)
            && match_children(rest, target, prepared, &mut trial, forbidden)
        {
            *env = trial;
            return true;
        }
    }

    let Some((t_head, t_rest)) = target.split_first() else {
        // Target exhausted, pattern not. Prism gives `foo()` no arguments node
        // at all, so `foo(*$REST)` -- whose argument list can absorb nothing --
        // has one child the target lacks. Let such a subtree vanish.
        return pattern
            .iter()
            .all(|p| vanishes(p, prepared, env, forbidden));
    };
    let mut trial = env.clone();
    if match_node(head, t_head, prepared, &mut trial, forbidden)
        && match_children(rest, t_rest, prepared, &mut trial, forbidden)
    {
        *env = trial;
        return true;
    }
    false
}

/// Whether a pattern subtree can match *nothing at all*, binding any sequence
/// metavariables inside it to the empty sequence.
///
/// This exists because absence and emptiness are different in Prism: a call
/// with no arguments has no arguments node, not an empty one.
fn vanishes<'pr>(
    pattern: &Node<'_>,
    prepared: &Prepared,
    env: &mut Env<'pr>,
    forbidden: &Forbidden,
) -> bool {
    if let Some(key) = splat_placeholder(pattern, &prepared.bindings) {
        return match prepared.bindings.get(key).and_then(|b| b.name.clone()) {
            None => true,
            Some(name) => bind(env, &name, Bound::Many(Vec::new()), forbidden),
        };
    }
    // `def foo(*$P)` against a target with no parameter list at all: Prism gives
    // a zero-arity `def` no `ParametersNode`, so the pattern has one child the
    // target lacks. "Any parameters" has to include none.
    if let Some(name) = lone_rest_placeholder(pattern, prepared) {
        return bind(env, &name, Bound::Many(Vec::new()), forbidden);
    }
    // A container with no atoms of its own vanishes if everything inside it does.
    generated::atoms(pattern).is_empty()
        && !generated::children(pattern).is_empty()
        && generated::children(pattern)
            .iter()
            .all(|c| vanishes(c, prepared, env, forbidden))
}

/// The pattern's meaningful root, with Prism's `ProgramNode`/`StatementsNode`
/// wrapper stripped. A single-statement pattern matches a node; the wrapper is
/// an artefact of parsing a fragment as a program.
pub(crate) fn pattern_root<'pr>(root: &Node<'pr>) -> Option<Node<'pr>> {
    let program = root.as_program_node()?;
    let body = program.statements().body();
    let first = body.iter().next()?;
    (body.iter().count() == 1).then_some(first)
}

/// Whether an environment satisfies a rule's `where:` constraints.
///
/// Checked after a structural match rather than threaded through it: the
/// bindings are already known by then, and keeping the two separate means a
/// constraint can never change *what* matched, only whether it counts.
/// Why a match was rejected, and whether a different binding could help.
///
/// The detail exists to be *reported*: a rule author iterating on a `where:`
/// clause needs to know which constraint declined a site and what the binding
/// actually was. It was computed and discarded until `-e` learned to print it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Ok,
    /// The match is in the wrong place; no rebinding can fix that.
    WrongScope(ScopeMiss),
    /// This capture's binding failed a constraint. A different binding of the
    /// same metavariable might satisfy it, so the match is worth retrying.
    BadBinding {
        capture: String,
        miss: ConstraintMiss,
    },
    /// A rule that `Rule::validate` should have refused before any file was
    /// read. Reaching here means the pre-validation has a hole -- distinct from
    /// a scope miss, which it used to be reported as.
    Bug(&'static str),
}

/// Why the match was in the wrong place.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScopeMiss {
    Inside { wanted: String, found: Vec<String> },
    Singleton { wanted: bool },
}

/// Which constraint declined the binding, and what it saw.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConstraintMiss {
    Name {
        actual: Option<String>,
        allowed: Vec<String>,
    },
    NameNot {
        actual: String,
    },
    /// `resolved: None` is the distinction worth having: receiver narrowing is
    /// conservative, so "rwr could not resolve this receiver at all" and "it
    /// resolved to the wrong class" are different problems with different
    /// fixes, and they were indistinguishable.
    Type {
        resolved: Option<String>,
        wanted: String,
        /// Resolved to the right class, but `Account.foo` where the rule means
        /// `account.foo` or the reverse.
        wrong_kind: bool,
    },
    Is {
        wanted: NodeKind,
    },
    Contains {
        pattern: String,
    },
    Length {
        actual: Option<usize>,
        wanted: usize,
    },
    SameNameAs {
        other: String,
    },
}

pub(crate) fn satisfies(
    found: &Match<'_>,
    constraints: &HashMap<String, Constraint>,
    scope: &Scope,
    hierarchy: &Hierarchy,
    sigs: &crate::sigs::Signatures,
) -> bool {
    verdict(found, constraints, &HashMap::new(), scope, hierarchy, sigs) == Verdict::Ok
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn verdict(
    found: &Match<'_>,
    constraints: &HashMap<String, Constraint>,
    contained: &HashMap<String, Prepared>,
    scope: &Scope,
    hierarchy: &Hierarchy,
    sigs: &crate::sigs::Signatures,
) -> Verdict {
    if let Some(wanted) = &scope.inside {
        let here = enclosing_class(&found.scope);
        let reached = here.as_deref().is_some_and(|s| {
            s == wanted || (scope.subclasses.unwrap_or(false) && hierarchy.descends_from(s, wanted))
        });
        if !reached {
            return Verdict::WrongScope(ScopeMiss::Inside {
                wanted: wanted.clone(),
                found: found.scope.clone(),
            });
        }
    }
    if let Some(wanted) = scope.singleton
        && found.singleton != wanted
    {
        return Verdict::WrongScope(ScopeMiss::Singleton { wanted });
    }

    for (key, constraint) in constraints {
        let short = key.trim_start_matches('$').to_string();
        let Some(bound) = found.env.get(&short) else {
            // A constraint naming a metavariable the pattern does not bind is a
            // rule bug. Refuse rather than silently ignoring it.
            return Verdict::Bug("a constraint names a capture the pattern never binds");
        };

        if let Some(other) = &constraint.same_name_as {
            let Some(other_bound) = found.env.get(other.trim_start_matches('$')) else {
                return Verdict::Bug("`same_name_as:` names a capture the pattern never binds");
            };
            match (identifier_of(bound), identifier_of(other_bound)) {
                (Some(a), Some(b)) if a == b => {}
                _ => {
                    return Verdict::BadBinding {
                        capture: short,
                        miss: ConstraintMiss::SameNameAs {
                            other: other.clone(),
                        },
                    };
                }
            }
        }

        if let Some(allowed) = &constraint.name {
            let actual = match bound {
                Bound::Name(bytes) => Some(bytes.clone()),
                Bound::One(node) => bare_name(node),
                Bound::Many(_) => None,
            };
            if !actual
                .as_ref()
                .is_some_and(|a| allowed.iter().any(|n| n.as_bytes() == a.as_slice()))
            {
                return Verdict::BadBinding {
                    capture: short,
                    miss: ConstraintMiss::Name {
                        actual: actual.map(|a| String::from_utf8_lossy(&a).into_owned()),
                        allowed: allowed.clone(),
                    },
                };
            }
        }

        if let Some(excluded) = &constraint.name_not {
            let actual = match bound {
                Bound::Name(bytes) => Some(bytes.clone()),
                Bound::One(node) => bare_name(node),
                Bound::Many(_) => None,
            };
            // No identifier passes: nothing that is not one can be one of these.
            if let Some(actual) = actual
                && excluded.iter().any(|n| n.as_bytes() == actual.as_slice())
            {
                return Verdict::BadBinding {
                    capture: short,
                    miss: ConstraintMiss::NameNot {
                        actual: String::from_utf8_lossy(&actual).into_owned(),
                    },
                };
            }
        }

        if let Some(wanted) = constraint.is
            && !is_kind(bound, wanted)
        {
            return Verdict::BadBinding {
                capture: short,
                miss: ConstraintMiss::Is { wanted },
            };
        }

        if constraint.contains.is_some() {
            let Some(sub) = contained.get(&short) else {
                // A `contains:` whose pattern did not prepare is a rule bug.
                return Verdict::Bug("a `contains:` sub-pattern failed to prepare");
            };
            let miss = || ConstraintMiss::Contains {
                pattern: sub.source.clone(),
            };
            let Bound::One(node) = bound else {
                return Verdict::BadBinding {
                    capture: short,
                    miss: miss(),
                };
            };
            if !holds_within(node, sub, &found.env) {
                return Verdict::BadBinding {
                    capture: short,
                    miss: miss(),
                };
            }
        }

        if let Some(wanted) = constraint.length {
            // Counted in characters rather than bytes: `tr` maps characters, so
            // a two-byte `é` is still one of them.
            let Some(content) = literal_content(bound) else {
                return Verdict::BadBinding {
                    capture: short,
                    miss: ConstraintMiss::Length {
                        actual: None,
                        wanted,
                    },
                };
            };
            let actual = content.chars().count();
            if actual != wanted {
                return Verdict::BadBinding {
                    capture: short,
                    miss: ConstraintMiss::Length {
                        actual: Some(actual),
                        wanted,
                    },
                };
            }
        }

        if let Some(wanted) = &constraint.receiver_type {
            let unresolved = |wrong_kind: bool, resolved: Option<String>| Verdict::BadBinding {
                capture: short.clone(),
                miss: ConstraintMiss::Type {
                    resolved,
                    wanted: wanted.clone(),
                    wrong_kind,
                },
            };
            let Bound::One(node) = bound else {
                return unresolved(false, None);
            };
            // Unresolved means "not known to be this type", never "assume yes".
            let at = Where {
                scope: &found.scope,
                singleton: found.singleton,
                locals: &found.locals,
                sigs,
            };
            let Some(resolved) = resolve_type(node, &at) else {
                return unresolved(false, None);
            };
            let matches_class = resolved.class_name() == wanted.as_str()
                || (constraint.subclasses.unwrap_or(false)
                    && hierarchy.descends_from(resolved.class_name(), wanted));
            if !matches_class {
                return unresolved(false, Some(resolved.class_name().to_string()));
            }
            // `Account.foo` and `account.foo` are different methods, so a
            // constraint must say which it means.
            if resolved.is_instance() != constraint.wants_instance() {
                return unresolved(true, Some(resolved.class_name().to_string()));
            }
        }
    }
    Verdict::Ok
}

/// Whether `sub` matches somewhere inside `node`, agreeing with `outer` on
/// every metavariable the two patterns share.
///
/// The agreement is what makes containment useful rather than merely true.
/// `$R.each { |$X| $B }` with `$B` containing `$X.$INNER` has to mean *that*
/// block's parameter; without the check it would match any call on anything.
fn holds_within(node: &Node<'_>, sub: &Prepared, outer: &Env<'_>) -> bool {
    let parsed = ruby_prism::parse(sub.source.as_bytes());
    let root = parsed.node();
    let Some(root) = pattern_root(&root) else {
        return false;
    };
    // The sub-pattern carries no constraints of its own: agreement with the
    // outer bindings is the only condition, and it is checked below.
    let criteria = Criteria::none();
    search(&root, node, sub, &criteria).iter().any(|hit| {
        hit.env
            .iter()
            .all(|(name, bound)| outer.get(name).is_none_or(|theirs| agree(bound, theirs)))
    })
}

/// Whether two bindings of the same metavariable refer to the same thing.
///
/// By *identifier* where both name one, and by source span otherwise. The
/// distinction matters for the case containment exists to serve: in
/// `$R.each { |$X| $B }` the outer `$X` binds the block's **parameter** while
/// the inner one binds a **read** of it. Same variable, different nodes, and
/// comparing spans would say they disagree.
fn agree(a: &Bound<'_>, b: &Bound<'_>) -> bool {
    match (identifier_of(a), identifier_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => fingerprint(a) == fingerprint(b),
    }
}

/// Whether a binding is of the kind a constraint asked for.
///
/// A capture in a *name* position -- `$C` in `$C = [...]`, `$M` in `$R.$M` --
/// binds an identifier rather than a node, because Prism carries those as atoms
/// on the parent. There is no node to classify, so the identifier's own spelling
/// answers the only question that can be asked of it: Ruby constants start with
/// an uppercase letter and nothing else does.
fn is_kind(bound: &Bound<'_>, wanted: NodeKind) -> bool {
    let node = match bound {
        Bound::One(node) => node,
        Bound::Name(bytes) => {
            return wanted == NodeKind::Constant
                && bytes.first().is_some_and(u8::is_ascii_uppercase);
        }
        Bound::Many(_) => return false,
    };
    match wanted {
        NodeKind::Constant => matches!(
            node,
            Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }
        ),
        NodeKind::Symbol => matches!(node, Node::SymbolNode { .. }),
        // An interpolated string is a different node, which is what keeps
        // `gsub("#{a}", "b")` out of a rule that assumes a literal.
        NodeKind::String => matches!(node, Node::StringNode { .. }),
        NodeKind::Integer => matches!(node, Node::IntegerNode { .. }),
        NodeKind::Array => matches!(node, Node::ArrayNode { .. }),
        NodeKind::Hash => matches!(node, Node::HashNode { .. }),
    }
}

/// The text a literal carries, for the bindings where that question has an
/// answer.
fn literal_content(bound: &Bound<'_>) -> Option<String> {
    let bytes = match bound {
        Bound::Name(bytes) => bytes.clone(),
        Bound::One(node) => match node {
            Node::StringNode { .. } => node.as_string_node()?.unescaped().to_vec(),
            Node::SymbolNode { .. } => node.as_symbol_node()?.unescaped().to_vec(),
            _ => return None,
        },
        Bound::Many(_) => return None,
    };
    String::from_utf8(bytes).ok()
}

/// The identifier a binding names, across node kinds.
///
/// A symbol key and a variable read are different nodes carrying the same name,
/// which is exactly the correspondence `{foo: foo}` turns on.
fn identifier_of(bound: &Bound<'_>) -> Option<Vec<u8>> {
    match bound {
        Bound::Name(bytes) => Some(bytes.clone()),
        Bound::One(node) => match node {
            Node::SymbolNode { .. } => Some(node.as_symbol_node()?.unescaped().to_vec()),
            _ => bare_name(node),
        },
        Bound::Many(_) => None,
    }
}

/// The class a variable is assigned from, when the assignment says so outright.
///
/// Covers locals and instance variables alike. Instance variables are 5.7% of
/// rails call receivers and overwhelmingly assigned once in `initialize`.
fn assigned_class(node: &Node<'_>) -> Option<(String, String)> {
    let (name, value) = match node {
        Node::LocalVariableWriteNode { .. } => {
            let write = node.as_local_variable_write_node()?;
            (write.name().as_slice().to_vec(), write.value())
        }
        Node::InstanceVariableWriteNode { .. } => {
            let write = node.as_instance_variable_write_node()?;
            (write.name().as_slice().to_vec(), write.value())
        }
        _ => return None,
    };
    let call = value.as_call_node()?;
    if call.name().as_slice() != b"new" {
        return None;
    }
    // `X.new` yields an *instance*, whatever the receiver's own kind was. The
    // receiver here is a constant, which needs nothing from the surroundings.
    let empty = crate::sigs::Signatures::default();
    let locals = HashMap::new();
    let class = resolve_type(
        &call.receiver()?,
        &Where {
            scope: &[],
            singleton: false,
            locals: &locals,
            sigs: &empty,
        },
    )?;
    Some((
        String::from_utf8(name).ok()?,
        class.class_name().to_string(),
    ))
}

/// Seed every instance-variable assignment in a class body.
///
/// Ruby does not care what order methods appear in, so neither should rwr: a
/// read written above the `initialize` that assigns it must still resolve.
fn collect_ivars(class: &Node<'_>, locals: &mut HashMap<String, String>) {
    let mut stack = vec![generated::dup(class)];
    while let Some(node) = stack.pop() {
        if matches!(node, Node::InstanceVariableWriteNode { .. })
            && let Some((name, class)) = assigned_class(&node)
        {
            locals.insert(name, class);
        }
        stack.extend(generated::children(&node));
    }
}

/// The variable a receiver reads, for looking up an inferred class.
///
/// An instance variable read is not a `bare_name` -- it has no method call
/// behind it -- so it needs its own accessor.
fn variable_name(node: &Node<'_>) -> Option<Vec<u8>> {
    match node {
        Node::InstanceVariableReadNode { .. } => Some(
            node.as_instance_variable_read_node()?
                .name()
                .as_slice()
                .to_vec(),
        ),
        _ => bare_name(node),
    }
}

/// Whether a receiver is the class object or an instance of it.
///
/// `Account.display_name` and `account.display_name` name **different
/// methods**. Collapsing them made a rename of one rewrite the other -- a
/// silent wrong edit, which is the failure the whole design exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Receiver {
    /// An instance, as in `Account#display_name`.
    Instance(String),
    /// The class object itself, as in `Account.display_name`.
    Class(String),
}

impl Receiver {
    pub(crate) fn class_name(&self) -> &str {
        match self {
            Receiver::Instance(n) | Receiver::Class(n) => n,
        }
    }

    pub(crate) fn is_instance(&self) -> bool {
        matches!(self, Receiver::Instance(_))
    }
}

/// The class a receiver node denotes, where syntax alone can tell.
///
/// Covers the buckets measurement (b) found dominant -- constants (14.3%) and
/// explicit self (0.8%) -- and returns `None` for locals, ivars and chains
/// (39.4%), which need inference this does not yet do. `None` narrows the match
/// away rather than admitting it, so the constraint is always conservative.
/// What is known about the place a receiver expression sits.
///
/// Bundled because resolution recurses through chains, and threading four
/// arguments through that by hand is how one of them ends up stale.
pub(crate) struct Where<'a> {
    /// Enclosing class and module names, outermost first.
    pub scope: &'a [String],
    /// Whether the match sits in singleton context.
    pub singleton: bool,
    /// Local and instance variables assigned from a constructor.
    pub locals: &'a HashMap<String, String>,
    /// Return types stated by Sorbet signatures, when the repo has any.
    pub sigs: &'a crate::sigs::Signatures,
}

pub(crate) fn resolve_type(node: &Node<'_>, at: &Where<'_>) -> Option<Receiver> {
    let (scope, singleton) = (at.scope, at.singleton);
    match node {
        // A bare constant names the class *object*, so a call on it dispatches
        // to a singleton method.
        Node::ConstantReadNode { .. } => {
            let name = node.as_constant_read_node()?.name().as_slice().to_vec();
            String::from_utf8(name).ok().map(Receiver::Class)
        }
        Node::ConstantPathNode { .. } => {
            // `A::B` denotes B; the path is how you reach it, not what it is.
            let name = node.as_constant_path_node()?.name()?.as_slice().to_vec();
            String::from_utf8(name).ok().map(Receiver::Class)
        }
        // `self` is the class inside `def self.x` or `class << self`, and an
        // instance inside an ordinary method body.
        Node::SelfNode { .. } => scope.last().cloned().map(|n| {
            if singleton {
                Receiver::Class(n)
            } else {
                Receiver::Instance(n)
            }
        }),
        // A chained receiver: `Widget.new.foo`, `thing.dup.foo`.
        //
        // Measured before building: chained receivers are 15.8% of call sites
        // in rails and 27% in a Rails app, but they are not one problem.
        // Following a chain in general needs to know what a method *returns*,
        // and only 2-4% of method definitions say that syntactically -- 70% end
        // in another call, so resolution recurses into more unknowns. What is
        // free is the chain that carries its own answer, and `new` is the
        // commonest such inner call in all three corpora (docs/internal/scaling.md).
        // A local or instance variable whose assignment named a class.
        Node::LocalVariableReadNode { .. } | Node::InstanceVariableReadNode { .. } => {
            variable_name(node)
                .and_then(|n| String::from_utf8(n).ok())
                .and_then(|n| at.locals.get(&n).cloned())
                .map(Receiver::Instance)
        }
        // A chained receiver: `Widget.new.foo`, `thing.dup.foo`, and -- where a
        // signature says so -- `parser.document.foo`.
        Node::CallNode { .. } => {
            let call = node.as_call_node()?;
            let name = call.name();
            match name.as_slice() {
                // `Widget.new` is an instance of Widget, said outright.
                b"new" => match call.receiver()? {
                    receiver @ (Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }) => {
                        match resolve_type(&receiver, at)? {
                            Receiver::Class(name) => Some(Receiver::Instance(name)),
                            Receiver::Instance(_) => None,
                        }
                    }
                    _ => None,
                },
                // Methods that hand their receiver back, so the type passes
                // through. Deliberately not `then`, which returns the block's
                // value, nor `presence`, which may return nil.
                b"freeze" | b"dup" | b"clone" | b"itself" | b"tap" => {
                    resolve_type(&call.receiver()?, at)
                }
                // Anything else needs to know what the method returns, which
                // only a signature states (D61, D62). The class it is called on
                // has to resolve first -- except for implicit self, where the
                // enclosing class *is* the answer.
                _ => {
                    let method = std::str::from_utf8(name.as_slice()).ok()?;
                    let on = match call.receiver() {
                        None => {
                            if singleton {
                                Receiver::Class(scope.last()?.clone())
                            } else {
                                Receiver::Instance(scope.last()?.clone())
                            }
                        }
                        Some(receiver) => resolve_type(&receiver, at)?,
                    };
                    at.sigs
                        .returns(on.class_name(), method, !on.is_instance())
                        .cloned()
                }
            }
        }
        _ => None,
    }
}

/// The class a match's receiver resolves to, when the match is a call on one.
///
/// Used to warn about a rule that renames across *several* classes without
/// saying which it meant. `Account#display_name` and `Company#display_name` are
/// different methods, and a rule with no `type:` constraint renames both at
/// exit 0 -- the clean, confident, wrong rewrite that Q10 calls the real danger.
pub(crate) fn receiver_class(found: &Match<'_>, sigs: &crate::sigs::Signatures) -> Option<String> {
    let receiver = found.node.as_call_node()?.receiver()?;
    let at = Where {
        scope: &found.scope,
        singleton: found.singleton,
        locals: &found.locals,
        sigs,
    };
    resolve_type(&receiver, &at).map(|r| r.class_name().to_string())
}

/// The class or module a node introduces, if any.
pub(crate) fn scope_name_of(node: &Node<'_>) -> Option<String> {
    match node {
        // The *path*, not the last segment: `class Account::Exporter` names a
        // class called `Account::Exporter`, and calling it `Exporter` loses the
        // only thing distinguishing it from every other `Exporter` in the repo.
        Node::ClassNode { .. } => qualified(&node.as_class_node()?.constant_path()),
        Node::ModuleNode { .. } => qualified(&node.as_module_node()?.constant_path()),
        Node::SingletonClassNode { .. } => Some(SINGLETON.to_string()),
        _ => None,
    }
}

/// The metavariable a parameter list's lone `*$P` stands for.
///
/// `def foo(*$P)` parses as a `ParametersNode` whose only content is a rest
/// parameter named after the placeholder -- not a splat over a run, which is
/// what `*$P` means in an argument list.
pub(crate) fn lone_rest_placeholder(node: &Node<'_>, prepared: &Prepared) -> Option<String> {
    let parameters = node.as_parameters_node()?;
    let kids = generated::children(node);
    if kids.len() != 1 {
        return None;
    }
    let rest = parameters.rest()?;
    let name = rest.as_rest_parameter_node()?.name()?;
    let key = std::str::from_utf8(name.as_slice()).ok()?;
    prepared.bindings.get(key).and_then(|b| b.name.clone())
}

/// The marker a singleton class body pushes onto the scope stack.
///
/// Not a class name: `class << self` opens a new *context*, not a new class, so
/// it is transparent when asking which class encloses a match.
const SINGLETON: &str = "<<self";

/// A constant path rendered whole -- `Billing::Account` rather than `Account`.
fn qualified(node: &Node<'_>) -> Option<String> {
    match node {
        Node::ConstantReadNode { .. } => {
            String::from_utf8(node.as_constant_read_node()?.name().as_slice().to_vec()).ok()
        }
        Node::ConstantPathNode { .. } => {
            let path = node.as_constant_path_node()?;
            let last = String::from_utf8(path.name()?.as_slice().to_vec()).ok()?;
            match path.parent() {
                Some(parent) => Some(format!("{}::{last}", qualified(&parent)?)),
                // `::Foo` -- rooted at the top level, which is where an
                // unqualified name already lives.
                None => Some(last),
            }
        }
        _ => None,
    }
}

/// The class a match sits in, fully qualified.
///
/// Lexical nesting is *namespacing*, not membership: `class Account; class Row`
/// declares `Account::Row`, which is a different class from `Account` and does
/// not inherit from it. Treating any enclosing name as a match meant a rule
/// scoped to `Account` rewrote code inside `Account::Row` and inside
/// `Billing::Account` -- two different classes that merely share a word.
pub(crate) fn enclosing_class(scope: &[String]) -> Option<String> {
    let named: Vec<&str> = scope
        .iter()
        .map(String::as_str)
        .filter(|s| *s != SINGLETON)
        .collect();
    (!named.is_empty()).then(|| named.join("::"))
}

/// Every node in `target` matching `pattern`, with its lexical scope.
///
/// `find` is reentrant (D15): nested matches are reported, because find is
/// observation and suppressing one would be a lie.
/// What a rule requires of a match, beyond matching structurally.
pub(crate) struct Criteria<'a> {
    pub constraints: &'a HashMap<String, Constraint>,
    /// Sub-patterns for `contains:` constraints, keyed by capture name.
    pub contained: &'a HashMap<String, Prepared>,
    pub scope: &'a Scope,
    pub hierarchy: &'a Hierarchy,
    pub sigs: &'a crate::sigs::Signatures,
    /// Whether to record why candidates were declined. Off, nothing is built.
    pub explain: bool,
}

/// The unconstrained defaults, held once so `Criteria::none()` can borrow them.
type Empty = (
    HashMap<String, Constraint>,
    HashMap<String, Prepared>,
    Scope,
    Hierarchy,
    crate::sigs::Signatures,
);

impl Criteria<'_> {
    /// Criteria that accept any structural match.
    pub(crate) fn none() -> Criteria<'static> {
        static EMPTY: std::sync::OnceLock<Empty> = std::sync::OnceLock::new();
        let (constraints, contained, scope, hierarchy, sigs) = EMPTY.get_or_init(|| {
            (
                HashMap::new(),
                HashMap::new(),
                Scope::default(),
                Hierarchy::default(),
                crate::sigs::Signatures::default(),
            )
        });
        Criteria {
            explain: false,
            constraints,
            contained,
            scope,
            hierarchy,
            sigs,
        }
    }
}

pub(crate) fn search<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
    criteria: &Criteria<'_>,
) -> Vec<Match<'pr>> {
    search_explaining(pattern, target, prepared, criteria).0
}

/// As [`search`], also returning why candidates were declined.
///
/// Empty unless `criteria.explain` is set.
pub(crate) fn search_explaining<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
    criteria: &Criteria<'_>,
) -> (Vec<Match<'pr>>, Vec<Rejection>) {
    let mut state = WalkState {
        scope: Vec::new(),
        locals: HashMap::new(),
        singleton: false,
        out: Vec::new(),
        rejections: Vec::new(),
    };
    walk(pattern, target, prepared, criteria, &mut state);
    (state.out, state.rejections)
}

impl Verdict {
    /// The capture this verdict is about, if it is about one.
    pub(crate) fn capture(&self) -> Option<String> {
        match self {
            Verdict::BadBinding { capture, .. } => Some(format!("${capture}")),
            _ => None,
        }
    }

    /// Which predicate declined the match, as a stable field value.
    pub(crate) fn constraint(&self) -> &'static str {
        match self {
            Verdict::Ok => "none",
            Verdict::Bug(_) => "rule-bug",
            Verdict::WrongScope(ScopeMiss::Inside { .. }) => "inside",
            Verdict::WrongScope(ScopeMiss::Singleton { .. }) => "singleton",
            Verdict::BadBinding { miss, .. } => match miss {
                ConstraintMiss::Name { .. } => "name",
                ConstraintMiss::NameNot { .. } => "name_not",
                ConstraintMiss::Type { .. } => "type",
                ConstraintMiss::Is { .. } => "is",
                ConstraintMiss::Contains { .. } => "contains",
                ConstraintMiss::Length { .. } => "length",
                ConstraintMiss::SameNameAs { .. } => "same_name_as",
            },
        }
    }

    /// What the constraint wanted and what it saw, in one line.
    pub(crate) fn detail(&self) -> String {
        match self {
            Verdict::Ok => String::new(),
            Verdict::Bug(why) => (*why).to_string(),
            Verdict::WrongScope(ScopeMiss::Inside { wanted, found }) => {
                let found = if found.is_empty() {
                    "the top level".to_string()
                } else {
                    found.join("::")
                };
                format!("needs `inside: {wanted}`, found {found}")
            }
            Verdict::WrongScope(ScopeMiss::Singleton { wanted }) => {
                format!(
                    "needs `singleton: {wanted}`, found {}",
                    if *wanted {
                        "an instance method"
                    } else {
                        "a singleton method"
                    }
                )
            }
            Verdict::BadBinding { miss, .. } => match miss {
                ConstraintMiss::Name { actual, allowed } => match actual {
                    Some(a) => format!("`{a}` is not one of {}", allowed.join(", ")),
                    None => format!(
                        "no identifier here; `name:` wants one of {}",
                        allowed.join(", ")
                    ),
                },
                // The distinction that matters most in this whole report:
                // receiver narrowing is conservative, so an unresolved receiver
                // is a different problem from a wrongly-resolved one, and they
                // read identically until you say which happened.
                ConstraintMiss::Type {
                    resolved: None,
                    wanted,
                    ..
                } => format!(
                    "receiver did not resolve; `type: {wanted}` only matches receivers rwr can resolve"
                ),
                ConstraintMiss::Type {
                    resolved: Some(got),
                    wanted,
                    wrong_kind,
                } => {
                    if *wrong_kind {
                        format!(
                            "resolved to {got}, but as the other of instance/class than `type: {wanted}` means"
                        )
                    } else {
                        format!("resolved to {got}, not {wanted}")
                    }
                }
                ConstraintMiss::NameNot { actual } => {
                    format!("`{actual}` is excluded by `name_not:`")
                }
                ConstraintMiss::Is { wanted } => format!("not {wanted:?}"),
                ConstraintMiss::Contains { pattern } => {
                    format!("does not contain `{pattern}`")
                }
                ConstraintMiss::Length { actual, wanted } => match actual {
                    Some(a) => format!("{a} character(s), not {wanted}"),
                    None => format!("not a literal, so `length: {wanted}` cannot apply"),
                },
                ConstraintMiss::SameNameAs { other } => {
                    format!("does not name the same identifier as {other}")
                }
            },
        }
    }
}

/// Where a binding sits in the source, for a report to quote it.
fn bound_range(bound: &Bound<'_>) -> Option<(usize, usize)> {
    match bound {
        Bound::One(node) => {
            let loc = node.location();
            Some((loc.start_offset(), loc.end_offset()))
        }
        // A name capture has no node, and a run of them has no single range.
        Bound::Name(_) | Bound::Many(_) => None,
    }
}

/// A site the pattern matched structurally, then a constraint declined.
///
/// Only recorded when `-e` asked for it: rejections are debugging detail about
/// sites a rule correctly refused, not a blind spot -- the account of what rwr
/// could not see stays unconditional.
#[derive(Debug)]
pub(crate) struct Rejection {
    /// Byte offset of the candidate, for the caller to turn into a line.
    pub start: usize,
    pub verdict: Verdict,
    /// Byte range of the binding that was refused, for the caller to slice.
    pub bound: Option<(usize, usize)>,
}

/// Everything the walk carries down the tree and mutates as it goes.
struct WalkState<'pr> {
    /// Enclosing class and module names, outermost first.
    scope: Vec<String>,
    /// Variables whose class is known from an assignment.
    locals: HashMap<String, String>,
    /// Inside `def self.x` or `class << self`, which decides what `self` means.
    singleton: bool,
    out: Vec<Match<'pr>>,
    rejections: Vec<Rejection>,
}

fn walk<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
    criteria: &Criteria<'_>,
    state: &mut WalkState<'pr>,
) {
    // Recorded before matching so an assignment is visible to uses that follow
    // it in the same body, which is the order source is written in.
    if let Some((name, class)) = assigned_class(target) {
        state.locals.insert(name, class);
    }

    // Retry on a constraint failure rather than discarding the match: a node
    // may admit several bindings, and only a later one may satisfy the rule
    // (Q13). Forbidding the rejected binding forces backtracking to a different
    // one, and terminates because bindings are finite.
    let mut forbidden = Forbidden::new();
    // Buffered rather than pushed directly: a later binding may satisfy the
    // rule, and a site that ultimately matched has nothing to explain.
    let mut attempts: Vec<Rejection> = Vec::new();
    for _ in 0..MAX_REBINDS {
        let mut env = Env::new();
        if !match_node(pattern, target, prepared, &mut env, &forbidden) {
            break;
        }
        let candidate = Match {
            node: generated::dup(target),
            env,
            scope: state.scope.clone(),
            singleton: state.singleton,
            locals: state.locals.clone(),
        };
        match verdict(
            &candidate,
            criteria.constraints,
            criteria.contained,
            criteria.scope,
            criteria.hierarchy,
            criteria.sigs,
        ) {
            Verdict::Ok => {
                state.out.push(candidate);
                attempts.clear();
                break;
            }
            // Wrong place, not wrong binding -- no rebinding can fix it.
            verdict @ (Verdict::WrongScope(_) | Verdict::Bug(_)) => {
                if criteria.explain {
                    attempts.push(Rejection {
                        start: target.location().start_offset(),
                        verdict,
                        bound: None,
                    });
                }
                break;
            }
            Verdict::BadBinding { capture, miss } => {
                let Some(bound) = candidate.env.get(&capture) else {
                    break;
                };
                if criteria.explain {
                    attempts.push(Rejection {
                        start: target.location().start_offset(),
                        bound: bound_range(bound),
                        verdict: Verdict::BadBinding {
                            capture: capture.clone(),
                            miss,
                        },
                    });
                }
                forbidden
                    .entry(capture)
                    .or_default()
                    .push(fingerprint(bound));
            }
        }
    }
    state.rejections.append(&mut attempts);

    let entered = scope_name_of(target);
    if let Some(name) = &entered {
        state.scope.push(name.clone());
        // Ruby does not care what order methods appear in, so neither should
        // rwr: a class's instance-variable assignments are collected up front
        // rather than discovered in source order, or `@account.foo` in a method
        // written above `initialize` would not resolve.
        collect_ivars(target, &mut state.locals);
    }
    // A method body is a fresh *local* scope, so locals must not leak across it
    // -- but an instance variable belongs to the class and is typically
    // assigned in `initialize` and read from every other method, so those are
    // carried through.
    let shadowed = matches!(target, Node::DefNode { .. }).then(|| {
        let carried: HashMap<String, String> = state
            .locals
            .iter()
            .filter(|(k, _)| k.starts_with('@'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        std::mem::replace(&mut state.locals, carried)
    });

    // `def self.x` and `class << self` both put their bodies in singleton
    // context, which is what makes `self` mean the class rather than an
    // instance.
    let inner_singleton = match target {
        // `|| state.singleton` because a plain `def` *inside* `class << self` is
        // a singleton method too. Overwriting instead of inheriting cleared the
        // flag on entry to every such method, so an instance rename rewrote the
        // call sites inside a class method -- introducing a NoMethodError into
        // code it was never about, which is the worst pairing available: a wrong
        // rewrite, in a method the rule had correctly declined to rename.
        Node::DefNode { .. } => {
            target.as_def_node().is_some_and(|d| d.receiver().is_some()) || state.singleton
        }
        Node::SingletonClassNode { .. } => true,
        // A class or module body starts a fresh instance context, however it was
        // reached -- otherwise `class << self; class Inner` would carry the flag
        // into an ordinary class.
        Node::ClassNode { .. } | Node::ModuleNode { .. } => false,
        _ => state.singleton,
    };

    let outer_singleton = std::mem::replace(&mut state.singleton, inner_singleton);
    for child in generated::children(target) {
        walk(pattern, &child, prepared, criteria, state);
    }
    state.singleton = outer_singleton;

    if let Some(saved) = shadowed {
        // Locals are discarded with the method that declared them, but an
        // instance variable belongs to the class -- typically assigned in
        // `initialize` and read from every other method -- so bindings learned
        // inside a `def` propagate back out.
        let learned: Vec<(String, String)> = state
            .locals
            .iter()
            .filter(|(k, _)| k.starts_with('@'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        state.locals = saved;
        state.locals.extend(learned);
    }
    if entered.is_some() {
        state.scope.pop();
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::pattern::prepare::prepare;
    use crate::rule::Kind;

    /// How many sites a whole rule -- pattern plus `where:` -- matches.
    fn applied(rule_yaml: &str, source: &str) -> usize {
        let rule: crate::rule::Rule = serde_yaml::from_str(rule_yaml).expect("rule parses");
        let prepared =
            crate::pattern::prepare::prepare_with(&rule.pattern, &rule.constant_captures())
                .expect("pattern prepares");
        let p_result = ruby_prism::parse(prepared.source.as_bytes());
        let p_root = pattern_root(&p_result.node()).expect("single-statement pattern");
        let t_result = ruby_prism::parse(source.as_bytes());
        assert_eq!(t_result.errors().count(), 0, "target does not parse");
        let hierarchy = Hierarchy::default();
        // Built from the same source, so a test can exercise signature-driven
        // resolution through the path a real run takes.
        let sigs = crate::sigs::Signatures::from_sources(&[crate::source::Source::Owned(
            source.as_bytes().to_vec(),
        )])
        .0;
        let contained = rule.contained().expect("sub-pattern prepares");
        let criteria = Criteria {
            explain: false,
            constraints: &rule.constraints,
            contained: &contained,
            scope: &rule.scope,
            hierarchy: &hierarchy,
            sigs: &sigs,
        };
        search(&p_root, &t_result.node(), &prepared, &criteria).len()
    }

    fn matches(pattern: &str, source: &str) -> usize {
        let prepared = prepare(pattern).expect("pattern prepares");
        let p_result = ruby_prism::parse(prepared.source.as_bytes());
        let p_root = pattern_root(&p_result.node()).expect("single-statement pattern");
        let t_result = ruby_prism::parse(source.as_bytes());
        assert_eq!(t_result.errors().count(), 0, "target does not parse");
        search(&p_root, &t_result.node(), &prepared, &Criteria::none()).len()
    }

    #[test]
    fn literal_pattern_matches_itself() {
        assert_eq!(matches("return nil", "def a; return nil; end"), 1);
        assert_eq!(matches("return nil", "def a; return 1; end"), 0);
    }

    /// The corpus 001 fixture in miniature: a text search finds four, a
    /// structural one finds one.
    #[test]
    fn strings_comments_and_heredocs_are_not_code() {
        let src = r#"
def a
  return nil if x
  # return nil
  s = "return nil"
  t = <<~T
    return nil
  T
end
"#;
        assert_eq!(matches("return nil", src), 1);
    }

    /// `return nil_value` must not match `return nil` -- the prefix bug that
    /// makes comby produce a different working program.
    #[test]
    fn identifier_prefixes_do_not_match() {
        assert_eq!(matches("return nil", "def a; return nil_value; end"), 0);
    }

    #[test]
    fn metavariables_bind_any_node() {
        assert_eq!(matches("foo($A)", "foo(1); foo(bar); foo(x.y)"), 3);
        assert_eq!(matches("foo($A)", "foo(); foo(1, 2)"), 0);
    }

    /// D16: a repeated metavariable requires AST equality, not equal text.
    #[test]
    fn repeated_metavariable_requires_equality() {
        assert_eq!(matches("foo($A, $A)", "foo(x, x)"), 1);
        assert_eq!(matches("foo($A, $A)", "foo(x, y)"), 0);
        // Layout differs, structure does not.
        assert_eq!(matches("foo($A, $A)", "foo( x , x )"), 1);
    }

    #[test]
    fn sequence_metavariables_absorb_siblings() {
        assert_eq!(matches("foo(*$REST)", "foo(); foo(1); foo(1, 2, 3)"), 3);
        assert_eq!(matches("foo($A, *$REST)", "foo(1); foo(1, 2)"), 2);
        assert_eq!(matches("foo($A, *$REST)", "foo()"), 0);
    }

    #[test]
    fn anonymous_wildcards_do_not_bind() {
        assert_eq!(matches("foo(_)", "foo(1); foo(x)"), 2);
        assert_eq!(matches("foo(_, _)", "foo(1, 2)"), 1);
    }

    /// Metavariables in method-name position, which only lex because of D18.
    #[test]
    fn metavariables_match_method_names() {
        assert_eq!(matches("x.$M", "x.foo; x.bar; y.foo"), 2);
    }

    /// A metavariable may bind in a name position and be met again as an
    /// expression. `{ |$P| $P.active? }` is the natural way to write this and
    /// would otherwise silently match nothing.
    #[test]
    fn a_metavariable_spans_name_and_node_positions() {
        assert_eq!(matches("x.each { |$P| $P.go }", "x.each { |a| a.go }"), 1);
        assert_eq!(matches("x.each { |$P| $P.go }", "x.each { |a| b.go }"), 0);
    }

    /// Brace and do/end blocks are the same structure, so one pattern matches
    /// both -- something a text or template tool has to special-case.
    #[test]
    fn brace_and_do_end_blocks_are_one_structure() {
        assert_eq!(
            matches("x.each { |$P| $B }", "x.each do |a|\n  go(a)\nend"),
            1
        );
    }

    fn hits_for<'a>(
        pattern: &str,
        src: &'a str,
        parsed: &'a ruby_prism::ParseResult<'a>,
        prepared: &'a Prepared,
        p_node: &'a Node<'a>,
    ) -> Vec<Match<'a>> {
        let _ = (pattern, src);
        let p_root = pattern_root(p_node).expect("single expression");
        search(&p_root, &parsed.node(), prepared, &Criteria::none())
    }

    /// Method-name alternation, the predicate ranked first in the backlog:
    /// `select` and `find_all` are synonyms and one rule must match both.
    #[test]
    fn name_constraints_narrow_a_match() {
        let prepared = prepare("$R.$SEL { |$P| $B }.first").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "a.select { |x| x.b }.first\nc.find_all { |y| y.d }.first\ne.reject { |z| z.f }.first\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 3, "structural match should find all three");

        let mut constraints = HashMap::new();
        constraints.insert(
            "$SEL".to_string(),
            Constraint {
                name: Some(vec!["select".into(), "find_all".into()]),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        let narrowed = hits
            .iter()
            .filter(|m| {
                satisfies(
                    m,
                    &constraints,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default(),
                )
            })
            .count();
        assert_eq!(narrowed, 2, "reject is not a synonym for select");
    }

    /// Receiver narrowing: the capability no other Ruby structural tool
    /// offers. `node_pattern` has no notion of a receiver and Ruby LSP matches
    /// by bare name, so both would rewrite all three of these.
    #[test]
    fn type_constraints_narrow_by_receiver() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "Account.display_name\nWidget.display_name\nthing.display_name\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 3, "all three match structurally");

        // These are constant receivers, so they are *class* calls. Asking for
        // Account's instance method correctly matches none of them -- the
        // distinction this test used to be blind to.
        let mut instances = HashMap::new();
        instances.insert(
            "$R".to_string(),
            Constraint {
                receiver_type: Some("Account".into()),
                kind: Some(Kind::Instance),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &instances,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            0
        );

        let mut classes = HashMap::new();
        classes.insert(
            "$R".to_string(),
            Constraint {
                receiver_type: Some("Account".into()),
                kind: Some(Kind::Class),
                ..Default::default()
            },
        );
        let narrowed = hits
            .iter()
            .filter(|m| {
                satisfies(
                    m,
                    &classes,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default(),
                )
            })
            .count();
        assert_eq!(narrowed, 1, "only Account's class call, not Widget's");
    }

    /// Q13: a constraint rejection must drive backtracking, not discard the
    /// match. Binding `$K` to an already-shortened `name:` fails
    /// `same_name_as` -- an implicit value has no identifier -- and the `size`
    /// pair, which would satisfy it, must still be found.
    /// An unresolved receiver and a wrongly-resolved one are different problems
    /// with different fixes, and they read identically until the report says
    /// which happened. Receiver narrowing is conservative, so this is the most
    /// common surprise the design produces.
    #[test]
    fn an_unresolved_receiver_reads_differently_from_a_wrong_one() {
        let unresolved = Verdict::BadBinding {
            capture: "R".to_string(),
            miss: ConstraintMiss::Type {
                resolved: None,
                wanted: "Widget".to_string(),
                wrong_kind: false,
            },
        };
        let wrong = Verdict::BadBinding {
            capture: "R".to_string(),
            miss: ConstraintMiss::Type {
                resolved: Some("Gadget".to_string()),
                wanted: "Widget".to_string(),
                wrong_kind: false,
            },
        };
        assert!(unresolved.detail().contains("did not resolve"));
        assert!(wrong.detail().contains("Gadget"));
        assert_ne!(unresolved.detail(), wrong.detail());
        // Both name the same predicate, so a machine consumer groups them.
        assert_eq!(unresolved.constraint(), "type");
        assert_eq!(wrong.constraint(), "type");
    }

    /// A rule bug is not a scope miss. Both used to report as `WrongScope`,
    /// which sent an author looking at their `scope:` for a typo'd `where:` key.
    #[test]
    fn a_rule_bug_is_distinct_from_a_scope_miss() {
        assert_eq!(Verdict::Bug("x").constraint(), "rule-bug");
        assert_eq!(
            Verdict::WrongScope(ScopeMiss::Singleton { wanted: true }).constraint(),
            "singleton"
        );
    }

    #[test]
    fn a_rejected_binding_is_retried_not_abandoned() {
        let prepared = prepare("{**$B, $K: $V, **$A}").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let p_root = pattern_root(&p_node).expect("single expression");

        let mut constraints = HashMap::new();
        constraints.insert(
            "$K".to_string(),
            Constraint {
                same_name_as: Some("$V".into()),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        let hierarchy = Hierarchy::default();
        let sigs = crate::sigs::Signatures::default();
        let contained = std::collections::HashMap::new();
        let criteria = Criteria {
            explain: false,
            constraints: &constraints,
            contained: &contained,
            scope: &scope,
            hierarchy: &hierarchy,
            sigs: &sigs,
        };

        // `name:` is already shorthand and cannot satisfy the constraint, so the
        // search must move on to `size: size` rather than reporting nothing.
        let src = "x = {name:, size: size}\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        assert_eq!(
            search(&p_root, &parsed.node(), &prepared, &criteria).len(),
            1
        );
    }

    /// `{foo: foo}` -> `{foo:}` turns on a symbol key and a variable read
    /// naming the same identifier -- the same name across different node kinds,
    /// which D16's AST equality cannot express.
    #[test]
    fn same_name_relates_two_captures() {
        let prepared = prepare("{$K => $V}").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "a = {:foo => foo}\nb = {:foo => bar}\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 2, "both match structurally");

        let mut constraints = HashMap::new();
        constraints.insert(
            "$K".to_string(),
            Constraint {
                same_name_as: Some("$V".into()),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &constraints,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1,
            "only the pair naming the same identifier"
        );
    }

    /// D51: a rename must reach subclass call sites, or it ships a
    /// NoMethodError. Off by default -- narrowing may only ever narrow -- and
    /// on for a rename, where leaving one behind breaks the code.
    #[test]
    fn subclasses_are_admitted_only_when_asked_for() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "premium = Premium.new\npremium.display_name\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 1);

        let hierarchy = Hierarchy::from_source("class Premium < Account; end");
        let scope = Scope::default();
        let constrain = |subclasses| {
            let mut c = HashMap::new();
            c.insert(
                "$R".to_string(),
                Constraint {
                    name: None,
                    receiver_type: Some("Account".into()),
                    kind: Some(Kind::Instance),
                    subclasses,
                    ..Default::default()
                },
            );
            c
        };

        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &constrain(Some(true)),
                    &scope,
                    &hierarchy,
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1,
            "a Premium receiver should match Account once subclasses are admitted"
        );
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &constrain(None),
                    &scope,
                    &hierarchy,
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            0,
            "and must not, by default"
        );
    }

    /// An unresolvable receiver must *not* match. Narrowing may only ever
    /// narrow -- a site rwr cannot resolve is missed and surfaces as residue,
    /// never silently rewritten.
    #[test]
    fn an_unresolvable_receiver_does_not_match() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "thing.display_name\n@memo.display_name\na.b.display_name\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);

        let mut constraints = HashMap::new();
        constraints.insert(
            "$R".to_string(),
            Constraint {
                receiver_type: Some("Account".into()),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &constraints,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            0
        );
    }

    /// Ruby does not care what order methods appear in. A read written above
    /// the `initialize` that assigns the ivar must still resolve.
    #[test]
    fn instance_variables_resolve_regardless_of_method_order() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "class Widget\n  def go\n    @account.display_name\n  end\n\n  def initialize\n    @account = Account.new\n  end\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);

        let mut constraints = HashMap::new();
        constraints.insert(
            "$R".to_string(),
            Constraint {
                receiver_type: Some("Account".into()),
                kind: Some(Kind::Instance),
                ..Default::default()
            },
        );
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &constraints,
                    &Scope::default(),
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1
        );
    }

    /// Instance variables are 5.7% of rails call receivers and are
    /// overwhelmingly assigned once in `initialize` and read from every other
    /// method -- so unlike a local, an ivar's binding must survive a `def`.
    #[test]
    fn instance_variables_resolve_across_methods() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "class Widget\n  def initialize\n    @account = Account.new\n    @other = Company.new\n  end\n\n  def go\n    @account.display_name\n  end\n\n  def stop\n    @other.display_name\n  end\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 2, "both match structurally");

        let mut constraints = HashMap::new();
        constraints.insert(
            "$R".to_string(),
            Constraint {
                receiver_type: Some("Account".into()),
                kind: Some(Kind::Instance),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &constraints,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1,
            "only the ivar assigned from Account.new"
        );
    }

    /// Locals assigned from a constructor resolve, which reaches the
    /// second-largest receiver bucket (17.9% of rails calls) with no inference
    /// beyond reading the assignment.
    #[test]
    fn locals_assigned_from_a_constructor_resolve() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "def a\n  account = Account.new\n  account.display_name\nend\ndef b\n  widget = Widget.new\n  widget.display_name\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 2, "both match structurally");

        let mut constraints = HashMap::new();
        constraints.insert(
            "$R".to_string(),
            Constraint {
                receiver_type: Some("Account".into()),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        let narrowed = hits
            .iter()
            .filter(|m| {
                satisfies(
                    m,
                    &constraints,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default(),
                )
            })
            .count();
        assert_eq!(narrowed, 1, "only the local assigned from Account.new");
    }

    /// `Account.display_name` and `account.display_name` name different
    /// methods. Conflating them made a rename of one silently rewrite the
    /// other, which is the exact failure the design exists to prevent.
    #[test]
    fn class_and_instance_receivers_are_different_methods() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "Account.display_name\naccount = Account.new\naccount.display_name\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 2, "both match structurally");

        let scope = Scope::default();
        let constrain = |kind| {
            let mut c = HashMap::new();
            c.insert(
                "$R".to_string(),
                Constraint {
                    name: None,
                    receiver_type: Some("Account".into()),
                    kind,
                    subclasses: None,
                    ..Default::default()
                },
            );
            c
        };

        let instances = constrain(Some(Kind::Instance));
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &instances,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1,
            "only account.display_name is an instance call"
        );

        let classes = constrain(Some(Kind::Class));
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &classes,
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1,
            "only Account.display_name is a class call"
        );

        // Default is instance, matching Ruby's `Account#display_name`.
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &constrain(None),
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1
        );
    }

    /// `self` means the class inside `def self.x` and an instance inside an
    /// ordinary method, so the same expression resolves differently.
    #[test]
    fn self_resolves_by_singleton_context() {
        let prepared = prepare("self.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "class Account\n  def self.a; self.display_name; end\n  def b; self.display_name; end\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits.iter().filter(|m| m.singleton).count(), 1);
    }

    /// Implicit-self calls split by singleton context, so a class-method
    /// rename cannot reach into an instance method's body or the reverse.
    #[test]
    fn inside_can_require_singleton_context() {
        let prepared = prepare("display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src =
            "class Account\n  def self.a; display_name; end\n  def b; display_name; end\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 2);

        let singleton = Scope {
            inside: Some("Account".into()),
            singleton: Some(true),
            ..Default::default()
        };
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &HashMap::new(),
                    &singleton,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1
        );

        let instance = Scope {
            inside: Some("Account".into()),
            singleton: Some(false),
            ..Default::default()
        };
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(
                    m,
                    &HashMap::new(),
                    &instance,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default()
                ))
                .count(),
            1
        );
    }

    /// `inside:` reaches implicit-self call sites, which measurement (b) found
    /// are 43.5% of all calls -- the largest bucket, and free from lexical
    /// scope alone.
    #[test]
    fn inside_narrows_to_a_lexical_scope() {
        let prepared = prepare("display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "class Account\n  def a; display_name; end\nend\nclass Widget\n  def b; display_name; end\nend\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 2, "both implicit-self calls match structurally");

        let scope = Scope {
            inside: Some("Account".into()),
            ..Default::default()
        };
        let narrowed = hits
            .iter()
            .filter(|m| {
                satisfies(
                    m,
                    &HashMap::new(),
                    &scope,
                    &Hierarchy::default(),
                    &crate::sigs::Signatures::default(),
                )
            })
            .count();
        assert_eq!(narrowed, 1, "only the call inside Account");
    }

    /// A constraint naming a metavariable the pattern never binds is a rule
    /// bug, and refusing surfaces it rather than silently passing.
    #[test]
    fn a_constraint_on_an_unbound_metavariable_refuses() {
        let prepared = prepare("foo($A)").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "foo(1)\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);

        let mut constraints = HashMap::new();
        constraints.insert(
            "$NOPE".to_string(),
            Constraint {
                name: Some(vec!["x".into()]),
                ..Default::default()
            },
        );
        let scope = Scope::default();
        assert!(hits.iter().all(|m| !satisfies(
            m,
            &constraints,
            &scope,
            &Hierarchy::default(),
            &crate::sigs::Signatures::default()
        )));
    }

    /// A metavariable in *label* position binds as a name, not a node -- the
    /// key's source is `foo:` including the colon, which a node capture would
    /// splice back in and produce `foo::`.
    #[test]
    fn a_label_key_binds_as_a_name() {
        let prepared = prepare("{$K: $V}").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "a = {foo: foo}\nb = {:bar => bar}\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        // One pattern covers both spellings: the rocket and the label parse to
        // the same node, and spelling is opt-in rather than baked into equality.
        assert_eq!(hits.len(), 2);
        assert!(matches!(hits[0].env.get("K"), Some(Bound::Name(_))));
    }

    /// Nested matches are reported: find is observation (D15).
    #[test]
    fn search_is_reentrant() {
        assert_eq!(matches("foo($A)", "foo(foo(1))"), 2);
    }

    /// A pattern matches a shape; `contains:` says "and somewhere inside it,
    /// this". Metavariables shared with the outer pattern must agree, which is
    /// the whole difference between a useful containment and a vacuous one.
    #[test]
    fn contains_ties_a_subpattern_to_the_outer_bindings() {
        let rule = "match: $R.each { |$X| $B }\nwhere:\n  $B:\n    contains: $X.$ASSOC.$FIELD\n";

        assert_eq!(applied(rule, "xs.each { |o| puts o.customer.name }\n"), 1);
        // Nothing of that shape inside.
        assert_eq!(applied(rule, "xs.each { |o| puts \"plain\" }\n"), 0);
        // The right shape on the *wrong* receiver: `other` is not the block's
        // parameter, and without the agreement check this would match.
        assert_eq!(
            applied(rule, "xs.each { |o| puts other.customer.name }\n"),
            0
        );
    }

    /// The outer binding is the block's *parameter* and the inner one a *read*
    /// of it -- same variable, different nodes, different spans. Comparing
    /// spans would call them different.
    #[test]
    fn a_parameter_and_a_read_of_it_agree() {
        let rule = "match: $R.each { |$X| $B }\nwhere:\n  $B:\n    contains: $X.$M\n";
        assert_eq!(applied(rule, "xs.each { |thing| thing.save }\n"), 1);
    }

    /// The part of the chained-receiver bucket that carries its own answer.
    ///
    /// Following a chain in general needs a method's return type, which only
    /// 2-4% of definitions state syntactically. `Widget.new` states it.
    #[test]
    fn a_constructor_chain_resolves_its_receiver() {
        let rule =
            "match: $R.display_name\nwhere:\n  $R:\n    type: Widget\nrewrite: $R.full_name\n";
        assert_eq!(applied(rule, "Widget.new.display_name\n"), 1);
        assert_eq!(applied(rule, "Widget.new(1, 2).display_name\n"), 1);
        assert_eq!(applied(rule, "Other.new.display_name\n"), 0);
        // Unresolved is not a match: narrowing may only ever narrow.
        assert_eq!(applied(rule, "something.display_name\n"), 0);
    }

    /// Methods that hand their receiver back pass the type through, and compose.
    #[test]
    fn identity_methods_pass_the_type_along() {
        let rule =
            "match: $R.display_name\nwhere:\n  $R:\n    type: Widget\nrewrite: $R.full_name\n";
        assert_eq!(applied(rule, "Widget.new.freeze.display_name\n"), 1);
        assert_eq!(applied(rule, "Widget.new.dup.itself.display_name\n"), 1);
        // `then` returns the *block's* value, so it is not one of them.
        assert_eq!(applied(rule, "Widget.new.then { |w| w }.display_name\n"), 0);
    }

    /// `Widget.new` is an instance; `Widget` is the class object. A constructor
    /// chain must not satisfy a class-method constraint.
    #[test]
    fn a_constructor_chain_is_an_instance_not_the_class() {
        let rule = "match: $R.display_name\nwhere:\n  $R:\n    type: Widget\n    kind: class\nrewrite: $R.full_name\n";
        assert_eq!(applied(rule, "Widget.new.display_name\n"), 0);
        assert_eq!(applied(rule, "Widget.display_name\n"), 1);
    }

    /// A signature is what makes the rest of the chained-receiver bucket
    /// reachable: `parser.document.foo` needs to know what `document` returns,
    /// and only a `sig` says so (D62).
    #[test]
    fn a_signature_resolves_a_chained_receiver() {
        let rule =
            "match: $R.display_name\nwhere:\n  $R:\n    type: Widget\nrewrite: $R.full_name\n";

        // Implicit self: the enclosing class is the receiver, so the signature
        // is looked up without needing to resolve anything first. This is the
        // largest slice of the chained bucket.
        let implicit = "class P\n  sig { returns(Widget) }\n  def widget; @w; end\n                          def go; widget.display_name; end\nend\n";
        assert_eq!(applied(rule, implicit), 1);

        // A type rwr cannot use yields nothing rather than a guess.
        let untyped = "class P\n  sig { returns(T.untyped) }\n  def widget; @w; end\n                         def go; widget.display_name; end\nend\n";
        assert_eq!(applied(rule, untyped), 0);

        // Resolution composes: a local from a constructor, then the signature.
        let chained = "class P\n  sig { returns(Widget) }\n  def widget; @w; end\nend\n\n                       p = P.new\np.widget.display_name\n";
        assert_eq!(applied(rule, chained), 1);

        // An unresolvable receiver stays unresolvable, signature or not.
        let unknown = "class P\n  sig { returns(Widget) }\n  def widget; @w; end\nend\n\n                       whatever.widget.display_name\n";
        assert_eq!(applied(rule, unknown), 0);
    }

    /// `gsub` -> `tr` is only valid character-for-character, so the predicate
    /// that makes it safe is the length -- not the shape, which is identical.
    #[test]
    fn length_separates_tr_from_gsub() {
        let rule = "match: $R.gsub($FROM, $TO)\nwhere:\n  $FROM:\n    is: string\n    length: 1\n  $TO:\n    is: string\n    length: 1\nrewrite: $R.tr($FROM, $TO)\n";
        assert_eq!(applied(rule, "a.gsub(\"-\", \"_\")\n"), 1);
        assert_eq!(applied(rule, "a.gsub(\"ab\", \"cd\")\n"), 0);
        assert_eq!(applied(rule, "a.gsub(/x/, \"y\")\n"), 0);
        // An interpolated string is a different node, so `is: string` excludes
        // it even though its content might be one character at runtime.
        assert_eq!(applied(rule, "a.gsub(\"#{v}\", \"y\")\n"), 0);
    }

    /// `is: constant` also picks the placeholder casing, because `FOO = 1` and
    /// `foo = 1` both parse and the case-repair loop only fires on a failure.
    #[test]
    fn is_constant_reaches_a_constant_assignment() {
        let rule = "match: $C = [*$ITEMS]\nwhere:\n  $C:\n    is: constant\nrewrite: $C = [*$ITEMS.sort]\n";
        assert_eq!(applied(rule, "FOO = [:b, :a]\n"), 1);
        assert_eq!(applied(rule, "foo = [:b, :a]\n"), 0);
    }
}
