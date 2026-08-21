//! rwr — Ruby structural search and rewrite.
//!
//! `rg`/`sed` for Ruby *programs* rather than Ruby *text*: find code by
//! structure, rewrite only what matches, preserve everything else, and refuse
//! when it can't be sure. See `DESIGN.md` for the design these modules
//! implement and `docs/decisions.md` for why each piece is shaped as it is.

pub mod cli;
pub(crate) mod pattern;
pub(crate) mod residue;
pub(crate) mod rewrite;
pub(crate) mod source;
