//! Source discovery and parsing.
//!
//! Walks the repo honouring `.gitignore`, skipping generated and vendored code
//! by default -- a rewrite that "succeeds" by editing `db/schema.rb` is a bug.
//! Parses with Prism (decision D1) in parallel.
//!
//! Phase 1 holds no persistent state: parse, answer, exit (D5, confirmed by
//! Phase 0 measurement (d) -- rails parses in under 200ms, so a cache would be
//! solving a problem that does not exist).

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// Paths skipped unless `--include-vendored` is given.
const EXCLUDED: &[&str] = &[
    "vendor/",
    "node_modules/",
    "db/schema.rb",
    "db/structure.sql",
    "/tmp/",
];

fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED.iter().any(|e| s.contains(e))
}

/// Ruby files under `roots`, gitignore-aware.
///
/// An empty `roots` means the current directory, matching rg.
pub(crate) fn ruby_files(roots: &[String], include_vendored: bool) -> Vec<PathBuf> {
    let roots: Vec<&str> = if roots.is_empty() {
        vec!["."]
    } else {
        roots.iter().map(String::as_str).collect()
    };

    let mut builder = WalkBuilder::new(roots[0]);
    for extra in &roots[1..] {
        builder.add(extra);
    }

    let mut files: Vec<PathBuf> = builder
        .build()
        .filter_map(Result::ok)
        .map(ignore::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|x| x == "rb"))
        .filter(|p| include_vendored || !is_excluded(p))
        .collect();
    files.sort();
    files
}

/// One-based line and column for a byte offset.
pub(crate) fn line_col(source: &[u8], offset: usize) -> (usize, usize) {
    let upto = &source[..offset.min(source.len())];
    let line = upto.iter().filter(|b| **b == b'\n').count() + 1;
    let col = upto
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(offset, |i| offset - i - 1)
        + 1;
    (line, col)
}

/// The source line containing `offset`, trimmed of trailing newline.
pub(crate) fn line_at(source: &[u8], offset: usize) -> String {
    let start = source[..offset.min(source.len())]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let end = source[start..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(source.len(), |i| start + i);
    String::from_utf8_lossy(&source[start..end])
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_are_one_based() {
        let src = b"a\nbb\nccc";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 2), (2, 1));
        assert_eq!(line_col(src, 3), (2, 2));
        assert_eq!(line_col(src, 5), (3, 1));
    }

    #[test]
    fn line_at_returns_the_containing_line() {
        let src = b"first\nsecond\nthird\n";
        assert_eq!(line_at(src, 0), "first");
        assert_eq!(line_at(src, 7), "second");
    }

    #[test]
    fn vendored_paths_are_excluded_by_default() {
        assert!(is_excluded(Path::new("vendor/gems/foo.rb")));
        assert!(is_excluded(Path::new("db/schema.rb")));
        assert!(!is_excluded(Path::new("app/models/account.rb")));
    }

    /// Decision D1 rests on Prism *reporting* what it could not parse, rather
    /// than recovering silently into a plausible-but-wrong tree. Pin both
    /// halves: clean source parses with no diagnostics, broken source yields
    /// diagnostics instead of a confident answer.
    #[test]
    fn prism_reports_parse_errors_rather_than_guessing() {
        let ok = ruby_prism::parse(b"foo(a, b)");
        assert_eq!(ok.errors().count(), 0);

        let broken = ruby_prism::parse(b"def foo(");
        assert!(broken.errors().count() > 0);
    }

    /// The heredoc hazard behind decision D14, pinned as an executable fact:
    /// the string node's own location stops at its opening token, nowhere near
    /// the body three lines down.
    #[test]
    fn heredoc_location_excludes_its_body() {
        let src: &[u8] = b"foo(<<~SQL, b)\n  SELECT 1\nSQL\n";
        let result = ruby_prism::parse(src);
        assert_eq!(result.errors().count(), 0);

        let body_offset = src
            .windows(6)
            .position(|w| w == b"SELECT")
            .expect("fixture contains the heredoc body");

        let node = result.node();
        let program = node.as_program_node().expect("root is a program");
        let statements = program.statements();
        let first = statements.body().iter().next().expect("one statement");
        let call = first.as_call_node().expect("statement is a call");
        let args = call.arguments().expect("call has arguments");
        let heredoc = args.arguments().iter().next().expect("first argument");

        assert!(
            heredoc.location().end_offset() < body_offset,
            "heredoc node location unexpectedly reached its body"
        );
    }

    /// Refutes an earlier design claim that concrete-syntax transformations
    /// are impossible because the trees are identical. The *node types* match,
    /// but Prism retains operator locations, so the spelling is recoverable.
    #[test]
    fn operator_spelling_survives_parsing() {
        for (src, expected) in [("a and b", "and"), ("a && b", "&&")] {
            let result = ruby_prism::parse(src.as_bytes());
            let node = result.node();
            let program = node.as_program_node().expect("program");
            let stmt = program
                .statements()
                .body()
                .iter()
                .next()
                .expect("one statement");
            let and = stmt.as_and_node().expect("an `and` node");
            let loc = and.operator_loc();
            assert_eq!(&src[loc.start_offset()..loc.end_offset()], expected);
        }
    }

    /// Same for hash syntax: the shorthand carries no operator location at all,
    /// the rocket carries one.
    #[test]
    fn hash_rocket_is_distinguishable_from_shorthand() {
        let spellings = ["{ a: 1 }", "{ :a => 1 }"];
        let found: Vec<bool> = spellings
            .iter()
            .map(|src| {
                let result = ruby_prism::parse(src.as_bytes());
                let node = result.node();
                let program = node.as_program_node().expect("program");
                let stmt = program
                    .statements()
                    .body()
                    .iter()
                    .next()
                    .expect("one statement");
                let hash = stmt.as_hash_node().expect("a hash");
                let first = hash.elements().iter().next().expect("one element");
                first
                    .as_assoc_node()
                    .expect("an assoc")
                    .operator_loc()
                    .is_some()
            })
            .collect();

        assert_eq!(
            found,
            vec![false, true],
            "hash spellings were indistinguishable"
        );
    }
}
