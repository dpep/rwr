//! Skipping files that cannot possibly match.
//!
//! rwr's scaling story is not "parse faster" but **parse fewer**. A pattern
//! naming `autoload_paths` cannot match a file whose bytes do not contain that
//! text, and on rails that rules out 3,229 of 3,249 files -- so the cost of a
//! query tracks how many files mention the identifier, not how large the
//! repository is.
//!
//! This is why D5's "no persistence" still holds at scale: a cache would make
//! re-parsing cheap, and a prefilter makes it unnecessary. No cache to
//! invalidate, no staleness, no coherence surface.
//!
//! **Conservative by construction.** A file is skipped only when it provably
//! cannot contribute, and a pattern with no literal text at all (`$A.$B`)
//! filters nothing.

use super::metavar;
use super::prepare::Prepared;
use crate::residue;
use ruby_prism::Node;

/// Identifiers a pattern requires the source to contain.
///
/// Extracted from the pattern *before* substitution, so placeholders are
/// excluded -- they stand for whatever they matched and constrain nothing.
pub(crate) fn required(pattern: &str) -> Vec<String> {
    let metavars = metavar::scan(pattern);
    let covered = |at: usize| metavars.iter().any(|m| at >= m.start && at < m.end);

    let bytes = pattern.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_start(bytes[i]) || covered(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_body(bytes[i]) {
            i += 1;
        }
        // Ruby method names may end in `?` or `!`, and those characters are part
        // of the text a matching file must contain.
        if i < bytes.len() && (bytes[i] == b'?' || bytes[i] == b'!') {
            i += 1;
        }
        if covered(start) {
            continue;
        }
        let word = pattern[start..i].to_string();
        if !out.contains(&word) {
            out.push(word);
        }
    }
    out
}

/// As [`required`], from the prepared form rather than the pattern text.
///
/// Substitution copies the pattern verbatim except at metavariable spans, so
/// dropping the placeholders leaves exactly what `required` extracts from the
/// original -- pinned by `substitution_does_not_change_the_required_literals`.
/// Deriving it here is what lets [`Filter::for_pattern`] take the pattern alone.
fn required_of(prepared: &Prepared) -> Vec<String> {
    required(&prepared.source)
        .into_iter()
        .filter(|word| !prepared.bindings.contains_key(word))
        .collect()
}

fn is_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_body(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// A compiled set of searchers, built once per run rather than per file.
///
/// The prefilter reads every file in the repository, so this search -- not
/// parsing -- is the scan's dominant cost. `memchr`'s SIMD `memmem` is
/// substantially faster than a naive window scan, and building the `Finder`
/// once matters because it precomputes a skip table.
pub(crate) struct Filter {
    required: Vec<memchr::memmem::Finder<'static>>,
    residue: Vec<memchr::memmem::Finder<'static>>,
}

impl Filter {
    /// Both literal sets derived from the one pattern, so neither can be
    /// forgotten.
    ///
    /// They were two arguments once, supplied independently as data, and
    /// `Engine::new` passed `&[]` for the residue side of every rule it built.
    /// That is well typed, silent, and indistinguishable from a pattern that
    /// anchors on nothing -- and it cost the blind-spot report on every file
    /// the required literals alone could not admit.
    pub(crate) fn for_pattern(root: &Node<'_>, prepared: &Prepared) -> Self {
        Filter::new(&required_of(prepared), &residue::reach(root, prepared))
    }

    /// The two sets supplied separately, which is how the residue side came to
    /// be forgotten. `cli::cmd_find` is the last caller; fold it into
    /// [`Filter::for_pattern`] and this can go `#[cfg(test)]`.
    pub(crate) fn new(required: &[String], residue: &[Vec<u8>]) -> Self {
        Filter {
            required: required
                .iter()
                .map(|r| memchr::memmem::Finder::new(r.as_bytes()).into_owned())
                .collect(),
            // `send` subsumes `public_send`, `method` subsumes `define_method`:
            // under an `any` test a literal containing another in the set can
            // never be the one that decides, and the prefilter reads every byte
            // of the repository once per searcher. Derived from the set rather
            // than curated, so it cannot fall out of step with it.
            residue: residue
                .iter()
                .filter(|a| {
                    !residue
                        .iter()
                        .any(|b| b.len() < a.len() && memchr::memmem::find(a, b).is_some())
                })
                .map(|a| memchr::memmem::Finder::new(a.as_slice()).into_owned())
                .collect(),
        }
    }

    /// Whether a file could contribute either a match or a residue occurrence.
    ///
    /// A match needs **every** required literal present. Residue needs any one
    /// of its own literals, and is reported from files a rule does not match --
    /// a declaration file, say -- so the two are checked separately rather than
    /// conjunctively. Getting that wrong would silently drop exactly the
    /// blind-spot report the design exists to produce.
    pub(crate) fn may_contribute(&self, source: &[u8]) -> bool {
        if self.required.is_empty() {
            return true;
        }
        if self.required.iter().all(|f| f.find(source).is_some()) {
            return true;
        }
        self.residue.iter().any(|f| f.find(source).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_not_required_text() {
        assert_eq!(required("$R.autoload_paths"), vec!["autoload_paths"]);
        assert_eq!(required("foo($A, $B)"), vec!["foo"]);
    }

    #[test]
    fn keywords_count_as_required_text() {
        assert_eq!(required("return nil"), vec!["return", "nil"]);
    }

    /// Ruby method names may end in `?` or `!`, and a file must contain that
    /// character too.
    #[test]
    fn predicate_and_bang_suffixes_are_kept() {
        assert_eq!(required("$R.active?"), vec!["active?"]);
        assert_eq!(required("$R.save!"), vec!["save!"]);
    }

    /// A pattern with no literal text constrains nothing, so nothing is
    /// skipped -- filtering may only ever be conservative.
    #[test]
    fn a_pattern_without_literals_filters_nothing() {
        assert!(required("$A.$B").is_empty());
        assert!(Filter::new(&[], &[]).may_contribute(b"anything at all"));
    }

    #[test]
    fn a_file_missing_a_required_literal_is_skipped() {
        let filter = Filter::new(&required("return nil"), &[]);
        assert!(filter.may_contribute(b"def a; return nil; end"));
        assert!(!filter.may_contribute(b"def a; 1; end"));
    }

    /// Trimming the subsumed searchers may not change the answer, only the
    /// number of passes over the file.
    #[test]
    fn a_subsuming_residue_literal_never_decides() {
        let of = |residue: &[Vec<u8>]| Filter::new(&required("$R.display_name"), residue);
        let full = of(&[b"send".to_vec(), b"public_send".to_vec(), b"try".to_vec()]);
        let trimmed = of(&[b"send".to_vec(), b"try".to_vec()]);
        for source in [
            &b"public_send(:x)"[..],
            b"__send__(:x)",
            b"foo.try(:x)",
            b"display_name",
            b"nothing here",
        ] {
            assert_eq!(
                full.may_contribute(source),
                trimmed.may_contribute(source),
                "{}",
                String::from_utf8_lossy(source)
            );
        }
    }

    /// `for_pattern` reads the required literals off the *substituted* source,
    /// which is only sound if substitution leaves everything else alone.
    #[test]
    fn substitution_does_not_change_the_required_literals() {
        for pattern in [
            "$R.display_name",
            "foo($A, $B)",
            "$R.select { |$P| $B }.first",
            "def display_name($A); $B; end",
            "$R.active?",
            "Foo::$C.bar(*$A)",
            "attr_reader :display_name",
        ] {
            let prepared = super::super::prepare::prepare(pattern).expect("prepares");
            let (mut from_text, mut from_prepared) = (required(pattern), required_of(&prepared));
            from_text.sort();
            from_prepared.sort();
            assert_eq!(from_text, from_prepared, "{pattern}");
        }
    }

    /// Residue is reported from files a rule does not match, so the anchor
    /// alone keeps a file in. Missing this would silently drop the blind-spot
    /// report -- a declaration file mentioning the name but never calling it.
    #[test]
    fn an_anchor_alone_keeps_a_file() {
        let filter = Filter::new(&required("$R.display_name"), &[b"display_name".to_vec()]);
        // No call, but the name appears -- residue territory.
        assert!(filter.may_contribute(b"class A; attr_reader :display_name; end"));
    }
}
