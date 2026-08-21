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
use std::sync::{Arc, Mutex};

/// Directory names skipped unless `--include-vendored` is given, matched as
/// whole path components rather than substrings.
///
/// Substring matching is a trap: an earlier version listed `/tmp/` to skip a
/// Rails app's tmp directory and silently excluded every file under the system
/// temp directory too. A skipped file that is never reported is precisely the
/// failure this design exists to avoid.
const EXCLUDED_DIRS: &[&str] = &["vendor", "node_modules", "tmp", "log"];

/// Specific generated files, matched by their trailing path.
const EXCLUDED_FILES: &[&str] = &["db/schema.rb", "db/structure.sql"];

fn is_excluded(path: &Path, root: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative
        .components()
        .any(|c| EXCLUDED_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref()))
    {
        return true;
    }
    let shown = relative.to_string_lossy().replace('\\', "/");
    EXCLUDED_FILES.iter().any(|f| shown.ends_with(f))
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

    // Parallel walk: with a literal prefilter making parsing cheap, file
    // discovery becomes the dominant cost on a large repository.
    let found = Arc::new(Mutex::new(Vec::new()));
    let owned_roots: Vec<String> = roots.iter().map(|r| (*r).to_string()).collect();

    builder.build_parallel().run(|| {
        let found = Arc::clone(&found);
        let roots = owned_roots.clone();
        Box::new(move |entry| {
            if let Ok(entry) = entry {
                let path = entry.into_path();
                let keep = path.extension().is_some_and(|x| x == "rb")
                    && (include_vendored
                        || !roots.iter().any(|r| is_excluded(&path, Path::new(r))));
                if keep && let Ok(mut sink) = found.lock() {
                    sink.push(path);
                }
            }
            ignore::WalkState::Continue
        })
    });

    let mut files = Arc::try_unwrap(found)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_default();
    // Sorted so output is deterministic regardless of walk order.
    files.sort();
    files
}

/// A file's bytes, mapped where that avoids a copy.
///
/// The prefilter reads every file and keeps almost none, so copying 81 MB to
/// discard 99% of it is the dominant cost. A mapping is a view: searching it
/// copies nothing, and only the files that survive are materialised.
pub(crate) enum Source {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl Source {
    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            Source::Mapped(m) => m,
            Source::Owned(v) => v,
        }
    }
}

/// Map a file, falling back to a read where mapping is unavailable.
///
/// An empty file cannot be mapped, and a mapping of a file that changes under
/// us would be a correctness hazard -- but rwr writes only after the scan, and
/// writes through the filesystem rather than the mapping.
pub(crate) fn open(path: &Path) -> Source {
    let Ok(file) = std::fs::File::open(path) else {
        return Source::Owned(Vec::new());
    };
    if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
        return Source::Owned(Vec::new());
    }
    // SAFETY: read-only view, and rwr does not modify files during the scan.
    if std::env::var_os("RWR_NO_MMAP").is_some() {
        return Source::Owned(std::fs::read(path).unwrap_or_default());
    }
    match unsafe { memmap2::Mmap::map(&file) } {
        Ok(map) => {
            // The prefilter scans a file end to end when the literal is absent,
            // which is the common case, so tell the kernel to read ahead.
            let _ = map.advise(memmap2::Advice::Sequential);
            Source::Mapped(map)
        }
        Err(_) => Source::Owned(std::fs::read(path).unwrap_or_default()),
    }
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
        let root = Path::new(".");
        assert!(is_excluded(Path::new("./vendor/gems/foo.rb"), root));
        assert!(is_excluded(Path::new("./tmp/cache/x.rb"), root));
        assert!(is_excluded(Path::new("./db/schema.rb"), root));
        assert!(!is_excluded(Path::new("./app/models/account.rb"), root));
    }

    /// The bug this rule replaced: a substring match on `/tmp/` silently
    /// excluded everything under the system temp directory, so files simply
    /// vanished from every search.
    #[test]
    fn exclusions_match_components_relative_to_the_root() {
        let root = Path::new("/var/folders/xyz/T/rwr-fixture");
        let file = Path::new("/var/folders/xyz/T/rwr-fixture/thing.rb");
        assert!(!is_excluded(file, root));
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
