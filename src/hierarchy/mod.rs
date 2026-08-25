//! The class hierarchy (D51).
//!
//! Renaming `Account#display_name` must reach `premium.display_name` where
//! `Premium < Account`, and must also rename `Premium`'s override -- otherwise
//! the rewrite ships a `NoMethodError`, which it demonstrably did before this
//! existed.
//!
//! This is the cross-file index Phase 1 deliberately avoided. It is affordable
//! because Phase 0 measurement (d) found a full rails parse takes under 200ms,
//! so the hierarchy is rebuilt per run rather than persisted -- no cache, no
//! invalidation, no staleness, and D5 still holds.

use crate::pattern::generated;
use rayon::prelude::*;
use ruby_prism::Node;
use std::collections::{HashMap, HashSet};

/// Superclass links, keyed by class name.
#[derive(Debug, Default, Clone)]
pub(crate) struct Hierarchy {
    superclass: HashMap<String, String>,
    /// Modules mixed into each class -- `include`, `prepend`, `extend`.
    ///
    /// Kept apart from `superclass` because the question they answer is
    /// different: a superclass link says what a class *is*, a mixin link says
    /// where else its methods are written. Rails puts a large share of a model's
    /// methods in concerns, so a report that only knows `class X < Y` is silent
    /// about most of the code the class actually runs.
    mixins: HashMap<String, Vec<String>>,
    /// `Alias = Account` -- another name for the same class.
    ///
    /// A constant alias is not inheritance and not a mixin: it is the *same*
    /// class reached by a second name, so a rename of `Account#foo` has to reach
    /// `Alias.new.foo` too. Kept apart from `superclass` for the reason the
    /// mixin map is: the question it answers is different.
    aliases: HashMap<String, String>,
    /// Modules whose instance methods are *also* singleton methods, through
    /// `extend self` or `module_function`.
    ///
    /// For these the two method tables hold the same method, so `Util.foo` and
    /// `Util#foo` are one thing and `kind:` stops discriminating. Without this a
    /// rename did half the job and reported the other half: renaming `Util#foo`
    /// rewrote the definition and filed every `Util.foo` call as residue.
    self_extended: HashSet<String>,
    /// Modules that *refine* each class, kept apart from the rest.
    ///
    /// A refinement is only in force in a file that says `using`, so it is not
    /// interchangeable with an `include`: a rename must not rewrite a call the
    /// refinement is intercepting, or the call quietly stops going through it.
    refines: HashMap<String, Vec<String>>,
}

/// The name a constant-ish node denotes, ignoring how it was reached.
pub(crate) fn constant_name(node: &Node<'_>) -> Option<String> {
    let bytes = match node {
        Node::ConstantReadNode { .. } => node.as_constant_read_node()?.name().as_slice().to_vec(),
        Node::ConstantPathNode { .. } => node.as_constant_path_node()?.name()?.as_slice().to_vec(),
        _ => return None,
    };
    String::from_utf8(bytes).ok()
}

/// The calls that attach one module's methods to another class.
const MIXINS: [&[u8]; 4] = [b"include", b"prepend", b"extend", b"refine"];

/// Whether a call makes the enclosing module's instance methods reachable on
/// the module itself.
///
/// `extend self` and `module_function` differ in visibility -- the latter makes
/// the instance copy private -- and not in the thing that matters here, which is
/// that one name now answers on both tables. `module_function :foo` names
/// particular methods and this does not track which: it marks the module, which
/// over-admits within a single module and never reaches outside one. The
/// alternative under-reports a rename, and a missed call site is a
/// NoMethodError where an extra candidate is a call that still resolves.
fn extends_itself(node: &Node<'_>) -> bool {
    let Some(call) = node.as_call_node() else {
        return false;
    };
    match call.name().as_slice() {
        b"module_function" => true,
        b"extend" => call.arguments().is_some_and(|a| {
            a.arguments()
                .iter()
                .any(|n| matches!(n, Node::SelfNode { .. }))
        }),
        _ => false,
    }
}

/// The modules a `include`/`prepend`/`extend` call names.
fn mixed_in(node: &Node<'_>) -> Vec<String> {
    let Some(call) = node.as_call_node() else {
        return Vec::new();
    };
    if !MIXINS.contains(&call.name().as_slice()) {
        return Vec::new();
    }
    call.arguments()
        .into_iter()
        .flat_map(|a| {
            a.arguments()
                .iter()
                .filter_map(|n| constant_name(&n))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The class a mixin call attaches to: the receiver if it names one, otherwise
/// the enclosing class.
///
/// `Account.prepend(Audit)` at the top of a file is the ordinary way to patch a
/// class you do not own, and it is exactly the shape that has no enclosing
/// class to attribute to.
fn mixin_host(node: &Node<'_>, enclosing: Option<&String>) -> Option<String> {
    let call = node.as_call_node()?;
    // `refine Account do` is inverted from `include`: the *argument* names the
    // class being extended and the enclosing module is what extends it. Same
    // relation, written the other way round.
    if call.name().as_slice() == b"refine" {
        return call
            .arguments()
            .and_then(|a| a.arguments().iter().next().and_then(|n| constant_name(&n)));
    }
    match call.receiver() {
        Some(receiver) => constant_name(&receiver),
        None => enclosing.cloned(),
    }
}

/// The class a `CONST = Other` assignment aliases, if it is one.
///
/// Only a bare constant on the right: `Alias = Account` is a second name for a
/// class, while `LIMIT = 5` or `Klass = Class.new` are not, and a value rwr
/// cannot name is left alone rather than guessed at.
fn constant_alias(node: &Node<'_>) -> Option<(String, String)> {
    let write = node.as_constant_write_node()?;
    let name = String::from_utf8(write.name().as_slice().to_vec()).ok()?;
    let target = constant_name(&write.value())?;
    (name != target).then_some((name, target))
}

/// Collect `class X < Y` pairs and the modules each class mixes in.
fn links(
    root: &Node<'_>,
    out: &mut Vec<(String, String)>,
    mixins: &mut Vec<(String, String)>,
    refined: &mut Vec<(String, String)>,
    selves: &mut Vec<String>,
    aliases: &mut Vec<(String, String)>,
) {
    // Carries the enclosing class, which a flat stack loses -- and without it an
    // `include` cannot be attributed to anything.
    let mut stack = vec![(generated::dup(root), None::<String>)];
    while let Some((node, enclosing)) = stack.pop() {
        let mut inner = enclosing.clone();
        if let Node::ClassNode { .. } = node
            && let Some(class) = node.as_class_node()
            && let Ok(name) = String::from_utf8(class.name().as_slice().to_vec())
        {
            if let Some(parent) = class.superclass()
                && let Some(parent) = constant_name(&parent)
            {
                out.push((name.clone(), parent));
            }
            inner = Some(name);
        }
        // Modules too: a module can `include` another, and a refinement's body
        // belongs to the module that wrote it. Tracking classes alone left
        // anything written inside a module with nothing to attribute it to.
        if let Node::ModuleNode { .. } = node
            && let Some(module) = node.as_module_node()
            && let Ok(name) = String::from_utf8(module.name().as_slice().to_vec())
        {
            inner = Some(name);
        }
        if let Some(pair) = constant_alias(&node) {
            aliases.push(pair);
        }
        if extends_itself(&node)
            && let Some(host) = enclosing.clone().or_else(|| inner.clone())
        {
            selves.push(host);
        }
        let modules = mixed_in(&node);
        if !modules.is_empty()
            && let Some(host) = mixin_host(&node, enclosing.as_ref())
        {
            let refines = node
                .as_call_node()
                .is_some_and(|c| c.name().as_slice() == b"refine");
            if refines {
                // The refinement's body belongs to the enclosing module, so that
                // is what contributes to the host.
                if let Some(module) = &enclosing {
                    refined.push((host.clone(), module.clone()));
                    mixins.push((host, module.clone()));
                }
            } else {
                for module in modules {
                    mixins.push((host.clone(), module));
                }
            }
        }
        for child in generated::children(&node) {
            stack.push((child, inner.clone()));
        }
    }
}

impl Hierarchy {
    /// Build only the part of the hierarchy reachable from `roots`.
    ///
    /// A rename names one class, and only its descendants matter -- so rather
    /// than parsing every file that declares any superclass, parse only those
    /// mentioning a class already known to be in the tree, and iterate to a
    /// fixpoint. `Gold < Premium < Account` is reached in two rounds: the first
    /// finds Premium, which puts "Premium" into the search set for the second.
    ///
    /// The full build parses ~8,700 files on the local Ruby corpus; this
    /// typically parses a handful, and is exact rather than approximate --
    /// nothing is guessed, only deferred until a name is known to matter.
    pub(crate) fn reachable_from(
        sources: &[crate::source::Source],
        roots: &[String],
    ) -> (Self, usize) {
        // No structural pre-filter. There used to be one -- a file was a
        // candidate only if it held `class` and `<`, or a mixin keyword -- and
        // it was wrong three times before it was measured.
        //
        // It cost two silent under-reports, each the same shape: the collector
        // learned something new, the filter did not, and the file was dropped
        // before parsing. `module_function` (D87) and a constant alias (D91)
        // both carry no structural signal at all, so the collector was correct
        // and never got to run. A filter that restates what the collector looks
        // for will drift from it every time the collector grows, and a dropped
        // file is indistinguishable from a file with nothing in it.
        //
        // It also bought nothing. The per-round search below already requires a
        // file to name a class known to be in the tree, and *that* is what keeps
        // the parse count down -- measured on rails, 60 files parsed of 3,321
        // either way. Removing the filter is a little faster, because running it
        // over every file cost more than the scans it saved: 42ms against 56ms,
        // minimum of seven runs.
        //
        // And it was hiding a real gap. A file is only a candidate once, so an
        // alias to a class discovered in a *later* round -- `Widget = Premium`
        // where Premium arrives in round two -- was filtered out before its
        // round came. Every file being a candidate closes that by construction.
        let candidates: Vec<&[u8]> = sources
            .par_iter()
            .map(crate::source::Source::bytes)
            .collect();

        let mut known: HashSet<String> = roots.iter().cloned().collect();
        let mut superclass: HashMap<String, String> = HashMap::new();
        let mut mixins: HashMap<String, Vec<String>> = HashMap::new();
        let mut self_extended: HashSet<String> = HashSet::new();
        let mut aliases: HashMap<String, String> = HashMap::new();
        let mut refines: HashMap<String, Vec<String>> = HashMap::new();
        let mut done = vec![false; candidates.len()];
        let mut parsed_total = 0usize;

        loop {
            let finders: Vec<memchr::memmem::Finder<'static>> = known
                .iter()
                .map(|n| memchr::memmem::Finder::new(n.as_bytes()).into_owned())
                .collect();

            type Round = (
                usize,
                Vec<(String, String)>,
                Vec<(String, String)>,
                Vec<(String, String)>,
                // Modules that extend themselves: one name, not a pair.
                Vec<String>,
                // Constant aliases: alias -> the class it names.
                Vec<(String, String)>,
            );
            let round: Vec<Round> = candidates
                .par_iter()
                .enumerate()
                .filter(|(i, _)| !done[*i])
                .filter_map(|(i, src)| {
                    let src: &[u8] = src;
                    // Only a file naming a class already known to be in the
                    // tree can extend it.
                    if !finders.iter().any(|f| f.find(src).is_some()) {
                        return None;
                    }
                    let parsed = ruby_prism::parse(src);
                    if parsed.errors().count() > 0 {
                        return None;
                    }
                    let (mut found, mut mixed, mut refined) = (Vec::new(), Vec::new(), Vec::new());
                    let mut selves = Vec::new();
                    let mut aliased = Vec::new();
                    links(
                        &parsed.node(),
                        &mut found,
                        &mut mixed,
                        &mut refined,
                        &mut selves,
                        &mut aliased,
                    );
                    Some((i, found, mixed, refined, selves, aliased))
                })
                .collect();

            parsed_total += round.len();
            let mut grew = false;
            for (i, found, mixed, refined, selves, aliased) in round {
                self_extended.extend(selves);
                aliases.extend(aliased);
                done[i] = true;
                for (child, parent) in found {
                    if known.contains(&parent) && known.insert(child.clone()) {
                        grew = true;
                    }
                    superclass.insert(child, parent);
                }
                for (class, module) in mixed {
                    mixins.entry(class).or_default().push(module);
                }
                for (class, module) in refined {
                    refines.entry(class).or_default().push(module);
                }
            }
            if !grew {
                break;
            }
        }

        (
            Hierarchy {
                superclass,
                mixins,
                aliases,
                self_extended,
                refines,
            },
            parsed_total,
        )
    }

    /// Whether `module` is mixed into `class` or into any of its descendants.
    ///
    /// The question a report asks about a concern: this occurrence sits in a
    /// module, so is that module part of the class the rule is about? Without
    /// it, everything a concern contributes -- and in Rails that is a large
    /// share of a model -- is dropped from the account with nothing said.
    pub(crate) fn contributes_to(&self, module: &str, class: &str) -> bool {
        self.mixins.iter().any(|(host, modules)| {
            modules.iter().any(|m| m == module)
                && (host == class || self.descends_from(host, class))
        })
    }

    /// The modules that refine `class`.
    ///
    /// A refinement only applies in a file that says `using`, so a call site in
    /// such a file may be dispatching to the refinement rather than the class --
    /// and renaming it there silently routes around the refinement.
    pub(crate) fn refined_by(&self, class: &str) -> &[String] {
        self.refines.get(class).map_or(&[], Vec::as_slice)
    }

    /// Whether `class` is `ancestor` or descends from it.
    ///
    /// Guards against a cycle, which valid Ruby cannot express but a
    /// half-written file can.
    /// The class a name really means, following constant aliases.
    ///
    /// `Alias = Account` makes `Alias` a second name for one class, so every
    /// question about it -- descent, method tables, a `type:` constraint -- is a
    /// question about `Account`. Chains resolve (`A = B; B = C`), and a cycle
    /// stops rather than spins: `A = B; B = A` is degenerate Ruby and must not
    /// hang a linter.
    pub(crate) fn canonical<'a>(&'a self, name: &'a str) -> &'a str {
        let mut current = name;
        for _ in 0..self.aliases.len() + 1 {
            match self.aliases.get(current) {
                Some(target) if target != current => current = target,
                _ => break,
            }
        }
        current
    }

    /// Whether this module's instance methods answer on the module itself, so
    /// that `Util.foo` and `Util#foo` name one method rather than two.
    pub(crate) fn extends_itself(&self, class: &str) -> bool {
        self.self_extended.contains(class)
    }

    pub(crate) fn descends_from(&self, class: &str, ancestor: &str) -> bool {
        let mut current = class;
        let mut seen = HashSet::new();
        loop {
            if current == ancestor {
                return true;
            }
            if !seen.insert(current.to_string()) {
                return false;
            }
            match self.superclass.get(current) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    /// Build from a single snippet. Test-only, and shared across modules so a
    /// matcher test can exercise real descent rather than an empty hierarchy.
    #[cfg(test)]
    pub(crate) fn from_source(source: &str) -> Self {
        let parsed = ruby_prism::parse(source.as_bytes());
        let (mut found, mut mixed) = (Vec::new(), Vec::new());
        let mut refined = Vec::new();
        let mut selves = Vec::new();
        let mut aliased = Vec::new();
        links(
            &parsed.node(),
            &mut found,
            &mut mixed,
            &mut refined,
            &mut selves,
            &mut aliased,
        );
        let mut mixins: HashMap<String, Vec<String>> = HashMap::new();
        for (class, module) in mixed {
            mixins.entry(class).or_default().push(module);
        }
        let mut refines: HashMap<String, Vec<String>> = HashMap::new();
        for (class, module) in refined {
            refines.entry(class).or_default().push(module);
        }
        Hierarchy {
            superclass: found.into_iter().collect(),
            mixins,
            aliases: aliased.into_iter().collect(),
            self_extended: selves.into_iter().collect(),
            refines,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `extend self` and `module_function` put one method on both tables, so
    /// `Util.foo` and `Util#foo` name the same thing. Without this a rename did
    /// half the job: it rewrote the definition and filed every call as residue.
    #[test]
    fn a_module_that_extends_itself_is_recorded() {
        for source in [
            "module Util\n  extend self\n  def foo; end\nend",
            "module Util\n  module_function\n  def foo; end\nend",
            "module Util\n  def foo; end\n  module_function :foo\nend",
        ] {
            let h = Hierarchy::from_source(source);
            assert!(h.extends_itself("Util"), "not recorded: {source}");
        }
    }

    /// `Alias = Account` is a second name for one class, so every question about
    /// the alias is a question about the class.
    #[test]
    fn a_constant_alias_resolves_to_the_class_it_names() {
        let h = Hierarchy::from_source("class Account; end\nAlias = Account\n");
        assert_eq!(h.canonical("Alias"), "Account");
        // A name with no alias is already canonical.
        assert_eq!(h.canonical("Account"), "Account");
        assert_eq!(h.canonical("Unknown"), "Unknown");
    }

    /// Chains resolve, and a cycle stops rather than spinning. `A = B; B = A` is
    /// degenerate Ruby and must not hang a linter.
    #[test]
    fn alias_chains_resolve_and_cycles_terminate() {
        let chain = Hierarchy::from_source("A = B\nB = C\nclass C; end\n");
        assert_eq!(chain.canonical("A"), "C");

        let cycle = Hierarchy::from_source("A = B\nB = A\n");
        // Whichever end it stops at, it stops.
        assert!(matches!(cycle.canonical("A"), "A" | "B"));
    }

    /// Only a bare constant is an alias. `LIMIT = 5` names no class, and
    /// `Klass = Class.new` names one rwr cannot follow -- both are left alone
    /// rather than guessed at.
    #[test]
    fn only_a_constant_valued_assignment_is_an_alias() {
        let h = Hierarchy::from_source("LIMIT = 5\nKlass = Class.new\nSelf = Self\n");
        assert_eq!(h.canonical("LIMIT"), "LIMIT");
        assert_eq!(h.canonical("Klass"), "Klass");
        // A self-assignment is not a chain to follow.
        assert_eq!(h.canonical("Self"), "Self");
    }

    /// `extend Other` is an ordinary mixin, not a self-extension, and must not
    /// collapse the two method tables of the extending module.
    #[test]
    fn extending_another_module_is_not_extending_itself() {
        let h = Hierarchy::from_source("module Util\n  extend Other\n  def foo; end\nend");
        assert!(!h.extends_itself("Util"));
        let plain = Hierarchy::from_source("module Util\n  def foo; end\nend");
        assert!(!plain.extends_itself("Util"));
    }

    #[test]
    fn a_class_descends_from_itself() {
        let h = Hierarchy::from_source("class A; end");
        assert!(h.descends_from("A", "A"));
    }

    #[test]
    fn descent_is_transitive() {
        let h = Hierarchy::from_source("class A; end\nclass B < A; end\nclass C < B; end");
        assert!(h.descends_from("C", "A"));
        assert!(h.descends_from("B", "A"));
        assert!(!h.descends_from("A", "C"));
    }

    /// A namespaced superclass resolves by its final name, since that is what
    /// a `type:` constraint names.
    #[test]
    fn namespaced_superclasses_resolve_by_name() {
        let h = Hierarchy::from_source("class Premium < Billing::Account; end");
        assert!(h.descends_from("Premium", "Account"));
    }

    /// A file that mixes a module in without writing `class X < Y` is still
    /// scanned for the edge.
    ///
    /// `reachable_from` prefilters candidates before parsing, and the filter
    /// required the *inheritance* shape -- so `Account.prepend(Audit)` in a file
    /// that never writes `class` was dropped before Prism saw it, and every
    /// `prepend`ed and `refine`d override went unreported. The testbed's own
    /// `prepend` case scored anyway, because a prose comment in it contains the
    /// words `class` and `<`; the corpus was green for a reason nobody intended
    /// and one comment edit away from silently losing two reaches.
    ///
    /// Pinned through `reachable_from` rather than `from_source`, since
    /// `from_source` parses unconditionally and never exercises the filter.
    #[test]
    fn a_mixin_needs_no_inheritance_line_to_be_found() {
        for patch in [
            "module Audit; end\nAccount.prepend(Audit)\n",
            "module Audit; end\nAccount.include(Audit)\n",
            "module Refined\n  refine Account do\n  end\nend\n",
        ] {
            assert!(
                !patch.contains("class") && !patch.contains('<'),
                "the fixture must not smuggle in the inheritance shape: {patch}"
            );
            let sources = vec![crate::source::Source::Owned(patch.as_bytes().to_vec())];
            let (h, parsed) = Hierarchy::reachable_from(&sources, &["Account".to_string()]);
            assert_eq!(parsed, 1, "the file must be parsed: {patch}");
            let module = if patch.contains("Refined") {
                "Refined"
            } else {
                "Audit"
            };
            assert!(
                h.contributes_to(module, "Account"),
                "{module} contributes to Account: {patch}"
            );
        }
    }

    /// Valid Ruby cannot express a cycle, but a half-written file can, and the
    /// walk must not hang on one.
    #[test]
    fn a_cycle_terminates() {
        let h = Hierarchy::from_source("class A < B; end\nclass B < A; end");
        assert!(!h.descends_from("A", "Nowhere"));
    }
}
