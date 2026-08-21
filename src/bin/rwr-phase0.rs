//! Phase 0 data collection, for corpora that cannot leave the machine they
//! live on.
//!
//! The interesting Phase 0 measurements want a large private codebase, which
//! rules out running them here. This emits a JSON report carrying only
//! *aggregates* -- counts, timings, and per-identifier statistics -- and never
//! source text, file contents, or paths below the corpus root, so the result
//! can be shared without leaking the code it measured.
//!
//! ```sh
//! rwr-phase0 --label laptop-b ~/src/monolith > phase0-laptop-b.json
//! ```

use ignore::WalkBuilder;
use rayon::prelude::*;
use ruby_prism::{Node, Visit};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
struct Report {
    schema: u32,
    rwr_version: &'static str,
    label: String,
    generated_at_unix: u64,
    threads: usize,
    repos: Vec<RepoReport>,
}

#[derive(Serialize)]
struct RepoReport {
    /// The corpus directory's own name only -- never its full path.
    name: String,
    /// Ruby files walked. `files_measured` is how many were actually read; the
    /// two differ when a file cannot be opened, and a report that showed only
    /// the second made those disappear.
    files: usize,
    files_measured: usize,
    files_unreadable: usize,
    bytes: u64,
    parse_ms: u128,
    unparsed: usize,
    call_sites: usize,
    distinct_names: usize,
    receiver_shapes: HashMap<String, usize>,
    /// The names with the most call sites, with how pinned-down their receivers
    /// are. This is the input to measurement (b): a name whose sites carry many
    /// receiver shapes is one a bare-name rename damages.
    hot_names: Vec<NameReport>,
    /// What `hot_names` left out, so a truncated list cannot be read as a whole
    /// one. Names below `hot_names_min_sites` are not counted as omitted --
    /// they were never candidates.
    hot_names_omitted: usize,
    hot_names_min_sites: usize,
}

#[derive(Serialize)]
struct NameReport {
    name: String,
    sites: usize,
    implicit_pct: u32,
    constant_pct: u32,
    distinct_shapes: usize,
}

/// One file's contribution: byte length, whether it failed to parse, and the
/// calls it contained.
type FileMeasurement = (u64, bool, Vec<(String, &'static str)>);

fn shape(receiver: Option<Node<'_>>) -> &'static str {
    match receiver {
        None => "implicit",
        Some(n) => match n {
            Node::SelfNode { .. } => "self",
            Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. } => "constant",
            Node::LocalVariableReadNode { .. } => "local",
            Node::InstanceVariableReadNode { .. } => "ivar",
            Node::CallNode { .. } => "chained",
            _ => "other",
        },
    }
}

#[derive(Default)]
struct Calls {
    seen: Vec<(String, &'static str)>,
}

impl<'pr> Visit<'pr> for Calls {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        self.seen.push((
            String::from_utf8_lossy(node.name().as_slice()).into_owned(),
            shape(node.receiver()),
        ));
        ruby_prism::visit_call_node(self, node);
    }
}

fn measure(root: &Path) -> RepoReport {
    let files: Vec<PathBuf> = WalkBuilder::new(root)
        .hidden(false)
        .build()
        .filter_map(Result::ok)
        .map(ignore::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|x| x == "rb"))
        .collect();

    let start = Instant::now();
    let attempted: Vec<Option<FileMeasurement>> = files
        .par_iter()
        .map(|path| {
            // A file that cannot be read is counted, not skipped: dropping it
            // shrank the denominator silently, which is the one thing an
            // aggregate report must never do.
            let Ok(src) = std::fs::read(path) else {
                return None;
            };
            let len = src.len() as u64;
            let parsed = ruby_prism::parse(&src);
            if parsed.errors().count() > 0 {
                return Some((len, true, Vec::new()));
            }
            let mut calls = Calls::default();
            calls.visit(&parsed.node());
            Some((len, false, calls.seen))
        })
        .collect();
    let unreadable = attempted.iter().filter(|m| m.is_none()).count();
    let per_file: Vec<FileMeasurement> = attempted.into_iter().flatten().collect();
    let parse_ms = start.elapsed().as_millis();

    let mut by_name: HashMap<String, Vec<&'static str>> = HashMap::new();
    let mut shapes: HashMap<String, usize> = HashMap::new();
    let mut sites = 0usize;
    for (_, _, calls) in &per_file {
        for (name, sh) in calls {
            by_name.entry(name.clone()).or_default().push(sh);
            *shapes.entry((*sh).to_string()).or_default() += 1;
            sites += 1;
        }
    }

    const MIN_SITES: usize = 25;
    const SHOWN: usize = 60;
    let mut hot: Vec<NameReport> = by_name
        .iter()
        .filter(|(_, v)| v.len() >= MIN_SITES)
        .map(|(name, v)| {
            let pct = |what: &str| {
                u32::try_from(v.iter().filter(|s| **s == what).count() * 100 / v.len())
                    .unwrap_or(u32::MAX)
            };
            NameReport {
                name: name.clone(),
                sites: v.len(),
                implicit_pct: pct("implicit"),
                constant_pct: pct("constant"),
                distinct_shapes: v.iter().collect::<std::collections::HashSet<_>>().len(),
            }
        })
        .collect();
    hot.sort_by_key(|n| std::cmp::Reverse(n.sites));
    let omitted = hot.len().saturating_sub(SHOWN);
    hot.truncate(SHOWN);

    RepoReport {
        name: repo_name(root),
        files: files.len(),
        files_measured: per_file.len(),
        files_unreadable: unreadable,
        bytes: per_file.iter().map(|(n, _, _)| n).sum(),
        parse_ms,
        unparsed: per_file.iter().filter(|(_, bad, _)| *bad).count(),
        call_sites: sites,
        distinct_names: by_name.len(),
        receiver_shapes: shapes,
        hot_names: hot,
        hot_names_omitted: omitted,
        hot_names_min_sites: MIN_SITES,
    }
}

/// The corpus directory's own name, and never more of the path than that.
///
/// `.` and `..` have no file name of their own, so they are resolved first --
/// a run labelled `discourse` that reported its repo as `corpus` was naming the
/// fallback rather than the corpus.
fn repo_name(root: &Path) -> String {
    let resolved = root.canonicalize();
    let path = resolved.as_deref().unwrap_or(root);
    path.file_name()
        .map_or_else(|| "corpus".into(), |n| n.to_string_lossy().into_owned())
}

const USAGE: &str = "rwr-phase0 [--label NAME] PATH...\n\n\
     Emits a JSON report of aggregates only -- counts, timings and\n\
     per-identifier statistics. No source text or paths are included.";

fn main() {
    let mut args = std::env::args().skip(1);
    let mut label = String::from("unlabelled");
    let mut roots: Vec<PathBuf> = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--label" => match args.next() {
                Some(value) => label = value,
                None => {
                    eprintln!("rwr-phase0: --label needs a name");
                    std::process::exit(2);
                }
            },
            "-V" | "--version" => {
                println!("rwr-phase0 {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return;
            }
            // An unrecognised flag used to be taken as a *path*, which then
            // failed the is-a-directory test and vanished -- so a typo produced
            // a clean-looking report measuring nothing.
            other if other.starts_with('-') => {
                eprintln!("rwr-phase0: unknown option {other}\n\n{USAGE}");
                std::process::exit(2);
            }
            other => roots.push(PathBuf::from(other)),
        }
    }
    if roots.is_empty() {
        eprintln!("rwr-phase0: give at least one path (try --help)");
        std::process::exit(2);
    }

    // Refuse rather than measure nothing. A path that is not a directory --
    // an unexpanded `~`, a typo, a file -- silently produced `"repos": []`,
    // which reads exactly like a corpus with no Ruby in it.
    let missing: Vec<&PathBuf> = roots.iter().filter(|r| !r.is_dir()).collect();
    if !missing.is_empty() {
        for path in &missing {
            eprintln!("rwr-phase0: not a directory: {}", path.display());
        }
        std::process::exit(2);
    }

    let repos: Vec<RepoReport> = roots.iter().map(|r| measure(r)).collect();

    let report = Report {
        // 2: `files` counts files *walked* where it used to count files read,
        // and the report carries `files_measured`, `files_unreadable` and the
        // `hot_names` cap alongside it.
        schema: 2,
        rwr_version: env!("CARGO_PKG_VERSION"),
        label,
        generated_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        threads: rayon::current_num_threads(),
        repos,
    };

    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("rwr-phase0: {e}");
            std::process::exit(2);
        }
    }
}
