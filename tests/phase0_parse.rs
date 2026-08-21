//! Phase 0 measurement (d): cold parse throughput and parser fidelity.
//!
//! Decides D5 — whether Phase 1 needs any persistence at all, or whether a cold
//! parallel parse is fast enough that a cache would be solving an unmeasured
//! problem. Also produces the first evidence for D1's fidelity claim: how much
//! real-world Ruby does Prism actually parse?
//!
//! Skips when the corpora are absent, so CI stays green without them. Run with:
//!
//! ```sh
//! cargo test --test phase0_parse -- --nocapture
//! ```

use ignore::WalkBuilder;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the reference repositories live. Override with `RWR_CORPUS_ROOT`.
fn corpus_root() -> PathBuf {
    std::env::var("RWR_CORPUS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("code/lib/ruby")
        })
}

struct Measured {
    files: usize,
    bytes: u64,
    failures: Vec<PathBuf>,
    elapsed_ms: u128,
}

fn ruby_files(root: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .hidden(false)
        .build()
        .filter_map(Result::ok)
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|x| x == "rb"))
        .collect()
}

fn measure(root: &Path) -> Measured {
    let files = ruby_files(root);
    let start = Instant::now();

    let results: Vec<(u64, Option<PathBuf>)> = files
        .par_iter()
        .filter_map(|path| {
            let src = std::fs::read(path).ok()?;
            let len = src.len() as u64;
            let failed = ruby_prism::parse(&src).errors().count() > 0;
            Some((len, failed.then(|| path.clone())))
        })
        .collect();

    let elapsed_ms = start.elapsed().as_millis();
    Measured {
        files: results.len(),
        bytes: results.iter().map(|(n, _)| n).sum(),
        failures: results.iter().filter_map(|(_, f)| f.clone()).collect(),
        elapsed_ms,
    }
}

#[test]
fn cold_parse_throughput_and_fidelity() {
    let root = corpus_root();
    if !root.is_dir() {
        eprintln!("skipping: no corpus at {}", root.display());
        return;
    }

    // CRuby's own tree is deliberately included: its test suite contains files
    // that are *supposed* to be syntactically invalid, so a nonzero failure
    // rate there is correct behaviour rather than a fidelity gap. Rails is the
    // honest fidelity signal — idiomatic application-shaped Ruby.
    let repos = ["rails", "rubocop", "graphql", "ruby"];
    let mut total_files = 0usize;
    let mut total_bytes = 0u64;
    let mut total_ms = 0u128;

    println!(
        "\n  Phase 0 (d) — cold parse, {} threads\n",
        rayon::current_num_threads()
    );
    println!(
        "  {:<12} {:>7} {:>10} {:>9} {:>12} {:>8}",
        "repo", "files", "MB", "ms", "MB/s", "unparsed"
    );

    for name in repos {
        let dir = root.join(name);
        if !dir.is_dir() {
            continue;
        }
        let m = measure(&dir);
        let mb = m.bytes as f64 / 1_048_576.0;
        let secs = m.elapsed_ms as f64 / 1000.0;
        let rate = if secs > 0.0 { mb / secs } else { f64::NAN };

        println!(
            "  {:<12} {:>7} {:>10.1} {:>9} {:>12.0} {:>8}",
            name,
            m.files,
            mb,
            m.elapsed_ms,
            rate,
            m.failures.len()
        );

        for f in m.failures.iter().take(5) {
            println!("      unparsed: {}", f.display());
        }

        if name == "rails" {
            // Application-shaped Ruby should parse essentially completely. This
            // is the number behind D1: a parser that tracks CRuby by
            // construction has no excuse for gaps here.
            let rate = m.failures.len() as f64 / m.files.max(1) as f64;
            assert!(
                rate < 0.01,
                "rails parse failure rate {:.3}% is too high: {:?}",
                rate * 100.0,
                &m.failures[..m.failures.len().min(5)]
            );
        }

        total_files += m.files;
        total_bytes += m.bytes;
        total_ms += m.elapsed_ms;
    }

    let mb = total_bytes as f64 / 1_048_576.0;
    let secs = total_ms as f64 / 1000.0;
    println!(
        "\n  total: {total_files} files, {mb:.1} MB, {total_ms} ms, {:.0} MB/s\n",
        if secs > 0.0 { mb / secs } else { f64::NAN }
    );
    assert!(
        total_files > 0,
        "no Ruby files found under {}",
        root.display()
    );
}
