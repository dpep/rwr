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

use crate::pattern::generated;
use crate::pattern::matcher::Receiver;
use crate::source::Source;
use rayon::prelude::*;
use ruby_prism::Node;
use std::collections::HashMap;

/// Which method of which class, what it returns, and what it takes.
type Signed = (
    (String, String, bool),
    Option<Receiver>,
    Vec<(String, Receiver)>,
);

/// What each signed method returns and accepts, keyed by the class defining it.
#[derive(Debug, Default)]
pub(crate) struct Signatures {
    /// `(class, method, singleton) -> return type`.
    returns: HashMap<(String, String, bool), Receiver>,
    /// `(class, method, singleton) -> parameter name -> its type`.
    ///
    /// Parameters are the half that makes an ordinary guard resolvable. A return
    /// type answers "what does this chain evaluate to"; a parameter type answers
    /// "what is this bare local", which is what most code actually asks about --
    /// `return if x.nil?` guards an argument far more often than a chain.
    params: HashMap<(String, String, bool), HashMap<String, Receiver>>,
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
        class: &str,
        method: &str,
        singleton: bool,
    ) -> Option<&HashMap<String, Receiver>> {
        self.params
            .get(&(class.to_string(), method.to_string(), singleton))
    }

    /// What `class#method` (or `class.method`) returns, if a signature said.
    pub(crate) fn returns(&self, class: &str, method: &str, singleton: bool) -> Option<&Receiver> {
        self.returns
            .get(&(class.to_string(), method.to_string(), singleton))
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
        let mut params: HashMap<(String, String, bool), HashMap<String, Receiver>> = HashMap::new();
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
            let Some(class) = scope.last() else { continue };
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
        if struct_body && let Some(class) = scope.last() {
            for statement in &body {
                if let Some((name, returns)) = struct_field(statement) {
                    out.push(((class.clone(), name, false), Some(returns), Vec::new()));
                }
            }
        }
    }

    let mut inner: Vec<String> = scope.to_vec();
    let mut inner_singleton = singleton;
    let mut inner_struct = struct_body;
    match node {
        Node::ClassNode { .. } => {
            if let Some(class) = node.as_class_node() {
                if let Ok(name) = String::from_utf8(class.name().as_slice().to_vec()) {
                    inner.push(name);
                    inner_singleton = false;
                }
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
        Node::ModuleNode { .. } => {
            if let Some(module) = node.as_module_node()
                && let Ok(name) = String::from_utf8(module.name().as_slice().to_vec())
            {
                inner.push(name);
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
fn receiver_type(node: &Node<'_>) -> Option<Receiver> {
    match node {
        Node::ConstantReadNode { .. } => {
            let name = String::from_utf8(node.as_constant_read_node()?.name().as_slice().to_vec());
            name.ok().map(Receiver::Instance)
        }
        // `A::B` denotes B, matching how a constant path resolves elsewhere.
        Node::ConstantPathNode { .. } => {
            let name = String::from_utf8(node.as_constant_path_node()?.name()?.as_slice().to_vec());
            name.ok().map(Receiver::Instance)
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

    fn returns_of(sig: &str) -> Option<Receiver> {
        let source = format!("class C\n  {sig}\n  def m; end\nend\n");
        index(&source).returns("C", "m", false).cloned()
    }

    fn params_of(sig: &str) -> Vec<(String, String)> {
        let source = format!("class C\n  {sig}\n  def m(a, b); end\nend\n");
        let index = index(&source);
        let mut found: Vec<(String, String)> = index
            .params("C", "m", false)
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
        assert!(index(source).returns("C", "m", false).is_none());
    }

    /// A repository whose signatures are all `params(..).void` has no return
    /// types and is emphatically not empty. Keying "did we read anything" off
    /// the return count reported "no Sorbet signatures were found" about a file
    /// that had just resolved a parameter two lines earlier.
    #[test]
    fn a_void_only_index_is_not_empty() {
        let index = index("class C\n  sig { params(a: String).void }\n  def m(a); end\nend\n");
        assert!(index.returns("C", "m", false).is_none());
        assert!(!index.is_empty());
        assert_eq!(index.len(), 1);
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
        // A constant path denotes its last name, as everywhere else in rwr.
        assert_eq!(
            returns_of("sig { returns(A::B::Widget) }"),
            instance("Widget")
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

    /// A signature describes the *next* definition, and that is not always a
    /// `def`: `attr_reader` is signed the same way and defines one method per
    /// symbol.
    #[test]
    fn attr_readers_carry_their_signature() {
        let sigs = index(
            "class C\n  sig { returns(Widget) }\n  attr_reader :one\n\n  \
             sig { returns(Widget) }\n  attr_accessor :two, :three\nend\n",
        );
        for name in ["one", "two", "three"] {
            assert_eq!(
                sigs.returns("C", name, false),
                Some(&Receiver::Instance("Widget".to_string())),
                "{name}"
            );
        }
    }

    /// `Account#build` and `Account.build` are different methods, so their
    /// return types must not share a key.
    #[test]
    fn singleton_and_instance_methods_are_separate() {
        let sigs = index(
            "class C\n  sig { returns(Widget) }\n  def self.build; end\n\n  \
             sig { returns(Gadget) }\n  def build; end\nend\n",
        );
        assert_eq!(
            sigs.returns("C", "build", true),
            Some(&Receiver::Instance("Widget".to_string()))
        );
        assert_eq!(
            sigs.returns("C", "build", false),
            Some(&Receiver::Instance("Gadget".to_string()))
        );
    }

    /// Everything inside `class << self` is a singleton method.
    #[test]
    fn a_singleton_class_body_is_singleton_context() {
        let sigs = index(
            "class C\n  class << self\n    sig { returns(Widget) }\n    def build; end\n  end\nend\n",
        );
        assert_eq!(
            sigs.returns("C", "build", true),
            Some(&Receiver::Instance("Widget".to_string()))
        );
        assert_eq!(sigs.returns("C", "build", false), None);
    }

    /// `T::Struct` states a field's type in the declaration itself, with no
    /// `sig` anywhere. Measured at 45,068 sites on a Sorbet monolith, a third
    /// again as many as its `sig` blocks.
    #[test]
    fn struct_fields_carry_their_type() {
        let sigs = index(
            "class Row < T::Struct\n  const :name, String\n  prop :widget, Widget\n  \
             const :maybe, T.nilable(Gadget)\n  const :untyped_thing, T.untyped\nend\n",
        );
        assert_eq!(
            sigs.returns("Row", "name", false),
            Some(&Receiver::Instance("String".to_string()))
        );
        assert_eq!(
            sigs.returns("Row", "widget", false),
            Some(&Receiver::Instance("Widget".to_string()))
        );
        assert_eq!(
            sigs.returns("Row", "maybe", false),
            Some(&Receiver::Instance("Gadget".to_string()))
        );
        // A type rwr cannot name yields nothing here as everywhere else.
        assert_eq!(sigs.returns("Row", "untyped_thing", false), None);
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
