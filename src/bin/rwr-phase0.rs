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
    files: usize,
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
}

#[derive(Serialize)]
struct NameReport {
    name: String,
    sites: usize,
    implicit_pct: u32,
    constant_pct: u32,
    distinct_shapes: usize,
}

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
    let per_file: Vec<(u64, bool, Vec<(String, &'static str)>)> = files
        .par_iter()
        .filter_map(|path| {
            let src = std::fs::read(path).ok()?;
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

    let mut hot: Vec<NameReport> = by_name
        .iter()
        .filter(|(_, v)| v.len() >= 25)
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
    hot.sort_by(|a, b| b.sites.cmp(&a.sites));
    hot.truncate(60);

    RepoReport {
        name: root
            .file_name()
            .map_or_else(|| "corpus".into(), |n| n.to_string_lossy().into_owned()),
        files: per_file.len(),
        bytes: per_file.iter().map(|(n, _, _)| n).sum(),
        parse_ms,
        unparsed: per_file.iter().filter(|(_, bad, _)| *bad).count(),
        call_sites: sites,
        distinct_names: by_name.len(),
        receiver_shapes: shapes,
        hot_names: hot,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut label = String::from("unlabelled");
    let mut roots: Vec<PathBuf> = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--label" => label = args.next().unwrap_or_default(),
            "-h" | "--help" => {
                eprintln!(
                    "rwr-phase0 [--label NAME] PATH...\n\n\
                     Emits a JSON report of aggregates only -- counts, timings and\n\
                     per-identifier statistics. No source text or paths are included."
                );
                return;
            }
            _ => roots.push(PathBuf::from(arg)),
        }
    }
    if roots.is_empty() {
        eprintln!("rwr-phase0: give at least one path (try --help)");
        std::process::exit(2);
    }

    let repos: Vec<RepoReport> = roots
        .iter()
        .filter(|r| r.is_dir())
        .map(|r| measure(r))
        .collect();

    let report = Report {
        schema: 1,
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
