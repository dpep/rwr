//! Pattern parsing and structural matching.
//!
//! A pattern is Ruby source with `$METAVAR` placeholders (decision D2), parsed
//! by Prism exactly like target source, then walked against the target tree in
//! lockstep with metavariable nodes acting as wildcards.
//!
//! Metavariable semantics follow decision D16: min/max occurrence counts unify
//! single, optional, sequence and must-not-appear under one mechanism, and a
//! repeated metavariable requires *AST* equality, never textual equality.

pub(crate) mod metavar;
pub(crate) mod prefilter;
pub(crate) mod prepare;

// The whole module is a drift guard: its table exists to be checked against
// Prism's vendored schema by its own tests, not to be called.
#[allow(dead_code)]
pub(crate) mod schema;

pub(crate) mod compare;
pub(crate) mod generated;
pub(crate) mod matcher;
