//! Phase 0 measurement (b): how bad is bare-name matching, really?
//!
//! D6 pulled the symbol index forward to Phase 2 on the *asserted* premise that
//! bare `foo(...)` matching has a false-positive rate high enough to make the
//! refusal contract fire constantly. The staff-engineer review flagged that as
//! asserted-not-measured. This measures it.
//!
//! No matcher needed: collect every method call in a corpus, group by name, and
//! classify each call site's receiver. A name whose call sites carry many
//! distinct receiver shapes is a name where renaming by bare identifier does
//! collateral damage — which is what Ruby LSP's `ReferenceFinder` does today.
//!
//! ```sh
//! cargo test --test phase0_receivers -- --nocapture
//! ```

use ignore::WalkBuilder;
use rayon::prelude::*;
use ruby_prism::{Node, Visit};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The shape of a call's receiver, as far as syntax alone can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Receiver {
    /// `foo` — implicit self. Resolvable only by knowing the enclosing class.
    Implicit,
    /// `self.foo`
    SelfExplicit,
    /// `Foo.bar` / `Foo::Bar.baz` — statically known, the easy case.
    Constant,
    /// `x.foo` where x is a local. Resolvable by local inference.
    Local,
    /// `@x.foo`
    Ivar,
    /// `a.b.foo` — a chain; needs the chain's type.
    Chained,
    /// Literals, blocks, and everything else.
    Other,
}

impl Receiver {
    fn of(node: Option<Node<'_>>) -> Self {
        match node {
            None => Receiver::Implicit,
            Some(n) => match n {
                Node::SelfNode { .. } => Receiver::SelfExplicit,
                Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. } => Receiver::Constant,
                Node::LocalVariableReadNode { .. } => Receiver::Local,
                Node::InstanceVariableReadNode { .. } => Receiver::Ivar,
                Node::CallNode { .. } => Receiver::Chained,
                _ => Receiver::Other,
            },
        }
    }
}

#[derive(Default)]
struct Collector {
    calls: Vec<(String, Receiver)>,
}

impl<'pr> Visit<'pr> for Collector {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        let name = String::from_utf8_lossy(node.name().as_slice()).into_owned();
        self.calls.push((name, Receiver::of(node.receiver())));
        ruby_prism::visit_call_node(self, node);
    }
}

fn corpus_root() -> PathBuf {
    std::env::var("RWR_CORPUS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("code/lib/ruby")
        })
}

fn collect(root: &Path) -> Vec<(String, Receiver)> {
    let files: Vec<PathBuf> = WalkBuilder::new(root)
        .hidden(false)
        .build()
        .filter_map(Result::ok)
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|x| x == "rb"))
        .collect();

    files
        .par_iter()
        .filter_map(|path| {
            let src = std::fs::read(path).ok()?;
            let result = ruby_prism::parse(&src);
            if result.errors().count() > 0 {
                return None;
            }
            let mut c = Collector::default();
            c.visit(&result.node());
            Some(c.calls)
        })
        .flatten()
        .collect()
}

#[test]
fn bare_name_match_collateral() {
    let root = corpus_root().join("rails");
    if !root.is_dir() {
        eprintln!("skipping: no rails corpus at {}", root.display());
        return;
    }

    let calls = collect(&root);
    assert!(!calls.is_empty(), "collected no calls");

    let mut by_name: HashMap<&str, Vec<Receiver>> = HashMap::new();
    for (name, recv) in &calls {
        by_name.entry(name.as_str()).or_default().push(*recv);
    }

    println!("\n  Phase 0 (b) — bare-name match collateral, rails");
    println!(
        "  {} call sites, {} distinct method names\n",
        calls.len(),
        by_name.len()
    );

    // Names the case studies singled out: the likeliest migration targets are
    // also the ones a bare-name match damages most.
    let watch = [
        "create", "update", "call", "perform", "save", "name", "id", "process", "build", "run",
        "execute", "value", "type", "key", "format",
    ];

    println!(
        "  {:<12} {:>8} {:>10} {:>10} {:>9}",
        "name", "sites", "implicit", "constant", "shapes"
    );
    for name in watch {
        let Some(sites) = by_name.get(name) else {
            continue;
        };
        let implicit = sites.iter().filter(|r| **r == Receiver::Implicit).count();
        let constant = sites.iter().filter(|r| **r == Receiver::Constant).count();
        let shapes = sites.iter().collect::<std::collections::HashSet<_>>().len();
        println!(
            "  {:<12} {:>8} {:>9.0}% {:>9.0}% {:>9}",
            name,
            sites.len(),
            100.0 * implicit as f64 / sites.len() as f64,
            100.0 * constant as f64 / sites.len() as f64,
            shapes
        );
    }

    // Aggregate: how much of the call graph is statically pinned by syntax alone?
    let total = calls.len() as f64;
    let mut dist: Vec<(Receiver, usize)> = {
        let mut m: HashMap<Receiver, usize> = HashMap::new();
        for (_, r) in &calls {
            *m.entry(*r).or_default() += 1;
        }
        m.into_iter().collect()
    };
    dist.sort_by_key(|(r, _)| *r);

    println!("\n  receiver shapes across all {} call sites:", calls.len());
    for (recv, n) in &dist {
        println!(
            "    {:<14} {:>8}  {:>5.1}%",
            format!("{recv:?}"),
            n,
            100.0 * *n as f64 / total
        );
    }
    println!();
}
