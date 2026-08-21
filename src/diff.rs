//! Restricting a run to the lines a change actually touched.
//!
//! The lint use: a rule that would flag two thousand pre-existing sites must not
//! fail CI on a pull request that added three. Scoping to the diff makes
//! `rwr check` adoptable on a codebase that has never run it -- the same move
//! that made `rubocop --auto-gen-config` and reviewdog necessary, done without
//! a todo file to go stale.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Lines added or changed, by absolute file path.
#[derive(Debug, Default)]
pub(crate) struct Changed {
    by_file: HashMap<PathBuf, Vec<(u32, u32)>>,
}

impl Changed {
    /// Whether any line in `start..=end` was touched.
    ///
    /// Overlap rather than containment: a match spanning a changed line and some
    /// unchanged ones is still a match this change is responsible for.
    pub(crate) fn touches(&self, file: &Path, start: usize, end: usize) -> bool {
        self.by_file.get(file).is_some_and(|ranges| {
            ranges
                .iter()
                .any(|(a, b)| start <= *b as usize && end >= *a as usize)
        })
    }

    /// Whether the file has any changed lines at all -- the cheap test, before
    /// a file is read.
    pub(crate) fn covers(&self, file: &Path) -> bool {
        self.by_file.contains_key(file)
    }

    pub(crate) fn files(&self) -> usize {
        self.by_file.len()
    }
}

/// What `--diff` was given, as a git revision range.
///
/// Bare `--diff` is the uncommitted work -- the pre-commit case. `--diff main`
/// is three-dot, `main...HEAD`, which is the change *this branch* introduces
/// rather than every way it differs from main's tip; two-dot would drag in
/// whatever main gained meanwhile and report it as yours.
fn spec(rev: Option<&str>) -> String {
    match rev {
        None => "HEAD".to_string(),
        Some(rev) if rev.contains("..") => rev.to_string(),
        Some(rev) => format!("{rev}..."),
    }
}

/// Ask git which lines changed, in the repository containing `start`.
///
/// `start` rather than the process's own directory: `rwr check ~/other/repo
/// --diff` must ask *that* repository, and asking the current one silently
/// scoped the run to a diff from somewhere else entirely.
pub(crate) fn from_git(rev: Option<&str>, start: &Path) -> Result<Changed, String> {
    let start = start
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", start.display()))?;
    let dir = if start.is_dir() {
        start.clone()
    } else {
        start.parent().unwrap_or(&start).to_path_buf()
    };

    let root = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !root.status.success() {
        return Err("not inside a git repository".to_string());
    }
    let root = PathBuf::from(String::from_utf8_lossy(&root.stdout).trim());

    let range = spec(rev);
    let out = Command::new("git")
        .args(["diff", "--unified=0", "--no-color", &range])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("cannot run git diff: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff {range} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Ok(parse(&String::from_utf8_lossy(&out.stdout), &root))
}

/// Read hunk headers out of a unified diff.
fn parse(diff: &str, root: &Path) -> Changed {
    let mut by_file: HashMap<PathBuf, Vec<(u32, u32)>> = HashMap::new();
    let mut current: Option<PathBuf> = None;

    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            // `/dev/null` is a deletion, which has no new lines to lint.
            current = path
                .strip_prefix("b/")
                .filter(|p| *p != "/dev/null")
                .map(|p| root.join(p));
            continue;
        }
        let Some(rest) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some(file) = &current else { continue };
        // `@@ -a,b +c,d @@` -- only the new-file side matters.
        let Some(plus) = rest.split_whitespace().find(|f| f.starts_with('+')) else {
            continue;
        };
        let mut parts = plus[1..].split(',');
        let Ok(start) = parts.next().unwrap_or_default().parse::<u32>() else {
            continue;
        };
        let count: u32 = parts.next().map_or(1, |c| c.parse().unwrap_or(1));
        if count == 0 {
            // A pure deletion: nothing was added here to be responsible for.
            continue;
        }
        by_file
            .entry(file.clone())
            .or_default()
            .push((start, start + count - 1));
    }
    Changed { by_file }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A branch's own change, not every way it differs from the base's tip.
    #[test]
    fn a_named_revision_uses_merge_base_semantics() {
        assert_eq!(spec(Some("main")), "main...");
        assert_eq!(spec(None), "HEAD");
        // An explicit range is passed through rather than doubled.
        assert_eq!(spec(Some("main..HEAD")), "main..HEAD");
    }

    #[test]
    fn hunk_headers_become_line_ranges() {
        let diff = "diff --git a/app/x.rb b/app/x.rb\n\
                    --- a/app/x.rb\n\
                    +++ b/app/x.rb\n\
                    @@ -3,0 +4,2 @@ class X\n\
                    +  a\n\
                    +  b\n\
                    @@ -10 +12 @@\n\
                    +  c\n";
        let root = Path::new("/repo");
        let changed = parse(diff, root);
        let file = root.join("app/x.rb");

        assert!(changed.touches(&file, 4, 4));
        assert!(changed.touches(&file, 5, 5));
        assert!(!changed.touches(&file, 6, 6));
        // A bare `+12` with no count is one line.
        assert!(changed.touches(&file, 12, 12));
        assert!(!changed.touches(&file, 13, 13));
    }

    /// A match spanning changed and unchanged lines belongs to the change.
    #[test]
    fn overlap_counts_not_containment() {
        let diff = "+++ b/x.rb\n@@ -1 +5 @@\n+x\n";
        let changed = parse(diff, Path::new("/repo"));
        let file = Path::new("/repo/x.rb");
        assert!(changed.touches(file, 3, 7), "spans the changed line");
        assert!(!changed.touches(file, 6, 9), "sits entirely below it");
    }

    /// A deleted file has no new lines, so nothing to lint.
    #[test]
    fn a_deletion_contributes_nothing() {
        let diff = "+++ /dev/null\n@@ -1,5 +0,0 @@\n-x\n";
        assert_eq!(parse(diff, Path::new("/repo")).files(), 0);
    }
}
