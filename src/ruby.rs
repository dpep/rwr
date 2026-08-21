//! What Ruby version the target codebase is, and what a rule's output needs.
//!
//! Some rewrites emit syntax that older Rubies cannot parse: `{foo:}` is a
//! syntax error before 3.1, `filter_map` does not exist before 2.7. rwr's own
//! `verify` cannot catch it, because Prism parses *modern* Ruby and the output
//! is valid there -- so the result is a clean, confident, wrong rewrite, which
//! DESIGN.md §4 calls the dangerous failure.
//!
//! The version is a property of the codebase rather than of the pattern, so it
//! is detected from the repo and a rule only declares its floor (Q6).

use std::path::{Path, PathBuf};

/// A Ruby version, to the precision that decides syntax.
///
/// Patch releases never add syntax, so comparing major and minor is enough and
/// keeps `3.1.0` and `3.1.4` from looking like different targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    pub major: u32,
    pub minor: u32,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl Version {
    /// The first `X.Y` in `text`, ignoring any trailing patch level.
    fn first_in(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_digit() {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let mut parts = text[start..i].split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next().unwrap_or("0").parse().unwrap_or(0);
            return Some(Version { major, minor });
        }
        None
    }

    /// Parse a rule's declared floor, e.g. `3.1`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::first_in(text)
    }
}

/// Where a detected version came from, so the report can say.
#[derive(Debug)]
pub(crate) struct Detected {
    pub version: Version,
    pub source: String,
}

/// The lowest version a gem-style requirement admits.
///
/// `>= 3.3.0', '< 4.1.0` is 3.3, not 4.1 -- an upper bound says nothing about
/// what the code may already use, so only lower bounds and `~>` count.
fn floor_of(constraint: &str) -> Option<Version> {
    let mut floor: Option<Version> = None;
    for clause in constraint.split(',') {
        let clause = clause.trim().trim_matches(['"', '\'', ' ']);
        if clause.starts_with('<') || clause.starts_with("!=") {
            continue;
        }
        if let Some(v) = Version::first_in(clause) {
            floor = Some(floor.map_or(v, |f: Version| f.min(v)));
        }
    }
    floor
}

/// The Ruby version the codebase at or above `start` targets.
///
/// Three places declare it and real repos use all three: rails has only a
/// gemspec `required_ruby_version`, discourse and mastodon only a Gemfile
/// `ruby` line, and many repos only `.ruby-version`.
pub(crate) fn detect(start: &Path) -> Option<Detected> {
    // Absolute first: a relative path's parent chain ends at `""` after one
    // step, so walking up from `app/` never reached the repo root that holds
    // the Gemfile.
    let absolute = start
        .canonicalize()
        .or_else(|_| std::env::current_dir())
        .ok()?;
    let mut dir: Option<PathBuf> = if absolute.is_dir() {
        Some(absolute)
    } else {
        absolute.parent().map(Path::to_path_buf)
    };

    while let Some(here) = dir {
        if let Some(found) = in_dir(&here) {
            return Some(found);
        }
        // A repository root is where the search stops: above it is somebody
        // else's project, whose Ruby version is not this one's. Checked *after*
        // the directory itself, since the root is usually where the answer is.
        if here.join(".git").exists() {
            return None;
        }
        dir = here.parent().map(Path::to_path_buf);
    }
    None
}

fn in_dir(dir: &Path) -> Option<Detected> {
    let named = |name: &str| dir.join(name);

    if let Ok(text) = std::fs::read_to_string(named(".ruby-version"))
        && let Some(version) = Version::first_in(&text)
    {
        return Some(Detected {
            version,
            source: ".ruby-version".to_string(),
        });
    }

    if let Ok(text) = std::fs::read_to_string(named("Gemfile"))
        && let Some(line) = text.lines().find(|l| l.trim_start().starts_with("ruby "))
        && let Some(version) = floor_of(line.trim_start().trim_start_matches("ruby "))
    {
        return Some(Detected {
            version,
            source: "Gemfile".to_string(),
        });
    }

    let gemspecs = std::fs::read_dir(dir).ok()?;
    for entry in gemspecs.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("gemspec") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(line) = text.lines().find(|l| l.contains("required_ruby_version")) else {
            continue;
        };
        // `split('=').nth(1)` stops at the `=` inside `>=`, leaving `">` --
        // which is why rails, whose only declaration is a gemspec, detected
        // nothing.
        let Some((_, after)) = line.split_once('=') else {
            continue;
        };
        if let Some(version) = floor_of(after) {
            return Some(Detected {
                version,
                source: path
                    .file_name()
                    .map_or_else(|| "gemspec".into(), |n| n.to_string_lossy().into_owned()),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three spellings real repos actually use, taken from them.
    #[test]
    fn a_lower_bound_is_the_floor() {
        // discourse
        assert_eq!(floor_of("\"~> 3.4\""), Some(Version { major: 3, minor: 4 }));
        // mastodon -- the upper bound must not become the floor
        assert_eq!(
            floor_of("'>= 3.3.0', '< 4.1.0'"),
            Some(Version { major: 3, minor: 3 })
        );
        // rails
        assert_eq!(
            floor_of("\">= 3.1.0\""),
            Some(Version { major: 3, minor: 1 })
        );
    }

    /// A bare `.ruby-version` is a version, not a requirement.
    #[test]
    fn a_plain_version_parses() {
        assert_eq!(
            Version::first_in("3.2.2\n"),
            Some(Version { major: 3, minor: 2 })
        );
        assert_eq!(
            Version::first_in("ruby-3.0.6"),
            Some(Version { major: 3, minor: 0 })
        );
    }

    /// A gemspec's `>=` contains an `=`, which a naive split on `=` stops at --
    /// leaving `">` and detecting nothing. rails declares its version only this
    /// way, so the bug was invisible against repos with a Gemfile.
    #[test]
    fn a_gemspec_requirement_survives_its_own_operator() {
        let dir = std::env::temp_dir().join("rwr-ruby-gemspec");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("thing.gemspec"),
            "Gem::Specification.new do |s|\n  s.required_ruby_version     = \">= 3.1.0\"\nend\n",
        )
        .expect("write");

        let found = detect(&dir).expect("detects");
        assert_eq!(found.version, Version { major: 3, minor: 1 });
        assert_eq!(found.source, "thing.gemspec");
    }

    /// The search walks up, so a path inside the repo finds the root's Gemfile.
    /// A *relative* path's parent chain ends at `""` after one step, which is
    /// why scoping to `app/` used to detect nothing.
    #[test]
    fn the_search_walks_up_from_a_subdirectory() {
        let dir = std::env::temp_dir().join("rwr-ruby-walkup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app/models")).expect("temp dir");
        std::fs::write(dir.join("Gemfile"), "source 'x'\nruby \"~> 3.4\"\n").expect("write");

        let found = detect(&dir.join("app/models")).expect("detects");
        assert_eq!(found.version, Version { major: 3, minor: 4 });
        assert_eq!(found.source, "Gemfile");
    }

    /// Above a repository root is somebody else's project.
    #[test]
    fn the_search_stops_at_a_repository_root() {
        let dir = std::env::temp_dir().join("rwr-ruby-boundary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inner/.git")).expect("temp dir");
        std::fs::write(dir.join(".ruby-version"), "3.4.1\n").expect("write");

        assert!(
            detect(&dir.join("inner")).is_none(),
            "the outer project's version is not this one's"
        );
    }

    /// Patch level never decides syntax, so it must not decide ordering.
    #[test]
    fn comparison_ignores_the_patch_level() {
        let target = Version::first_in("3.1.4").expect("parses");
        let floor = Version::parse("3.1").expect("parses");
        assert!(target >= floor);
        assert!(target < Version::parse("3.2").expect("parses"));
    }
}
