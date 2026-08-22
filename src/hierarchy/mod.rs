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
}

/// The name a constant-ish node denotes, ignoring how it was reached.
fn constant_name(node: &Node<'_>) -> Option<String> {
    let bytes = match node {
        Node::ConstantReadNode { .. } => node.as_constant_read_node()?.name().as_slice().to_vec(),
        Node::ConstantPathNode { .. } => node.as_constant_path_node()?.name()?.as_slice().to_vec(),
        _ => return None,
    };
    String::from_utf8(bytes).ok()
}

/// The calls that attach one module's methods to another class.
const MIXINS: [&[u8]; 4] = [b"include", b"prepend", b"extend", b"refine"];

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

/// Collect `class X < Y` pairs and the modules each class mixes in.
fn links(root: &Node<'_>, out: &mut Vec<(String, String)>, mixins: &mut Vec<(String, String)>) {
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
        let class = memchr::memmem::Finder::new(b"class").into_owned();
        let inherits = memchr::memmem::Finder::new(b"<").into_owned();
        // A mixin edge needs no `class X < Y` line: `Account.prepend(Audit)` in
        // a file that never writes `class` is the ordinary way to patch a model
        // you do not own. Admitting only the inheritance shape dropped every
        // such file before it was ever parsed -- and the testbed still scored
        // its `prepend` case, because a prose comment in it happens to contain
        // the words `class` and `<`. Delete that comment and recall fell by two
        // with nothing said.
        let mixes: Vec<_> = MIXINS
            .iter()
            .map(|k| memchr::memmem::Finder::new(k).into_owned())
            .collect();

        // Sources are read once by the caller and shared with the scan. Reading
        // them here as well made the two phases each pay full I/O, which was
        // most of the run -- parsing 72 files instead of 8,700 changed nothing
        // until the reads stopped repeating.
        let candidates: Vec<&[u8]> = sources
            .par_iter()
            .map(crate::source::Source::bytes)
            .filter(|src| {
                (class.find(src).is_some() && inherits.find(src).is_some())
                    || mixes.iter().any(|f| f.find(src).is_some())
            })
            .collect();

        let mut known: HashSet<String> = roots.iter().cloned().collect();
        let mut superclass: HashMap<String, String> = HashMap::new();
        let mut mixins: HashMap<String, Vec<String>> = HashMap::new();
        let mut done = vec![false; candidates.len()];
        let mut parsed_total = 0usize;

        loop {
            let finders: Vec<memchr::memmem::Finder<'static>> = known
                .iter()
                .map(|n| memchr::memmem::Finder::new(n.as_bytes()).into_owned())
                .collect();

            type Round = (usize, Vec<(String, String)>, Vec<(String, String)>);
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
                    let (mut found, mut mixed) = (Vec::new(), Vec::new());
                    links(&parsed.node(), &mut found, &mut mixed);
                    Some((i, found, mixed))
                })
                .collect();

            parsed_total += round.len();
            let mut grew = false;
            for (i, found, mixed) in round {
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
            }
            if !grew {
                break;
            }
        }

        (Hierarchy { superclass, mixins }, parsed_total)
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

    /// Whether `class` is `ancestor` or descends from it.
    ///
    /// Guards against a cycle, which valid Ruby cannot express but a
    /// half-written file can.
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
        links(&parsed.node(), &mut found, &mut mixed);
        let mut mixins: HashMap<String, Vec<String>> = HashMap::new();
        for (class, module) in mixed {
            mixins.entry(class).or_default().push(module);
        }
        Hierarchy {
            superclass: found.into_iter().collect(),
            mixins,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
