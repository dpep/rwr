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
use crate::rule::{Constraint, Scope};
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

/// A sequence placeholder, i.e. `*$NAME` — a splat wrapping a placeholder.
fn splat_placeholder<'a>(
    node: &Node<'_>,
    bindings: &'a HashMap<String, Binding>,
) -> Option<&'a str> {
    let Node::SplatNode { .. } = node else {
        return None;
    };
    let splat = node.as_splat_node()?;
    placeholder(&splat.expression()?, bindings)
}

/// Bind `key`, enforcing D16: a repeated metavariable must match AST-equal
/// nodes, never merely equal source text.
fn bind<'pr>(env: &mut Env<'pr>, key: &str, value: Bound<'pr>) -> bool {
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
) -> bool {
    // A bare placeholder is a wildcard: it matches any node, and binds unless
    // it is anonymous.
    if let Some(key) = placeholder(pattern, &prepared.bindings) {
        // Key on the *metavariable* name: `foo($A, $A)` substitutes to two
        // distinct placeholders, so keying on those would never share a
        // binding and D16's equality check would never fire.
        return match prepared.bindings.get(key).and_then(|b| b.name.clone()) {
            None => true,
            Some(name) => bind(env, &name, Bound::One(generated::dup(target))),
        };
    }

    if std::mem::discriminant(pattern) != std::mem::discriminant(target) {
        return false;
    }

    if !match_atoms(pattern, target, prepared, env) {
        return false;
    }

    match_children(
        &generated::children(pattern),
        &generated::children(target),
        prepared,
        env,
    )
}

/// Atoms match pairwise, except that a name atom which *is* a placeholder acts
/// as a wildcard over the corresponding target name (`x.$M`, `def $M`).
fn match_atoms<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
    env: &mut Env<'pr>,
) -> bool {
    let (pa, ta) = (generated::atoms(pattern), generated::atoms(target));
    if pa.len() != ta.len() {
        return false;
    }
    pa.iter().zip(&ta).all(|(p, t)| match (p, t) {
        (Atom::Name(pn), Atom::Name(tn)) => match std::str::from_utf8(pn)
            .ok()
            .and_then(|s| prepared.bindings.get_key_value(s))
        {
            Some((_, binding)) => match &binding.name {
                None => true,
                Some(name) => bind(env, name, Bound::Name(tn.clone())),
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
                && !bind(&mut trial, name, Bound::Many(absorbed))
            {
                continue;
            }
            if match_children(rest, &target[take..], prepared, &mut trial) {
                *env = trial;
                return true;
            }
        }
        return false;
    }

    let Some((t_head, t_rest)) = target.split_first() else {
        // Target exhausted, pattern not. Prism gives `foo()` no arguments node
        // at all, so `foo(*$REST)` -- whose argument list can absorb nothing --
        // has one child the target lacks. Let such a subtree vanish.
        return pattern.iter().all(|p| vanishes(p, prepared, env));
    };
    let mut trial = env.clone();
    if match_node(head, t_head, prepared, &mut trial)
        && match_children(rest, t_rest, prepared, &mut trial)
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
fn vanishes<'pr>(pattern: &Node<'_>, prepared: &Prepared, env: &mut Env<'pr>) -> bool {
    if let Some(key) = splat_placeholder(pattern, &prepared.bindings) {
        return match prepared.bindings.get(key).and_then(|b| b.name.clone()) {
            None => true,
            Some(name) => bind(env, &name, Bound::Many(Vec::new())),
        };
    }
    // A container with no atoms of its own vanishes if everything inside it does.
    generated::atoms(pattern).is_empty()
        && !generated::children(pattern).is_empty()
        && generated::children(pattern)
            .iter()
            .all(|c| vanishes(c, prepared, env))
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
pub(crate) fn satisfies(
    found: &Match<'_>,
    constraints: &HashMap<String, Constraint>,
    scope: &Scope,
) -> bool {
    if let Some(wanted) = &scope.inside
        && !found.scope.iter().any(|s| s == wanted)
    {
        return false;
    }

    constraints.iter().all(|(key, constraint)| {
        let Some(bound) = found.env.get(key.trim_start_matches('$')) else {
            // A constraint naming a metavariable the pattern does not bind is a
            // rule bug. Refuse the match rather than silently ignoring it.
            return false;
        };

        if let Some(allowed) = &constraint.name {
            let actual = match bound {
                Bound::Name(bytes) => Some(bytes.clone()),
                Bound::One(node) => bare_name(node),
                Bound::Many(_) => None,
            };
            if !actual.is_some_and(|a| allowed.iter().any(|n| n.as_bytes() == a.as_slice())) {
                return false;
            }
        }

        if let Some(wanted) = &constraint.receiver_type {
            let Bound::One(node) = bound else {
                return false;
            };
            // Unresolved means "not known to be this type", never "assume yes".
            let resolved = resolve_type(node, &found.scope).or_else(|| {
                bare_name(node)
                    .and_then(|n| String::from_utf8(n).ok())
                    .and_then(|n| found.locals.get(&n).cloned())
            });
            if resolved.as_deref() != Some(wanted.as_str()) {
                return false;
            }
        }
        true
    })
}

/// The class a receiver node denotes, where syntax alone can tell.
///
/// Covers the buckets measurement (b) found dominant -- constants (14.3%) and
/// explicit self (0.8%) -- and returns `None` for locals, ivars and chains
/// (39.4%), which need inference this does not yet do. `None` narrows the match
/// away rather than admitting it, so the constraint is always conservative.
pub(crate) fn resolve_type(node: &Node<'_>, scope: &[String]) -> Option<String> {
    match node {
        Node::ConstantReadNode { .. } => {
            let name = node.as_constant_read_node()?.name().as_slice().to_vec();
            String::from_utf8(name).ok()
        }
        Node::ConstantPathNode { .. } => {
            // `A::B` denotes B; the path is how you reach it, not what it is.
            let path = node.as_constant_path_node()?;
            let name = path.name()?.as_slice().to_vec();
            String::from_utf8(name).ok()
        }
        Node::SelfNode { .. } => scope.last().cloned(),
        _ => None,
    }
}

/// The class or module a node introduces, if any.
fn scope_name(node: &Node<'_>) -> Option<String> {
    let name = match node {
        Node::ClassNode { .. } => node.as_class_node()?.name().as_slice().to_vec(),
        Node::ModuleNode { .. } => node.as_module_node()?.name().as_slice().to_vec(),
        Node::SingletonClassNode { .. } => b"<<self".to_vec(),
        _ => return None,
    };
    String::from_utf8(name).ok()
}

/// Every node in `target` matching `pattern`, with its lexical scope.
///
/// `find` is reentrant (D15): nested matches are reported, because find is
/// observation and suppressing one would be a lie.
pub(crate) fn search<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
) -> Vec<Match<'pr>> {
    let mut out = Vec::new();
    let mut scope = Vec::new();
    let mut locals = HashMap::new();
    walk(pattern, target, prepared, &mut scope, &mut locals, &mut out);
    out
}

/// The class a local is assigned from, when the assignment says so outright.
fn assigned_class(node: &Node<'_>) -> Option<(String, String)> {
    let write = node.as_local_variable_write_node()?;
    let call = write.value().as_call_node()?;
    if call.name().as_slice() != b"new" {
        return None;
    }
    let class = resolve_type(&call.receiver()?, &[])?;
    let name = String::from_utf8(write.name().as_slice().to_vec()).ok()?;
    Some((name, class))
}

fn walk<'pr>(
    pattern: &Node<'_>,
    target: &Node<'pr>,
    prepared: &Prepared,
    scope: &mut Vec<String>,
    locals: &mut HashMap<String, String>,
    out: &mut Vec<Match<'pr>>,
) {
    // Recorded before matching so an assignment is visible to uses that follow
    // it in the same body, which is the order source is written in.
    if let Some((name, class)) = assigned_class(target) {
        locals.insert(name, class);
    }

    let mut env = Env::new();
    if match_node(pattern, target, prepared, &mut env) {
        out.push(Match {
            node: generated::dup(target),
            env,
            scope: scope.clone(),
            locals: locals.clone(),
        });
    }

    let entered = scope_name(target);
    if let Some(name) = &entered {
        scope.push(name.clone());
    }
    // A method body is a fresh local scope, so bindings must not leak across.
    let shadowed = matches!(target, Node::DefNode { .. }).then(|| std::mem::take(locals));

    for child in generated::children(target) {
        walk(pattern, &child, prepared, scope, locals, out);
    }

    if let Some(saved) = shadowed {
        *locals = saved;
    }
    if entered.is_some() {
        scope.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::prepare::prepare;

    fn matches(pattern: &str, source: &str) -> usize {
        let prepared = prepare(pattern).expect("pattern prepares");
        let p_result = ruby_prism::parse(prepared.source.as_bytes());
        let p_root = pattern_root(&p_result.node()).expect("single-statement pattern");
        let t_result = ruby_prism::parse(source.as_bytes());
        assert_eq!(t_result.errors().count(), 0, "target does not parse");
        search(&p_root, &t_result.node(), &prepared).len()
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
        search(&p_root, &parsed.node(), prepared)
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
                receiver_type: None,
            },
        );
        let scope = Scope::default();
        let narrowed = hits
            .iter()
            .filter(|m| satisfies(m, &constraints, &scope))
            .count();
        assert_eq!(narrowed, 2, "reject is not a synonym for select");
    }

    /// Receiver narrowing: the capability no other Ruby structural tool offers.
    /// `node_pattern` has no notion of a receiver and Ruby LSP matches by bare
    /// name, so both would rewrite all three of these.
    #[test]
    fn type_constraints_narrow_by_receiver() {
        let prepared = prepare("$R.display_name").expect("prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let src = "Account.display_name\nWidget.display_name\nthing.display_name\n";
        let parsed = ruby_prism::parse(src.as_bytes());
        let hits = hits_for("", src, &parsed, &prepared, &p_node);
        assert_eq!(hits.len(), 3, "all three match structurally");

        let mut constraints = HashMap::new();
        constraints.insert(
            "$R".to_string(),
            Constraint {
                name: None,
                receiver_type: Some("Account".into()),
            },
        );
        let scope = Scope::default();
        let narrowed: Vec<_> = hits
            .iter()
            .filter(|m| satisfies(m, &constraints, &scope))
            .collect();
        assert_eq!(narrowed.len(), 1, "only the Account receiver should match");
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
                name: None,
                receiver_type: Some("Account".into()),
            },
        );
        let scope = Scope::default();
        assert_eq!(
            hits.iter()
                .filter(|m| satisfies(m, &constraints, &scope))
                .count(),
            0
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
                name: None,
                receiver_type: Some("Account".into()),
            },
        );
        let scope = Scope::default();
        let narrowed = hits
            .iter()
            .filter(|m| satisfies(m, &constraints, &scope))
            .count();
        assert_eq!(narrowed, 1, "only the local assigned from Account.new");
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
        };
        let narrowed = hits
            .iter()
            .filter(|m| satisfies(m, &HashMap::new(), &scope))
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
                receiver_type: None,
            },
        );
        let scope = Scope::default();
        assert!(hits.iter().all(|m| !satisfies(m, &constraints, &scope)));
    }

    /// Nested matches are reported: find is observation (D15).
    #[test]
    fn search_is_reentrant() {
        assert_eq!(matches("foo($A)", "foo(foo(1))"), 2);
    }
}
