//! The documented examples, actually run.
//!
//! `cli_e2e.rs` drives the binary; nothing drove the docs. An example that had
//! stopped matching the engine looked exactly like a working one, because both
//! exit 0 — and two shipped that way, including a `-d` deletion whose pattern
//! matched a single arity, deleted nothing, and exited 0.
//!
//! So an exit code is not the assertion. Each example must produce the effect
//! its verb promises: `find` finds, `check` reports work, `rewrite` changes a
//! file, `test` passes its fixtures. That is what catches a deletion example
//! that deletes nothing.
//!
//! **Extracted**: every line beginning `rwr` inside a ```sh or ```bash fence in
//! `README.md`, `docs/*.md` (public only — `docs/internal/` is not a manual)
//! and `claude/rwr-skill.md`.
//!
//! **Not extracted, and how an author says so in the doc itself:**
//!
//! | In the doc | Means |
//! |---|---|
//! | ```` ```sh ignore ```` | the block is illustrative, not runnable |
//! | a `<placeholder>` on the line | syntax being described, not a command |
//! | a line not starting with `rwr` | not an rwr example (`cargo install rwr`) |
//!
//! There is deliberately no skip list here. A second copy of "which examples
//! are real" would drift from the docs, and a doc author would never see it.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------- extraction

struct Example {
    doc: String,
    line: usize,
    command: String,
}

/// Drop a trailing `# comment`. Quote-aware, because `'Account#display_name'`
/// is the notation the docs are mostly about.
fn strip_trailing_comment(line: &str) -> &str {
    let mut quote: Option<u8> = None;
    for (i, &c) in line.as_bytes().iter().enumerate() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'\'' || c == b'"' => quote = Some(c),
            None if c == b'#' => return line[..i].trim_end(),
            None => {}
        }
    }
    line.trim_end()
}

/// `<rule>` is a hole in a syntax sketch. `--sarif > rwr.sarif` is a redirect,
/// so the opening bracket is what distinguishes them.
fn has_placeholder(cmd: &str) -> bool {
    cmd.split_once('<')
        .is_some_and(|(_, rest)| rest.contains('>'))
}

fn extract(doc: &Path) -> Vec<Example> {
    let text = std::fs::read_to_string(doc).expect("doc is readable");
    let name = doc
        .file_name()
        .expect("doc has a name")
        .to_string_lossy()
        .into_owned();

    let mut out = Vec::new();
    let mut runnable = false;
    let mut open = false;

    for (i, raw) in text.lines().enumerate() {
        if let Some(info) = raw.strip_prefix("```") {
            runnable = if open {
                false
            } else {
                let lang = info.split_whitespace().next().unwrap_or("");
                matches!(lang, "sh" | "bash") && !info.split_whitespace().any(|w| w == "ignore")
            };
            open = !open;
            continue;
        }
        if !runnable {
            continue;
        }
        let cmd = strip_trailing_comment(raw.trim());
        if cmd != "rwr" && !cmd.starts_with("rwr ") {
            continue;
        }
        if has_placeholder(cmd) {
            continue;
        }
        out.push(Example {
            doc: name.clone(),
            line: i + 1,
            command: cmd.to_string(),
        });
    }
    out
}

/// The public manuals. `docs/` is read non-recursively on purpose, so a new
/// guide is gated automatically while `docs/internal/` stays out.
fn doc_files(root: &Path) -> Vec<PathBuf> {
    let mut guides: Vec<PathBuf> = std::fs::read_dir(root.join("docs"))
        .expect("docs/ exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    guides.sort();

    let mut all = vec![root.join("README.md"), root.join("claude/rwr-skill.md")];
    all.append(&mut guides);
    for p in &all {
        assert!(p.exists(), "{} is gone — fix this list", p.display());
    }
    all
}

// --------------------------------------------------------------- expectations

/// What the example's own verb promises. Derived from the command, never from
/// a table keyed by doc line — a table would be the drifting second source.
#[derive(Clone, Copy, PartialEq)]
enum Expect {
    /// `find`, and the bare-pattern shorthand: exit 0 with sites.
    Matches,
    /// `check`: exit 1, and it names the work it found.
    Work,
    /// `rewrite`: exit 0, and a file on disk is different.
    Changes,
    /// `test` and the flag-only invocations: it runs and succeeds.
    Succeeds,
    /// `--sarif`: exit 0 or 1, and the document it emitted has results.
    Sarif,
}

fn expectation(cmd: &str) -> Expect {
    if cmd.contains("--sarif") {
        return Expect::Sarif;
    }
    match cmd.split_whitespace().nth(1).unwrap_or("") {
        "check" => Expect::Work,
        "rewrite" => Expect::Changes,
        "test" => Expect::Succeeds,
        "find" => Expect::Matches,
        v if v.starts_with('-') => Expect::Succeeds,
        _ => Expect::Matches,
    }
}

/// `-j` is how the outcome is read back. It changes the output format, never
/// what the run did, so the example still proves what the doc claims.
fn instrumented(cmd: &str, expect: Expect) -> String {
    let already = cmd.split_whitespace().any(|w| w == "-j" || w == "-J");
    if already || matches!(expect, Expect::Succeeds | Expect::Sarif) {
        cmd.to_string()
    } else {
        format!("{cmd} -j")
    }
}

fn count(doc: &serde_json::Value, key: &str) -> usize {
    doc.get(key).and_then(|v| v.as_array()).map_or(0, Vec::len)
}

/// `Ok` or the one sentence that says what the example failed to do.
fn verdict(
    expect: Expect,
    code: Option<i32>,
    stdout: &str,
    cwd: &Path,
    cmd: &str,
) -> Result<(), String> {
    if expect == Expect::Sarif {
        if !matches!(code, Some(0 | 1)) {
            return Err(format!("exited {code:?}; --sarif should exit 0 or 1"));
        }
        let Some(target) = cmd.rsplit_once('>').map(|(_, f)| f.trim()) else {
            return Ok(());
        };
        let written = std::fs::read_to_string(cwd.join(target))
            .map_err(|e| format!("wrote no {target}: {e}"))?;
        let sarif: serde_json::Value =
            serde_json::from_str(&written).map_err(|e| format!("{target} is not JSON: {e}"))?;
        let results = sarif["runs"][0]["results"].as_array().map_or(0, Vec::len);
        return if results == 0 {
            Err("emitted SARIF with no results".into())
        } else {
            Ok(())
        };
    }

    // `test` prints its tally and `--completions` its script, so a silent
    // success would mean the example ran and produced nothing.
    if expect == Expect::Succeeds {
        return match code {
            Some(0) if !stdout.trim().is_empty() => Ok(()),
            Some(0) => Err("exited 0 but printed nothing".into()),
            _ => Err(format!("exited {code:?}, not 0")),
        };
    }

    let doc: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|e| format!("did not print a -j document ({e}); exited {code:?}"))?;

    match expect {
        Expect::Matches => {
            if code != Some(0) {
                Err(format!("exited {code:?}, not 0 — it found nothing"))
            } else if count(&doc, "matches") == 0 {
                Err("matched nothing".into())
            } else {
                Ok(())
            }
        }
        Expect::Work => {
            if code != Some(1) {
                Err(format!("exited {code:?}; check exits 1 when there is work"))
            } else if count(&doc, "changed") + count(&doc, "findings") == 0 {
                Err("reported no work".into())
            } else {
                Ok(())
            }
        }
        // The historical `-d` defect lands here: residue, an untouched file,
        // and exit 0.
        Expect::Changes => {
            if code != Some(0) {
                Err(format!("exited {code:?}, not 0"))
            } else if count(&doc, "changed") == 0 {
                // Residue is the usual culprit: the pattern named the sites and
                // then matched none of them.
                Err(format!(
                    "changed nothing; {} occurrence(s) came back as residue",
                    count(&doc, "residue")
                ))
            } else {
                Ok(())
            }
        }
        Expect::Succeeds | Expect::Sarif => unreachable!("handled above"),
    }
}

// ------------------------------------------------------------------- fixture

const ACCOUNT_RB: &str = r##"class Account
  def display_name
    @display_name
  end

  def self.display_name
    "Account"
  end

  def summary
    "#{display_name} account"
  end

  # legacy_total and legacy carry no one-parameter spelling on purpose: the
  # docs say to write `(*$A)` because it covers every arity, and a fixture
  # holding the one-parameter version would let `($A)` pass too.
  def legacy_total
    0
  end

  def legacy_total(base, rate)
    base * rate
  end

  def legacy
    nil
  end

  def legacy(base, rate)
    base + rate
  end

  def lookup(id)
    Account.find(id)
  end

  def first_open(rows)
    rows.select { |r| r.open? }.first
  end

  def missing
    return nil
  end

  def wrapped(x)
    foo(x)
  end
end
"##;

const COMPANY_RB: &str = "class Company
  def display_name
    @name
  end

  def blank
    return nil
  end
end
";

const PREMIUM_RB: &str = r#"class PremiumAccount < Account
  def display_name
    "premium #{super}"
  end
end
"#;

const REPORT_RB: &str = "class Report
  def initialize(rows)
    @rows = rows
  end

  def first_ready
    @rows.select { |r| r.ready? }.first
  end

  def empty
    return nil
  end

  # performance/detect is held back as unsafe, so the safe half of that family
  # needs a site of its own for `check performance` to have work.
  def replay(xs)
    xs.reverse.each { |x| p x }
  end
end
";

const SYNC_RB: &str = "class Sync
  def run(account)
    account.display_name
    Account.display_name
    foo(account)
  end
end
";

/// Findings sit between lines 3 and 15, which is the range `app/x.rb:3-15`
/// names in four of the docs.
const X_RB: &str = "class Widget
  def a
    return nil
  end

  def b(rows)
    rows.select { |r| r.ok? }.first
  end

  def c
    return nil
  end

  def d(x)
    foo(x)
  end
end
";

const SPEC_RB: &str = r#"RSpec.describe Account do
  it "has a display_name" do
    expect(Account.new.display_name).to be_nil
  end
end
"#;

const DETECT_YML: &str = r#"description: Prefer detect over select-then-first.
match: $R.$SEL { |$P| $B }.first
where:
  $SEL: { name: [select, find_all] }
rewrite: $R.detect { |$P| $B }
tests:
  - input: "a = xs.select { |x| x.ok? }.first\n"
    output: "a = xs.detect { |x| x.ok? }\n"
  - input: "a = xs.select { |x| x.ok? }.last\n"
    unchanged: true
"#;

const RENAME_YML: &str = "method: Account#display_name
rename: full_name
";

/// A later commit and an uncommitted edit, so `--since main` and `--diff` each
/// have something of their own to see.
const BRANCH_RB: &str = "
class Branch
  def code
    return nil
  end
end
";

const DRAFT_RB: &str = "
class Draft
  def slug
    return nil
  end
end
";

fn git(dir: &Path, args: &[&str]) {
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

fn write(path: PathBuf, body: &str) {
    std::fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

/// A Rails-shaped repo holding exactly what the documented examples name:
/// `app/models`, `app/services`, `app/x.rb`, `rule.yml`, `my-rules/`, a `main`
/// branch to diff against.
fn build_fixture(root: &Path) {
    write(root.join(".ruby-version"), "3.3\n");
    write(root.join("app/models/account.rb"), ACCOUNT_RB);
    write(root.join("app/models/company.rb"), COMPANY_RB);
    write(root.join("app/models/premium_account.rb"), PREMIUM_RB);
    write(root.join("app/services/report.rb"), REPORT_RB);
    write(root.join("app/jobs/sync.rb"), SYNC_RB);
    write(root.join("app/x.rb"), X_RB);
    write(root.join("spec/account_spec.rb"), SPEC_RB);
    write(root.join("rule.yml"), DETECT_YML);
    write(root.join("my-rules/detect.yml"), DETECT_YML);
    write(root.join("rename.yml"), RENAME_YML);

    git(root, &["init", "-q", "--initial-branch=main", "."]);
    git(root, &["config", "user.email", "t@e.st"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);

    git(root, &["checkout", "-q", "-b", "feature"]);
    let company = root.join("app/models/company.rb");
    write(company.clone(), &format!("{COMPANY_RB}{BRANCH_RB}"));
    git(root, &["commit", "-qam", "feature"]);

    let report = root.join("app/services/report.rb");
    write(report, &format!("{REPORT_RB}{DRAFT_RB}"));
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readdir") {
        let entry = entry.expect("entry");
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
}

// ---------------------------------------------------------------- the gate

#[test]
fn every_documented_example_still_works() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let examples: Vec<Example> = doc_files(root).iter().flat_map(|d| extract(d)).collect();
    assert!(
        examples.len() >= 50,
        "only {} example(s) extracted — the docs moved or the extractor broke, \
         and a gate that runs nothing passes silently",
        examples.len()
    );

    let tmp = tempfile::tempdir().expect("temp dir");
    let pristine = tmp.path().join("fixture");
    build_fixture(&pristine);

    // A shim on PATH, so each command runs with the exact text the doc shows.
    let shim = tmp.path().join("bin");
    std::fs::create_dir_all(&shim).expect("mkdir");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_rwr"), shim.join("rwr")).expect("symlink");
    let path = format!(
        "{}:{}",
        shim.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut failures = Vec::new();
    for (n, ex) in examples.iter().enumerate() {
        let cwd = tmp.path().join(format!("run{n}"));
        copy_dir(&pristine, &cwd);

        let expect = expectation(&ex.command);
        let line = instrumented(&ex.command, expect);
        let out = Command::new("sh")
            .arg("-c")
            .arg(&line)
            .current_dir(&cwd)
            .env("PATH", &path)
            // The docs write these two; a test shell has neither.
            .env("GITHUB_BASE_REF", "main")
            .env("SHELL", "/bin/bash")
            .output()
            .expect("shell runs");

        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Err(why) = verdict(expect, out.status.code(), &stdout, &cwd, &line) {
            failures.push(format!(
                "{}:{}\n  {}\n  {why}\n  stderr: {}",
                ex.doc,
                ex.line,
                ex.command,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        std::fs::remove_dir_all(&cwd).ok();
    }

    assert!(
        failures.is_empty(),
        "{} of {} documented example(s) no longer do what the doc says:\n\n{}\n\n\
         Fix the doc, not this test. If an example is illustrative rather than \
         runnable, mark its fence ```sh ignore — never add a skip list here.",
        failures.len(),
        examples.len(),
        failures.join("\n\n")
    );
}

/// The three ways a doc opts an example out, all visible to whoever edits it.
#[test]
fn the_docs_say_which_examples_are_runnable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let md = dir.path().join("sample.md");
    std::fs::write(
        &md,
        "```sh\n\
         rwr find 'Account#display_name' app/   # a trailing comment is prose\n\
         rwr check <rule> app/\n\
         cargo install rwr\n\
         ```\n\n\
         ```sh ignore\n\
         rwr this one is illustrative\n\
         ```\n\n\
         ```yaml\n\
         rwr: not a shell fence\n\
         ```\n",
    )
    .expect("write");

    let kept: Vec<String> = extract(&md).into_iter().map(|e| e.command).collect();
    assert_eq!(kept, vec!["rwr find 'Account#display_name' app/"]);
}

/// The effect assertion, not the exit code, is what catches a doc example that
/// quietly stopped working — an exit code cannot tell a deletion that deleted
/// everything from one that deleted nothing.
#[test]
fn an_example_that_does_nothing_is_a_failure() {
    let doc: serde_json::Value =
        serde_json::from_str(r#"{"changed": [], "findings": [], "residue": [{"x": 1}]}"#)
            .expect("json");
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(verdict(Expect::Changes, Some(0), &doc.to_string(), dir.path(), "").is_err());
    assert!(verdict(Expect::Work, Some(0), &doc.to_string(), dir.path(), "").is_err());
}
