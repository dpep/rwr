//! Name-scoped residue reporting.
//!
//! For name-anchored rules only — a rename or signature change has a target
//! identifier; `return nil` -> `return` does not (DESIGN.md §4).
//!
//! After the structural match, enumerate every remaining occurrence of the
//! target identifier the match did not account for, classified by syntactic
//! context: symbol literals, strings, interpolation fragments whose static
//! parts are consistent, and elements of literal arrays feeding `send`,
//! `define_method`, `delegate` or `alias_method`.
//!
//! This is lexical plus AST context, not dataflow. It is what a careful human
//! does with `rg` after a rename.
