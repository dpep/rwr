//! The class hierarchy (D51).
//!
//! Renaming `Account#display_name` must reach `premium.display_name` where
//! `Premium < Account`, and must also rename `Premium`'s override -- otherwise
//! the rewrite ships a `NoMethodError`, which it demonstrably did before this
//! existed.
//!
//! This is the cross-file index Phase 1 deliberately avoided. It is affordable
//! because Phase 0 measurement (d) found a full rails parse takes under 200ms,
//! so the hierarchy is rebuilt per run rather than persisted -- no cache, no
//! invalidation, no staleness, and D5 still holds.

use crate::pattern::generated;
use ignore::WalkBuilder;
use rayon::prelude::*;
use ruby_prism::Node;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Superclass links, keyed by class name.
#[derive(Debug, Default, Clone)]
pub(crate) struct Hierarchy {
    superclass: HashMap<String, String>,
}

/// The name a constant-ish node denotes, ignoring how it was reached.
fn constant_name(node: &Node<'_>) -> Option<String> {
    let bytes = match node {
        Node::ConstantReadNode { .. } => node.as_constant_read_node()?.name().as_slice().to_vec(),
        Node::ConstantPathNode { .. } => node.as_constant_path_node()?.name()?.as_slice().to_vec(),
        _ => return None,
    };
    String::from_utf8(bytes).ok()
}

/// Collect `class X < Y` pairs from one tree.
fn links(root: &Node<'_>, out: &mut Vec<(String, String)>) {
    let mut stack = vec![generated::dup(root)];
    while let Some(node) = stack.pop() {
        if let Node::ClassNode { .. } = node
            && let Some(class) = node.as_class_node()
            && let Some(parent) = class.superclass()
            && let Some(parent) = constant_name(&parent)
            && let Ok(name) = String::from_utf8(class.name().as_slice().to_vec())
        {
            out.push((name, parent));
        }
        stack.extend(generated::children(&node));
    }
}

impl Hierarchy {
    /// Build from every Ruby file under `roots`.
    pub(crate) fn build(roots: &[String]) -> Self {
        let roots: Vec<&str> = if roots.is_empty() {
            vec!["."]
        } else {
            roots.iter().map(String::as_str).collect()
        };
        let mut builder = WalkBuilder::new(roots[0]);
        for extra in &roots[1..] {
            builder.add(extra);
        }
        let files: Vec<PathBuf> = builder
            .build()
            .filter_map(Result::ok)
            .map(ignore::DirEntry::into_path)
            .filter(|p| p.extension().is_some_and(|x| x == "rb"))
            .collect();
        Self::from_files(&files)
    }

    pub(crate) fn from_files(files: &[PathBuf]) -> Self {
        let pairs: Vec<(String, String)> = files
            .par_iter()
            .filter_map(|path| {
                let src = std::fs::read(path).ok()?;
                let parsed = ruby_prism::parse(&src);
                if parsed.errors().count() > 0 {
                    return None;
                }
                let mut found = Vec::new();
                links(&parsed.node(), &mut found);
                Some(found)
            })
            .flatten()
            .collect();

        Hierarchy {
            superclass: pairs.into_iter().collect(),
        }
    }

    /// Whether `class` is `ancestor` or descends from it.
    ///
    /// Guards against a cycle, which valid Ruby cannot express but a
    /// half-written file can.
    pub(crate) fn descends_from(&self, class: &str, ancestor: &str) -> bool {
        let mut current = class;
        let mut seen = HashSet::new();
        loop {
            if current == ancestor {
                return true;
            }
            if !seen.insert(current.to_string()) {
                return false;
            }
            match self.superclass.get(current) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    /// Every known class that is `ancestor` or descends from it.
    pub(crate) fn descendants_of(&self, ancestor: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .superclass
            .keys()
            .filter(|c| self.descends_from(c, ancestor))
            .cloned()
            .collect();
        if !out.iter().any(|c| c == ancestor) {
            out.push(ancestor.to_string());
        }
        out.sort();
        out
    }

    /// Build from a single snippet. Test-only, and shared across modules so a
    /// matcher test can exercise real descent rather than an empty hierarchy.
    #[cfg(test)]
    pub(crate) fn from_source(source: &str) -> Self {
        let parsed = ruby_prism::parse(source.as_bytes());
        let mut found = Vec::new();
        links(&parsed.node(), &mut found);
        Hierarchy {
            superclass: found.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_descends_from_itself() {
        let h = Hierarchy::from_source("class A; end");
        assert!(h.descends_from("A", "A"));
    }

    #[test]
    fn descent_is_transitive() {
        let h = Hierarchy::from_source("class A; end\nclass B < A; end\nclass C < B; end");
        assert!(h.descends_from("C", "A"));
        assert!(h.descends_from("B", "A"));
        assert!(!h.descends_from("A", "C"));
    }

    /// A namespaced superclass resolves by its final name, since that is what
    /// a `type:` constraint names.
    #[test]
    fn namespaced_superclasses_resolve_by_name() {
        let h = Hierarchy::from_source("class Premium < Billing::Account; end");
        assert!(h.descends_from("Premium", "Account"));
    }

    #[test]
    fn descendants_include_the_ancestor_itself() {
        let h = Hierarchy::from_source("class A; end\nclass B < A; end");
        assert_eq!(
            h.descendants_of("A"),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    /// Valid Ruby cannot express a cycle, but a half-written file can, and the
    /// walk must not hang on one.
    #[test]
    fn a_cycle_terminates() {
        let h = Hierarchy::from_source("class A < B; end\nclass B < A; end");
        assert!(!h.descends_from("A", "Nowhere"));
    }
}
