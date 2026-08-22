//! rwr — Ruby structural search and rewrite.
//!
//! `rg`/`sed` for Ruby *programs* rather than Ruby *text*: find code by
//! structure, rewrite only what matches, preserve everything else, and refuse
//! when it can't be sure. See `DESIGN.md` for the design these modules
//! implement and `docs/decisions.md` for why each piece is shaped as it is.

pub mod cli;
#[allow(dead_code)]
pub(crate) mod diff;
pub(crate) mod erb;
pub(crate) mod hierarchy;
pub(crate) mod pattern;
#[allow(dead_code)]
pub(crate) mod profile;
#[allow(dead_code)]
pub(crate) mod residue;
pub(crate) mod rewrite;
#[allow(dead_code)]
pub(crate) mod ruby;
pub(crate) mod rule;
pub(crate) mod sigs;
pub(crate) mod source;
