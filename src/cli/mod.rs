//! CLI surface: argument parsing, structured output, exit codes.
//!
//! The contract here is public from v0.1 (decision D17) and is specified in
//! `docs/internal/cli-conventions.md`. Conventions are inherited from `rq` so that an
//! agent which has learned one of these tools has learned the others.

use crate::engine::{Finding, Rejection, Residue};
use crate::pattern::{matcher, prefilter, prepare};
use crate::profile;
use crate::residue;
use crate::rewrite;
use crate::rule;
use crate::source;
use clap::{Args, CommandFactory, Parser, Subcommand};
use rayon::prelude::*;
use serde::Serialize;
use std::process::ExitCode;

/// Process exit status.
///
/// The split that matters is [`Exit::Retryable`] vs [`Exit::Refused`]: an agent
/// branches on the exit code before it parses any JSON, and collapsing the two
/// would make it either abandon recoverable work or spin on unrecoverable work.
// Variants land as the verbs are implemented; the numeric map is already a
// public contract and is pinned by `exit_codes_are_stable`. Drop this allow
// once every verb constructs its own statuses.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Verb-dependent success. `find`: matched. `check`: clean. `rewrite`:
    /// applied, *or* nothing to apply -- writing nothing is not a failure, so
    /// `rewrite` never returns [`Exit::Negative`].
    Ok,
    /// Verb-dependent negative result — not an error in either polarity.
    /// `find`: nothing matched. `check`: violations found.
    Negative,
    /// I/O, internal, or usage failure. `2` because grep, ripgrep, ruff,
    /// rubocop, biome, jq and semgrep all agree it means "something went
    /// wrong" — an agent that learned any of them would misread anything else.
    Error,
    /// The *pattern or rule* failed to parse, as distinct from a source file
    /// failing to parse. jq splits these the same way (compile vs runtime).
    PatternError,
    /// Matches were skipped because they sat inside a rewritten range.
    /// Rerunning the same command makes progress.
    Retryable,
    /// Ambiguity that needs judgement. Rerunning changes nothing.
    Refused,
}

impl Exit {
    /// The numeric status. Public contract — see the table in
    /// `docs/internal/cli-conventions.md`; `exit_codes_are_stable` pins it.
    pub(crate) fn code(self) -> u8 {
        match self {
            Exit::Ok => 0,
            Exit::Negative => 1,
            Exit::Error => 2,
            Exit::PatternError => 3,
            Exit::Retryable => 4,
            Exit::Refused => 5,
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(e: Exit) -> Self {
        ExitCode::from(e.code())
    }
}

/// How results are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Output {
    /// Human-readable. Progress UI is allowed only in this mode, and only on a TTY.
    Text,
    /// One pretty-printed array (multi-row) or object (single result).
    Json,
    /// One compact object per line.
    Ndjson,
}

/// Flags shared by every subcommand.
///
/// Every command that prints anything honors `--json`/`--ndjson`, not just the
/// search path — see `docs/internal/cli-conventions.md`.
#[derive(Debug, Args)]
pub(crate) struct Common {
    /// Emit results as JSON (pretty array or object).
    #[arg(short = 'j', long, global = true)]
    json: bool,

    /// Emit results as newline-delimited JSON, one compact object per line.
    #[arg(short = 'J', long, global = true, conflicts_with = "json")]
    ndjson: bool,

    /// Restrict to files under this repo-relative directory (repeatable).
    #[arg(short = 'p', long, value_name = "DIR", global = true)]
    path: Vec<String>,

    /// Include generated and vendored code, which is skipped by default.
    #[arg(long, global = true)]
    include_vendored: bool,

    /// Explain each result: which constraint declined a candidate, and why a
    /// rule was held back.
    ///
    /// Scoped, this is the rule-authoring loop: `rwr check r.yml app.rb:5 -e`
    /// says what stopped the match at one site.
    #[arg(short = 'e', long, global = true)]
    explain: bool,

    /// Restrict to lines that are not committed yet -- the pre-commit case.
    ///
    /// What makes `check` adoptable on a codebase that has never run it: a rule
    /// with two thousand pre-existing sites must not fail a pull request that
    /// added three.
    ///
    /// Takes no value, deliberately. As `--diff [<REV>]` it swallowed a
    /// following path as its revision, so `--diff app/` built the range
    /// `app/...` and failed inside git. Deciding by looking at whether `app/`
    /// exists on disk would be the guess D31 already refused for `-r`.
    #[arg(long, global = true)]
    diff: bool,

    /// Restrict to lines this branch introduces, as `REV...HEAD`. With
    /// `--diff`, the working tree too.
    ///
    /// The CI half of the pair: a pull-request gate knows its base branch
    /// (`--since "$GITHUB_BASE_REF"`) but not which lines moved.
    #[arg(long, global = true, value_name = "REV")]
    since: Option<String>,

    /// The Ruby version to target, e.g. `3.1`.
    ///
    /// Detected from `.ruby-version`, a Gemfile `ruby` line, or a gemspec's
    /// `required_ruby_version` when not given.
    #[arg(long, global = true, value_name = "X.Y")]
    ruby: Option<String>,

    /// Include rules that can change behaviour, printing why for each.
    ///
    /// Ruby is dynamically typed, so most interesting rewrites have an input
    /// that breaks them. Those rules are held back by default and the run says
    /// how many, rather than reporting a zero that reads like a clean tree.
    #[arg(long = "unsafe", global = true)]
    unsafe_rules: bool,

    /// Report where the time went, as a phase table on stderr.
    ///
    /// `RWR_PROFILE` in the environment enables it too, so a shipped binary can
    /// be measured in place.
    #[arg(long, global = true)]
    profile: bool,
}

/// The shell rwr is being run from, if `$SHELL` names one it can generate for.
///
/// Naming your own shell to a tool that is already running inside it is the
/// kind of small friction nobody reports and everybody feels.
fn current_shell() -> Option<clap_complete::Shell> {
    clap_complete::Shell::from_shell_path(std::env::var_os("SHELL")?)
}

/// The replacement a run was given, with `--delete` spelled as the empty one.
fn template(replace: Option<&str>, delete: bool) -> Option<&str> {
    if delete { Some("") } else { replace }
}

/// Where this run is pointed, for questions that are properties of the *repo*
/// rather than of a file: which git repository, and which Ruby version.
fn scope_start(paths: &[String], common: &Common) -> std::path::PathBuf {
    paths
        .first()
        .or_else(|| common.path.first())
        .map_or_else(|| std::path::PathBuf::from("."), std::path::PathBuf::from)
}

/// The paths a run was given, resolved into somewhere to walk and -- when they
/// carried `:N` or `:N-M` suffixes -- the lines to restrict to.
///
/// A path that does not exist is an error rather than an empty walk. `rwr check
/// all app/typo` exited 0 and reported a clean tree, which in CI is a green gate
/// that checked nothing: the same vacuous pass that ruled out guessing a default
/// branch.
fn targets(
    paths: &[String],
    common: &Common,
) -> Result<(Vec<String>, Option<crate::diff::Changed>), String> {
    let mut walk: Vec<String> = Vec::new();
    let mut lines: Vec<(std::path::PathBuf, (u32, u32))> = Vec::new();
    let mut bare: Vec<&str> = Vec::new();

    let exists = |p: &str| std::path::Path::new(p).exists();
    for arg in paths.iter().chain(common.path.iter()) {
        match crate::diff::split_lines(arg)? {
            // A file named `foo.rb:3` is legal and vanishingly rare, so the
            // literal reading is the fallback rather than a coin flip -- and
            // when neither reading exists the error names both.
            Some(crate::diff::Lines { path, range }) if exists(path) => {
                let absolute = std::path::Path::new(path)
                    .canonicalize()
                    .map_err(|e| format!("cannot resolve {path}: {e}"))?;
                lines.push((absolute, range));
                walk.push(path.to_string());
            }
            Some(crate::diff::Lines { path, .. }) if !exists(arg) => {
                return Err(format!("no such path: {path} (nor {arg})"));
            }
            // `--diff main` used to work. It now reads `main` as a path,
            // which is right but unhelpful on its own.
            _ if !exists(arg) && common.diff => {
                return Err(format!(
                    "no such path: {arg} -- for a revision, --since {arg}"
                ));
            }
            _ if !exists(arg) => return Err(format!("no such path: {arg}")),
            _ => {
                bare.push(arg);
                walk.push(arg.clone());
            }
        }
    }

    if lines.is_empty() {
        return Ok((walk, None));
    }
    // Mixing the two would silently drop the unscoped paths: a `Changed` covers
    // the files it names and nothing else, so `app/ lib/x.rb:3` would check
    // three lines and call `app/` clean.
    if let Some(unscoped) = bare.first() {
        return Err(format!(
            "{unscoped} names no lines, but another path does -- give every path \
             a `:N` range, or none"
        ));
    }
    if common.diff || common.since.is_some() {
        return Err(
            "--diff/--since and a `:N` range are two answers to which lines to check; \
             give one"
                .to_string(),
        );
    }
    Ok((walk, Some(crate::diff::Changed::from_lines(lines))))
}

/// Narrow a walked file list to the files a change touched.
///
/// Cheap and first: a diff-scoped run over a large repo should not read files
/// the change never went near.
fn only_changed(
    files: Vec<std::path::PathBuf>,
    changed: Option<&crate::diff::Changed>,
) -> Vec<std::path::PathBuf> {
    let Some(changed) = changed else { return files };
    files
        .into_iter()
        .filter(|p| {
            p.canonicalize()
                .is_ok_and(|absolute| changed.covers(&absolute))
        })
        .collect()
}

impl Common {
    /// The changed lines to restrict to, when `--diff` was given.
    ///
    /// `Err` is a hard failure rather than an empty scope: "no lines changed"
    /// and "git could not tell me" produce the same clean exit otherwise, and
    /// only one of them means the tree is clean.
    fn changed(&self, start: &std::path::Path) -> Result<Option<crate::diff::Changed>, String> {
        if !self.diff && self.since.is_none() {
            return Ok(None);
        }
        crate::diff::from_git(self.since.as_deref(), self.diff, start).map(Some)
    }

    pub(crate) fn output(&self) -> Output {
        match (self.json, self.ndjson) {
            (_, true) => Output::Ndjson,
            (true, _) => Output::Json,
            _ => Output::Text,
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "rwr", version, about, long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Shorthand: the pattern to find. `rwr 'foo($A)'` is `rwr find 'foo($A)'`;
    /// adding `-r` makes it `rwr check`.
    ///
    /// The shorthand is **read-only by construction** — writing always requires
    /// typing `rewrite`, so terseness never buys a foot-gun (D30).
    #[arg(value_name = "PATTERN", value_hint = clap::ValueHint::Other)]
    pattern: Option<String>,

    /// Files or directories to search, rg-style. Sugar for repeated `--path`.
    #[arg(value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,

    /// Replacement template — previews the diff. A flag rather than a second
    /// positional so that trailing arguments are unambiguously paths: deciding
    /// between the two by probing the filesystem would be a guess, and
    /// principle 2 is refuse rather than guess (D31).
    #[arg(short = 'r', long = "replace", value_name = "TEMPLATE")]
    replace: Option<String>,

    /// Delete what matches, rather than replacing it — previews the diff.
    #[arg(short = 'd', long = "delete", conflicts_with = "replace")]
    delete: bool,

    /// Print a shell completion script. Defaults to the shell you are in.
    #[arg(long, value_name = "SHELL", num_args = 0..=1)]
    completions: Option<Option<clap_complete::Shell>>,

    #[command(flatten)]
    common: Common,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Find code matching a structural pattern. Read-only.
    ///
    /// Reports every match including nested ones, with nesting metadata —
    /// find is observation, and suppressing would be a lie (decision D15).
    Find {
        /// Ruby source with `$METAVAR` placeholders, e.g. `foo($A, $B)`.
        #[arg(value_name = "PATTERN", value_hint = clap::ValueHint::Other)]
        pattern: String,

        /// Files or directories to search. Sugar for repeated `--path`.
        #[arg(value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<String>,
    },

    /// Show what a rule would change, without writing. Serves both CI
    /// enforcement and human preview.
    ///
    /// Polarity is deliberately inverted from `find`: a rule that matches
    /// nothing is the *success* state here, because pre-commit's contract is
    /// "exit nonzero on failure" and a clean tree must not block a commit.
    /// That same polarity reads correctly as a preview — exit 1 means "there is
    /// work to do." ast-grep splits `run` and `scan` the same way.
    Check {
        /// A rule file or directory of them, or a bare pattern with `-r`.
        #[arg(value_name = "RULE", value_hint = clap::ValueHint::AnyPath)]
        rule: String,

        /// Files or directories to scope to. Sugar for repeated `--path`.
        #[arg(value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<String>,

        /// Replacement template. Given, `rule` is read as a pattern, not a file.
        #[arg(short = 'r', long = "replace", value_name = "TEMPLATE")]
        replace: Option<String>,

        /// Delete what matches, rather than replacing it. Given, `rule` is read
        /// as a pattern, not a file.
        ///
        /// Deletion takes the whole *unit*: the match, the comments written
        /// directly above it, and its line. `-r ''` means the same thing and is
        /// harder to read.
        #[arg(short = 'd', long = "delete", conflicts_with = "replace")]
        delete: bool,
    },

    /// Run a rule's own fixtures, pinning what it does.
    ///
    /// The object is the *rule*, not a codebase -- which is why this is a verb
    /// rather than a mode of `check`. It walks nothing and takes no paths, so
    /// two thirds of `check`'s flags would have had to be rejected as a mode.
    Test {
        /// A rule file, a directory of them, or a built-in name.
        #[arg(value_name = "RULE", value_hint = clap::ValueHint::AnyPath)]
        rule: String,
    },

    /// Apply a rewrite rule, writing the changes to disk.
    ///
    /// The verb carries the mode — there is no `--write` or `--dry-run`, because
    /// a command named `rewrite` that did not rewrite would be a mismatch no
    /// documentation fixes. To see what would happen, use `check` (D29).
    Rewrite {
        /// A rule file, or a bare pattern with `-r`.
        #[arg(value_name = "RULE", value_hint = clap::ValueHint::AnyPath)]
        rule: String,

        /// Files or directories to scope to. Sugar for repeated `--path`.
        #[arg(value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<String>,

        /// Replacement template. Given, `rule` is read as a pattern, not a file.
        #[arg(short = 'r', long = "replace", value_name = "TEMPLATE")]
        replace: Option<String>,

        /// Delete what matches, rather than replacing it. Given, `rule` is read
        /// as a pattern, not a file.
        ///
        /// Deletion takes the whole *unit*: the match, the comments written
        /// directly above it, and its line. `-r ''` means the same thing and is
        /// harder to read.
        #[arg(short = 'd', long = "delete", conflicts_with = "replace")]
        delete: bool,
    },
}

/// Parse arguments and dispatch. Returns the process exit status.
pub fn run() -> ExitCode {
    let cli = Cli::parse();

    // First: this prints a script and exits, so it must not depend on the rest
    // of the arguments making sense.
    if let Some(asked) = cli.completions {
        let Some(shell) = asked.or_else(current_shell) else {
            eprintln!(
                "rwr: cannot tell which shell this is — name one: \
                 --completions bash|zsh|fish|elvish|powershell"
            );
            return Exit::Error.into();
        };
        clap_complete::generate(shell, &mut Cli::command(), "rwr", &mut std::io::stdout());
        return Exit::Ok.into();
    }

    let out = cli.common.output();
    profile::enable_from(cli.common.profile);

    // The shorthand desugars to a verb; it can only ever reach a read-only one.
    let command = match (cli.command, cli.pattern) {
        (Some(c), _) => c,
        (None, Some(pattern)) => match (cli.replace, cli.delete) {
            (None, false) => Command::Find {
                pattern,
                paths: cli.paths,
            },
            // Still read-only: the shorthand previews, and writing always
            // requires typing `rewrite` (D30).
            (replace, delete) => Command::Check {
                rule: pattern,
                paths: cli.paths,
                replace,
                delete,
            },
        },
        (None, None) => {
            eprintln!("rwr: give a pattern, or a subcommand — see `rwr --help`");
            return Exit::Error.into();
        }
    };

    match command {
        Command::Find { pattern, paths } => cmd_find(&pattern, &paths, &cli.common, out),
        Command::Test { rule } => cmd_test(&rule, out),
        // `-d` is `-r ''` with a name: an empty template is a deletion, and
        // spelling it as a flag says so out loud.
        Command::Check {
            rule,
            paths,
            replace,
            delete,
        } => cmd_apply(
            &rule,
            &paths,
            template(replace.as_deref(), delete),
            false,
            &cli.common,
            out,
        ),
        Command::Rewrite {
            rule,
            paths,
            replace,
            delete,
        } => cmd_apply(
            &rule,
            &paths,
            template(replace.as_deref(), delete),
            true,
            &cli.common,
            out,
        ),
    }
}

/// Emit a row set: `--json` one pretty array, `--ndjson` one compact object
/// per line (D23). Returns `Some(exit)` only on a serialisation failure.
/// Emit one JSON document, rather than a list of them.
///
/// `-j` is a document and `-J` a stream, so a single report is an object under
/// the first and one line under the second. Serialising it through the row path
/// wrapped it in a one-element array, which every consumer then had to index
/// past for no reason.
fn emit_document<T: Serialize>(out: Output, value: &T) -> Option<ExitCode> {
    let rendered = match out {
        Output::Json => serde_json::to_string_pretty(value),
        Output::Ndjson => serde_json::to_string(value),
        Output::Text => return None,
    };
    match rendered {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("rwr: {e}");
            return Some(Exit::Error.into());
        }
    }
    None
}

fn emit_rows<T: Serialize>(out: Output, rows: &[T]) -> Option<ExitCode> {
    match out {
        Output::Json => match serde_json::to_string_pretty(rows) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("rwr: {e}");
                return Some(Exit::Error.into());
            }
        },
        Output::Ndjson => {
            for row in rows {
                match serde_json::to_string(row) {
                    Ok(line) => println!("{line}"),
                    Err(e) => {
                        eprintln!("rwr: {e}");
                        return Some(Exit::Error.into());
                    }
                }
            }
        }
        Output::Text => {}
    }
    None
}

/// How many residue occurrences to list before summarising the rest.
///
/// A broad rule can leave thousands, and an unbounded list is not a report
/// anyone reads. Counts by class stay exact; only the detail is capped.
const RESIDUE_DETAIL_CAP: usize = 40;

/// Print which rules of a pack accounted for the edits.
///
/// Only when more than one fired: a single-rule run already said everything in
/// its file lines, and naming the rule there would be noise.
fn report_by_rule(changed: &[Changed]) {
    // First-seen order is the order the rules ran in, which is the order a
    // reader of the pack expects.
    let mut totals: Vec<(String, usize)> = Vec::new();
    for hit in changed.iter().flat_map(|c| &c.rules) {
        match totals.iter_mut().find(|(id, _)| *id == hit.rule) {
            Some((_, n)) => *n += hit.sites,
            None => totals.push((hit.rule.clone(), hit.sites)),
        }
    }
    if totals.len() < 2 {
        return;
    }
    let width = totals.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    println!();
    for (id, n) in &totals {
        println!("  {id:<width$}  {n} site(s)");
    }
}

/// Say what a suppression accepted, and what it no longer accepts.
///
/// Unconditional, unlike rejections. A mechanism that can silence a run must
/// never be able to silence itself -- a baseline or a directive nobody sees is
/// how RuboCop's todo file became permanent. The counts print even when the
/// reader would rather they did not.
fn report_suppressions(
    suppressed: &[crate::suppress::Suppressed],
    stale: &[crate::suppress::Stale],
    malformed: &[crate::suppress::Malformed],
) {
    if !suppressed.is_empty() {
        eprintln!(
            "rwr: {} finding(s) accepted by rwr:ignore directive(s)",
            suppressed.len()
        );
    }
    if !stale.is_empty() {
        // Reported, not failed: the drain is not forced. A stale directive
        // cannot keep silencing anything -- its finding is already gone -- so
        // what is left is tidying, and tidying does not block a commit.
        eprintln!(
            "rwr: {} stale rwr:ignore directive(s) -- nothing left to accept there:",
            stale.len()
        );
        for d in stale.iter().take(RESIDUE_DETAIL_CAP) {
            eprintln!("  {}:{}: {} -- delete the comment", d.file, d.line, d.rule);
        }
    }
    for d in malformed {
        eprintln!("rwr: {}:{}: rwr:ignore {}", d.file, d.line, d.why);
    }
}

/// Say why candidates were declined.
///
/// Behind `-e`, unlike residue: a rejection is detail about a site the rule
/// *correctly* refused, not a blind spot. The account of what rwr could not see
/// stays unconditional; this is debugging.
fn report_rejections(rejections: &[Rejection]) {
    if rejections.is_empty() {
        return;
    }
    println!();
    for r in rejections {
        let rule = r.rule.as_deref().unwrap_or("pattern");
        println!(
            "{}:{}:{}: {rule}: matched, then declined",
            r.file, r.line, r.col
        );
        match (&r.capture, &r.bound) {
            (Some(capture), Some(bound)) => {
                println!("  {capture} bound `{bound}` -- {}", r.detail);
            }
            (Some(capture), None) => println!("  {capture} -- {}", r.detail),
            _ => println!("  {}", r.detail),
        }
    }
}

/// Report what a text search found in the files rwr cannot parse.
///
/// Grep-grade, and labelled as such. Templates embed Ruby that no parser here
/// reads, so the choice is between saying nothing about them -- which makes a
/// rename under-report, the dangerous direction -- and saying something weaker
/// than usual and marking it weaker. The second is better, as long as the mark
/// is honest.
fn report_text_residue(found: &[Residue], templates: usize) {
    if templates == 0 {
        return;
    }
    if found.is_empty() {
        eprintln!(
            "\n{templates} template file(s) could not be parsed and were searched by \
             text instead; they mention nothing."
        );
        return;
    }
    eprintln!(
        "\n{} occurrence(s) in {templates} template file(s) that could not be parsed, \
         found by text search instead -- these may be comments or unrelated text:",
        found.len()
    );
    for r in found.iter().take(RESIDUE_DETAIL_CAP) {
        eprintln!("  {}:{}:{}: {}", r.file, r.line, r.col, r.text.trim());
    }
    if found.len() > RESIDUE_DETAIL_CAP {
        eprintln!("  ... and {} more", found.len() - RESIDUE_DETAIL_CAP);
    }
}

/// Print what the finding rules flagged.
///
/// Separate from the edit list because it is a different kind of answer: these
/// are shapes a human has to judge, not changes a tool is proposing.
fn report_findings(findings: &[Finding]) {
    if findings.is_empty() {
        return;
    }
    let mut rules: Vec<&str> = findings.iter().map(|f| f.rule.as_str()).collect();
    rules.sort_unstable();
    rules.dedup();
    println!(
        "\n{} finding(s) for review, no edit proposed:",
        findings.len()
    );
    for rule in rules {
        let mine: Vec<&Finding> = findings.iter().filter(|f| f.rule == rule).collect();
        let note = mine.first().map_or("", |f| f.note.as_str());
        println!("\n  {rule} — {note}");
        for f in mine.iter().take(RESIDUE_DETAIL_CAP) {
            println!("    {}:{}:{}: {}", f.file, f.line, f.col, f.text);
        }
        if mine.len() > RESIDUE_DETAIL_CAP {
            println!("    ... and {} more", mine.len() - RESIDUE_DETAIL_CAP);
        }
    }
}

/// Warn when one rule renamed across more than one class.
///
/// `Account#display_name` and `Company#display_name` are different methods. A
/// rule with no `type:` constraint renames both, at exit 0, and nothing else in
/// the tool notices -- Q10 calls this the real danger, against which the refusal
/// contract does not protect, because there is no conflict to detect.
///
/// A warning rather than a refusal: a genuinely repo-wide rename is legitimate,
/// and refusing it would train people to reach for a flag that disables the
/// check. What they need is to be told, once, with the fix.
fn report_spread(classes: &[&String]) {
    let mut distinct: Vec<&str> = classes.iter().map(|c| c.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < 2 {
        return;
    }
    eprintln!(
        "\nwarning: rewrote receivers of {} different classes ({}). These are \
         different methods that share a name -- narrow with \
         `where: {{ $R: {{ type: ... }} }}` if only one was meant.",
        distinct.len(),
        distinct.join(", ")
    );
}

/// Say why each unsafe rule that actually fired is unsafe.
///
/// At the moment of the edit, not in a config file -- the caveat is only useful
/// where the diff is.
fn report_unsafe(changed: &[Changed], rules: &[rule::Rule]) {
    let fired: Vec<&str> = changed
        .iter()
        .flat_map(|c| &c.rules)
        .map(|h| h.rule.as_str())
        .collect();
    let mut said: Vec<&str> = Vec::new();
    for r in rules {
        let (Some(id), Some(why)) = (r.id.as_deref(), r.unsafe_because.as_deref()) else {
            continue;
        };
        if fired.contains(&id) && !said.contains(&id) {
            if said.is_empty() {
                println!("\nunsafe rule(s) applied:");
            }
            said.push(id);
            println!("  {id}: {why}");
        }
    }
}

/// Print the account of what the rule could not see, grouped by class.
///
/// Grouping matters as much as the total: the classes mean different things.
/// Symbols and strings are metaprogramming reaches -- genuine blind spots.
/// Calls and definitions are usually a different method that happens to share
/// the name, which only receiver resolution can rule out.
fn report_residue(residues: &[Residue]) {
    // Printed even when the residue list is empty: a rule that accounted for
    // everything in Ruby still did not look at ERB, and a blind spot that
    // appears and vanishes with unrelated results is not a report. The caller
    // passes zero when the run makes no completeness claim at all.
    if residues.is_empty() {
        return;
    }
    let count = |c: residue::Context| residues.iter().filter(|r| r.context == c).count();
    // Name the rule when the run had one. An unlabelled block after several
    // rules fired leaves the reader to guess which one it belongs to, and a
    // real run guessed wrong.
    let mut named: Vec<&str> = residues.iter().filter_map(|r| r.rule.as_deref()).collect();
    named.sort_unstable();
    named.dedup();
    let whose = match named.as_slice() {
        [] => "this rule".to_string(),
        [one] => format!("`{one}`"),
        many => format!("{} rules ({})", many.len(), many.join(", ")),
    };
    // Every class the report can hold, so a category cannot go missing from the
    // summary while its entries sit in the list below it.
    let breakdown: Vec<String> = [
        ("symbol", residue::Context::Symbol),
        ("string", residue::Context::String),
        ("call", residue::Context::Call),
        ("definition", residue::Context::Definition),
        ("comment", residue::Context::Comment),
        ("dynamic", residue::Context::Dynamic),
    ]
    .iter()
    .filter_map(|(label, context)| match count(*context) {
        0 => None,
        n => Some(format!("{n} {label}")),
    })
    .collect();
    eprintln!(
        "\n{} occurrence(s) {whose} could not account for ({}):",
        residues.len(),
        breakdown.join(", ")
    );
    for r in residues.iter().take(RESIDUE_DETAIL_CAP) {
        // The rule is shown per line only when several contributed, since
        // repeating one name on every line is noise.
        let tag = if named.len() > 1 {
            r.rule
                .as_deref()
                .map_or(String::new(), |id| format!("[{id}] "))
        } else {
            String::new()
        };
        eprintln!(
            "  {}{}:{}:{}: {:?}: {}",
            tag, r.file, r.line, r.col, r.context, r.text
        );
    }
    if residues.len() > RESIDUE_DETAIL_CAP {
        eprintln!("  ... and {} more.", residues.len() - RESIDUE_DETAIL_CAP);
        degradation(residues);
    }
}

/// Say plainly that an account this long is not a reviewable one, and where to
/// start with it.
///
/// The old message advised narrowing the rule, which is wrong for the case that
/// produces these volumes: a rename wants *completeness*, so narrowing it would
/// make it miss sites. Q1 asks whether this degradation is honest-and-useful or
/// a polite failure, and "here are 8,074 more, try scoping" was the latter.
fn degradation(residues: &[Residue]) {
    let count = |c: residue::Context| residues.iter().filter(|r| r.context == c).count();
    let (calls, symbols) = (
        count(residue::Context::Call),
        count(residue::Context::Symbol),
    );

    eprintln!(
        "\n  This identifier is too common here for that account to be reviewed \
         one line at a time. Where to start:"
    );
    if symbols > 0 {
        eprintln!(
            "    - {symbols} symbol(s): a method name handed to something that will \
             dispatch on it. These are the ones that break -- read them first."
        );
    }
    if calls > 0 {
        eprintln!(
            "    - {calls} call(s): the receiver did not resolve, so most of these \
             are probably other classes' methods that share the name. A Sorbet \
             `sig` on what they chain through would resolve them (D62)."
        );
    }
    eprintln!("    - `-j` emits all of them, to filter by context or path.");
}

/// One structural match, as reported.
/// What a `find` run has to say, for machine consumers.
#[derive(Debug, Serialize)]
struct Matches<'a> {
    schema: u32,
    rwr_version: &'static str,
    matches: &'a [Found],
}

#[derive(Debug, Serialize)]
struct Found {
    file: String,
    line: usize,
    col: usize,
    byte_start: usize,
    byte_end: usize,
    text: String,
}

fn cmd_find(pattern: &str, paths: &[String], common: &Common, out: Output) -> ExitCode {
    let prepared = match prepare::prepare(pattern) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::PatternError.into();
        }
    };

    // Validate the pattern once, up front, for a clean error message.
    {
        let parsed = ruby_prism::parse(prepared.source.as_bytes());
        if matcher::pattern_root(&parsed.node()).is_none() {
            eprintln!("rwr: a pattern must be a single expression");
            return Exit::PatternError.into();
        }
    }

    let (scoped, named) = match targets(paths, common) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::Error.into();
        }
    };
    let changed = match named {
        Some(c) => Some(c),
        None => match common.changed(&scope_start(&scoped, common)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("rwr: {e}");
                return Exit::Error.into();
            }
        },
    };

    let (files, _templates) = profile::span_noted(
        "walk",
        || {
            let (found, templates) = source::walk(&scoped, common.include_vendored);
            (only_changed(found, changed.as_ref()), templates)
        },
        |(f, t)| format!("{} files, {} template(s)", f.len(), t.len()),
    );

    // Residue is collected across the parallel walk, so it needs a shared sink.
    let residues: std::sync::Mutex<Vec<Residue>> = std::sync::Mutex::new(Vec::new());

    // Literals the pattern requires, so a file that cannot contribute is never
    // parsed. This is what keeps the cost proportional to how many files
    // mention the identifier rather than to repository size.
    let required_literals = prefilter::required(pattern);
    let anchors_for_filter: Vec<Vec<u8>> = {
        let parsed = ruby_prism::parse(prepared.source.as_bytes());
        matcher::pattern_root(&parsed.node())
            .map(|root| residue::anchors(&root, &prepared))
            .unwrap_or_default()
    };
    let filter = prefilter::Filter::new(&required_literals, &anchors_for_filter);
    let skipped = std::sync::atomic::AtomicUsize::new(0);

    let scanning = profile::now();
    let mut found: Vec<Found> = files
        .par_iter()
        .flat_map_iter(|path| {
            let Ok(src) = std::fs::read(path) else {
                return Vec::new().into_iter();
            };
            if !filter.may_contribute(&src) {
                skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Vec::new().into_iter();
            }
            let parsed = ruby_prism::parse(&src);
            // An unparseable file is reported and skipped, never guessed at.
            if parsed.errors().count() > 0 {
                return Vec::new().into_iter();
            }
            // Prism nodes are not Sync, so the pattern tree cannot be shared
            // across threads. Reparsing it per file costs microseconds against
            // a file parse and keeps the walk parallel.
            let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
            let p_node = p_parsed.node();
            let Some(p_root) = matcher::pattern_root(&p_node) else {
                return Vec::new().into_iter();
            };
            let mut hits = matcher::search(
                &p_root,
                &parsed.node(),
                &prepared,
                &matcher::Criteria::none(),
            );
            if let Some(changed) = changed.as_ref() {
                let absolute = path.canonicalize().unwrap_or_else(|_| path.clone());
                hits.retain(|m| {
                    let (start, end) = rewrite::effective_range(&m.node);
                    let (first, _) = source::line_col(&src, start);
                    let (last, _) = source::line_col(&src, end);
                    changed.touches(&absolute, first, last)
                });
            }

            // The account of what the rule could not see (D7). Name-anchored
            // rules only: a pattern with no literal identifier has nothing to
            // track, and reports nothing.
            // Deliberately *not* scoped to files containing a match. That
            // heuristic was tried and removes the best signal: declarations
            // like `attr_accessor :autoload_paths` and `def autoload_paths`
            // live in files that declare rather than call, so they have no
            // match to co-locate with. Correct scoping needs to know which
            // class the anchor belongs to -- which is receiver resolution, and
            // therefore Phase 2.
            let anchors = residue::anchors(&p_root, &prepared);
            if !anchors.is_empty() {
                let matched: Vec<(usize, usize)> = hits
                    .iter()
                    .map(|m| {
                        let l = m.node.location();
                        (l.start_offset(), l.end_offset())
                    })
                    .collect();
                // Unscoped, and it stays that way: `find` takes a bare
                // pattern, so there is never a class to scope by. Class
                // anchoring is `check`/`rewrite`'s, where a rule names one.
                let extra = residue::find(&parsed.node(), &anchors, &matched, &src);
                if let Ok(mut sink) = residues.lock() {
                    sink.extend(extra.into_iter().map(|o| {
                        let (line, col) = source::line_col(&src, o.byte_start);
                        Residue {
                            file: path.display().to_string(),
                            line,
                            col,
                            context: o.context,
                            // `find` takes a bare pattern, which has no rule id.
                            rule: None,
                            text: source::line_at(&src, o.byte_start),
                        }
                    }));
                }
            }

            hits.iter()
                .map(|m| {
                    let loc = m.node.location();
                    let (line, col) = source::line_col(&src, loc.start_offset());
                    Found {
                        file: path.display().to_string(),
                        line,
                        col,
                        byte_start: loc.start_offset(),
                        byte_end: loc.end_offset(),
                        text: source::line_at(&src, loc.start_offset()),
                    }
                })
                .collect::<Vec<_>>()
                .into_iter()
        })
        .collect();

    found.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));

    profile::mark("scan", scanning, || {
        let skipped = skipped.load(std::sync::atomic::Ordering::Relaxed);
        format!(
            "{} matches, {} parsed, {} skipped",
            found.len(),
            files.len() - skipped,
            skipped
        )
    });

    let mut residues = residues.into_inner().unwrap_or_default();
    residues.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    match out {
        Output::Text => {
            for f in &found {
                println!("{}:{}:{}: {}", f.file, f.line, f.col, f.text);
            }
            report_residue(&residues);
        }
        _ => {
            // `-j` is one document, so it carries what produced it. `-J` is a
            // row per line by definition and cannot: a consumer choosing it has
            // chosen a stream over a document.
            let emitted = if out == Output::Json {
                emit_document(
                    out,
                    &Matches {
                        schema: REPORT_SCHEMA,
                        rwr_version: env!("CARGO_PKG_VERSION"),
                        matches: &found,
                    },
                )
            } else {
                emit_rows(out, &found)
            };
            if emitted.is_some() {
                return Exit::Error.into();
            }
        }
    }

    profile::report();
    if found.is_empty() {
        Exit::Negative.into()
    } else {
        Exit::Ok.into()
    }
}

/// A file rwr would change, or did.
#[derive(Debug, Serialize, Clone)]
struct Changed {
    file: String,
    /// Matched locations changed -- not edits. One site can take several edits
    /// when the rewrite changes shape, and a reader counts sites in the diff.
    sites: usize,
    /// Which rules of the set accounted for those edits. Empty for the inline
    /// `-r` form, which has no rule to name, so a one-pattern run's output is
    /// unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rules: Vec<RuleHits>,
}

/// The output contract's version, bumped when the shape changes.
///
/// Paired with `rwr_version` for the same reason `rwr-phase0` carries both: a
/// consumer needs to know what produced a document it is parsing, and the
/// schema number is what it can branch on without a version comparison.
///
/// 1 was a bare array of changed files, with no account of residue at all.
const REPORT_SCHEMA: u32 = 5;

/// Everything a `check` or `rewrite` run has to say, for machine consumers.
///
/// A single object rather than a bare array of changes: the changes alone are
/// only half the answer, and the half that shipped without the other was the
/// half that flatters the tool.
#[derive(Debug, Serialize)]
struct Report<'a> {
    schema: u32,
    rwr_version: &'static str,
    changed: &'a [Changed],
    /// Matches of rules that propose no edit -- lints rather than rewrites.
    findings: &'a [Finding],
    /// Occurrences found by text search in files rwr cannot parse. Kept apart
    /// from `residue` because it is a weaker kind of evidence and saying so is
    /// the point.
    template_residue: &'a [Residue],
    /// Occurrences the rule could not account for. Present and empty when the
    /// rule is name-anchored and found none; absent means it made no claim.
    residue: &'a [Residue],
    /// Template files not searched, since they embed Ruby rwr does not read.
    templates_skipped: usize,
    /// Why candidates were declined. Present only under `-e` -- absent means
    /// nobody asked, not that nothing was declined.
    #[serde(skip_serializing_if = "Option::is_none")]
    rejections: Option<&'a [Rejection]>,
    /// Findings a suppression accepted. Always present: a run that silenced
    /// something must say so in the machine-readable output too, or an agent
    /// reads a clean tree.
    suppressed: &'a [crate::suppress::Suppressed],
    /// Suppressions with nothing left to accept.
    stale_suppressions: &'a [crate::suppress::Stale],
    /// Directives naming no rule.
    malformed_directives: &'a [crate::suppress::Malformed],
    /// Ruby files that did not parse, so nothing was read from them. Always
    /// present: a file rwr could not open is exactly what the account of blind
    /// spots exists to name.
    unparsed: &'a [String],
}

/// What running the rules over one template produced.
struct TemplateOutcome {
    file: String,
    sites: usize,
    /// Why an edit could not be made. The `.rb` path reports a refusal and
    /// exits 5; this path used to `continue` past both a plan refusal and a
    /// cross-tag splice refusal, which is the one thing rwr promises never to
    /// do (principle 2, and the failure DESIGN.md names ast-grep for).
    refusal: Option<String>,
    rewritten: Option<Vec<u8>>,
    residue: Vec<Residue>,
}

/// One rule's share of a file's edits.
#[derive(Debug, Serialize, Clone)]
struct RuleHits {
    rule: String,
    sites: usize,
}

/// `check` and `rewrite` differ only in whether they write and in how their
/// exit codes read -- the verb carries the mode (D29) and the polarity (D22).
#[allow(clippy::too_many_lines)]
fn cmd_apply(
    rule_arg: &str,
    paths: &[String],
    replace: Option<&str>,
    write: bool,
    common: &Common,
    out: Output,
) -> ExitCode {
    let (scoped, named) = match targets(paths, common) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::Error.into();
        }
    };
    let changed = match named {
        Some(c) => Some(c),
        None => match common.changed(&scope_start(&scoped, common)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("rwr: {e}");
                return Exit::Error.into();
            }
        },
    };

    let rules = match rule::load_all(rule_arg, replace) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::PatternError.into();
        }
    };

    // Rules that can change behaviour are held back unless asked for -- and the
    // holding back is *reported*, because a zero that means "not run" reads
    // exactly like a zero that means "already clean" (D57).
    let (rules, held): (Vec<rule::Rule>, Vec<rule::Rule>) = if common.unsafe_rules {
        (rules, Vec::new())
    } else {
        rules.into_iter().partition(|r| r.unsafe_because.is_none())
    };
    if !held.is_empty() {
        // The count is unconditional -- a rule that did not run must never look
        // like a rule that found nothing. The reasons are not: six lines of
        // stderr on every pre-commit run is how a report trains people to stop
        // reading it, which is the failure DESIGN.md names Semgrep for.
        eprintln!(
            "rwr: {} rule(s) held back as unsafe; --unsafe to include them{}",
            held.len(),
            if common.explain { ":" } else { ", -e for why" }
        );
        if common.explain {
            for r in &held {
                eprintln!(
                    "  {}: {}",
                    r.id.as_deref().unwrap_or("(unnamed)"),
                    r.unsafe_because.as_deref().unwrap_or_default()
                );
            }
        }
    }
    // Some rewrites emit syntax an older Ruby cannot parse, and `verify` cannot
    // catch it: Prism parses modern Ruby, so the output is valid there (Q6).
    let target = match common.ruby.as_deref() {
        Some(text) => match crate::ruby::Version::parse(text) {
            Some(version) => Some(crate::ruby::Detected {
                version,
                source: "--ruby".to_string(),
            }),
            None => {
                eprintln!("rwr: --ruby wants a version like 3.1, not {text:?}");
                return Exit::Error.into();
            }
        },
        None => crate::ruby::detect(&scope_start(&scoped, common)),
    };

    // Checked before the gate below, which would otherwise read an unparseable
    // version as "too new" and hold the rule back with a misleading reason.
    for r in &rules {
        if let Some(text) = &r.ruby
            && crate::ruby::Version::parse(text).is_none()
        {
            eprintln!(
                "rwr: {}: `ruby: {text}` is not a version like 3.1",
                r.id.as_deref().unwrap_or("rule")
            );
            return Exit::PatternError.into();
        }
    }

    let (rules, too_new): (Vec<rule::Rule>, Vec<rule::Rule>) =
        rules.into_iter().partition(|r| match &r.ruby {
            None => true,
            Some(floor) => match (&target, crate::ruby::Version::parse(floor)) {
                (Some(t), Some(f)) => t.version >= f,
                // An undetected version is not permission to assume the newest.
                _ => false,
            },
        });
    if !too_new.is_empty() {
        let more = if common.explain {
            ":"
        } else {
            ", -e for which"
        };
        match &target {
            Some(t) => eprintln!(
                "rwr: {} rule(s) need a newer Ruby than {} (from {}){more}",
                too_new.len(),
                t.version,
                t.source
            ),
            None => eprintln!(
                "rwr: {} rule(s) declare a Ruby version and none was detected; \
                 pass --ruby X.Y or add a .ruby-version{more}",
                too_new.len()
            ),
        }
        if common.explain {
            for r in &too_new {
                eprintln!(
                    "  {}: needs {}",
                    r.id.as_deref().unwrap_or("(unnamed)"),
                    r.ruby.as_deref().unwrap_or_default()
                );
            }
        }
    }

    if rules.is_empty() {
        return Exit::Ok.into();
    }
    // A rule with no `rewrite:` is a *finding*: it flags a shape for a human
    // without proposing an edit. Some things are worth surfacing and not worth
    // rewriting -- `.size` on a relation means one thing loaded and another
    // not, and only the caller knows which was meant.
    //
    // There is deliberately no attempt to tell a lint from a forgotten
    // template. Nothing distinguishes them in the file, and the output says
    // "no edit proposed" plainly enough that a missing `rewrite:` announces
    // itself. A bare pattern with no `-r` never reaches here: it is not a path,
    // so it fails to resolve as a rule at all.
    let engine = match crate::engine::Engine::new(rules) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::PatternError.into();
        }
    };
    let rules = engine.rules();

    let (files, templates) = profile::span_noted(
        "walk",
        || {
            let (found, templates) = source::walk(&scoped, common.include_vendored);
            (only_changed(found, changed.as_ref()), templates)
        },
        |(f, t)| format!("{} files, {} template(s)", f.len(), t.len()),
    );

    // Read once and shared between the hierarchy and the scan. Each phase
    // reading independently doubled the I/O, which profiling showed was most of
    // the run -- the files are the cost, not the parsing.
    let reading = profile::now();
    let sources: Vec<source::Source> = files.par_iter().map(|p| source::open(p)).collect();
    profile::mark("read", reading, || {
        let bytes: usize = sources.iter().map(|s| s.bytes().len()).sum();
        format!(
            "{} files, {:.1} MB",
            sources.len(),
            bytes as f64 / 1_048_576.0
        )
    });

    let context = engine.context(&sources);

    /// One source's result, with the label the report prints for it.
    struct Outcome {
        file: String,
        scanned: crate::engine::Scanned,
        refusal: Option<String>,
    }

    let skipped = std::sync::atomic::AtomicUsize::new(0);
    let unparsed: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let scanning = profile::now();
    let outcomes: Vec<Outcome> = files
        .par_iter()
        .zip(&sources)
        .filter_map(|(path, mapped)| {
            let mapped = mapped.bytes();
            if !engine.may_contribute(mapped) {
                skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return None;
            }
            // Materialised only now, for the few files that survive.
            let file = path.display().to_string();
            // git reports paths from the repository root; the walk may have
            // produced relative ones. Resolved only under `--diff`, since it is
            // a syscall and there are eleven thousand files.
            let absolute = changed
                .as_ref()
                .map(|_| path.canonicalize().unwrap_or_else(|_| path.clone()));
            let only = changed
                .as_ref()
                .zip(absolute.as_ref())
                .map(|(changed, absolute)| crate::engine::Only { changed, absolute });

            match engine.scan(&file, mapped, &context, only, common.explain) {
                // A file rwr could not read is a blind spot, and blind spots are
                // reported unconditionally. Templates already had
                // `templates_skipped`; Ruby that does not parse had nothing at
                // all, so a generator template with a `.rb` extension -- or any
                // broken file -- vanished with the run still exiting 0.
                crate::engine::ScanOutcome::Unparseable => {
                    unparsed.lock().map(|mut v| v.push(file)).ok();
                    None
                }
                crate::engine::ScanOutcome::Quiet => None,
                crate::engine::ScanOutcome::Refused(reason) => Some(Outcome {
                    file,
                    scanned: crate::engine::Scanned::default(),
                    refusal: Some(reason),
                }),
                crate::engine::ScanOutcome::Scanned(scanned) => Some(Outcome {
                    file,
                    scanned: *scanned,
                    refusal: None,
                }),
            }
        })
        .collect();
    profile::mark("scan", scanning, || {
        format!("{} files changed", outcomes.len())
    });

    let mut refused = false;
    let mut changed: Vec<Changed> = Vec::new();
    for outcome in &outcomes {
        if let Some(reason) = &outcome.refusal {
            refused = true;
            eprintln!("rwr: refused {}: {reason}", outcome.file);
            continue;
        }
        if write
            && let Some(text) = &outcome.scanned.rewritten
            && let Err(e) = std::fs::write(&outcome.file, text)
        {
            eprintln!("rwr: cannot write {}: {e}", outcome.file);
            return Exit::Error.into();
        }
        // A file that only contributed residue is not a changed file.
        if outcome.scanned.sites == 0 {
            continue;
        }
        changed.push(Changed {
            file: outcome.file.clone(),
            sites: outcome.scanned.sites,
            rules: outcome
                .scanned
                .by_rule
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .filter_map(|(i, n)| {
                    rules[i].id.as_ref().map(|id| RuleHits {
                        rule: id.clone(),
                        sites: *n,
                    })
                })
                .collect(),
        });
    }

    // Templates *can* be parsed, where their tags stitch into a valid program
    // (95% of them do). Those get the same structural treatment as Ruby, and
    // only the rest fall back to the text search below.
    let template_outcomes: Vec<TemplateOutcome> = templates
        .par_iter()
        .filter_map(|path| {
            let source = source::open(path);
            let original = source.bytes().to_vec();
            let mut current = original.clone();
            let mut sites = 0usize;
            let mut residue = Vec::new();
            let mut parsed_ok = false;
            let mut refusal: Option<String> = None;

            // Naive by design: re-translate per rule. There are a few hundred
            // templates and they are small, so the simple version costs nothing
            // worth the complexity of threading a live map through the loop.
            for (index, (rule, prepared)) in engine.prepared().enumerate() {
                // No tags at all: a page of static HTML, nothing to do.
                let translated = crate::erb::translate(&current)?;
                let ruby = ruby_prism::parse(&translated.ruby);
                if ruby.errors().count() > 0 {
                    // Stitching failed: leave it to the text search, which is
                    // weaker and says so.
                    return None;
                }
                parsed_ok = true;
                if !engine.may_contribute(&translated.ruby) {
                    continue;
                }
                let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
                let p_node = p_parsed.node();
                let Some(p_root) = matcher::pattern_root(&p_node) else {
                    continue;
                };
                let criteria = engine.criteria(index, &context);
                let hits = matcher::search(&p_root, &ruby.node(), prepared, &criteria);

                // Residue first: it reads the *current* text either way.
                if engine.claims_completeness() {
                    let anchors = residue::anchors(&p_root, prepared);
                    if !anchors.is_empty() {
                        let mut found =
                            residue::find(&ruby.node(), &anchors, &[], &translated.ruby);
                        found.extend(residue::in_comments(&ruby, &anchors, &translated.ruby));
                        for o in found {
                            let Some(at) = crate::erb::template_offset(&translated, o.byte_start)
                            else {
                                continue;
                            };
                            let (line, col) = source::line_col(&current, at);
                            residue.push(Residue {
                                file: path.display().to_string(),
                                line,
                                col,
                                context: o.context,
                                rule: rule.id.clone(),
                                text: source::line_at(&current, at),
                            });
                        }
                    }
                }

                let Some(template) = rule.rewrite.as_deref() else {
                    continue;
                };
                if hits.is_empty() {
                    continue;
                }
                let planned = match rewrite::plan(
                    &hits,
                    &p_root,
                    prepared,
                    template,
                    &translated.ruby,
                    &rule.constant_captures(),
                ) {
                    Ok(planned) => planned,
                    Err(r) => {
                        refusal = Some(format!("{r:?}"));
                        break;
                    }
                };
                // An edit spanning two tags covers template text that is not
                // Ruby; `splice` refuses it rather than writing HTML into an
                // expression.
                match crate::erb::splice(&translated, &current, &planned.edits) {
                    Some(next) => {
                        sites += planned.sites;
                        current = next;
                    }
                    None => {
                        refusal = Some(
                            "an edit spans two ERB tags, so the text between them is not Ruby"
                                .to_string(),
                        );
                        break;
                    }
                }
            }

            // A rename expands to several rules and each anchors on the same
            // name, so one occurrence is found once per rule.
            residue.sort_by_key(|r| (r.line, r.col));
            residue.dedup_by_key(|r| (r.line, r.col));

            // Produced whenever the template *parsed*, findings or not: this is
            // also the record of which templates need no text fallback, and a
            // clean template is exactly one with nothing to report.
            let refusal_free = refusal.is_none();
            parsed_ok.then(|| TemplateOutcome {
                file: path.display().to_string(),
                sites,
                refusal,
                // A refused template keeps its bytes: partial application of a
                // rule set that could not finish is worse than none.
                rewritten: (sites > 0 && refusal_free).then(|| current.clone()),
                residue,
            })
        })
        .collect();

    // Templates cannot be parsed, but they can be searched. This is what turns
    // "356 files were not searched" into "here are the three views that mention
    // the name you are renaming" -- the difference between naming a blind spot
    // and doing something about it.
    // A template rwr parsed needs no text search: it has real evidence.
    let parsed_templates: std::collections::HashSet<&str> =
        template_outcomes.iter().map(|o| o.file.as_str()).collect();

    let mut left_over_text: Vec<Residue> = Vec::new();
    if engine.claims_completeness() && !templates.is_empty() {
        let anchors: Vec<(Option<String>, Vec<u8>)> = engine
            .prepared()
            .flat_map(|(rule, prepared)| {
                let parsed = ruby_prism::parse(prepared.source.as_bytes());
                let found = matcher::pattern_root(&parsed.node())
                    .map(|root| residue::anchors(&root, prepared))
                    .unwrap_or_default();
                found
                    .into_iter()
                    .map(|a| (rule.id.clone(), a))
                    .collect::<Vec<_>>()
            })
            .collect();

        left_over_text = templates
            .par_iter()
            .flat_map_iter(|path| {
                let mut here = Vec::new();
                if parsed_templates.contains(path.display().to_string().as_str()) {
                    return here.into_iter();
                }
                let bytes = source::open(path);
                let bytes = bytes.bytes().to_vec();
                for (rule, anchor) in &anchors {
                    for at in source::identifier_offsets(&bytes, anchor) {
                        let (line, col) = source::line_col(&bytes, at);
                        here.push(Residue {
                            file: path.display().to_string(),
                            line,
                            col,
                            context: residue::Context::Text,
                            rule: rule.clone(),
                            text: source::line_at(&bytes, at),
                        });
                    }
                }
                here.into_iter()
            })
            .collect();
        left_over_text.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        left_over_text.dedup_by_key(|r| (r.file.clone(), r.line, r.col));
    }

    for outcome in &template_outcomes {
        // Same treatment as a `.rb` refusal: named on stderr, and it sets the
        // exit code. A refusal nobody hears is a silently dropped edit.
        if let Some(reason) = &outcome.refusal {
            refused = true;
            eprintln!("rwr: refused {}: {reason}", outcome.file);
            continue;
        }
        if write
            && let Some(text) = &outcome.rewritten
            && let Err(e) = std::fs::write(&outcome.file, text)
        {
            eprintln!("rwr: cannot write {}: {e}", outcome.file);
            return Exit::Error.into();
        }
        if outcome.sites > 0 {
            changed.push(Changed {
                file: outcome.file.clone(),
                sites: outcome.sites,
                rules: Vec::new(),
            });
        }
    }

    let mut unparsed = unparsed.into_inner().unwrap_or_default();
    unparsed.sort();

    let suppressed: Vec<crate::suppress::Suppressed> = outcomes
        .iter()
        .flat_map(|o| o.scanned.suppressed.iter().cloned())
        .collect();
    let stale: Vec<crate::suppress::Stale> = outcomes
        .iter()
        .flat_map(|o| o.scanned.stale.iter().cloned())
        .collect();
    let malformed: Vec<crate::suppress::Malformed> = outcomes
        .iter()
        .flat_map(|o| o.scanned.malformed.iter().cloned())
        .collect();

    let mut rejections: Vec<Rejection> = outcomes
        .iter()
        .flat_map(|o| o.scanned.rejections.iter().cloned())
        .collect();
    rejections.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));

    let mut findings: Vec<Finding> = outcomes
        .iter()
        .flat_map(|o| o.scanned.flagged.iter().cloned())
        .collect();
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let mut left_over: Vec<Residue> = outcomes
        .iter()
        .flat_map(|o| o.scanned.residue.iter().cloned())
        .collect();
    left_over.extend(
        template_outcomes
            .iter()
            .flat_map(|o| o.residue.iter().cloned()),
    );
    left_over.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    changed.sort_by(|a, b| a.file.cmp(&b.file));

    match out {
        Output::Text => {
            for c in &changed {
                let verb = if write { "rewrote" } else { "would rewrite" };
                println!("{}: {verb} {} site(s)", c.file, c.sites);
            }
            report_findings(&findings);
            report_by_rule(&changed);
            report_spread(
                &outcomes
                    .iter()
                    .flat_map(|o| o.scanned.spread.iter())
                    .collect::<Vec<_>>(),
            );
            report_unsafe(&changed, rules);
            report_rejections(&rejections);
            report_suppressions(&suppressed, &stale, &malformed);
            if !unparsed.is_empty() {
                eprintln!(
                    "rwr: {} Ruby file(s) did not parse and were not read:",
                    unparsed.len()
                );
                for file in unparsed.iter().take(RESIDUE_DETAIL_CAP) {
                    eprintln!("  {file}");
                }
            }
            report_residue(&left_over);
            // Only the templates that fell back: one rwr parsed has real
            // evidence and does not belong in a paragraph about guesses.
            report_text_residue(&left_over_text, templates.len() - parsed_templates.len());
        }
        _ => {
            // Residue is the product, not a diagnostic, so it cannot be text-only:
            // an agent runs `-j` and was getting the edits with no account of what
            // they missed at all (D7, principle 3).
            let report = Report {
                schema: REPORT_SCHEMA,
                rwr_version: env!("CARGO_PKG_VERSION"),
                changed: &changed,
                findings: &findings,
                residue: &left_over,
                template_residue: &left_over_text,
                // The templates that got *no* structural read, matching what
                // the text report says. Counting every template here claimed a
                // blind spot over files rwr had in fact parsed -- and claimed it
                // in the machine-readable plane, where an agent acts on it.
                templates_skipped: if engine.claims_completeness() {
                    templates.len() - parsed_templates.len()
                } else {
                    0
                },
                rejections: common.explain.then_some(rejections.as_slice()),
                suppressed: &suppressed,
                stale_suppressions: &stale,
                malformed_directives: &malformed,
                unparsed: &unparsed,
            };
            if emit_document(out, &report).is_some() {
                return Exit::Error.into();
            }
        }
    }

    profile::report();
    let deferred: usize = outcomes.iter().map(|o| o.scanned.deferred).sum();
    if deferred > 0 {
        eprintln!(
            "rwr: {deferred} further match(es) sat inside a rewritten range; rerun to apply them"
        );
    }

    if refused {
        return Exit::Refused.into();
    }
    if deferred > 0 && write {
        return Exit::Retryable.into();
    }
    // `check` is enforcement polarity: nothing to change is success, and
    // something to change is the signal a hook or CI acts on (D22). `rewrite`
    // succeeds either way, having done whatever there was to do.
    // A finding is work to do, exactly as an edit is: `check` exists to fail a
    // gate on it, and a lint that exits 0 gates nothing.
    if write || (changed.is_empty() && findings.is_empty()) {
        Exit::Ok.into()
    } else {
        Exit::Negative.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Agents branch on the exit code before parsing any output, so these
    /// numbers are as much a public contract as the JSON field names.
    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(Exit::Ok.code(), 0);
        assert_eq!(Exit::Negative.code(), 1);
        assert_eq!(Exit::Error.code(), 2);
        assert_eq!(Exit::PatternError.code(), 3);
        assert_eq!(Exit::Retryable.code(), 4);
        assert_eq!(Exit::Refused.code(), 5);
    }

    /// `2` is near-universally "something went wrong" (grep, rg, ruff, rubocop,
    /// biome, jq, semgrep). Handing it any other meaning misleads every agent
    /// that has learned one of those tools.
    #[test]
    fn two_means_error() {
        assert_eq!(Exit::Error.code(), 2);
    }

    /// Retryable and Refused must stay distinguishable: collapsed, an agent
    /// either abandons recoverable work or spins on unrecoverable work.
    #[test]
    fn retryable_is_distinct_from_refused() {
        assert_ne!(Exit::Retryable.code(), Exit::Refused.code());
    }

    #[test]
    fn ndjson_wins_over_json() {
        let c = Common {
            json: true,
            ndjson: true,
            path: vec![],
            include_vendored: false,
            diff: false,
            since: None,
            explain: false,
            ruby: None,
            unsafe_rules: false,
            profile: false,
        };
        assert_eq!(c.output(), Output::Ndjson);
    }
}

/// One fixture's verdict.
///
/// `kind` is a closed vocabulary so a caller can branch without parsing prose.
#[derive(Debug, Serialize)]
struct CaseResult {
    rule: String,
    case: usize,
    outcome: &'static str,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<String>,
}

#[derive(Debug, Serialize)]
struct TestReport<'a> {
    schema: u32,
    rwr_version: &'static str,
    cases: &'a [CaseResult],
    /// Rules in the set that declare no fixtures. Named rather than counted --
    /// a pack whose rules are untested must not read like a pack that passed.
    untested: &'a [String],
    passed: usize,
    failed: usize,
}

/// Run a rule set's fixtures.
///
/// Two policy differences from `check`, both deliberate. Gating does not apply:
/// `unsafe:` holdback and `ruby:` version checks are application-time policy
/// about *whether* to run a rule, and a fixture tests what it does. And an
/// unparseable snippet is a failing case rather than a skip -- in `check`,
/// skipping a file that does not parse is the contract; here the same behaviour
/// would make a typo'd snippet pass every negative assertion vacuously, which is
/// the commonest fixture bug there is.
fn cmd_test(rule_arg: &str, out: Output) -> ExitCode {
    let rules = match rule::load_all(rule_arg, None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::PatternError.into();
        }
    };
    // By id, not by rule: a case runs the whole document's set (D69) and an id
    // is the file's path within the pack, so a list file whose fixtures sit on
    // its first rule has covered every rule in it.
    let tested: std::collections::HashSet<&str> = rules
        .iter()
        .filter(|r| !r.tests.is_empty())
        .filter_map(|r| r.id.as_deref())
        .collect();
    let mut untested: Vec<String> = rules
        .iter()
        .filter(|r| r.tests.is_empty())
        .map(|r| r.id.clone().unwrap_or_else(|| "(unnamed)".to_string()))
        .filter(|id| !tested.contains(id.as_str()))
        .collect();
    untested.sort();
    untested.dedup();
    let cases = match rule::cases(&rules) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rwr: {rule_arg}: {e}");
            return Exit::PatternError.into();
        }
    };
    if cases.is_empty() {
        // A green nothing is the failure this command exists to prevent, so it
        // must not be how a fixture-less pack reports.
        eprintln!("rwr: {rule_arg} declares no fixtures -- add `tests:` to a rule");
        return Exit::Error.into();
    }

    // The names before the set moves into the engine.
    let names: Vec<String> = rules
        .iter()
        .map(|r| r.id.clone().unwrap_or_else(|| "(unnamed)".to_string()))
        .collect();
    let label = names.first().cloned().unwrap_or_default();
    let engine = match crate::engine::Engine::new(rules) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::PatternError.into();
        }
    };

    let mut results: Vec<CaseResult> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let bytes = case.input.as_bytes();
        // The snippet supplies its own context: a rule needing a class or a
        // signature writes it into the input, rather than being handed one.
        let sources = [source::Source::Owned(bytes.to_vec())];
        let context = engine.context(&sources);
        let outcome = engine.scan("fixture.rb", bytes, &context, None, false);

        let (kind, actual, findings) = match &outcome {
            crate::engine::ScanOutcome::Unparseable => ("invalid_ruby", None, 0),
            crate::engine::ScanOutcome::Refused(reason) => ("refused", Some(reason.clone()), 0),
            crate::engine::ScanOutcome::Quiet => ("no_match", None, 0),
            crate::engine::ScanOutcome::Scanned(s) => (
                if s.sites > 0 { "rewrote" } else { "reported" },
                s.rewritten.clone(),
                s.flagged.len(),
            ),
        };
        let text = actual.clone().unwrap_or_else(|| case.input.clone());

        let (outcome_word, kind, expected, actual) = if kind == "invalid_ruby" {
            ("fail", "invalid_ruby", None, None)
        } else if kind == "refused" {
            ("fail", "refused", None, actual)
        } else if let Some(want) = &case.output {
            if &text == want {
                ("pass", "rewrote", None, None)
            } else {
                ("fail", "output_mismatch", Some(want.clone()), Some(text))
            }
        } else if case.unchanged == Some(true) {
            if text == case.input {
                ("pass", "unchanged", None, None)
            } else {
                (
                    "fail",
                    "unexpected_rewrite",
                    Some(case.input.clone()),
                    Some(text),
                )
            }
        } else if let Some(want) = case.finds {
            if findings == want {
                ("pass", "reported", None, None)
            } else {
                (
                    "fail",
                    "wrong_finds",
                    Some(want.to_string()),
                    Some(findings.to_string()),
                )
            }
        } else {
            // `rule::cases` refuses a case that asserts nothing, so this is
            // unreachable rather than a silent pass.
            ("fail", "asserts_nothing", None, None)
        };

        results.push(CaseResult {
            rule: label.clone(),
            case: index + 1,
            outcome: outcome_word,
            kind,
            expected,
            actual,
        });
    }

    let failed = results.iter().filter(|r| r.outcome == "fail").count();
    let passed = results.len() - failed;

    match out {
        Output::Text => {
            for r in results.iter().filter(|r| r.outcome == "fail") {
                println!("FAIL {} case {} — {}", r.rule, r.case, explain_kind(r.kind));
                if let (Some(want), Some(got)) = (&r.expected, &r.actual) {
                    for line in diff_lines(want, got) {
                        println!("  {line}");
                    }
                }
            }
            println!(
                "{passed} passed, {failed} failed{}",
                if untested.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; {} rule(s) declare no fixtures: {}",
                        untested.len(),
                        untested.join(", ")
                    )
                }
            );
        }
        _ => {
            let report = TestReport {
                schema: 1,
                rwr_version: env!("CARGO_PKG_VERSION"),
                cases: &results,
                untested: &untested,
                passed,
                failed,
            };
            if let Some(code) = emit_document(out, &report) {
                return code;
            }
        }
    }

    if failed > 0 {
        Exit::Negative.into()
    } else {
        Exit::Ok.into()
    }
}

/// One line on what a failure kind means, so the diff is not the only clue.
fn explain_kind(kind: &str) -> &'static str {
    match kind {
        "invalid_ruby" => {
            "the snippet does not parse; a fixture that cannot be read cannot test anything"
        }
        "refused" => "the rule refused to edit this snippet",
        "output_mismatch" => "the rewrite did not produce the expected source",
        "unexpected_rewrite" => "expected no change, but the rule rewrote it",
        "wrong_finds" => "wrong number of findings",
        _ => "asserts nothing",
    }
}

/// A minimal line diff, enough to see which line moved.
fn diff_lines(want: &str, got: &str) -> Vec<String> {
    let mut out = vec!["--- expected".to_string(), "+++ actual".to_string()];
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    for line in &w {
        if !g.contains(line) {
            out.push(format!("-{line}"));
        }
    }
    for line in &g {
        if !w.contains(line) {
            out.push(format!("+{line}"));
        }
    }
    // A trailing-newline difference is invisible line by line, and is the
    // commonest YAML slip (`|` versus `|-`).
    if want.ends_with('\n') != got.ends_with('\n') {
        out.push(format!(
            "(expected {} trailing newline, actual {})",
            if want.ends_with('\n') { "a" } else { "no" },
            if got.ends_with('\n') {
                "has one"
            } else {
                "has none"
            }
        ));
    }
    out
}
