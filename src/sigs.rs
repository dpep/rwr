//! Return types read from Sorbet signatures.
//!
//! Chained receivers need to know what a method returns, and D61 measured that
//! syntax alone answers that for only 2-4% of definitions -- 70% of methods end
//! in another call. A `sig` block states the answer outright.
//!
//! **This needs no Sorbet and no RBI parser.** A signature is ordinary Ruby --
//! `sig { returns(String) }` is a method call with a block -- so it is already
//! in the tree rwr parses. The cost when a repository has no signatures is one
//! substring search that finds nothing.
//!
//! Deliberately partial: a type rwr cannot turn into a class name yields
//! nothing rather than a guess, so `T.untyped`, `T.any(...)` and `void` simply
//! do not appear in the index. Narrowing may only ever narrow.

use crate::hierarchy::Hierarchy;
use crate::pattern::generated;
use crate::pattern::matcher::{Receiver, enclosing_class, scope_name_of};
use crate::source::Source;
use rayon::prelude::*;
use ruby_prism::Node;
use std::collections::HashMap;

/// Which method of which class it is: `(class, method, singleton)`.
type Key = (String, String, bool);

/// Which method of which class, what it returns, and what it takes.
type Signed = (Key, Option<Written>, Vec<(String, Written)>);

/// A type a signature named, as it was spelled and where it was spelled.
///
/// Resolved at lookup rather than here, because which class a constant names
/// depends on the lexical position it is written at, and that is the
/// hierarchy's question (D100). Reading `Helpers::Thing` as `Thing` -- or a
/// bare `Thing` inside `module App` as the top-level one -- claimed a
/// namesake's method and rewrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Written {
    /// The constant as the signature spelled it, with its kind.
    receiver: Receiver,
    /// The class or module the signature sits in, fully qualified.
    enclosing: Option<String>,
}

impl Written {
    /// The class the signature named, read the way Ruby reads it there.
    fn at(&self, hierarchy: &Hierarchy) -> Receiver {
        let name = hierarchy.written_at(self.receiver.class_name(), self.enclosing.as_deref());
        match self.receiver {
            Receiver::Instance(_) => Receiver::Instance(name),
            Receiver::Class(_) => Receiver::Class(name),
        }
    }
}

/// What each signed method returns and accepts, keyed by the class defining it.
#[derive(Debug, Default)]
pub(crate) struct Signatures {
    /// `(class, method, singleton) -> return type`.
    returns: HashMap<Key, Written>,
    /// `(class, method, singleton) -> parameter name -> its type`.
    ///
    /// Parameters are the half that makes an ordinary guard resolvable. A return
    /// type answers "what does this chain evaluate to"; a parameter type answers
    /// "what is this bare local", which is what most code actually asks about --
    /// `return if x.nil?` guards an argument far more often than a chain.
    params: HashMap<Key, HashMap<String, Written>>,
}

impl Signatures {
    /// Whether nothing at all was read. Distinct from `len()`, which counts
    /// methods: a repository whose signatures are all `params(..).void` has no
    /// return types and is emphatically not empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.returns.is_empty() && self.params.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        // Methods known, not facts stored: one method with both a return type
        // and parameter types is one signature, and counting it twice would
        // overstate what was read.
        self.returns
            .keys()
            .chain(self.params.keys())
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    /// The types a signature gave `class#method`'s parameters, if it gave any.
    pub(crate) fn params(
        &self,
        hierarchy: &Hierarchy,
        class: &str,
        method: &str,
        singleton: bool,
    ) -> Option<Vec<(String, Receiver)>> {
        self.params
            .get(&(class.to_string(), method.to_string(), singleton))
            .map(|found| {
                found
                    .iter()
                    .map(|(name, written)| (name.clone(), written.at(hierarchy)))
                    .collect()
            })
    }

    /// What `class#method` (or `class.method`) returns, if a signature said.
    pub(crate) fn returns(
        &self,
        hierarchy: &Hierarchy,
        class: &str,
        method: &str,
        singleton: bool,
    ) -> Option<Receiver> {
        self.returns
            .get(&(class.to_string(), method.to_string(), singleton))
            .map(|written| written.at(hierarchy))
    }

    /// Read every signature in the corpus.
    ///
    /// Returns how many files were parsed alongside, since a repository with no
    /// signatures should cost nothing and the profile should show that.
    pub(crate) fn from_sources(sources: &[Source]) -> (Self, usize) {
        // `sig ` rather than `sig`: the bare word occurs inside "design",
        // "assign" and "signature", and prefiltering on it parsed 1,584 files
        // of Discourse to find nothing. The trailing space covers both spellings
        // a signature can have -- `sig { ... }` and `sig do ... end` -- in one
        // pass, where a finder per spelling cost a scan of the corpus each.
        let opener = memchr::memmem::Finder::new(b"sig ").into_owned();
        // A `T::Struct` may declare typed fields with no `sig` block anywhere in
        // the file, so the opener alone would skip it. `T::` costs a second scan
        // and appears in no untyped codebase at all.
        let typed = memchr::memmem::Finder::new(b"T::").into_owned();

        let found: Vec<Vec<Signed>> = sources
            .par_iter()
            .filter_map(|source| {
                let bytes = source.bytes();
                if opener.find(bytes).is_none() && typed.find(bytes).is_none() {
                    return None;
                }
                let parsed = ruby_prism::parse(bytes);
                if parsed.errors().count() > 0 {
                    return None;
                }
                let mut out = Vec::new();
                collect(&parsed.node(), &[], false, false, &mut out);
                Some(out)
            })
            .collect();

        let parsed = found.len();
        let mut returns = HashMap::new();
        let mut params: HashMap<Key, HashMap<String, Written>> = HashMap::new();
        for (key, ret, args) in found.into_iter().flatten() {
            if let Some(ret) = ret {
                returns.insert(key.clone(), ret);
            }
            if !args.is_empty() {
                params.entry(key).or_default().extend(args);
            }
        }
        (Signatures { returns, params }, parsed)
    }
}

/// Walk a tree, recording each signature against the class it sits in.
fn collect(
    node: &Node<'_>,
    scope: &[String],
    singleton: bool,
    struct_body: bool,
    out: &mut Vec<Signed>,
) {
    // A signature and the definition it describes are *adjacent statements*, so
    // pairs are what has to be walked rather than nodes.
    if let Some(statements) = node.as_statements_node() {
        let body: Vec<Node<'_>> = statements.body().iter().collect();
        // The class these signatures are on, and the position their types are
        // written at -- one and the same place. Spelled by the matcher's own
        // functions, because the matcher is what looks this index up, and two
        // spellings of one class is the split D100 exists to close.
        let here = enclosing_class(scope);
        let written = |receiver: Receiver| Written {
            receiver,
            enclosing: here.clone(),
        };
        for pair in body.windows(2) {
            // Not gated on a usable return type: `sig { params(x: String).void }`
            // states nothing this index can use about the *result* and everything
            // about the argument, and gating on `returns` dropped every one of
            // them -- which is most signatures on a command or a setter.
            let Some((returns, params)) = signature_types(&pair[0]) else {
                continue;
            };
            if returns.is_none() && params.is_empty() {
                continue;
            }
            let Some(class) = &here else { continue };
            let returns = returns.clone().map(&written);
            let params: Vec<(String, Written)> = params
                .iter()
                .map(|(name, receiver)| (name.clone(), written(receiver.clone())))
                .collect();
            for (name, on_class) in described_methods(&pair[1], singleton) {
                out.push((
                    (class.clone(), name, on_class),
                    returns.clone(),
                    params.clone(),
                ));
            }
        }
        // `T::Struct` declares typed readers without a `sig`, in a single call:
        // `const :name, String`. Measured at 45,068 sites on a Sorbet monolith
        // against its 148,052 `sig` blocks -- far too many to leave unread.
        if struct_body && let Some(class) = &here {
            for statement in &body {
                if let Some((name, returns)) = struct_field(statement) {
                    out.push((
                        (class.clone(), name, false),
                        Some(written(returns)),
                        Vec::new(),
                    ));
                }
            }
        }
    }

    let mut inner: Vec<String> = scope.to_vec();
    let mut inner_singleton = singleton;
    let mut inner_struct = struct_body;
    // The matcher's own naming, not a second implementation of it: `class
    // Account::Exporter` names `Account::Exporter`, and a `class << Foo` body
    // belongs to Foo however it is nested.
    if let Some(name) = scope_name_of(node) {
        inner.push(name);
    }
    match node {
        Node::ClassNode { .. } => {
            if let Some(class) = node.as_class_node() {
                inner_singleton = false;
                // Only a `T::Struct` and its kin declare fields this way, and
                // `const` is an ordinary enough word that reading it anywhere
                // else would invent types rather than narrow by them.
                inner_struct = class
                    .superclass()
                    .and_then(|s| s.as_constant_path_node())
                    .and_then(|path| path.parent())
                    .and_then(|parent| {
                        parent
                            .as_constant_read_node()
                            .map(|c| c.name().as_slice() == b"T")
                    })
                    .unwrap_or(false);
            }
        }
        // Everything inside `class << self` defines singleton methods.
        Node::SingletonClassNode { .. } => inner_singleton = true,
        _ => {}
    }

    for child in generated::children(node) {
        collect(&child, &inner, inner_singleton, inner_struct, out);
    }
}

/// The methods a statement following a signature defines.
///
/// A `sig` describes the next definition, and that is not always a `def`:
/// `attr_reader` is signed the same way and defines one method per symbol.
fn described_methods(node: &Node<'_>, singleton: bool) -> Vec<(String, bool)> {
    if let Some(def) = node.as_def_node() {
        let Ok(name) = String::from_utf8(def.name().as_slice().to_vec()) else {
            return Vec::new();
        };
        // `def self.x` is a singleton method wherever it appears.
        return vec![(name, singleton || def.receiver().is_some())];
    }
    let Some(call) = node.as_call_node() else {
        return Vec::new();
    };
    if !matches!(
        call.name().as_slice(),
        b"attr_reader" | b"attr_accessor" | b"attr_writer"
    ) {
        return Vec::new();
    }
    let Some(arguments) = call.arguments() else {
        return Vec::new();
    };
    arguments
        .arguments()
        .iter()
        .filter_map(|argument| {
            let symbol = argument.as_symbol_node()?;
            String::from_utf8(symbol.unescaped().to_vec())
                .ok()
                .map(|name| (name, singleton))
        })
        .collect()
}

/// A `T::Struct` field declaration: `const :name, String`, `prop :age, Integer`.
///
/// The type is the second argument rather than a preceding `sig`, so this is a
/// different shape from every other signature and needs its own reader.
fn struct_field(node: &Node<'_>) -> Option<(String, Receiver)> {
    let call = node.as_call_node()?;
    if !matches!(call.name().as_slice(), b"const" | b"prop") {
        return None;
    }
    let mut arguments = call.arguments()?.arguments().iter();
    let name = arguments.next()?.as_symbol_node()?;
    let name = String::from_utf8(name.unescaped().to_vec()).ok()?;
    // A field whose type rwr cannot name yields nothing, as everywhere else.
    Some((name, receiver_type(&arguments.next()?)?))
}

/// What one signature states: its return type, and its parameter types. Either
/// half may be absent -- they fail independently.
type Stated = (Option<Receiver>, Vec<(String, Receiver)>);

/// What a `sig { ... }` states: its return type, and its parameter types.
///
/// `None` only when the node is not a signature at all. A signature that states
/// nothing usable yields empty halves rather than nothing, because the two
/// halves fail independently -- `params(x: String).void` has no return type this
/// index can use and a perfectly good argument type.
fn signature_types(node: &Node<'_>) -> Option<Stated> {
    let call = node.as_call_node()?;
    if call.name().as_slice() != b"sig" {
        return None;
    }
    let body = call.block()?.as_block_node()?.body()?;
    let statements = body.as_statements_node()?;
    let expression = statements.body().iter().next()?;

    // Both sit somewhere in one chain: `returns(X)`, `params(..).returns(X)`,
    // `overridable.params(..).void`. Walk the receivers once, taking whichever
    // turns up, rather than walking it again per half.
    let (mut returns, mut params) = (None, Vec::new());
    let mut current = expression;
    while let Some(call) = current.as_call_node() {
        match call.name().as_slice() {
            b"returns" if returns.is_none() => {
                returns = call
                    .arguments()
                    .and_then(|a| a.arguments().iter().next())
                    .and_then(|a| receiver_type(&a));
            }
            b"params" if params.is_empty() => {
                params = signature_params(&call);
            }
            _ => {}
        }
        match call.receiver() {
            Some(receiver) => current = receiver,
            None => break,
        }
    }
    Some((returns, params))
}

/// The named parameter types in `params(x: String, y: T.nilable(Integer))`.
///
/// A type rwr cannot turn into a class name is dropped rather than guessed at,
/// so a `T.untyped` parameter simply does not appear and the local stays
/// unresolved -- which declines a constraint rather than passing one.
fn signature_params(call: &ruby_prism::CallNode<'_>) -> Vec<(String, Receiver)> {
    let mut out = Vec::new();
    let Some(arguments) = call.arguments() else {
        return out;
    };
    for argument in arguments.arguments().iter() {
        // `params(x: String)` puts every pair in one keyword hash.
        let elements = match &argument {
            Node::KeywordHashNode { .. } => argument.as_keyword_hash_node().map(|h| h.elements()),
            Node::HashNode { .. } => argument.as_hash_node().map(|h| h.elements()),
            _ => None,
        };
        let Some(elements) = elements else { continue };
        for element in elements.iter() {
            let Some(assoc) = element.as_assoc_node() else {
                continue;
            };
            let Some(key) = assoc.key().as_symbol_node() else {
                continue;
            };
            let Ok(name) = String::from_utf8(key.unescaped().to_vec()) else {
                continue;
            };
            if let Some(receiver) = receiver_type(&assoc.value()) {
                out.push((name, receiver));
            }
        }
    }
    out
}

/// Turn a Sorbet type expression into a receiver, when it names a class.
///
/// The name is the constant *as written*: spelled whole (`A::B`, never `B`) and
/// left unresolved, because what it names depends on where the signature sits
/// and only the caller knows that. `Written` carries it there.
fn receiver_type(node: &Node<'_>) -> Option<Receiver> {
    match node {
        // Shared with the matcher rather than respelled, so a signature's type
        // and a call's receiver cannot come out as different class names.
        //
        // `T::` is stripped: it is Sorbet's type language, not a Ruby
        // namespace, and what `T::Array[Widget]` dispatches on is `Array`.
        Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. } => {
            crate::pattern::matcher::qualified(node)
                .map(|name| name.strip_prefix("T::").unwrap_or(&name).to_string())
                .map(Receiver::Instance)
        }
        Node::CallNode { .. } => {
            let call = node.as_call_node()?;
            match call.name().as_slice() {
                // `T::Array[String]` is a call to `[]` on the constant path.
                // The element type is erased: what dispatches is Array.
                b"[]" => receiver_type(&call.receiver()?),
                // A nilable value that reaches a call site is not nil there, so
                // the inner type is what dispatches.
                b"nilable" => receiver_type(&call.arguments()?.arguments().iter().next()?),
                // `T.class_of(X)` is the class object, not an instance.
                b"class_of" => {
                    match receiver_type(&call.arguments()?.arguments().iter().next()?)? {
                        Receiver::Instance(name) | Receiver::Class(name) => {
                            Some(Receiver::Class(name))
                        }
                    }
                }
                // `T.untyped`, `T.any(..)`, `T.all(..)`, `void` -- no single
                // class dispatches, so there is nothing to narrow by.
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(source: &str) -> Signatures {
        Signatures::from_sources(&[Source::Owned(source.as_bytes().to_vec())]).0
    }

    /// A lookup reads a type where it was written, so every assertion needs the
    /// hierarchy of the same source the signatures came from.
    fn looked_up(source: &str) -> (Signatures, Hierarchy) {
        (index(source), Hierarchy::from_source(source))
    }

    fn returns_of(sig: &str) -> Option<Receiver> {
        let source = format!("class C\n  {sig}\n  def m; end\nend\n");
        let (sigs, hierarchy) = looked_up(&source);
        sigs.returns(&hierarchy, "C", "m", false)
    }

    fn params_of(sig: &str) -> Vec<(String, String)> {
        let source = format!("class C\n  {sig}\n  def m(a, b); end\nend\n");
        let (sigs, hierarchy) = looked_up(&source);
        let mut found: Vec<(String, String)> = sigs
            .params(&hierarchy, "C", "m", false)
            .map(|p| {
                p.iter()
                    .map(|(k, v)| (k.clone(), v.class_name().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        found
    }

    /// Parameter types come from the same chain as the return type, and must not
    /// be gated on it. `params(..).void` states nothing usable about the result
    /// and everything about the argument, and it is the commonest shape on a
    /// command or a setter -- gating dropped every one of them.
    #[test]
    fn parameter_types_survive_a_signature_with_no_usable_return() {
        assert_eq!(
            params_of("sig { params(a: String, b: Integer).void }"),
            vec![
                ("a".to_string(), "String".to_string()),
                ("b".to_string(), "Integer".to_string())
            ]
        );
        // And the same signature yields no return type, which is the half that
        // used to decide whether any of it was recorded.
        let source = "class C\n  sig { params(a: String).void }\n  def m(a); end\nend\n";
        let (sigs, hierarchy) = looked_up(source);
        assert!(sigs.returns(&hierarchy, "C", "m", false).is_none());
    }

    /// A repository whose signatures are all `params(..).void` has no return
    /// types and is emphatically not empty. Keying "did we read anything" off
    /// the return count reported "no Sorbet signatures were found" about a file
    /// that had just resolved a parameter two lines earlier.
    #[test]
    fn a_void_only_index_is_not_empty() {
        let (sigs, hierarchy) =
            looked_up("class C\n  sig { params(a: String).void }\n  def m(a); end\nend\n");
        assert!(sigs.returns(&hierarchy, "C", "m", false).is_none());
        assert!(!sigs.is_empty());
        assert_eq!(sigs.len(), 1);
    }

    /// The nilable unwrapping that makes a guard resolvable: what reaches the
    /// call site is the inner type, and `T::Boolean` arrives under the name its
    /// constant path ends in.
    #[test]
    fn nilable_parameters_resolve_to_what_they_wrap() {
        assert_eq!(
            params_of("sig { params(a: T.nilable(String), b: T.nilable(T::Boolean)).void }"),
            vec![
                ("a".to_string(), "String".to_string()),
                ("b".to_string(), "Boolean".to_string())
            ]
        );
    }

    /// A type that names no single class yields nothing rather than a guess, so
    /// the local stays unresolved and a constraint declines rather than passes.
    #[test]
    fn an_untyped_parameter_is_absent_rather_than_guessed() {
        assert_eq!(
            params_of("sig { params(a: T.untyped, b: String).void }"),
            vec![("b".to_string(), "String".to_string())]
        );
    }

    /// Every spelling of a signature a real codebase uses, and what each one
    /// yields. The `T.` forms that name no single class must yield nothing --
    /// narrowing may only ever narrow.
    #[test]
    fn return_types_are_read_from_every_spelling() {
        let instance = |name: &str| Some(Receiver::Instance(name.to_string()));

        assert_eq!(returns_of("sig { returns(String) }"), instance("String"));
        assert_eq!(
            returns_of("sig { params(a: Integer).returns(Widget) }"),
            instance("Widget")
        );
        assert_eq!(
            returns_of("sig { overridable.returns(Widget) }"),
            instance("Widget")
        );
        // A constant path names the class it spells, as everywhere else in rwr
        // since D100. Reading it as `Widget` claimed a namesake's method.
        assert_eq!(
            returns_of("sig { returns(A::B::Widget) }"),
            instance("A::B::Widget")
        );
        // A value that reaches a call site is not nil there.
        assert_eq!(
            returns_of("sig { returns(T.nilable(Widget)) }"),
            instance("Widget")
        );
        // The element type is erased: what dispatches is Array.
        assert_eq!(
            returns_of("sig { returns(T::Array[Widget]) }"),
            instance("Array")
        );
        assert_eq!(
            returns_of("sig { returns(T::Hash[String, T.untyped]) }"),
            instance("Hash")
        );
        // A class object, not an instance of one.
        assert_eq!(
            returns_of("sig { returns(T.class_of(Widget)) }"),
            Some(Receiver::Class("Widget".to_string()))
        );

        // Nothing here names a single class to dispatch on.
        assert_eq!(returns_of("sig { returns(T.untyped) }"), None);
        assert_eq!(returns_of("sig { returns(T.any(String, Integer)) }"), None);
        assert_eq!(returns_of("sig { void }"), None);
        assert_eq!(returns_of("sig { params(a: Integer).void }"), None);
    }

    /// Which class a signature's type names depends on where the signature is
    /// written, exactly as it does for a call's receiver (D100). Reading a path
    /// as its last segment, or a bare name at the top level, handed a
    /// namesake's method to a rewrite -- `Helpers::Thing#display_name` renamed
    /// under the name `Thing`, for a `NoMethodError` at runtime.
    #[test]
    fn a_signature_type_is_read_where_it_was_written() {
        let nested = "module App\n  class Thing; end\n\n  class Parser\n    \
                      sig { returns(Thing) }\n    def thing; end\n  end\nend\n\nclass Thing; end\n";
        let (sigs, hierarchy) = looked_up(nested);
        assert_eq!(
            sigs.returns(&hierarchy, "App::Parser", "thing", false),
            Some(Receiver::Instance("App::Thing".to_string()))
        );

        let path = "module Helpers\n  class Thing; end\nend\n\nclass Thing; end\n\n\
                    class Parser\n  sig { returns(Helpers::Thing) }\n  def thing; end\nend\n";
        let (sigs, hierarchy) = looked_up(path);
        assert_eq!(
            sigs.returns(&hierarchy, "Parser", "thing", false),
            Some(Receiver::Instance("Helpers::Thing".to_string()))
        );
    }

    /// Two classes with one short name are two classes. Keyed on the bare name
    /// the index silently overwrote, so which type a call got depended on which
    /// other files happened to be in the run's path set -- and on their order.
    #[test]
    fn namesake_classes_do_not_share_a_signature() {
        let source = "class Widget; end\nclass Gadget; end\n\n\
                      module Alpha\n  class Parser\n    sig { returns(Widget) }\n    \
                      def thing; end\n  end\nend\n\n\
                      module Beta\n  class Parser\n    sig { returns(Gadget) }\n    \
                      def thing; end\n  end\nend\n";
        let (sigs, hierarchy) = looked_up(source);
        assert_eq!(
            sigs.returns(&hierarchy, "Alpha::Parser", "thing", false),
            Some(Receiver::Instance("Widget".to_string()))
        );
        assert_eq!(
            sigs.returns(&hierarchy, "Beta::Parser", "thing", false),
            Some(Receiver::Instance("Gadget".to_string()))
        );
        // Neither of them answers to the segment they share.
        assert_eq!(sigs.returns(&hierarchy, "Parser", "thing", false), None);
    }

    /// A signature describes the *next* definition, and that is not always a
    /// `def`: `attr_reader` is signed the same way and defines one method per
    /// symbol.
    #[test]
    fn attr_readers_carry_their_signature() {
        let (sigs, hierarchy) = looked_up(
            "class C\n  sig { returns(Widget) }\n  attr_reader :one\n\n  \
             sig { returns(Widget) }\n  attr_accessor :two, :three\nend\n",
        );
        for name in ["one", "two", "three"] {
            assert_eq!(
                sigs.returns(&hierarchy, "C", name, false),
                Some(Receiver::Instance("Widget".to_string())),
                "{name}"
            );
        }
    }

    /// `Account#build` and `Account.build` are different methods, so their
    /// return types must not share a key.
    #[test]
    fn singleton_and_instance_methods_are_separate() {
        let (sigs, hierarchy) = looked_up(
            "class C\n  sig { returns(Widget) }\n  def self.build; end\n\n  \
             sig { returns(Gadget) }\n  def build; end\nend\n",
        );
        assert_eq!(
            sigs.returns(&hierarchy, "C", "build", true),
            Some(Receiver::Instance("Widget".to_string()))
        );
        assert_eq!(
            sigs.returns(&hierarchy, "C", "build", false),
            Some(Receiver::Instance("Gadget".to_string()))
        );
    }

    /// Everything inside `class << self` is a singleton method.
    #[test]
    fn a_singleton_class_body_is_singleton_context() {
        let (sigs, hierarchy) = looked_up(
            "class C\n  class << self\n    sig { returns(Widget) }\n    def build; end\n  end\nend\n",
        );
        assert_eq!(
            sigs.returns(&hierarchy, "C", "build", true),
            Some(Receiver::Instance("Widget".to_string()))
        );
        assert_eq!(sigs.returns(&hierarchy, "C", "build", false), None);
    }

    /// `T::Struct` states a field's type in the declaration itself, with no
    /// `sig` anywhere. Measured at 45,068 sites on a Sorbet monolith, a third
    /// again as many as its `sig` blocks.
    #[test]
    fn struct_fields_carry_their_type() {
        let (sigs, hierarchy) = looked_up(
            "class Row < T::Struct\n  const :name, String\n  prop :widget, Widget\n  \
             const :maybe, T.nilable(Gadget)\n  const :untyped_thing, T.untyped\nend\n",
        );
        assert_eq!(
            sigs.returns(&hierarchy, "Row", "name", false),
            Some(Receiver::Instance("String".to_string()))
        );
        assert_eq!(
            sigs.returns(&hierarchy, "Row", "widget", false),
            Some(Receiver::Instance("Widget".to_string()))
        );
        assert_eq!(
            sigs.returns(&hierarchy, "Row", "maybe", false),
            Some(Receiver::Instance("Gadget".to_string()))
        );
        // A type rwr cannot name yields nothing here as everywhere else.
        assert_eq!(
            sigs.returns(&hierarchy, "Row", "untyped_thing", false),
            None
        );
    }

    /// `const` is an ordinary enough word that reading it outside a `T::` struct
    /// would invent types rather than narrow by them.
    #[test]
    fn const_outside_a_typed_struct_is_not_a_field() {
        let sigs = index("class Config < Base\n  const :name, String\nend\n");
        assert!(sigs.is_empty());
    }

    /// A repository with no signatures costs one substring search and yields an
    /// empty index -- never a guess.
    #[test]
    fn a_file_without_signatures_yields_nothing() {
        assert!(index("class C\n  def design; assign_thing; end\nend\n").is_empty());
    }
}
