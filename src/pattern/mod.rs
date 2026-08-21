//! Pattern parsing and structural matching.
//!
//! A pattern is Ruby source with `$METAVAR` placeholders (decision D2), parsed
//! by Prism exactly like target source, then walked against the target tree in
//! lockstep with metavariable nodes acting as wildcards.
//!
//! Metavariable semantics follow decision D16: min/max occurrence counts unify
//! single, optional, sequence and must-not-appear under one mechanism, and a
//! repeated metavariable requires *AST* equality, never textual equality.

// Reachable only from its own tests until the matcher consumes it. The scanner
// is the ground truth for D32's syntax, so it is worth having pinned by tests
// before anything depends on it. Drop this allow when the matcher lands.
#[allow(dead_code)]
pub(crate) mod metavar;

#[allow(dead_code)]
pub(crate) mod prepare;

#[allow(dead_code)]
pub(crate) mod schema;
