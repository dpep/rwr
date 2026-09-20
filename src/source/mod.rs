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

/// Extensions that hold Ruby source.
///
/// `.rb` is not the whole language: discourse carries 11,854 lines of Ruby in
/// `.rake` files, a Gemfile and a gemspec, none of which rwr opened. A rename
/// silently skipped them *and* the residue report claimed completeness without
/// having read them, which is the worse half (Q11).
const RUBY_EXTENSIONS: &[&str] = &[
    "rb", "rake", "ru", "gemspec", "jbuilder", "jb", "thor", "podspec", "rbw", "arb", "builder",
    "rabl", "opal",
];

/// Files that hold Ruby but are named rather than suffixed.
const RUBY_FILENAMES: &[&str] = &[
    "Rakefile",
    "rakefile",
    "Gemfile",
    "Guardfile",
    "Capfile",
    "Appraisals",
    "Berksfile",
    "Brewfile",
    "Buildfile",
    "Cheffile",
    "Dangerfile",
    "Fastfile",
    "Jarfile",
    "Mavenfile",
    "Podfile",
    "Puppetfile",
    "Schemafile",
    "Snapfile",
    "Steepfile",
    "Thorfile",
    "Vagrantfile",
    ".irbrc",
    ".pryrc",
    ".simplecov",
];

/// Template extensions that embed Ruby rwr does not read.
///
/// Counted rather than parsed. The residue report's whole claim is "here is
/// what I could not account for", and a Rails app keeps a large share of its
/// call sites in ERB and Haml -- so a report that silently omits them
/// over-claims exactly where the product's credibility lives (Q11).
const TEMPLATE_EXTENSIONS: &[&str] = &["erb", "haml", "slim", "rhtml", "erubi", "liquid"];

/// Whether a path is a template embedding Ruby rwr cannot read.
pub(crate) fn is_template(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| TEMPLATE_EXTENSIONS.contains(&e))
}

/// Whether a path holds Ruby source.
///
/// Deliberately narrower than RuboCop's default include list, which also claims
/// `.spec`, `.schema` and `.god` -- extensions that are Ruby in some projects
/// and something else entirely in others. A file rwr cannot parse is skipped,
/// so a wrong guess here is quiet rather than loud, which is the argument for
/// keeping the list to what is unambiguous.
pub(crate) fn is_ruby(path: &Path) -> bool {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RUBY_EXTENSIONS.contains(&e))
    {
        return true;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| RUBY_FILENAMES.contains(&n))
}

/// Drop every root another root already covers.
///
/// Resolved before comparing, because `z.rb` and `./z.rb` are the same file
/// spelled two ways and the walk would otherwise yield both. Compared as paths
/// rather than as strings, so `/a/bc` is not "inside" `/a/b`.
///
/// Duplicates keep the first spelling, so the report names the path the caller
/// wrote first rather than one it picked.
fn prune_contained<'a>(roots: &[&'a str]) -> Vec<&'a str> {
    if roots.len() < 2 {
        return roots.to_vec();
    }
    let real: Vec<PathBuf> = roots
        .iter()
        .map(|r| {
            Path::new(r)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(r))
        })
        .collect();
    let kept: Vec<&str> = roots
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            !real.iter().enumerate().any(|(j, other)| {
                // Dropped by a strict ancestor, or by an identical root
                // earlier in the list -- so one of a duplicated pair lives.
                j != *i && real[*i].starts_with(other) && (real[*i] != *other || j < *i)
            })
        })
        .map(|(_, r)| *r)
        .collect();
    // Nothing can eliminate every root, but a walk over no roots would silently
    // check nothing -- the vacuous green this tool exists to refuse.
    if kept.is_empty() {
        roots.to_vec()
    } else {
        kept
    }
}

/// Ruby files under `roots`, gitignore-aware, and how many templates were
/// walked past.
///
/// An empty `roots` means the current directory, matching rg. The templates are
/// counted in the same pass, because a second walk to answer "what did you not
/// look at" would cost as much as the first.
pub(crate) fn walk(roots: &[String], include_vendored: bool) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let roots: Vec<&str> = if roots.is_empty() {
        vec!["."]
    } else {
        roots.iter().map(String::as_str).collect()
    };

    // Overlapping roots name one tree, not two. `rwr rewrite all w.rb w.rb`
    // reported "rewrote 1 site(s)" twice and `rwr check all z.rb .` counted
    // every file in the repo twice, the suppression audit included -- and
    // `check app/ app/models/` is the same bug wearing a plausible shape.
    //
    // Pruned at the root rather than deduplicated per file: a contained root
    // adds nothing its container does not already walk, so the walker never
    // visits the file twice and there is no per-file syscall on a tree of
    // eleven thousand. Deduplicated rather than refused because the union of
    // two overlapping path sets has one unambiguous answer -- refusal is for
    // ambiguity, and there is none here.
    let roots = prune_contained(&roots);
    let mut builder = WalkBuilder::new(roots[0]);
    for extra in &roots[1..] {
        builder.add(extra);
    }

    // Parallel walk: with a literal prefilter making parsing cheap, file
    // discovery becomes the dominant cost on a large repository.
    let found = Arc::new(Mutex::new(Vec::new()));
    // Kept rather than counted: a template cannot be parsed, but it can still be
    // *searched*, and "here are the three views that mention this name" beats
    // "356 files were not searched" by a wide margin.
    let templates = Arc::new(Mutex::new(Vec::new()));
    let owned_roots: Vec<String> = roots.iter().map(|r| (*r).to_string()).collect();

    builder.build_parallel().run(|| {
        let found = Arc::clone(&found);
        let templates = Arc::clone(&templates);
        let roots = owned_roots.clone();
        Box::new(move |entry| {
            if let Ok(entry) = entry {
                let path = entry.into_path();
                let wanted =
                    !include_vendored && roots.iter().any(|r| is_excluded(&path, Path::new(r)));
                if !wanted {
                    if is_ruby(&path) {
                        if let Ok(mut sink) = found.lock() {
                            sink.push(path);
                        }
                    } else if is_template(&path)
                        && let Ok(mut sink) = templates.lock()
                    {
                        sink.push(path);
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    let mut files = Arc::try_unwrap(found)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_default();
    let mut templates = Arc::try_unwrap(templates)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_default();
    // Sorted so output is deterministic regardless of walk order.
    files.sort();
    templates.sort();
    (files, templates)
}

/// Whole-identifier occurrences of `needle` in `haystack`, as byte offsets.
///
/// The lexical fallback for files rwr cannot parse. A substring match would
/// report `display_names` for `display_name`, which is the sloppiness that
/// makes people stop reading a report.
pub(crate) fn identifier_offsets(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    memchr::memmem::find_iter(haystack, needle)
        .filter(|at| {
            let before = at.checked_sub(1).is_none_or(|i| !is_word(haystack[i]));
            let after = haystack.get(at + needle.len()).is_none_or(|b| !is_word(*b));
            before && after
        })
        .collect()
}

/// A file's bytes, mapped where that avoids a copy.
///
/// The prefilter reads every file and keeps almost none, so copying 81 MB to
/// discard 99% of it is the dominant cost. A mapping is a view: searching it
/// copies nothing, and only the files that survive are materialised.
pub(crate) enum Source {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
    /// The file could not be opened or read. Distinct from an empty one: both
    /// yield zero bytes and mean opposite things, and collapsing them made an
    /// unreadable file answer exactly as a clean one does.
    Unreadable,
}

impl Source {
    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            Source::Mapped(m) => m,
            Source::Owned(v) => v,
            Source::Unreadable => &[],
        }
    }

    pub(crate) fn unreadable(&self) -> bool {
        matches!(self, Source::Unreadable)
    }
}

/// Map a file, falling back to a read where mapping is unavailable.
///
/// An empty file cannot be mapped, and a mapping of a file that changes under
/// us would be a correctness hazard -- but rwr writes only after the scan, and
/// writes through the filesystem rather than the mapping.
pub(crate) fn open(path: &Path) -> Source {
    let Ok(file) = std::fs::File::open(path) else {
        return Source::Unreadable;
    };
    if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
        return Source::Owned(Vec::new());
    }
    // SAFETY: read-only view, and rwr does not modify files during the scan.
    if std::env::var_os("RWR_NO_MMAP").is_some() {
        return read_owned(path);
    }
    match unsafe { memmap2::Mmap::map(&file) } {
        Ok(map) => {
            // The prefilter scans a file end to end when the literal is absent,
            // which is the common case, so tell the kernel to read ahead.
            let _ = map.advise(memmap2::Advice::Sequential);
            Source::Mapped(map)
        }
        Err(_) => read_owned(path),
    }
}

fn read_owned(path: &Path) -> Source {
    std::fs::read(path).map_or(Source::Unreadable, Source::Owned)
}

/// One-based line and column for a byte offset.
///
/// The column counts **characters**, which is the contract every consumer reads
/// (`coordinate_conventions`: `columns: "1-based-chars"`). Counted as
/// non-continuation bytes rather than by decoding: one pass over the current
/// line, no UTF-8 validation, no allocation, and nothing to panic on in a file
/// that is not valid UTF-8.
pub(crate) fn line_col(source: &[u8], offset: usize) -> (usize, usize) {
    let upto = &source[..offset.min(source.len())];
    let line = upto.iter().filter(|b| **b == b'\n').count() + 1;
    let start = upto.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    // 0b10xxxxxx is the tail of a character already counted at its lead byte.
    let col = upto[start..].iter().filter(|b| *b & 0xC0 != 0x80).count() + 1;
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

/// How much of a collapsed multi-line site to show before cutting it.
///
/// Only multi-line sites are cut. A single line is shown whole however long it
/// is, exactly as it always was.
const SPAN_CHARS: usize = 120;

/// The source a site occupies, rendered as one line.
///
/// A site that starts on its own line reduces to its first line's worth of
/// text, which for a leading-dot chain is the bare receiver -- `dns`, where
/// the site is `dns.getresources(...).to_a.map { |e| e.exchange.to_s }`. A
/// finding list is read by skimming, and that is not skimmable.
///
/// One line still, because `text` sits in a `file:line:col: text` column that a
/// pipe splits on newlines. A site that spans lines is joined at the line
/// breaks, with the indentation that follows one dropped, and cut at
/// [`SPAN_CHARS`]. A site on a single line takes the whole line, unchanged and
/// uncut -- the common case is byte-identical to reading the line directly.
pub(crate) fn span_text(source: &[u8], start: usize, end: usize) -> String {
    let start = start.min(source.len());
    let end = end.clamp(start, source.len());
    if !source[start..end].contains(&b'\n') {
        return line_at(source, start);
    }

    let span = &source[start..end];
    let mut out: Vec<u8> = Vec::with_capacity(span.len());
    let mut i = 0;
    while i < span.len() {
        if span[i] != b'\n' {
            out.push(span[i]);
            i += 1;
            continue;
        }
        while i < span.len() && span[i].is_ascii_whitespace() {
            i += 1;
        }
        while matches!(out.last(), Some(b' ' | b'\t')) {
            out.pop();
        }
        // A joining space would read as a typo where the break sits inside an
        // expression: `dns .getresources`, `foo( bar`.
        let tight = span.get(i).is_some_and(|n| b".,)]}".contains(n))
            || matches!(out.last(), Some(b'(' | b'['));
        if !tight {
            out.push(b' ');
        }
    }

    let out = String::from_utf8_lossy(&out).trim().to_string();
    match out.char_indices().nth(SPAN_CHARS) {
        Some((cut, _)) => format!("{}…", &out[..cut]),
        None => out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roots that exist on disk, since the resolved paths are what is compared
    /// and a test over names that do not exist only ever exercises the
    /// fallback.
    fn in_a_tree<T>(dirs: &[&str], f: impl FnOnce(&std::path::Path) -> T) -> T {
        let tmp = tempfile::tempdir().expect("temp dir");
        for dir in dirs {
            std::fs::create_dir_all(tmp.path().join(dir)).expect("mkdir");
        }
        f(tmp.path())
    }

    /// A sibling whose name merely *starts with* another root's is not inside
    /// it. Comparing the spellings as strings would drop `app/modelsx` for
    /// `app/models`, and a root dropped by mistake is a tree nobody checked --
    /// silent, because a pruned root is indistinguishable from an empty one.
    #[test]
    fn a_name_prefix_is_not_containment() {
        in_a_tree(&["app/models", "app/modelsx"], |root| {
            let (a, b) = (
                root.join("app/models").display().to_string(),
                root.join("app/modelsx").display().to_string(),
            );
            assert_eq!(prune_contained(&[&a, &b]), vec![a.as_str(), b.as_str()]);
        });
    }

    /// Unrelated roots are all kept; a strict ancestor absorbs its descendant,
    /// whichever order they were written in.
    #[test]
    fn only_a_contained_root_is_dropped() {
        in_a_tree(&["app/models", "lib"], |root| {
            let (app, models, lib) = (
                root.join("app").display().to_string(),
                root.join("app/models").display().to_string(),
                root.join("lib").display().to_string(),
            );
            assert_eq!(
                prune_contained(&[&app, &lib]),
                vec![app.as_str(), lib.as_str()]
            );
            assert_eq!(prune_contained(&[&app, &models]), vec![app.as_str()]);
            assert_eq!(prune_contained(&[&models, &app]), vec![app.as_str()]);
        });
    }

    /// The two spellings of one file, which is how `rwr check z.rb .` counted
    /// the whole repo twice.
    #[test]
    fn the_same_place_spelled_two_ways_is_one_root() {
        in_a_tree(&["app"], |root| {
            let (plain, dotted) = (
                root.join("app").display().to_string(),
                root.join("./app").display().to_string(),
            );
            assert_eq!(prune_contained(&[&plain, &dotted]), vec![plain.as_str()]);
        });
    }

    /// One of a duplicated pair survives, and it is the first spelling -- a
    /// filter that dropped both would leave the walk with nothing to do.
    #[test]
    fn a_repeated_root_survives_once() {
        in_a_tree(&["app"], |root| {
            let app = root.join("app").display().to_string();
            assert_eq!(prune_contained(&[&app, &app, &app]), vec![app.as_str()]);
        });
    }

    /// `.rb` is not the whole language. Discourse keeps 11,854 lines of Ruby in
    /// `.rake` files, a Gemfile and a gemspec; a rename skipped every one, and
    /// the residue report claimed completeness without having read them.
    #[test]
    fn ruby_is_more_than_the_rb_extension() {
        for name in [
            "a.rb",
            "lib/tasks/db.rake",
            "config.ru",
            "thing.gemspec",
            "Rakefile",
            "Gemfile",
            "Vagrantfile",
            "views/show.jbuilder",
        ] {
            assert!(is_ruby(Path::new(name)), "{name}");
        }
        for name in ["a.py", "README.md", "schema.sql", "app/views/show.html.erb"] {
            assert!(!is_ruby(Path::new(name)), "{name}");
        }
    }

    /// A template embeds Ruby but is not Ruby, so it is counted rather than
    /// parsed -- the residue report's claim has to say what it did not read.
    #[test]
    fn templates_are_recognised_but_not_claimed_as_ruby() {
        for name in ["show.html.erb", "index.haml", "form.slim"] {
            assert!(is_template(Path::new(name)), "{name}");
            assert!(!is_ruby(Path::new(name)), "{name}");
        }
        // A jbuilder view *is* Ruby, so it belongs to the other list.
        assert!(!is_template(Path::new("show.json.jbuilder")));
    }

    #[test]
    fn line_and_column_are_one_based() {
        let src = b"a\nbb\nccc";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 2), (2, 1));
        assert_eq!(line_col(src, 3), (2, 2));
        assert_eq!(line_col(src, 5), (3, 1));
    }

    /// The column counts characters, not bytes. Every consumer -- the text
    /// report, `-j`, an editor jumping to the site -- gets a column past the
    /// match otherwise, on any line carrying an accented name, an i18n string
    /// or a pasted curly quote.
    #[test]
    fn the_column_counts_characters_not_bytes() {
        let two_byte = "x = \"caf\u{e9}\"; Foo".as_bytes();
        assert_eq!(line_col(two_byte, two_byte.len() - 3), (1, 13));
        let four_byte = "x = \"\u{1f389}\"; Foo".as_bytes();
        assert_eq!(line_col(four_byte, four_byte.len() - 3), (1, 10));
    }

    /// The byte count restarts at each newline, so a multi-byte character on an
    /// earlier line must not follow the count onto this one.
    #[test]
    fn a_multi_byte_character_on_an_earlier_line_does_not_leak() {
        let src = "\u{e9}\u{e9}\u{e9}\nab".as_bytes();
        assert_eq!(line_col(src, src.len() - 1), (2, 2));
    }

    #[test]
    fn line_at_returns_the_containing_line() {
        let src = b"first\nsecond\nthird\n";
        assert_eq!(line_at(src, 0), "first");
        assert_eq!(line_at(src, 7), "second");
    }

    /// A site on one line is reported exactly as reading that line was, whole
    /// and uncut, including what sits beside it.
    #[test]
    fn span_text_of_a_single_line_site_is_its_whole_line() {
        let src = b"  total = xs.size\n";
        let (start, end) = (10, 17);
        assert_eq!(span_text(src, start, end), "  total = xs.size");
        assert_eq!(span_text(src, start, end), line_at(src, start));
    }

    /// The defect: a leading-dot chain reported as its first line is the bare
    /// receiver, and a receiver is not a finding anyone can act on.
    #[test]
    fn span_text_joins_a_leading_dot_chain() {
        let src = b"def go\n  orders\n    .each { |o| puts o.customer.name }\nend\n";
        let start = 9;
        let end = src.len() - 5;
        assert_eq!(line_at(src, start), "  orders");
        assert_eq!(
            span_text(src, start, end),
            "orders.each { |o| puts o.customer.name }"
        );
    }

    /// A break inside an argument list joins without a space too, and one
    /// between two words keeps it.
    #[test]
    fn span_text_joins_without_inventing_spaces() {
        let src = b"foo(\n  a,\n  b\n) do |x|\n  x\nend\n";
        assert_eq!(span_text(src, 0, src.len() - 1), "foo(a, b) do |x| x end");
    }

    /// One line, always: `text` sits in a column a pipe splits on newlines, so
    /// a long block is cut rather than allowed to bury the list.
    #[test]
    fn span_text_cuts_a_long_block() {
        let body = "  puts :x\n".repeat(40);
        let src = format!("xs.each do |x|\n{body}end\n");
        let out = span_text(src.as_bytes(), 0, src.len() - 1);
        assert!(!out.contains('\n'), "{out}");
        assert!(out.ends_with('…'), "{out}");
        assert_eq!(out.chars().count(), SPAN_CHARS + 1);
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
