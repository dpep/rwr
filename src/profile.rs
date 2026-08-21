//! Phase timing for `--profile`.
//!
//! Answers "where did the time go?", as a table meant to be compared against
//! another run rather than read as prose.
//!
//! Off by default and free when off: a span reads no clock and allocates
//! nothing unless profiling is on, so the only cost on the hot path is a
//! relaxed atomic load.
//!
//! The measurement that matters most here is **files parsed** against **files
//! walked**. rwr's scaling story is not "parse faster" but "parse fewer" -- a
//! literal prefilter skips a file whose bytes cannot contain the pattern's
//! anchors -- so a change that speeds the total while parsing more files is
//! usually the wrong direction.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static PHASES: Mutex<Vec<Phase>> = Mutex::new(Vec::new());

/// One measured phase.
struct Phase {
    name: &'static str,
    elapsed: Duration,
    /// What the phase did -- file counts, a skip ratio, a hierarchy size.
    note: Option<String>,
}

/// Enable from the `--profile` flag; `RWR_PROFILE` in the environment also
/// enables it, so a shipped binary can be measured in place.
pub(crate) fn enable_from(flag: bool) {
    let on = flag || std::env::var_os("RWR_PROFILE").is_some();
    ENABLED.store(on, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Time a phase, recording it only when profiling is on.
pub(crate) fn span<T>(name: &'static str, body: impl FnOnce() -> T) -> T {
    if !enabled() {
        return body();
    }
    let started = Instant::now();
    let out = body();
    record(name, started.elapsed(), None);
    out
}

/// Record a phase timed by the caller.
///
/// The closure form does not fit a long parallel chain without contorting it,
/// and a contorted hot path is worse than an explicit `Instant`.
pub(crate) fn mark(name: &'static str, started: Instant, note: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    record(name, started.elapsed(), Some(note()));
}

/// A clock that costs nothing when profiling is off.
pub(crate) fn now() -> Instant {
    Instant::now()
}

/// Time a phase and describe what it did.
pub(crate) fn span_noted<T>(
    name: &'static str,
    body: impl FnOnce() -> T,
    note: impl FnOnce(&T) -> String,
) -> T {
    if !enabled() {
        return body();
    }
    let started = Instant::now();
    let out = body();
    let elapsed = started.elapsed();
    record(name, elapsed, Some(note(&out)));
    out
}

fn record(name: &'static str, elapsed: Duration, note: Option<String>) {
    if let Ok(mut phases) = PHASES.lock() {
        phases.push(Phase {
            name,
            elapsed,
            note,
        });
    }
}

/// Print the table. No-op when profiling is off or nothing was measured.
pub(crate) fn report() {
    if !enabled() {
        return;
    }
    let Ok(phases) = PHASES.lock() else { return };
    if phases.is_empty() {
        return;
    }
    let total: Duration = phases.iter().map(|p| p.elapsed).sum();
    eprintln!("\n  phase              ms      %  note");
    for phase in phases.iter() {
        let ms = phase.elapsed.as_secs_f64() * 1000.0;
        let share = if total.is_zero() {
            0.0
        } else {
            100.0 * phase.elapsed.as_secs_f64() / total.as_secs_f64()
        };
        eprintln!(
            "  {:<14} {:>7.1} {:>5.0}%  {}",
            phase.name,
            ms,
            share,
            phase.note.as_deref().unwrap_or("")
        );
    }
    eprintln!("  {:<14} {:>7.1}", "total", total.as_secs_f64() * 1000.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off by default, and a span still returns its value.
    #[test]
    fn a_span_is_transparent_when_disabled() {
        ENABLED.store(false, Ordering::Relaxed);
        assert_eq!(span("noop", || 41 + 1), 42);
    }
}
