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

    /// Restrict to lines named on the command line, as `file.rb:3` or
    /// `file.rb:3-15`.
    ///
    /// The same scope git produces, supplied by hand: rwr *prints* `file:line`,
    /// so an output line pastes back in as an input.
    pub(crate) fn from_lines(named: Vec<(PathBuf, (u32, u32))>) -> Self {
        let mut by_file: HashMap<PathBuf, Vec<(u32, u32)>> = HashMap::new();
        for (path, range) in named {
            by_file.entry(path).or_default().push(range);
        }
        Changed { by_file }
    }
}

/// A path and the lines named after it, as `file.rb:3` or `file.rb:3-15`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Lines<'a> {
    pub(crate) path: &'a str,
    pub(crate) range: (u32, u32),
}

/// Split a `PATH:N` or `PATH:N-M` argument into its path and line range.
///
/// Returns `None` when there is no line suffix at all, which is the ordinary
/// case. A malformed one (`foo.rb:` or `foo.rb:9-3`) is an error rather than a
/// filename, because silently reading it as a path is how a scoped run turns
/// into an unscoped one.
pub(crate) fn split_lines(arg: &str) -> Result<Option<Lines<'_>>, String> {
    // `rfind`, so a Windows-style `C:\...` or any earlier colon is left alone.
    let Some(colon) = arg.rfind(':') else {
        return Ok(None);
    };
    let (path, suffix) = (&arg[..colon], &arg[colon + 1..]);
    // Not a line suffix at all -- a filename that happens to contain a colon.
    if suffix.is_empty() || !suffix.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(None);
    }

    let (start, end) = match suffix.split_once('-') {
        Some((a, b)) => (a, b),
        None => (suffix, suffix),
    };
    let parse = |s: &str| {
        s.parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("{arg}: line numbers start at 1"))
    };
    let (start, end) = (parse(start)?, parse(end)?);
    if end < start {
        return Err(format!("{arg}: line range ends before it starts"));
    }
    Ok(Some(Lines {
        path,
        range: (start, end),
    }))
}

/// What `--since` and `--diff` were given, as a git revision range.
///
/// `--diff` alone is the uncommitted work -- the pre-commit case. `--since main`
/// is three-dot, `main...HEAD`, which is the change *this branch* introduces
/// rather than every way it differs from main's tip; two-dot would drag in
/// whatever main gained meanwhile and report it as yours.
///
/// Together they are the merge base against the *working tree*: what this branch
/// introduces including what is not committed yet. Neither flag says that alone,
/// and it is what a human at a terminal usually means -- `--since main` on its
/// own is commit-to-commit and silently leaves your unstaged work out of scope.
fn spec(since: Option<&str>, uncommitted: bool, root: &Path) -> Result<String, String> {
    match (since, uncommitted) {
        (None, _) => Ok("HEAD".to_string()),
        // An explicit range is passed through rather than doubled.
        (Some(rev), false) if rev.contains("..") => Ok(rev.to_string()),
        (Some(rev), false) => Ok(format!("{rev}...")),
        // A range already names both ends, so there is no merge base to take
        // and no honest way to fold the working tree into it.
        (Some(rev), true) if rev.contains("..") => Err(format!(
            "--since {rev} is already a range; drop --diff, or name a single revision"
        )),
        (Some(rev), true) => merge_base(rev, root),
    }
}

/// Where this branch left the base, as a sha.
///
/// `git diff <sha>` compares a commit against the working tree, which is the
/// only spelling that reaches uncommitted lines -- `<rev>...` does not.
fn merge_base(rev: &str, root: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .args(["merge-base", rev, "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git merge-base: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git merge-base {rev} HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Ask git which lines changed, in the repository containing `start`.
///
/// `start` rather than the process's own directory: `rwr check ~/other/repo
/// --diff` must ask *that* repository, and asking the current one silently
/// scoped the run to a diff from somewhere else entirely.
pub(crate) fn from_git(
    since: Option<&str>,
    uncommitted: bool,
    start: &Path,
) -> Result<Changed, String> {
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

    let range = spec(since, uncommitted, &root)?;
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

    let mut changed = parse(&String::from_utf8_lossy(&out.stdout), &root);
    if uncommitted {
        untracked(&root, &mut changed)?;
    }
    Ok(changed)
}

/// Fold in files git is not tracking yet.
///
/// `git diff` cannot see them at all, so a brand-new file full of violations
/// reported as a clean tree -- the pre-commit case this flag exists for, failing
/// exactly when a change is largest. Every line of a new file is a new line.
fn untracked(root: &Path, changed: &mut Changed) -> Result<(), String> {
    let out = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    for name in String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|n| !n.is_empty())
    {
        changed
            .by_file
            .entry(root.join(name))
            .or_default()
            .push((1, u32::MAX));
    }
    Ok(())
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
        let root = Path::new(".");
        assert_eq!(spec(Some("main"), false, root).unwrap(), "main...");
        assert_eq!(spec(None, true, root).unwrap(), "HEAD");
        // An explicit range is passed through rather than doubled.
        assert_eq!(spec(Some("main..HEAD"), false, root).unwrap(), "main..HEAD");
        // A range has both ends already; folding the working tree in is not
        // something it can mean.
        assert!(spec(Some("a..b"), true, root).is_err());
    }

    /// `file.rb:3` and `file.rb:3-15`, the form rwr already prints.
    #[test]
    fn a_line_suffix_is_split_off_the_path() {
        let at = |path, range| Some(Lines { path, range });
        assert_eq!(split_lines("x.rb:3").unwrap(), at("x.rb", (3, 3)));
        assert_eq!(split_lines("a/x.rb:3-15").unwrap(), at("a/x.rb", (3, 15)));
        // No suffix, and a colon that is part of the name, are both just paths.
        assert_eq!(split_lines("x.rb").unwrap(), None);
        assert_eq!(split_lines("odd:name.rb").unwrap(), None);
        // Malformed is an error, never silently the whole string as a path --
        // that turns a scoped run into an unscoped one.
        assert!(split_lines("x.rb:0").is_err());
        assert!(split_lines("x.rb:9-3").is_err());
    }

    #[test]
    fn named_lines_scope_the_files_they_name() {
        let file = PathBuf::from("/repo/x.rb");
        let changed = Changed::from_lines(vec![(file.clone(), (3, 5))]);
        assert!(changed.touches(&file, 4, 4));
        assert!(!changed.touches(&file, 6, 6));
        assert!(!changed.covers(Path::new("/repo/other.rb")));
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
