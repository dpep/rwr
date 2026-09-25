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
//!
//! Every class in here is named by its **qualified** name, the way the matcher's
//! scope stack names it, and every written constant is resolved to one of those
//! before anything is compared (D100).

use crate::pattern::generated;
use crate::pattern::matcher::{enclosing_class, scope_name_of};
use rayon::prelude::*;
use ruby_prism::Node;
use std::collections::{HashMap, HashSet};

/// The name a constant-ish node denotes, qualified -- `Billing::Account`, not
/// `Account`.
///
/// The matcher's own, rather than a copy: a class has to be spelled the same way
/// on both sides or the two are talking about different classes (D100).
pub(crate) use crate::pattern::matcher::qualified as constant_name;

/// Superclass links, keyed by qualified class name.
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
    /// The subset of `mixins` that lands on the host's *instance* table --
    /// `include` and `prepend`, not `extend`.
    ///
    /// Kept apart for the reason `refines` is: lumping the spellings together is
    /// right for a report, which only asks where a method might be written, and
    /// wrong for a rewrite. `extend M` puts M's `def foo` on the host's
    /// *singleton* table, so a rename of `Host#foo` must not move it while a
    /// rename of `Host.foo` must.
    included: HashMap<String, Vec<String>>,
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
    /// Every qualified constant this run saw name a class, indexed by last
    /// segment -- what an unqualified name is resolved against.
    ///
    /// References as well as declarations: `class Premium < Billing::Account`
    /// names a class whether or not the file declaring it was parsed, and
    /// dropping it would make `Account` unable to reach `Premium`.
    by_segment: HashMap<String, Vec<String>>,
}

/// The calls that attach one module's methods to another class.
const MIXINS: [&[u8]; 4] = [b"include", b"prepend", b"extend", b"refine"];

/// A constant as written, and the class body it was written in.
///
/// Ruby resolves a constant lexically, so `Account` inside `module Billing`
/// means `Billing::Account` when that exists. A reference cannot be resolved
/// where it is collected -- the declaration may be in a file not parsed yet --
/// so it is carried whole and resolved once the run has seen everything.
#[derive(Debug, Clone)]
struct Ref {
    written: String,
    enclosing: Option<String>,
}

/// What one file contributes, before any name is resolved.
#[derive(Debug, Default)]
struct Collected {
    /// Qualified names of the classes and modules the file declares.
    declared: Vec<String>,
    superclass: Vec<(String, Ref)>,
    mixins: Vec<(Ref, Ref)>,
    included: Vec<(Ref, Ref)>,
    refines: Vec<(Ref, Ref)>,
    self_extended: Vec<String>,
    aliases: Vec<(String, Ref)>,
}

impl Collected {
    fn absorb(&mut self, other: Collected) {
        self.declared.extend(other.declared);
        self.superclass.extend(other.superclass);
        self.mixins.extend(other.mixins);
        self.included.extend(other.included);
        self.refines.extend(other.refines);
        self.self_extended.extend(other.self_extended);
        self.aliases.extend(other.aliases);
    }

    /// Every constant name this file saw, declared or referred to.
    fn constants(&self) -> impl Iterator<Item = &str> {
        self.declared
            .iter()
            .chain(self.self_extended.iter())
            .map(String::as_str)
            .chain(
                self.superclass
                    .iter()
                    .flat_map(|(c, r)| [c.as_str(), r.written.as_str()]),
            )
            .chain(
                self.aliases
                    .iter()
                    .flat_map(|(c, r)| [c.as_str(), r.written.as_str()]),
            )
            .chain(
                self.mixins
                    .iter()
                    .chain(self.refines.iter())
                    .flat_map(|(h, m)| [h.written.as_str(), m.written.as_str()]),
            )
    }
}

/// The last segment of a qualified name -- what an unqualified one has to agree
/// with before it can mean this class.
fn last_segment(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

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

/// Collect what a file says about the classes in it.
///
/// The walk carries the matcher's own scope stack and names every class through
/// `enclosing_class`, so a class is spelled here exactly as a match inside it is
/// spelled there. Reimplementing the spelling is what produced D100's bug.
fn links(root: &Node<'_>) -> Collected {
    let mut out = Collected::default();
    let mut stack = vec![(generated::dup(root), Vec::<String>::new())];
    while let Some((node, scope)) = stack.pop() {
        let enclosing = enclosing_class(&scope);
        let mut inner = scope.clone();
        if let Some(entry) = scope_name_of(&node) {
            inner.push(entry);
        }
        // Modules too: a module can `include` another, and a refinement's body
        // belongs to the module that wrote it. Tracking classes alone left
        // anything written inside a module with nothing to attribute it to.
        let declares = matches!(node, Node::ClassNode { .. } | Node::ModuleNode { .. });
        let here = if declares {
            enclosing_class(&inner)
        } else {
            enclosing.clone()
        };
        if declares && let Some(name) = &here {
            out.declared.push(name.clone());
            if let Some(class) = node.as_class_node()
                && let Some(parent) = class.superclass()
                && let Some(written) = constant_name(&parent)
            {
                out.superclass.push((
                    name.clone(),
                    Ref {
                        written,
                        // The superclass expression is read before the class
                        // body opens, so it resolves against what is outside.
                        enclosing: enclosing.clone(),
                    },
                ));
            }
        }
        if let Some((name, target)) = constant_alias(&node) {
            let alias = match &enclosing {
                Some(outer) => format!("{outer}::{name}"),
                None => name,
            };
            out.aliases.push((
                alias,
                Ref {
                    written: target,
                    enclosing: enclosing.clone(),
                },
            ));
        }
        if extends_itself(&node)
            && let Some(host) = here.clone()
        {
            out.self_extended.push(host);
        }
        let modules = mixed_in(&node);
        if !modules.is_empty()
            && let Some(host) = mixin_host(&node, enclosing.as_ref())
        {
            let host = Ref {
                written: host,
                enclosing: enclosing.clone(),
            };
            let refines = node
                .as_call_node()
                .is_some_and(|c| c.name().as_slice() == b"refine");
            if refines {
                // The refinement's body belongs to the enclosing module, so that
                // is what contributes to the host.
                if let Some(module) = &enclosing {
                    let module = Ref {
                        written: module.clone(),
                        enclosing: None,
                    };
                    out.refines.push((host.clone(), module.clone()));
                    out.mixins.push((host, module));
                }
            } else {
                // `extend M` reaches the host's singleton table and the other
                // two reach its instance table, so which spelling it was decides
                // whether a `#` rename may move a `def` written in M.
                let instance_side = node
                    .as_call_node()
                    .is_some_and(|c| matches!(c.name().as_slice(), b"include" | b"prepend"));
                for module in modules {
                    let module = Ref {
                        written: module,
                        enclosing: enclosing.clone(),
                    };
                    if instance_side {
                        out.included.push((host.clone(), module.clone()));
                    }
                    out.mixins.push((host.clone(), module));
                }
            }
        }
        for child in generated::children(&node) {
            stack.push((child, inner.clone()));
        }
    }
    out
}

/// Index every known constant by its last segment, so an unqualified name can
/// be resolved without scanning them all.
fn index(constants: HashSet<String>) -> HashMap<String, Vec<String>> {
    let mut by_segment: HashMap<String, Vec<String>> = HashMap::new();
    for name in constants {
        by_segment
            .entry(last_segment(&name).to_string())
            .or_default()
            .push(name);
    }
    by_segment
}

impl Hierarchy {
    /// The class a written constant names, as far as this run can tell.
    ///
    /// Ruby's lexical lookup first -- innermost enclosing module outwards, then
    /// the top level. A name that matches nothing seen widens to the single
    /// class whose qualified name ends with it, and stays as written when
    /// several do: rwr has no way to choose between `Billing::Account` and
    /// `Sales::Account`, and choosing is the failure this design exists to
    /// prevent (D100).
    fn resolve(&self, r: &Ref) -> String {
        let candidates = |name: &str| self.by_segment.get(last_segment(name));
        // `Billing::Account` inside `module Deep` may be `Deep::Billing::Account`.
        let mut outer = r.enclosing.as_deref();
        while let Some(scope) = outer {
            let nested = format!("{scope}::{}", r.written);
            if candidates(&nested).is_some_and(|c| c.contains(&nested)) {
                return nested;
            }
            outer = scope.rfind("::").map(|i| &scope[..i]);
        }
        // Only an *unqualified* name widens. A written namespace is the caller
        // saying which of the namesakes it means, and it is the documented
        // remedy for an ambiguous short name -- so widening it retargets the
        // one caller who was explicit, and does it whenever the run cannot see
        // the class named (a path scope, a file that did not parse).
        if r.written.contains("::") {
            return r.written.clone();
        }
        match candidates(&r.written) {
            Some(names) if names.iter().any(|n| n == &r.written) => r.written.clone(),
            // Exactly one class ends with it, so that is what it means.
            Some(names) if names.len() == 1 => names[0].clone(),
            _ => r.written.clone(),
        }
    }

    /// The class a constant written in source names, read where it was written.
    ///
    /// The call side's half of D100's lexical rule. A superclass and a mixin
    /// already go through `resolve`; a receiver did not, so `Account.new`
    /// inside `module Billing` meant the top-level namesake there and
    /// `Billing::Account` everywhere else in the same run.
    pub(crate) fn written_at(&self, name: &str, enclosing: Option<&str>) -> String {
        self.resolve(&Ref {
            written: name.to_string(),
            enclosing: enclosing.map(str::to_string),
        })
    }

    /// The class a bare name means, followed through any constant aliases.
    ///
    /// `Alias = Account` makes `Alias` a second name for one class, so every
    /// question about it -- descent, method tables, a `type:` constraint -- is a
    /// question about `Account`. Chains resolve (`A = B; B = C`), and a cycle
    /// stops rather than spins: `A = B; B = A` is degenerate Ruby and must not
    /// hang a linter.
    pub(crate) fn canonical(&self, name: &str) -> String {
        let mut current = self.resolve(&Ref {
            written: name.to_string(),
            enclosing: None,
        });
        for _ in 0..self.aliases.len() + 1 {
            match self.aliases.get(&current) {
                Some(target) if *target != current => current = target.clone(),
                _ => break,
            }
        }
        current
    }

    /// Whether two names mean one class.
    pub(crate) fn same_class(&self, one: &str, other: &str) -> bool {
        one == other || self.canonical(one) == self.canonical(other)
    }

    /// Build only the part of the hierarchy reachable from `roots`.
    ///
    /// A rename names one class, and only what contributes to it matters -- so
    /// rather than parsing every file that declares any superclass, parse only
    /// those mentioning a class already known to be in the tree, and iterate to
    /// a fixpoint. `Gold < Premium < Account` is reached in two rounds: the
    /// first finds Premium, which puts "Premium" into the search set for the
    /// second.
    ///
    /// Mixins grow the set the same way, and used not to. A module is known once
    /// something known mixes it in, which then reaches every *other* class that
    /// mixes in the same module -- a file cannot include a module without naming
    /// it. Without that link a concern's other includers were never parsed, and
    /// the answer to "does any unrelated class share this module" was a confident
    /// no from a walk that had not looked: on mastodon, 1 of the 59 classes that
    /// include `Authorization`.
    ///
    /// The search set holds *last segments*, because that is what the bytes of a
    /// file carry: `Billing::Account` is reached by a file writing `Account`
    /// inside `module Billing`, and a finder for the qualified name misses the
    /// very file that declares it. Over-admitting a candidate costs a parse;
    /// missing one is silent.
    ///
    /// Measured on rails against the worst case, `ActiveRecord::Base#save`:
    /// 3,215 of 3,321 files parsed, 138ms of a 442ms run (minimum of seven).
    /// Requiring *every* segment of a known name instead narrows that to 2,441
    /// files and is **slower** at 251ms -- last segments collapse a whole
    /// subclass tree onto one finder, where per-name segment sets do not.
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

        let mut known: HashSet<String> =
            roots.iter().map(|r| last_segment(r).to_string()).collect();
        let mut all = Collected::default();
        let mut constants: HashSet<String> = HashSet::new();
        let mut done = vec![false; candidates.len()];
        let mut parsed_total = 0usize;

        loop {
            let finders: Vec<memchr::memmem::Finder<'static>> = known
                .iter()
                .map(|n| memchr::memmem::Finder::new(n.as_bytes()).into_owned())
                .collect();

            let round: Vec<(usize, Collected)> = candidates
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
                    Some((i, links(&parsed.node())))
                })
                .collect();

            parsed_total += round.len();
            let mut grew = false;
            for (i, found) in round {
                done[i] = true;
                constants.extend(found.constants().map(str::to_string));
                for (child, parent) in &found.superclass {
                    if known.contains(last_segment(&parent.written))
                        && known.insert(last_segment(child).to_string())
                    {
                        grew = true;
                    }
                }
                // A mixed-in module is known once its host is, for the reason a
                // subclass is known once its parent is: a file that mixes a
                // module in has to *name* it, so naming the module reaches every
                // one of them.
                //
                // Only superclass links grew the search set before, so a module
                // name never entered it and a file whose only interesting line
                // was `include Authorization` was never parsed. The docstring's
                // claim that the walk is exact held for inheritance and not for
                // mixins: on mastodon the hierarchy found 1 of the 59 classes
                // that include `Authorization`, so any question about who *else*
                // has a concern was answered on the strength of not having
                // looked -- silently, and in the unsafe direction.
                for (host, module) in &found.mixins {
                    if known.contains(last_segment(&host.written))
                        && known.insert(last_segment(&module.written).to_string())
                    {
                        grew = true;
                    }
                }
                all.absorb(found);
            }
            if !grew {
                break;
            }
        }

        (Hierarchy::from_collected(all, constants), parsed_total)
    }

    /// Turn written references into class names, once every file is in.
    fn from_collected(all: Collected, constants: HashSet<String>) -> Self {
        let mut h = Hierarchy {
            by_segment: index(constants),
            ..Hierarchy::default()
        };
        // Aliases first: everything else resolves through them.
        h.aliases = all
            .aliases
            .iter()
            .map(|(name, target)| (h.resolve_name(name), h.resolve(target)))
            .filter(|(name, target)| name != target)
            .collect();
        for (child, parent) in &all.superclass {
            h.superclass
                .insert(h.resolve_name(child), h.resolve(parent));
        }
        for (host, module) in &all.mixins {
            let (host, module) = (h.resolve(host), h.resolve(module));
            h.mixins.entry(host).or_default().push(module);
        }
        for (host, module) in &all.included {
            let (host, module) = (h.resolve(host), h.resolve(module));
            h.included.entry(host).or_default().push(module);
        }
        for (host, module) in &all.refines {
            let (host, module) = (h.resolve(host), h.resolve(module));
            h.refines.entry(host).or_default().push(module);
        }
        h.self_extended = all
            .self_extended
            .iter()
            .map(|n| h.resolve_name(n))
            .collect();
        h
    }

    /// A name already qualified where it was collected.
    fn resolve_name(&self, name: &str) -> String {
        self.resolve(&Ref {
            written: name.to_string(),
            enclosing: None,
        })
    }

    /// Whether `module` is mixed into `class` or into any of its descendants.
    ///
    /// The question a report asks about a concern: this occurrence sits in a
    /// module, so is that module part of the class the rule is about? Without
    /// it, everything a concern contributes -- and in Rails that is a large
    /// share of a model -- is dropped from the account with nothing said.
    pub(crate) fn contributes_to(&self, module: &str, class: &str) -> bool {
        let module = self.canonical(module);
        self.mixins
            .iter()
            .any(|(host, modules)| modules.contains(&module) && self.descends_from(host, class))
    }

    /// Whether a rename anchored on `class` may move an instance-method
    /// definition written in `module`.
    ///
    /// Two conditions, and the second is the one doing the work. The module has
    /// to reach `class` on the *instance* side, so `extend` is out. And every
    /// class rwr saw mix it in has to be `class` or a descendant: a concern
    /// shared with an unrelated class defines that class's method too, so moving
    /// the definition answers a wider question than the designator asked -- and
    /// the sibling's call sites sit outside the receiver narrowing, so they would
    /// not move with it. Declining leaves the definition as residue, which is an
    /// account the caller can act on; moving it would leave a break that nothing
    /// reports.
    ///
    /// A module included into another module and only then into `class` is
    /// declined as well: the chain is not followed, so the honest answer is the
    /// conservative one.
    pub(crate) fn may_rename_into(&self, module: &str, class: &str) -> bool {
        let module = self.canonical(module);
        let mut hosts = self
            .included
            .iter()
            .filter(|(_, modules)| modules.contains(&module))
            .map(|(host, _)| host)
            .peekable();
        hosts.peek().is_some() && hosts.all(|host| self.descends_from(host, class))
    }

    /// The modules that refine `class`.
    ///
    /// A refinement only applies in a file that says `using`, so a call site in
    /// such a file may be dispatching to the refinement rather than the class --
    /// and renaming it there silently routes around the refinement.
    pub(crate) fn refined_by(&self, class: &str) -> &[String] {
        self.refines
            .get(&self.canonical(class))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether this module's instance methods answer on the module itself, so
    /// that `Util.foo` and `Util#foo` name one method rather than two.
    pub(crate) fn extends_itself(&self, class: &str) -> bool {
        self.self_extended.contains(&self.canonical(class))
    }

    /// Whether `class` is `ancestor` or descends from it.
    ///
    /// Guards against a cycle, which valid Ruby cannot express but a
    /// half-written file can.
    pub(crate) fn descends_from(&self, class: &str, ancestor: &str) -> bool {
        let ancestor = self.canonical(ancestor);
        let mut current = self.canonical(class);
        let mut seen = HashSet::new();
        loop {
            if current == ancestor {
                return true;
            }
            if !seen.insert(current.clone()) {
                return false;
            }
            match self.superclass.get(&current) {
                Some(parent) => current = self.canonical(parent),
                None => return false,
            }
        }
    }

    /// Build from a single snippet. Test-only, and shared across modules so a
    /// matcher test can exercise real descent rather than an empty hierarchy.
    #[cfg(test)]
    pub(crate) fn from_source(source: &str) -> Self {
        let parsed = ruby_prism::parse(source.as_bytes());
        let found = links(&parsed.node());
        let constants = found.constants().map(str::to_string).collect();
        Hierarchy::from_collected(found, constants)
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
        assert!(matches!(cycle.canonical("A").as_str(), "A" | "B"));
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

    /// An unqualified name means the one class that answers to it, and the run
    /// only knows the classes it has seen -- declared or named as a superclass.
    #[test]
    fn an_unqualified_name_reaches_the_only_class_that_ends_with_it() {
        let h = Hierarchy::from_source("class Premium < Billing::Account; end");
        assert!(h.descends_from("Premium", "Account"));
        assert!(h.descends_from("Premium", "Billing::Account"));

        let declared = Hierarchy::from_source(
            "module Billing\n  class Account; end\nend\nclass Premium < Billing::Account; end",
        );
        assert!(declared.descends_from("Premium", "Account"));
    }

    /// And two classes sharing a last segment are two classes. Renaming
    /// `Account#foo` reached `class Premium < Billing::Account` before this,
    /// because the hierarchy kept only the last segment of every name.
    #[test]
    fn a_top_level_class_does_not_absorb_its_namespaced_namesake() {
        let h = Hierarchy::from_source(
            "class Account; end\nmodule Billing\n  class Account; end\nend\n\
             class Premium < Billing::Account; end",
        );
        assert!(h.descends_from("Premium", "Billing::Account"));
        assert!(
            !h.descends_from("Premium", "Account"),
            "Billing::Account is not the top-level Account"
        );
        assert!(!h.same_class("Account", "Billing::Account"));
    }

    /// A *qualified* name says which namespace it means, so it never widens.
    ///
    /// The widening rule is stated for an unqualified name only (D100), but the
    /// code applied it to any name whose last segment had one candidate --
    /// so `Sales::Account`, on a run that had seen only `Billing::Account`,
    /// answered about Billing. Qualifying the name is the documented remedy for
    /// an ambiguous short one, and it retargeted the rename instead.
    #[test]
    fn a_qualified_name_never_widens_to_a_sibling_namespace() {
        let h = Hierarchy::from_source("module Billing\n  class Account; end\nend");
        assert_eq!(h.canonical("Sales::Account"), "Sales::Account");
        assert!(!h.same_class("Sales::Account", "Billing::Account"));
        // The unqualified form still widens -- that is D100's rule, unchanged.
        assert_eq!(h.canonical("Account"), "Billing::Account");
    }

    /// A reference written inside a module resolves the way Ruby resolves it:
    /// the sibling in the same namespace wins over the top-level namesake.
    #[test]
    fn a_reference_resolves_against_its_enclosing_module_first() {
        let h = Hierarchy::from_source(
            "class Base; end\nmodule Billing\n  class Base; end\n  class Account < Base; end\nend",
        );
        assert!(h.descends_from("Billing::Account", "Billing::Base"));
        assert!(!h.descends_from("Billing::Account", "Base"));
    }

    /// An alias names the class it points at, namespace and all. Keeping only
    /// the last segment made `Alias = Billing::Account` a second name for the
    /// *top-level* Account, which a rename of that class then rewrote.
    #[test]
    fn a_constant_alias_keeps_the_namespace_of_what_it_names() {
        let h = Hierarchy::from_source(
            "class Account; end\nmodule Billing\n  class Account; end\nend\nAlias = Billing::Account\n",
        );
        assert_eq!(h.canonical("Alias"), "Billing::Account");
        assert!(!h.same_class("Alias", "Account"));
    }

    /// A namespaced concern is not the top-level module of the same name.
    #[test]
    fn a_namespaced_mixin_is_not_its_top_level_namesake() {
        let h = Hierarchy::from_source(
            "module Helpers; end\nmodule Billing\n  module Helpers; end\nend\n\
             class Account\n  include Billing::Helpers\nend",
        );
        assert!(h.contributes_to("Billing::Helpers", "Account"));
        assert!(!h.contributes_to("Helpers", "Account"));
    }

    /// A constant is read where it was written -- on the call side too.
    ///
    /// D100's lexical rule was applied to superclasses and mixins and not to a
    /// receiver, so `Account.new` inside `module Billing` meant the top-level
    /// namesake there and `Billing::Account` everywhere else in the same run.
    /// Each case here is one widening cannot rescue: a top-level namesake
    /// exists, so a literal reading gives a *different* answer rather than none.
    #[test]
    fn a_written_constant_is_read_where_it_was_written() {
        let h = Hierarchy::from_source(
            "class Account; end\nmodule Helpers\n  module Numeric; end\nend\n\
             module Billing\n  class Account; end\n  module Helpers\n    module Numeric; end\n  \
             end\nend",
        );
        assert_eq!(
            h.written_at("Account", Some("Billing::Invoice")),
            "Billing::Account"
        );
        assert_eq!(h.written_at("Account", None), "Account");
        // A relative path is a written constant too.
        assert_eq!(
            h.written_at("Helpers::Numeric", Some("Billing::Invoice")),
            "Billing::Helpers::Numeric"
        );
        // And a lexical position never makes a qualified name widen: the
        // namespace written is the caller saying which namesake it meant.
        assert_eq!(
            h.written_at("Sales::Account", Some("Billing")),
            "Sales::Account"
        );
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

    /// A namespaced class is reached from a file that only ever writes its short
    /// name, so the search set holds last segments rather than qualified names.
    #[test]
    fn the_search_set_reaches_a_class_written_unqualified() {
        let sources = vec![crate::source::Source::Owned(
            b"module Billing\n  class Account; end\n  class Premium < Account; end\nend\n".to_vec(),
        )];
        let (h, _) = Hierarchy::reachable_from(&sources, &["Billing::Account".to_string()]);
        assert!(h.descends_from("Billing::Premium", "Billing::Account"));
    }

    /// A rename may move a definition written in a module the class includes --
    /// but only where no unrelated class shares the module.
    #[test]
    fn a_rename_reaches_an_exclusively_included_module() {
        let h = Hierarchy::from_source(
            "module Naming\n  def display_name; end\nend\nclass Account\n  include Naming\nend\n",
        );
        assert!(h.may_rename_into("Naming", "Account"));

        // A subclass of the anchor is not an outsider: the method it gets from
        // the module is the anchor's method.
        let with_subclass = Hierarchy::from_source(
            "module Naming; end\nclass Account\n  include Naming\nend\n\
             class Premium < Account\n  include Naming\nend\n",
        );
        assert!(with_subclass.may_rename_into("Naming", "Account"));

        // An unrelated class sharing the module is: moving the definition would
        // rename its method too, and its call sites sit outside the receiver
        // narrowing, so they would not move with it.
        let shared = Hierarchy::from_source(
            "module Naming; end\nclass Account\n  include Naming\nend\n\
             class Invoice\n  include Naming\nend\n",
        );
        assert!(!shared.may_rename_into("Naming", "Account"));

        // `extend` puts the module's instance methods on the *singleton* table,
        // so a `#` rename must not claim them.
        let extended =
            Hierarchy::from_source("module Naming; end\nclass Account\n  extend Naming\nend\n");
        assert!(!extended.may_rename_into("Naming", "Account"));

        // A module nobody was seen to mix in says nothing about the class.
        let orphan = Hierarchy::from_source("module Naming\n  def display_name; end\nend\n");
        assert!(!orphan.may_rename_into("Naming", "Account"));
    }

    /// The premise the sharing check rests on: the walk has to *find* the other
    /// includers, and it only ever grew its search set through superclass links.
    ///
    /// Measured on mastodon before this: 1 of the 59 classes that include
    /// `Authorization` was parsed, so "no unrelated class shares this module"
    /// was answered on the strength of not having looked. The file that includes
    /// a module has to name it, so naming the module reaches all of them.
    #[test]
    fn the_search_set_reaches_a_module_a_distant_class_includes() {
        let sources = vec![
            crate::source::Source::Owned(b"class Account\n  include Naming\nend\n".to_vec()),
            crate::source::Source::Owned(b"module Naming\n  def display_name; end\nend\n".to_vec()),
            // Names neither Account nor any subclass of it -- reachable only
            // through the module.
            crate::source::Source::Owned(b"class Invoice\n  include Naming\nend\n".to_vec()),
        ];
        let (h, _) = Hierarchy::reachable_from(&sources, &["Account".to_string()]);
        assert!(
            !h.may_rename_into("Naming", "Account"),
            "the distant includer has to be found, or the rename silently widens"
        );
    }

    /// Valid Ruby cannot express a cycle, but a half-written file can, and the
    /// walk must not hang on one.
    #[test]
    fn a_cycle_terminates() {
        let h = Hierarchy::from_source("class A < B; end\nclass B < A; end");
        assert!(!h.descends_from("A", "Nowhere"));
    }
}
