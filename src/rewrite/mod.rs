//! Edit computation and splicing.
//!
//! The rewriter is a port of `Parser::Source::TreeRewriter`'s tree of actions
//! (decision D13). Its invariants — children strictly contained by parent,
//! siblings disjoint and ordered, only non-replacing actions may have children
//! — give order-independence by construction and a clean error on partial
//! overlap rather than corrupt output.
//!
//! The conflict unit is the *edit* range, not the match range (decision D15).
//!
//! Splicing goes through `effective_range()` only (decision D14): a node's
//! effective range is the transitive closure over its descendants unioning each
//! heredoc's closing location. Raw node locations are deliberately not exposed
//! from the capture API — a heredoc body lives far from its `<<~FOO` token, and
//! detaching one still *parses*, so no downstream check catches the mistake.
