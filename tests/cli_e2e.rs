//! End-to-end tests driving the built binary.
//!
//! Per CLAUDE.md, CLI behavior is verified here rather than by hand-running
//! `rwr` — reproducible, CI-checked, and immune to a stale `target/debug/rwr`.

use std::process::{Command, Output};

fn rwr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(args)
        .output()
        .expect("binary runs")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A temp directory holding one Ruby file.
fn fixture(source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("fixture.rb"), source).expect("write fixture");
    dir
}

/// The shorthand really searches: one argument routes to `find` and reports
/// structural matches, not a stub.
#[test]
fn bare_pattern_is_shorthand_for_find() {
    let dir = fixture("def a\n  return nil if x\n  # return nil\n  s = \"return nil\"\nend\n");
    let out = rwr(&["return nil", dir.path().to_str().expect("utf8")]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        text.lines().count(),
        1,
        "comment and string literal are not code: {text}"
    );
}

/// Exit 1 means "no match" and is a clean result, not an error.
#[test]
fn no_match_exits_one() {
    let dir = fixture("def a\n  1\nend\n");
    let out = rwr(&["return nil", dir.path().to_str().expect("utf8")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

/// A pattern that is not valid Ruby gets its own code, distinct from an I/O or
/// internal failure -- the caller must fix the rule, not the invocation.
#[test]
fn unparseable_pattern_exits_three() {
    let out = rwr(&["def foo("]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
}

/// Trailing positionals are paths, rg-style, and must not be mistaken for a
/// replacement -- which is why the replacement is a flag (D31). Scoping to a
/// directory without matches must find nothing.
#[test]
fn trailing_positionals_scope_the_search() {
    let has = fixture("def a\n  return nil\nend\n");
    let hasnt = fixture("def a\n  1\nend\n");

    let hit = rwr(&["return nil", has.path().to_str().expect("utf8")]);
    assert_eq!(hit.status.code(), Some(0), "{}", stderr(&hit));

    let miss = rwr(&["return nil", hasnt.path().to_str().expect("utf8")]);
    assert_eq!(miss.status.code(), Some(1), "{}", stderr(&miss));
}

/// The safety property behind D30, asserted on the filesystem rather than on a
/// message: the shorthand is read-only *by construction*. A pattern plus a
/// replacement previews and must never write, because a terse two-argument
/// command that silently mutated a repo is exactly the foot-gun D29 removed the
/// mode flags to avoid.
#[test]
fn shorthand_with_replacement_cannot_reach_rewrite() {
    let dir = fixture("def a\n  return nil\nend\n");
    let file = dir.path().join("fixture.rb");
    let before = std::fs::read(&file).expect("read");

    let out = rwr(&[
        "return nil",
        "-r",
        "return",
        dir.path().to_str().expect("utf8"),
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected preview: {}",
        stderr(&out)
    );

    let after = std::fs::read(&file).expect("read");
    assert_eq!(before, after, "the shorthand wrote to disk");
}

/// And the verb does write, so the invariant above is about the shorthand
/// rather than about rwr being unable to rewrite at all.
#[test]
fn the_rewrite_verb_writes() {
    let dir = fixture("def a\n  return nil\nend\n");
    let file = dir.path().join("fixture.rb");

    let out = rwr(&[
        "rewrite",
        "return nil",
        "-r",
        "return",
        dir.path().to_str().expect("utf8"),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let after = String::from_utf8(std::fs::read(&file).expect("read")).expect("utf8");
    assert_eq!(after, "def a\n  return\nend\n");
}

/// A rule with no `rewrite:` is a lint: it flags a shape for a human without
/// proposing an edit. Some things are worth surfacing and not worth rewriting.
#[test]
fn a_rule_without_a_template_is_a_finding() {
    let dir = fixture("a = Company.where(x: 1).size\n");
    let rule = dir.path().join("r.yml");
    std::fs::write(
        &rule,
        "id: relation-size\ndescription: say which you meant\nmatch: $R.where($C).size\n",
    )
    .expect("write");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("finding(s) for review"), "{text}");
    assert!(text.contains("say which you meant"), "{text}");
    // A finding is work to do, exactly as an edit is -- a lint that exits 0
    // gates nothing.
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    // And it writes nothing.
    let after = std::fs::read_to_string(dir.path().join("fixture.rb")).expect("read");
    assert_eq!(after, "a = Company.where(x: 1).size\n");
}

/// A bare pattern is not a rule file, so it never reaches the lint path.
#[test]
fn a_bare_pattern_without_a_template_still_fails() {
    let dir = fixture("def a\n  return nil\nend\n");
    let out = rwr(&["check", "return nil", dir.path().to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
}

/// A rule argument that is neither a path nor a built-in names what it tried,
/// rather than reporting the *next* problem it would have hit.
#[test]
fn an_unknown_rule_lists_the_built_ins() {
    let err = stderr(&rwr(&["check", "no-such-rule"]));
    assert!(err.contains("no-such-rule"), "{err}");
    assert!(err.contains("style/return-nil"), "{err}");
}

/// The pack is compiled in, so an installed binary can run it from anywhere --
/// there is no `rules/` next to the executable.
#[test]
fn the_built_in_pack_resolves_by_name() {
    let dir = fixture("def a\n  return nil\nend\n");
    let out = rwr(&["check", "style/return-nil", dir.path().to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

/// `check` inverts polarity deliberately (D22): a clean tree is success, so a
/// git hook does not block a commit where a rule correctly matches nothing.
#[test]
fn check_exits_zero_when_clean_and_one_when_there_is_work() {
    let clean = fixture("def a\n  1\nend\n");
    let dirty = fixture("def a\n  return nil\nend\n");
    let rule = clean.path().join("r.yml");
    std::fs::write(&rule, "match: return nil\nrewrite: return\n").expect("write rule");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        clean.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "clean tree is success");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dirty.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1), "work to do is the signal");
}

/// `check` never writes, whatever it finds.
#[test]
fn check_never_writes() {
    let dir = fixture("def a\n  return nil\nend\n");
    let file = dir.path().join("fixture.rb");
    let before = std::fs::read(&file).expect("read");
    let rule = dir.path().join("r.yml");
    std::fs::write(&rule, "match: return nil\nrewrite: return\n").expect("write rule");

    rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    assert_eq!(std::fs::read(&file).expect("read"), before);
}

/// The account of what a rule could not see reaches the output, and names the
/// class of each occurrence. Without this the blind-spot report could regress
/// to silence and nothing would notice.
#[test]
fn a_rename_reports_what_it_could_not_account_for() {
    // The macro sits inside the class -- both the realistic shape and what the
    // class-anchored scoping keeps, since an unrelated class's symbol is
    // correctly filtered out.
    let dir =
        fixture("class Account\n  attr_reader :display_name\n  def display_name; 1; end\nend\n");
    let rule = dir.path().join("r.yml");
    std::fs::write(&rule, "method: Account#display_name\nrename: full_name\n").expect("write");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    let err = stderr(&out);
    assert!(err.contains("could not account for"), "{err}");
    assert!(
        err.contains("Symbol"),
        "the attr_reader symbol is a blind spot: {err}"
    );
}

/// The shipped pack loads as a directory and says which rule did what.
///
/// Pins two things at once: `--help` promises a directory of rules, and a pack
/// is useless without attribution -- "27 sites changed" across five rules is
/// not a reviewable answer.
#[test]
fn the_shipped_pack_names_the_rule_that_fired() {
    let dir = fixture(
        "class Widget\n  def go(items)\n    items.select { |i| i.ready? }.first\n  end\n\n  def nothing\n    return nil\n  end\nend\n",
    );
    let pack = concat!(env!("CARGO_MANIFEST_DIR"), "/rules");

    let out = rwr(&[
        "check",
        pack,
        dir.path().to_str().unwrap(),
        "-j",
        "--unsafe",
    ]);
    let text = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&text).expect("json");
    let rules = &report["changed"][0]["rules"];
    let named: Vec<&str> = rules
        .as_array()
        .expect("rules array")
        .iter()
        .map(|r| r["rule"].as_str().expect("rule id"))
        .collect();
    assert!(named.contains(&"performance/detect"), "{text}");
    assert!(named.contains(&"style/return-nil"), "{text}");
}

/// A site counts once however many edits it takes.
///
/// `select { }.first` -> `detect { }` is a shape change that the structural
/// diff splits into two edits. Reporting edits would say "2 sites" for one
/// place a reader sees in the diff.
#[test]
fn a_site_counts_once_however_many_edits_it_takes() {
    let dir = fixture("items.select { |i| i.ready? }.first\n");
    let rule = dir.path().join("r.yml");
    std::fs::write(
        &rule,
        "match: $R.select { |$P| $B }.first\nrewrite: $R.detect { |$P| $B }\n",
    )
    .expect("write");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
        "-j",
    ]);
    let text = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(report["changed"][0]["sites"], 1, "{text}");
}

/// A rule held back for being unsafe must not look like a rule that found
/// nothing: the run always says how many were skipped (D57). The reasons are
/// one flag away rather than on every run -- six lines of stderr per
/// pre-commit is how a report trains people to stop reading it.
#[test]
fn held_back_rules_are_reported_not_silent() {
    let dir = fixture("a = xs.inject(:+)\n");

    let quiet = rwr(&["check", "performance/sum", dir.path().to_str().unwrap()]);
    let err = stderr(&quiet);
    assert!(err.contains("held back"), "{err}");
    assert!(err.contains("--unsafe"), "{err}");
    assert!(!err.contains("empty collection"), "terse by default: {err}");

    let why = rwr(&[
        "check",
        "performance/sum",
        dir.path().to_str().unwrap(),
        "-e",
    ]);
    assert!(
        stderr(&why).contains("empty collection"),
        "the reason is one flag away: {}",
        stderr(&why)
    );
    assert_eq!(
        quiet.status.code(),
        Some(0),
        "nothing ran, so nothing to do"
    );

    let asked = rwr(&[
        "check",
        "performance/sum",
        dir.path().to_str().unwrap(),
        "--unsafe",
    ]);
    assert_eq!(asked.status.code(), Some(1), "{}", stderr(&asked));
    let text = String::from_utf8_lossy(&asked.stdout);
    assert!(
        text.contains("empty collection"),
        "the caveat prints next to the diff: {text}"
    );
}

/// `{foo:}` is a syntax error before Ruby 3.1, and `verify` cannot catch it --
/// Prism parses modern Ruby, so the output is valid there. The guard has to come
/// from the codebase's declared version (Q6).
#[test]
fn a_rule_needing_a_newer_ruby_is_held_back() {
    let dir = fixture("a = { x: x }\n");
    std::fs::write(dir.path().join(".ruby-version"), "2.7.8\n").expect("write");

    let out = rwr(&[
        "check",
        "style/hash-shorthand",
        dir.path().to_str().unwrap(),
    ]);
    let err = stderr(&out);
    assert!(err.contains("newer Ruby"), "{err}");
    assert!(err.contains("2.7"), "says what it found, and where: {err}");
    assert_eq!(out.status.code(), Some(0), "nothing ran, so nothing to do");

    // The same tree on a Ruby that has the syntax.
    std::fs::write(dir.path().join(".ruby-version"), "3.2.2\n").expect("write");
    let ok = rwr(&[
        "check",
        "style/hash-shorthand",
        dir.path().to_str().unwrap(),
    ]);
    assert_eq!(ok.status.code(), Some(1), "{}", stderr(&ok));
}

/// An undetected version is not permission to assume the newest one.
#[test]
fn an_undetected_ruby_version_holds_the_rule_back() {
    let dir = fixture("a = { x: x }\n");
    let out = rwr(&[
        "check",
        "style/hash-shorthand",
        dir.path().to_str().unwrap(),
    ]);
    let err = stderr(&out);
    assert!(err.contains("none was detected"), "{err}");
    assert!(err.contains("--ruby"), "says how to answer it: {err}");

    let forced = rwr(&[
        "check",
        "style/hash-shorthand",
        dir.path().to_str().unwrap(),
        "--ruby",
        "3.2",
    ]);
    assert_eq!(forced.status.code(), Some(1), "{}", stderr(&forced));
}

/// Run a git command in `dir`, failing loudly.
fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--diff` is what makes `check` adoptable on a codebase that has never run
/// it: a rule with three pre-existing sites must not fail a change that added
/// one.
#[test]
fn diff_scoping_ignores_pre_existing_sites() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path();
    std::fs::write(
        path.join("app.rb"),
        "def one\n  return nil\nend\n\ndef two\n  return nil\nend\n",
    )
    .expect("write");
    git(path, &["init", "-q", "--initial-branch=main", "."]);
    git(path, &["config", "user.email", "t@e.st"]);
    git(path, &["config", "user.name", "t"]);
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);

    // A third site arrives; the two already there are not this change's doing.
    std::fs::write(
        path.join("app.rb"),
        "def one\n  return nil\nend\n\ndef two\n  return nil\nend\n\ndef three\n  return nil\nend\n",
    )
    .expect("write");

    let all = rwr(&["check", "style/return-nil", path.to_str().unwrap()]);
    assert_eq!(all.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&all.stdout).contains("3 site(s)"),
        "{}",
        String::from_utf8_lossy(&all.stdout)
    );

    let scoped = rwr(&[
        "check",
        "style/return-nil",
        path.to_str().unwrap(),
        "--diff",
    ]);
    assert!(
        String::from_utf8_lossy(&scoped.stdout).contains("1 site(s)"),
        "only the added site: {}",
        String::from_utf8_lossy(&scoped.stdout)
    );
}

/// `--since main` is `main...HEAD`, not `main..HEAD`. Two-dot reports whatever
/// the base gained meanwhile as though this branch had written it.
#[test]
fn a_named_base_excludes_what_the_base_gained() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path();
    std::fs::write(path.join("app.rb"), "def one\n  1\nend\n").expect("write");
    git(path, &["init", "-q", "--initial-branch=main", "."]);
    git(path, &["config", "user.email", "t@e.st"]);
    git(path, &["config", "user.name", "t"]);
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);

    git(path, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(path.join("mine.rb"), "def mine\n  return nil\nend\n").expect("write");
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "mine"]);

    // Meanwhile main gains a violation of its own.
    git(path, &["checkout", "-q", "main"]);
    std::fs::write(path.join("theirs.rb"), "def theirs\n  return nil\nend\n").expect("write");
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "theirs"]);
    git(path, &["checkout", "-q", "feature"]);

    let out = rwr(&[
        "check",
        "style/return-nil",
        path.to_str().unwrap(),
        "--since",
        "main",
    ]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("mine.rb"), "{text}");
    assert!(
        !text.contains("theirs.rb"),
        "main's own work is not this branch's: {text}"
    );
}

/// The bug the split exists to make unrepresentable: as `--diff [<REV>]`, a
/// following path was swallowed as the revision and `app.rb...` was handed to
/// git. `--diff` now takes no value, so a path after it is a path.
#[test]
fn a_path_after_diff_is_a_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path();
    std::fs::write(path.join("app.rb"), "def one\n  1\nend\n").expect("write");
    git(path, &["init", "-q", "--initial-branch=main", "."]);
    git(path, &["config", "user.email", "t@e.st"]);
    git(path, &["config", "user.name", "t"]);
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);
    std::fs::write(path.join("app.rb"), "def one\n  return nil\nend\n").expect("write");

    let out = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(["check", "style/return-nil", "--diff", "app.rb"])
        .current_dir(path)
        .output()
        .expect("binary runs");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("1 site(s)"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// `--since main` is commit-to-commit, so uncommitted work sits outside it.
/// With `--diff` the range runs from the merge base to the working tree, which
/// is the only spelling that covers both.
#[test]
fn since_with_diff_reaches_uncommitted_work() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path();
    std::fs::write(path.join("app.rb"), "def one\n  1\nend\n").expect("write");
    git(path, &["init", "-q", "--initial-branch=main", "."]);
    git(path, &["config", "user.email", "t@e.st"]);
    git(path, &["config", "user.name", "t"]);
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);

    git(path, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(path.join("committed.rb"), "def a\n  return nil\nend\n").expect("write");
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "committed"]);
    std::fs::write(
        path.join("app.rb"),
        "def one\n  1\nend\n\ndef b\n  return nil\nend\n",
    )
    .expect("write");

    let since = rwr(&[
        "check",
        "style/return-nil",
        path.to_str().unwrap(),
        "--since",
        "main",
    ]);
    let text = String::from_utf8_lossy(&since.stdout);
    assert!(text.contains("committed.rb"), "{text}");
    assert!(!text.contains("app.rb"), "commit-to-commit: {text}");

    let both = rwr(&[
        "check",
        "style/return-nil",
        path.to_str().unwrap(),
        "--since",
        "main",
        "--diff",
    ]);
    let text = String::from_utf8_lossy(&both.stdout);
    assert!(text.contains("committed.rb"), "{text}");
    assert!(text.contains("app.rb"), "the working tree too: {text}");
}

/// `git diff` cannot see a file it is not tracking, so a brand-new file full of
/// violations reported as a clean tree -- the pre-commit case failing exactly
/// when the change is largest.
#[test]
fn a_brand_new_file_is_in_the_uncommitted_scope() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path();
    std::fs::write(path.join("app.rb"), "def one\n  1\nend\n").expect("write");
    git(path, &["init", "-q", "--initial-branch=main", "."]);
    git(path, &["config", "user.email", "t@e.st"]);
    git(path, &["config", "user.name", "t"]);
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);

    std::fs::write(path.join("brand_new.rb"), "def b\n  return nil\nend\n").expect("write");

    let out = rwr(&[
        "check",
        "style/return-nil",
        path.to_str().unwrap(),
        "--diff",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("brand_new.rb"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A typo'd path used to walk nothing and exit 0 -- in CI, a green gate that
/// checked no files at all.
#[test]
fn a_path_that_does_not_exist_is_an_error() {
    let dir = fixture("def a\n  return nil\nend\n");
    let out = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(["check", "style/return-nil", "typo-dir"])
        .current_dir(dir.path())
        .output()
        .expect("binary runs");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("no such path"), "{}", stderr(&out));
}

/// `file.rb:3-15` scopes to those lines -- the form rwr already prints, pasted
/// back in.
#[test]
fn a_line_range_scopes_the_run() {
    let dir = fixture("def one\n  return nil\nend\n\ndef two\n  return nil\nend\n");
    let run = |arg: &str| {
        Command::new(env!("CARGO_BIN_EXE_rwr"))
            .args(["check", "style/return-nil", arg])
            .current_dir(dir.path())
            .output()
            .expect("binary runs")
    };

    let all = run("fixture.rb");
    assert!(
        String::from_utf8_lossy(&all.stdout).contains("2 site(s)"),
        "{}",
        String::from_utf8_lossy(&all.stdout)
    );

    let scoped = run("fixture.rb:1-3");
    assert!(
        String::from_utf8_lossy(&scoped.stdout).contains("1 site(s)"),
        "only the first def: {}",
        String::from_utf8_lossy(&scoped.stdout)
    );

    // A single line is a range of one.
    let one = run("fixture.rb:6");
    assert!(
        String::from_utf8_lossy(&one.stdout).contains("1 site(s)"),
        "{}",
        String::from_utf8_lossy(&one.stdout)
    );

    // Two ways to say which lines is a refusal, not a silent precedence rule.
    let both = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(["check", "style/return-nil", "fixture.rb:1-3", "--diff"])
        .current_dir(dir.path())
        .output()
        .expect("binary runs");
    assert_eq!(both.status.code(), Some(2), "{}", stderr(&both));
}

/// Outside a repository, `--diff` has no answer -- and "no lines changed" and
/// "git could not tell me" must not produce the same clean exit.
#[test]
fn diff_outside_a_repository_is_an_error() {
    let dir = fixture("def a\n  return nil\nend\n");
    let out = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(["check", "style/return-nil", ".", "--diff"])
        .current_dir(dir.path())
        .env("GIT_CEILING_DIRECTORIES", dir.path())
        .output()
        .expect("binary runs");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

/// A rule's `contains:` sub-patterns are its own, wherever it sits in the set.
///
/// The ERB pass built every rule's criteria from the *first* rule's sub-pattern
/// map, so a set whose second rule used `contains:` silently matched nothing in
/// templates -- while the identical rule matched the identical code in a `.rb`
/// file, and alone in a set of one.
#[test]
fn a_contains_rule_reaches_templates_from_any_position() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path();
    std::fs::write(
        path.join("rules.yml"),
        "- id: first\n  \
           description: Matches nothing here.\n  \
           match: never_called($Z)\n  \
           where:\n    $Z: { contains: nothing_at_all }\n  \
           rewrite: gone($Z)\n\
         - id: second\n  \
           description: Log calls that mention a widget.\n  \
           match: log($M)\n  \
           where:\n    $M: { contains: widget }\n  \
           rewrite: audit($M)\n",
    )
    .expect("write");
    std::fs::write(
        path.join("page.erb"),
        "<div>\n  <% log(widget.name) %>\n</div>\n",
    )
    .expect("write");
    std::fs::write(path.join("plain.rb"), "log(widget.name)\n").expect("write");

    let rules = path.join("rules.yml");
    let run = |target: &str| {
        Command::new(env!("CARGO_BIN_EXE_rwr"))
            .args(["check", rules.to_str().unwrap(), target])
            .current_dir(path)
            .output()
            .expect("binary runs")
    };

    // The same rule, the same call, in the two file kinds.
    let ruby = run("plain.rb");
    assert_eq!(ruby.status.code(), Some(1), "{}", stderr(&ruby));
    let erb = run("page.erb");
    assert_eq!(
        erb.status.code(),
        Some(1),
        "the template too: {}",
        String::from_utf8_lossy(&erb.stdout)
    );
    assert!(
        String::from_utf8_lossy(&erb.stdout).contains("page.erb"),
        "{}",
        String::from_utf8_lossy(&erb.stdout)
    );
}

/// ERB is parsed and Haml is searched, and the report distinguishes them.
///
/// A Rails app keeps a large share of its call sites in templates, and a rename
/// that misses them under-reports -- the dangerous direction (Q11).
#[test]
fn erb_is_parsed_and_haml_is_searched() {
    let dir = fixture("class Account\n  def display_name; 1; end\nend\n");
    std::fs::write(
        dir.path().join("show.html.erb"),
        "<%= account.display_name %>\n",
    )
    .expect("write");
    std::fs::write(dir.path().join("index.haml"), "= account.display_name\n").expect("write");
    let rule = dir.path().join("r.yml");
    std::fs::write(&rule, "method: Account#display_name\nrename: full_name\n").expect("write");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    let err = stderr(&out);
    // ERB is parsed: its tags stitch into a Ruby program, so the call site is
    // real evidence and appears in the account proper.
    assert!(err.contains("show.html.erb"), "the ERB call site: {err}");
    // Haml is not, so it falls back to a text search that says it is weaker.
    assert!(err.contains("index.haml"), "the Haml call site: {err}");
    assert!(err.contains("found by text search"), "{err}");
    assert!(
        err.contains("1 template file(s)"),
        "only Haml fell back: {err}"
    );
}

/// A `.rake` file is Ruby, and was invisible until it was not.
#[test]
fn rake_files_are_searched() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("db.rake"),
        "task :x do\n  return nil\nend\n",
    )
    .expect("write");

    let out = rwr(&["return nil", dir.path().to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("db.rake"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The failure the refusal contract cannot catch: a rule that renames across
/// two classes at exit 0, because there is no conflict to detect (Q10).
#[test]
fn a_rename_across_two_classes_warns() {
    let dir = fixture(
        "class Account\n  def display_name; 1; end\nend\n         class Company\n  def display_name; 2; end\nend\n         account = Account.new\ncompany = Company.new\n         account.display_name\ncompany.display_name\n",
    );
    let loose = dir.path().join("loose.yml");
    std::fs::write(&loose, "match: $R.display_name\nrewrite: $R.full_name\n").expect("write");

    let out = rwr(&[
        "check",
        loose.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    let err = stderr(&out);
    assert!(err.contains("2 different classes"), "{err}");
    assert!(err.contains("Account"), "{err}");
    assert!(err.contains("Company"), "{err}");

    // Saying which class was meant is the fix, and silences it.
    let narrow = dir.path().join("narrow.yml");
    std::fs::write(
        &narrow,
        "match: $R.display_name\nwhere:\n  $R:\n    type: Account\nrewrite: $R.full_name\n",
    )
    .expect("write");
    let scoped = rwr(&[
        "check",
        narrow.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    assert!(
        !stderr(&scoped).contains("different classes"),
        "{}",
        stderr(&scoped)
    );
}

/// A machine consumer needs to know what produced the document it is parsing,
/// especially across a shape change.
#[test]
fn structured_output_names_its_own_shape() {
    let dir = fixture("def a\n  return nil\nend\n");
    for args in [
        vec![
            "check",
            "style/return-nil",
            dir.path().to_str().unwrap(),
            "-j",
        ],
        vec!["return nil", dir.path().to_str().unwrap(), "-j"],
    ] {
        let out = rwr(&args);
        let text = String::from_utf8_lossy(&out.stdout);
        let doc: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert!(doc.is_object(), "a document, not a list of one: {text}");
        assert_eq!(doc["schema"], 2, "{text}");
        assert_eq!(doc["rwr_version"], env!("CARGO_PKG_VERSION"), "{text}");
    }
}

/// A rule that only rewrites call sites has nothing to be incomplete about.
///
/// `$R.gsub($F, $T)` -> `$R.tr($F, $T)` is shaped exactly like a rename -- a
/// literal name applied to metavariables -- but `String#gsub` still exists
/// afterwards, so every `.gsub` it declined to rewrite is fine. Reporting them
/// as unaccounted-for was a false claim, found by a real run.
#[test]
fn a_call_site_rewrite_reports_no_residue() {
    let dir = fixture("a = s.gsub(\"-\", \"_\")\nb = s.gsub(\"hello\", \"world\")\n");
    let out = rwr(&[
        "check",
        "performance/string-replacement",
        dir.path().to_str().unwrap(),
        "--unsafe",
    ]);
    let err = stderr(&out);
    assert!(!err.contains("could not account for"), "{err}");
}

/// With several renames in one run, an unlabelled block leaves the reader to
/// guess which rule an occurrence belongs to -- and a real run guessed wrong.
#[test]
fn residue_names_the_rule_it_belongs_to() {
    let dir = fixture(
        "class Account\n  def display_name; 1; end\nend\n         class Company\n  def legal_name; 2; end\nend\n         class AccountSerializer\n  delegate :display_name, to: :account\nend\n         class CompanySerializer\n  delegate :legal_name, to: :company\nend\n",
    );
    let pack = dir.path().join("pack");
    std::fs::create_dir_all(&pack).expect("mkdir");
    std::fs::write(
        pack.join("rename-account.yml"),
        "method: Account#display_name\nrename: full_name\n",
    )
    .expect("write");
    std::fs::write(
        pack.join("rename-company.yml"),
        "method: Company#legal_name\nrename: registered_name\n",
    )
    .expect("write");

    let out = rwr(&[
        "check",
        pack.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    let err = stderr(&out);
    // Both rules report: scoping every rule by the *set's* first class dropped
    // the second rule's account entirely.
    assert!(err.contains("[rename-account]"), "{err}");
    assert!(err.contains("[rename-company]"), "{err}");
}

/// The point of parsing ERB rather than searching it: rwr can rewrite through
/// a template and leave the HTML exactly where it was.
#[test]
fn a_rewrite_reaches_inside_erb() {
    let dir = fixture("class Account\n  def display_name; 1; end\nend\n");
    let view = dir.path().join("show.html.erb");
    std::fs::write(
        &view,
        "<h1><%= @account.display_name %></h1>\n         <% @accounts.each do |account| %>\n         <li><%= account.display_name %></li>\n         <% end %>\n",
    )
    .expect("write");
    let rule = dir.path().join("r.yml");
    std::fs::write(&rule, "match: $R.display_name\nrewrite: $R.full_name\n").expect("write");

    let out = rwr(&[
        "rewrite",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let after = std::fs::read_to_string(&view).expect("read");
    assert_eq!(
        after,
        "<h1><%= @account.full_name %></h1>\n         <% @accounts.each do |account| %>\n         <li><%= account.full_name %></li>\n         <% end %>\n",
        "both call sites rewritten, every byte of HTML untouched"
    );
}

/// Deletion takes the whole unit: the match, the comments above it, its line,
/// and one of the blank lines that separated it from its neighbours.
#[test]
fn delete_removes_a_definition_and_its_comment() {
    let dir = fixture(
        "class Widget\n  def keep_me\n    1\n  end\n\n           # a comment on the doomed method\n  def remove_me\n    2\n  end\n\n           def also_keep\n    3\n  end\nend\n",
    );
    let out = rwr(&[
        "rewrite",
        "def remove_me; $B; end",
        dir.path().to_str().unwrap(),
        "-d",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let after = std::fs::read_to_string(dir.path().join("fixture.rb")).expect("read");
    assert_eq!(
        after,
        "class Widget\n  def keep_me\n    1\n  end\n\n           def also_keep\n    3\n  end\nend\n",
        "comment gone, spacing unchanged"
    );
}

/// A doc block is part of the method, however many lines it runs to -- and the
/// comment belonging to the *neighbour* is not.
#[test]
fn delete_takes_the_whole_doc_block() {
    let dir = fixture(
        "class Widget\n  # Keep this one.\n  def keep; 1; end\n\n           # Computes the legacy total.\n  #\n  # @return [Integer]\n           # @deprecated\n  def legacy_total(x)\n    x * 2\n  end\n\n           def after; 3; end\nend\n",
    );
    let out = rwr(&[
        "rewrite",
        "def legacy_total($A); $B; end",
        dir.path().to_str().unwrap(),
        "-d",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let after = std::fs::read_to_string(dir.path().join("fixture.rb")).expect("read");
    assert_eq!(
        after,
        "class Widget\n  # Keep this one.\n  def keep; 1; end\n\n           def after; 3; end\nend\n"
    );
}

/// `-d` is a name for the empty template; `-r ''` and `rewrite: ''` mean the
/// same thing, because one mechanism is easier to trust than three.
#[test]
fn every_spelling_of_deletion_agrees() {
    let expected = "class W\n  def a; 1; end\n\n  def b; 3; end\nend\n";
    for args in [vec!["-d"], vec!["-r", ""]] {
        let dir =
            fixture("class W\n  def a; 1; end\n\n  def doomed; 2; end\n\n  def b; 3; end\nend\n");
        let mut call = vec![
            "rewrite",
            "def doomed; $B; end",
            dir.path().to_str().unwrap(),
        ];
        call.extend(args);
        let out = rwr(&call);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("fixture.rb")).expect("read"),
            expected
        );
    }
}

/// Deleting a sub-expression is not deletion. `x = a.name` would become `x = `
/// and swallow the line below into `x = y`, which still parses -- the clean,
/// confident, wrong rewrite this design exists to prevent.
#[test]
fn delete_refuses_a_partial_match() {
    let dir = fixture("def go\n  x = a.display_name\n  y = 2\nend\n");
    let before = std::fs::read_to_string(dir.path().join("fixture.rb")).expect("read");

    let out = rwr(&[
        "rewrite",
        "$R.display_name",
        dir.path().to_str().unwrap(),
        "-d",
    ]);
    assert_eq!(out.status.code(), Some(5), "refused: {}", stderr(&out));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("fixture.rb")).expect("read"),
        before,
        "a refusal leaves the file alone"
    );
}

/// YAML's flow mapping reads better than three indented lines, but inside
/// `{ ... }` a comma belongs to YAML -- so `{ contains: log($A, $B) }` arrives
/// as `log($A`. That must be loud: it had been swallowed, leaving a rule that
/// ran clean and matched nothing.
#[test]
fn a_pattern_yaml_truncated_refuses_loudly() {
    let dir = fixture("log(a, b)\n");
    let rule = dir.path().join("r.yml");
    std::fs::write(
        &rule,
        "match: $R\nwhere:\n  $R: { contains: log($A, $B) }\n",
    )
    .expect("write");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    // The cause, not just the symptom: serde reports it as an unknown field,
    // which describes what YAML did without saying why.
    assert!(stderr(&out).contains("Quote it"), "{}", stderr(&out));

    // Quoted, the same rule works -- and flow style needs no quotes at all
    // when the pattern holds none of YAML's structural characters.
    std::fs::write(
        &rule,
        "match: $R\nwhere:\n  $R: { contains: \"log($A, $B)\" }\n",
    )
    .expect("write");
    let ok = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    assert_ne!(ok.status.code(), Some(3), "{}", stderr(&ok));
}

/// A template whose tags do not stitch into valid Ruby is not silently skipped.
///
/// It falls back to the text search, which says it is weaker -- the one thing
/// that must not happen is for the file to vanish from the account, because
/// then a rename under-reports and says nothing about it.
#[test]
fn an_unstitchable_template_falls_back_to_text() {
    let dir = fixture("class Account\n  def display_name; 1; end\nend\n");
    // `end` with nothing opened: valid ERB, not valid Ruby.
    std::fs::write(
        dir.path().join("broken.html.erb"),
        "<% end %>\n<p><%= account.display_name %></p>\n",
    )
    .expect("write");
    let rule = dir.path().join("r.yml");
    std::fs::write(&rule, "method: Account#display_name\nrename: full_name\n").expect("write");

    let out = rwr(&[
        "check",
        rule.to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ]);
    let err = stderr(&out);
    assert!(err.contains("found by text search"), "{err}");
    assert!(err.contains("broken.html.erb"), "{err}");
}

/// Completions, and `--completions` on its own uses the shell you are in --
/// naming your own shell to a tool already running inside it is friction
/// nobody reports and everybody feels.
#[test]
fn completions_generate_and_default_to_the_current_shell() {
    for shell in ["bash", "zsh", "fish"] {
        let out = rwr(&["--completions", shell]);
        assert_eq!(out.status.code(), Some(0), "{shell}: {}", stderr(&out));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("rwr"),
            "{shell} script does not mention the binary"
        );
    }

    let guessed = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .arg("--completions")
        .env("SHELL", "/bin/zsh")
        .output()
        .expect("binary runs");
    assert_eq!(guessed.status.code(), Some(0), "{}", stderr(&guessed));
    assert!(
        String::from_utf8_lossy(&guessed.stdout).contains("#compdef"),
        "should have produced a zsh script"
    );

    // A shell it cannot name is a usage error saying what to pass, not a
    // silent empty script that looks like it worked.
    let unknown = Command::new(env!("CARGO_BIN_EXE_rwr"))
        .arg("--completions")
        .env("SHELL", "/opt/nonesuch")
        .output()
        .expect("binary runs");
    assert_eq!(unknown.status.code(), Some(2), "{}", stderr(&unknown));
    assert!(
        stderr(&unknown).contains("--completions bash"),
        "{}",
        stderr(&unknown)
    );
}

/// No arguments is a usage error, not a silent no-op.
#[test]
fn bare_invocation_explains_itself() {
    let out = rwr(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--help"), "{}", stderr(&out));
}
