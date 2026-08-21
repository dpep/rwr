//! Structural equality over Prism nodes (D36).
//!
//! Two nodes are equal iff they are the **same variant**, carry **equal atoms**,
//! and have **pairwise-equal children**. Locations never participate: spelling
//! is opt-in through `where:` predicates (DESIGN.md §2), not baked into
//! equality. That is what makes `foo(a, b)`, `foo(a,b)` and a multiline spelling
//! all match one pattern, while `foo(a)` and `bar(a)` do not.
//!
//! One comparator serves three consumers -- matching, D16's repeated-metavariable
//! equality, and §7's reparse-verify. If each grew its own notion of equality
//! they would drift, and the verify step would stop guarding the matcher.

use super::generated;
use ruby_prism::Node;
use std::mem::discriminant;

/// A name or value carried by a node but not represented as a child.
///
/// `CallNode::name` is the motivating case: comparing variant and children
/// alone would match `foo(a)` against `bar(a)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Atom {
    /// An identifier, compared by **resolved bytes** rather than by constant id
    /// -- pattern and target come from different parses with different pools.
    Name(Vec<u8>),
    /// A literal's **unescaped** value, so `"x"` equals `'x'` and heredoc bodies
    /// compare correctly with no heredoc-specific code.
    Value(Vec<u8>),
    /// Numeric values, normalised through their debug form so `1_000` equals
    /// `1000`.
    Number(String),
}

impl Atom {
    pub(crate) fn name(bytes: &[u8]) -> Self {
        Atom::Name(bytes.to_vec())
    }

    pub(crate) fn value(bytes: &[u8]) -> Self {
        Atom::Value(bytes.to_vec())
    }

    pub(crate) fn debug<T: std::fmt::Debug>(value: &T) -> Self {
        Atom::Number(format!("{value:?}"))
    }

    /// Prism's `Debug for Integer` prints the *pointer*, so debug-formatting an
    /// integer would make every literal compare unequal -- silently, and in the
    /// direction that loses matches. Compare the digits instead.
    pub(crate) fn integer(value: &ruby_prism::Integer<'_>) -> Self {
        let (negative, digits) = value.to_u32_digits();
        Atom::Number(format!("{negative}:{digits:?}"))
    }
}

/// Structural equality, ignoring source position entirely.
pub(crate) fn node_eq(a: &Node<'_>, b: &Node<'_>) -> bool {
    if discriminant(a) != discriminant(b) {
        return false;
    }
    if generated::atoms(a) != generated::atoms(b) {
        return false;
    }
    let (ca, cb) = (generated::children(a), generated::children(b));
    ca.len() == cb.len() && ca.iter().zip(&cb).all(|(x, y)| node_eq(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compare the sole statement of two snippets.
    fn eq(a: &str, b: &str) -> bool {
        let (ra, rb) = (
            ruby_prism::parse(a.as_bytes()),
            ruby_prism::parse(b.as_bytes()),
        );
        assert_eq!(ra.errors().count(), 0, "{a:?} does not parse");
        assert_eq!(rb.errors().count(), 0, "{b:?} does not parse");
        node_eq(&ra.node(), &rb.node())
    }

    /// DESIGN.md §3A: formatting is not structure. This is what tree
    /// comparison gives by construction and text matching cannot.
    #[test]
    fn layout_does_not_affect_equality() {
        assert!(eq("foo(a, b)", "foo(a,b)"));
        assert!(eq("foo(a, b)", "foo(\n  a,\n  b\n)"));
    }

    /// A trailing comma is invisible to the AST: `[a, b]` and `[a, b,]` are the
    /// same program by rwr's own equality. That places it in the same class as
    /// indentation -- presentation, not structure -- and therefore with the
    /// formatter rather than with rwr (principle 7, D34).
    #[test]
    fn a_trailing_comma_is_not_structure() {
        assert!(eq("[a, b]", "[a, b,]"));
        assert!(eq("foo(a, b)", "foo(a, b,)"));
        assert!(eq("{a: 1}", "{a: 1,}"));
    }

    /// The trap that motivated D36: the method name is an atom, not a child,
    /// so variant-plus-children would call these equal.
    #[test]
    fn method_name_is_compared() {
        assert!(!eq("foo(a)", "bar(a)"));
        assert!(eq("foo(a)", "foo(a)"));
    }

    #[test]
    fn arguments_are_compared() {
        assert!(!eq("foo(a)", "foo(b)"));
        assert!(!eq("foo(a)", "foo(a, b)"));
    }

    /// Comparing unescaped values means quoting style is not structure -- and
    /// it is what makes heredoc bodies compare correctly for free, which the
    /// rejected interstitial design could not do.
    #[test]
    fn string_quoting_is_not_structure() {
        assert!(eq(r#"foo("x")"#, "foo('x')"));
        assert!(!eq(r#"foo("x")"#, r#"foo("y")"#));
    }

    #[test]
    fn heredoc_bodies_are_compared() {
        let a = "foo(<<~SQL)\n  SELECT 1\nSQL\n";
        let b = "foo(<<~SQL)\n  SELECT 2\nSQL\n";
        assert!(eq(a, a), "identical heredocs compared unequal");
        assert!(!eq(a, b), "different heredoc bodies compared equal");
    }

    /// Numeric spelling is not structure.
    #[test]
    fn integer_spelling_is_not_structure() {
        assert!(eq("foo(1000)", "foo(1_000)"));
        assert!(!eq("foo(1000)", "foo(1001)"));
    }

    /// Receivers are children, so they participate.
    #[test]
    fn receivers_are_compared() {
        assert!(!eq("a.foo", "b.foo"));
        assert!(!eq("foo", "a.foo"));
    }
}
