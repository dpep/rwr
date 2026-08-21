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
    /// How many method definitions there are, and how their return values
    /// classify. Sizes how much of a return-type index syntax alone can supply,
    /// which is what resolving a chained receiver needs.
    method_definitions: usize,
    return_shapes: HashMap<String, usize>,
    /// The commonest inner calls of a chained receiver, most frequent first.
    chain_inner_names: Vec<(String, usize)>,
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

/// One file's contribution: byte length, whether it failed to parse, the calls
/// it contained, and how each method definition's return value classifies.
type FileMeasurement = (
    u64,
    bool,
    Vec<(String, &'static str)>,
    Vec<&'static str>,
    Vec<String>,
);

/// How a method definition's return value classifies.
///
/// Resolving a chained receiver means knowing what the inner call *returns*, so
/// the question this answers is how much of a return-type index is even
/// derivable from syntax. A method whose last expression is `Widget.new` has an
/// answer; one that ends in a conditional does not, without real inference.
fn return_shape(def: &ruby_prism::DefNode<'_>) -> &'static str {
    let Some(body) = def.body() else {
        return "empty";
    };
    let Some(statements) = body.as_statements_node() else {
        return "other";
    };
    let Some(last) = statements.body().iter().last() else {
        return "empty";
    };
    classify_return(&last)
}

fn classify_return(node: &Node<'_>) -> &'static str {
    match node {
        // `def build; Widget.new; end` -- the answer is written down.
        Node::CallNode { .. } => {
            let Some(call) = node.as_call_node() else {
                return "other";
            };
            if call.name().as_slice() == b"new"
                && matches!(
                    call.receiver(),
                    Some(Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. })
                )
            {
                "constructor"
            } else {
                "call"
            }
        }
        Node::InstanceVariableReadNode { .. } => "ivar",
        // `@foo ||= Widget.new` -- the memoisation idiom, worth counting apart
        // because it is where a constructor hides in a Rails codebase.
        Node::InstanceVariableOperatorWriteNode { .. } => "ivar_memo",
        Node::InstanceVariableOrWriteNode { .. } => {
            let Some(write) = node.as_instance_variable_or_write_node() else {
                return "ivar_memo";
            };
            match classify_return(&write.value()) {
                "constructor" => "ivar_memo_constructor",
                _ => "ivar_memo",
            }
        }
        Node::LocalVariableReadNode { .. } => "local",
        Node::SelfNode { .. } => "self",
        Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. } => "constant",
        Node::StringNode { .. }
        | Node::InterpolatedStringNode { .. }
        | Node::SymbolNode { .. }
        | Node::IntegerNode { .. }
        | Node::FloatNode { .. }
        | Node::ArrayNode { .. }
        | Node::HashNode { .. }
        | Node::TrueNode { .. }
        | Node::FalseNode { .. }
        | Node::NilNode { .. } => "literal",
        Node::IfNode { .. } | Node::UnlessNode { .. } | Node::CaseNode { .. } => "branchy",
        Node::ReturnNode { .. } => {
            let Some(ret) = node.as_return_node() else {
                return "other";
            };
            match ret.arguments().and_then(|a| a.arguments().iter().next()) {
                Some(first) => classify_return(&first),
                None => "literal",
            }
        }
        _ => "other",
    }
}

fn shape(receiver: Option<Node<'_>>) -> &'static str {
    match receiver {
        None => "implicit",
        Some(n) => match n {
            Node::SelfNode { .. } => "self",
            Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. } => "constant",
            Node::LocalVariableReadNode { .. } => "local",
            Node::InstanceVariableReadNode { .. } => "ivar",
            Node::CallNode { .. } => chain_shape(&n),
            _ => "other",
        },
    }
}

/// What kind of chain a chained receiver is.
///
/// "Chained" is the largest unresolved bucket, but it is not one problem. Some
/// chains carry their own answer -- `Widget.new.foo` names the class outright --
/// and others need to know what a method returns, which is a cross-file index.
/// Sizing the two decides how much machinery the bucket is worth.
fn chain_shape(node: &Node<'_>) -> &'static str {
    let Some(call) = node.as_call_node() else {
        return "chained:other";
    };
    let name = call.name();
    let name = name.as_slice();

    // `Widget.new.foo` -- the receiver's type is written right there.
    if name == b"new" {
        return match call.receiver() {
            Some(Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }) => {
                "chained:constructor"
            }
            _ => "chained:other",
        };
    }
    // Identity-ish methods pass their receiver's type straight through.
    if matches!(name, b"freeze" | b"dup" | b"clone" | b"itself" | b"tap") {
        return "chained:identity";
    }
    match call.receiver() {
        None => "chained:implicit",
        Some(Node::SelfNode { .. }) => "chained:self",
        Some(Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }) => "chained:constant",
        Some(Node::InstanceVariableReadNode { .. }) => "chained:ivar",
        Some(Node::LocalVariableReadNode { .. }) => "chained:local",
        Some(Node::CallNode { .. }) => "chained:deeper",
        _ => "chained:other",
    }
}

#[derive(Default)]
struct Calls {
    seen: Vec<(String, &'static str)>,
    /// How each method definition's return value classifies.
    returns: Vec<&'static str>,
    /// The name of the *inner* call of a chained receiver. A bucket dominated
    /// by spec DSL is not the same problem as one dominated by domain methods.
    inner: Vec<String>,
}

impl<'pr> Visit<'pr> for Calls {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        self.seen.push((
            String::from_utf8_lossy(node.name().as_slice()).into_owned(),
            shape(node.receiver()),
        ));
        if let Some(receiver) = node.receiver()
            && let Some(inner) = receiver.as_call_node()
        {
            self.inner
                .push(String::from_utf8_lossy(inner.name().as_slice()).into_owned());
        }
        ruby_prism::visit_call_node(self, node);
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        self.returns.push(return_shape(node));
        ruby_prism::visit_def_node(self, node);
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
                return Some((len, true, Vec::new(), Vec::new(), Vec::new()));
            }
            let mut calls = Calls::default();
            calls.visit(&parsed.node());
            Some((len, false, calls.seen, calls.returns, calls.inner))
        })
        .collect();
    let unreadable = attempted.iter().filter(|m| m.is_none()).count();
    let per_file: Vec<FileMeasurement> = attempted.into_iter().flatten().collect();
    let parse_ms = start.elapsed().as_millis();

    let mut by_name: HashMap<String, Vec<&'static str>> = HashMap::new();
    let mut shapes: HashMap<String, usize> = HashMap::new();
    let mut sites = 0usize;
    let mut returns: HashMap<String, usize> = HashMap::new();
    let mut defs = 0usize;
    let mut inner: HashMap<String, usize> = HashMap::new();
    for (_, _, _, _, names) in &per_file {
        for name in names {
            *inner.entry(name.clone()).or_default() += 1;
        }
    }
    let mut hot_inner: Vec<(String, usize)> = inner.into_iter().collect();
    hot_inner.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    hot_inner.truncate(25);

    for (_, _, _, shapes, _) in &per_file {
        for shape in shapes {
            *returns.entry((*shape).to_string()).or_default() += 1;
            defs += 1;
        }
    }

    for (_, _, calls, _, _) in &per_file {
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
        bytes: per_file.iter().map(|(n, _, _, _, _)| n).sum(),
        parse_ms,
        unparsed: per_file.iter().filter(|(_, bad, _, _, _)| *bad).count(),
        call_sites: sites,
        distinct_names: by_name.len(),
        receiver_shapes: shapes,
        method_definitions: defs,
        return_shapes: returns,
        chain_inner_names: hot_inner,
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
