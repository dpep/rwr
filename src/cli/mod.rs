//! CLI surface: argument parsing, structured output, exit codes.
//!
//! The contract here is public from v0.1 (decision D17) and is specified in
//! `docs/cli-conventions.md`. Conventions are inherited from `rq` so that an
//! agent which has learned one of these tools has learned the others.

use crate::pattern::{matcher, prefilter, prepare};
use crate::profile;
use crate::residue;
use crate::rewrite;
use crate::rule;
use crate::source;
use clap::{Args, Parser, Subcommand};
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
    /// Verb-dependent success. `find`/`rewrite`: matched. `check`: clean.
    Ok,
    /// Verb-dependent negative result — not an error in either polarity.
    /// `find`/`rewrite`: nothing matched. `check`: violations found.
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
    /// `docs/cli-conventions.md`; `exit_codes_are_stable` pins it.
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
/// search path — see `docs/cli-conventions.md`.
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

    /// Explain each result: which constraint rejected a candidate, which
    /// conflict suppressed a match, how a residue occurrence was classified.
    #[arg(short = 'e', long, global = true)]
    explain: bool,

    /// Restrict to lines a change touched. Bare, that is the uncommitted work;
    /// with a revision, what this branch introduces (`main...HEAD`).
    ///
    /// What makes `check` adoptable on a codebase that has never run it: a rule
    /// with two thousand pre-existing sites must not fail a pull request that
    /// added three.
    #[arg(long, global = true, value_name = "REV", num_args = 0..=1, default_missing_value = "")]
    diff: Option<String>,

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

/// Where this run is pointed, for questions that are properties of the *repo*
/// rather than of a file: which git repository, and which Ruby version.
fn scope_start(paths: &[String], common: &Common) -> std::path::PathBuf {
    paths
        .first()
        .or_else(|| common.path.first())
        .map_or_else(|| std::path::PathBuf::from("."), std::path::PathBuf::from)
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
        let Some(rev) = self.diff.as_deref() else {
            return Ok(None);
        };
        let rev = (!rev.is_empty()).then_some(rev);
        crate::diff::from_git(rev, start).map(Some)
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
    },
}

/// Parse arguments and dispatch. Returns the process exit status.
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let out = cli.common.output();
    profile::enable_from(cli.common.profile);

    // The shorthand desugars to a verb; it can only ever reach a read-only one.
    let command = match (cli.command, cli.pattern) {
        (Some(c), _) => c,
        (None, Some(pattern)) => match cli.replace {
            None => Command::Find {
                pattern,
                paths: cli.paths,
            },
            Some(replace) => Command::Check {
                rule: pattern,
                paths: cli.paths,
                replace: Some(replace),
            },
        },
        (None, None) => {
            eprintln!("rwr: give a pattern, or a subcommand — see `rwr --help`");
            return Exit::Error.into();
        }
    };

    match command {
        Command::Find { pattern, paths } => cmd_find(&pattern, &paths, &cli.common, out),
        Command::Check {
            rule,
            paths,
            replace,
        } => cmd_apply(&rule, &paths, replace.as_deref(), false, &cli.common, out),
        Command::Rewrite {
            rule,
            paths,
            replace,
        } => cmd_apply(&rule, &paths, replace.as_deref(), true, &cli.common, out),
    }
}

/// Emit a row set: `--json` one pretty array, `--ndjson` one compact object
/// per line (D23). Returns `Some(exit)` only on a serialisation failure.
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
fn report_residue(residues: &[Residue], templates: usize) {
    // Printed even when the residue list is empty: a rule that accounted for
    // everything in Ruby still did not look at ERB, and a blind spot that
    // appears and vanishes with unrelated results is not a report. The caller
    // passes zero when the run makes no completeness claim at all.
    if templates > 0 {
        eprintln!(
            "\nnote: {templates} template file(s) were not searched. rwr reads Ruby, \
             and .erb/.haml embed it -- so this account covers Ruby only (Q11)."
        );
    }
    if residues.is_empty() {
        return;
    }
    let count = |c: residue::Context| residues.iter().filter(|r| r.context == c).count();
    eprintln!(
        "\n{} occurrence(s) this rule could not account for \
         ({} symbol, {} string, {} call, {} definition):",
        residues.len(),
        count(residue::Context::Symbol),
        count(residue::Context::String),
        count(residue::Context::Call),
        count(residue::Context::Definition),
    );
    for r in residues.iter().take(RESIDUE_DETAIL_CAP) {
        eprintln!(
            "  {}:{}:{}: {:?}: {}",
            r.file, r.line, r.col, r.context, r.text
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
/// An occurrence the rule could not account for, as reported.
#[derive(Debug, Serialize, Clone)]
struct Residue {
    file: String,
    line: usize,
    col: usize,
    context: residue::Context,
    text: String,
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

    // `find` takes a bare pattern, so it has no rule to draw a class from.
    let class_anchor: Option<&str> = None;

    let changed = match common.changed(&scope_start(paths, common)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::Error.into();
        }
    };

    let mut scoped: Vec<String> = paths.to_vec();
    scoped.extend(common.path.iter().cloned());
    let (files, templates) = profile::span_noted(
        "walk",
        || {
            let (found, templates) = source::walk(&scoped, common.include_vendored);
            (only_changed(found, changed.as_ref()), templates)
        },
        |(f, t)| format!("{} files, {t} template(s) skipped", f.len()),
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
                let extra = residue::find(&parsed.node(), &anchors, &matched, &src);
                // A class-anchored rule scopes its own report: the payoff of
                // receiver narrowing, since an unscoped report's bulk is
                // unrelated classes sharing an identifier.
                let extra = match class_anchor {
                    // `find` takes a bare pattern, so it never has a class to
                    // scope by and never builds a hierarchy.
                    Some(class) => {
                        residue::scoped_to(extra, class, &crate::hierarchy::Hierarchy::default())
                    }
                    None => extra,
                };
                if let Ok(mut sink) = residues.lock() {
                    sink.extend(extra.into_iter().map(|o| {
                        let (line, col) = source::line_col(&src, o.byte_start);
                        Residue {
                            file: path.display().to_string(),
                            line,
                            col,
                            context: o.context,
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
            report_residue(&residues, templates);
        }
        _ => {
            if emit_rows(out, &found).is_some() {
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

/// Everything a `check` or `rewrite` run has to say, for machine consumers.
///
/// A single object rather than a bare array of changes: the changes alone are
/// only half the answer, and the half that shipped without the other was the
/// half that flatters the tool.
#[derive(Debug, Serialize)]
struct Report<'a> {
    changed: &'a [Changed],
    /// Occurrences the rule could not account for. Present and empty when the
    /// rule is name-anchored and found none; absent means it made no claim.
    residue: &'a [Residue],
    /// Template files not searched, since they embed Ruby rwr does not read.
    templates_skipped: usize,
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
    let changed = match common.changed(&scope_start(paths, common)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::Error.into();
        }
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
        None => crate::ruby::detect(&scope_start(paths, common)),
    };

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
    if rules.iter().any(|r| r.rewrite.is_none()) {
        eprintln!("rwr: {}", rule::RuleError::NoTemplate);
        return Exit::PatternError.into();
    }
    // A rule set that names no class cannot tell `Account#display_name` from
    // `Company#display_name`, so its matches are tallied by resolved receiver.
    let unnarrowed = !rules
        .iter()
        .any(|r| r.constraints.values().any(|c| c.receiver_type.is_some()));

    // Each rule is prepared once; they apply in order, each seeing the
    // previous one's output. A rename genuinely needs a set, since the
    // definition and the call sites are different shapes.
    let mut prepareds = Vec::with_capacity(rules.len());
    for r in &rules {
        match prepare::prepare_with(&r.pattern, &r.constant_captures()) {
            Ok(p) => {
                // Scoped so the parse's borrow of `p` ends before it moves.
                let single = {
                    let parsed = ruby_prism::parse(p.source.as_bytes());
                    matcher::pattern_root(&parsed.node()).is_some()
                };
                if !single {
                    eprintln!("rwr: a pattern must be a single expression: {}", r.pattern);
                    return Exit::PatternError.into();
                }
                prepareds.push(p);
            }
            Err(e) => {
                eprintln!("rwr: {e}");
                return Exit::PatternError.into();
            }
        }
    }

    // Whether this run claims completeness at all. Residue applies to
    // name-anchored rules only (D7), and the templates note is part of that
    // claim -- a pack of shape rules makes no claim, so noting what it did not
    // read there is noise on every run.
    let claims_completeness = prepareds.iter().any(|prepared| {
        let parsed = ruby_prism::parse(prepared.source.as_bytes());
        matcher::pattern_root(&parsed.node())
            .is_some_and(|root| !residue::anchors(&root, prepared).is_empty())
    });

    let mut scoped: Vec<String> = paths.to_vec();
    scoped.extend(common.path.iter().cloned());
    let (files, templates) = profile::span_noted(
        "walk",
        || {
            let (found, templates) = source::walk(&scoped, common.include_vendored);
            (only_changed(found, changed.as_ref()), templates)
        },
        |(f, t)| format!("{} files, {t} template(s) skipped", f.len()),
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

    // Built per run rather than cached: a full rails parse is under 200ms
    // (Phase 0 measurement (d)), so there is no staleness to manage.
    let hierarchy = if rules.iter().any(|r| {
        r.scope.subclasses.unwrap_or(false)
            || r.constraints
                .values()
                .any(|c| c.subclasses.unwrap_or(false))
    }) {
        {
            let started = profile::now();
            // Only the part of the hierarchy reachable from the classes the
            // rules name is needed, which is a handful rather than all of them.
            let roots: Vec<String> = rules
                .iter()
                .filter_map(|r| {
                    r.scope
                        .inside
                        .clone()
                        .or_else(|| r.constraints.values().find_map(|c| c.receiver_type.clone()))
                })
                .collect();
            let (h, parsed) = crate::hierarchy::Hierarchy::reachable_from(&sources, &roots);
            let total = files.len();
            profile::mark("hierarchy", started, || {
                format!("{parsed} parsed, {} skipped", total.saturating_sub(parsed))
            });
            h
        }
    } else {
        crate::hierarchy::Hierarchy::default()
    };

    // Return types stated by Sorbet signatures. Built only when a rule narrows
    // by receiver, and costing a single substring search per file in a
    // repository that has none (D62).
    let sigs = if rules
        .iter()
        .any(|r| r.constraints.values().any(|c| c.receiver_type.is_some()))
    {
        let started = profile::now();
        let (found, parsed) = crate::sigs::Signatures::from_sources(&sources);
        profile::mark("signatures", started, || {
            format!("{} signature(s) from {parsed} file(s)", found.len())
        });
        found
    } else {
        crate::sigs::Signatures::default()
    };

    // Each file is independent: one refusal declines that file and is reported,
    // rather than aborting work already proven safe elsewhere (DESIGN.md §4).
    struct Outcome {
        file: String,
        sites: usize,
        /// Classes this file's matched receivers resolved to, for the
        /// cross-class warning. Empty unless the rule set narrows by none.
        spread: Vec<String>,
        /// Edits per rule, positionally. Attribution is per file because that
        /// is where the work happened; totals aggregate from it.
        by_rule: Vec<usize>,
        rewritten: Option<String>,
        refusal: Option<String>,
        residue: Vec<Residue>,
        deferred: usize,
    }

    // The class a rule set is about, used to scope its own residue report.
    let class_anchor: Option<String> = rules.iter().find_map(|r| {
        r.scope
            .inside
            .clone()
            .or_else(|| r.constraints.values().find_map(|c| c.receiver_type.clone()))
    });

    // Union across the rule set: a file kept by any rule must still be parsed.
    // One filter per rule, checked disjunctively: any rule needing the file is
    // enough to parse it.
    let filters: Vec<prefilter::Filter> = rules
        .iter()
        .map(|r| prefilter::Filter::new(&prefilter::required(&r.pattern), &[]))
        .collect();
    let skipped = std::sync::atomic::AtomicUsize::new(0);

    let scanning = profile::now();
    let outcomes: Vec<Outcome> = files
        .par_iter()
        .zip(&sources)
        .filter_map(|(path, mapped)| {
            let mapped = mapped.bytes();
            // A rule set's literals are checked disjunctively -- any one rule
            // matching is enough to need this file.
            if !filters.iter().any(|f| f.may_contribute(mapped)) {
                skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return None;
            }
            // Materialised only now, for the few files that survive.
            let original = mapped.to_vec();
            let file = path.display().to_string();
            // git reports paths from the repository root; the walk may have
            // produced relative ones.
            let absolute = path.canonicalize().unwrap_or_else(|_| path.clone());
            let mut current = original.clone();
            let mut total = 0usize;
            // Matches a wider edit covered. Non-zero means a rerun makes
            // further progress, which is the retryable outcome rather than a
            // failure (D15).
            let mut deferred = 0usize;

            let mut by_rule = vec![0usize; rules.len()];
            let mut spread: Vec<String> = Vec::new();
            for (index, (rule, prepared)) in rules.iter().zip(&prepareds).enumerate() {
                // Scoped so every borrow of `current` ends before it is
                // replaced with this rule's output.
                let step: Result<Option<(String, usize, usize)>, String> = {
                    let parsed = ruby_prism::parse(&current);
                    if parsed.errors().count() > 0 {
                        return None;
                    }
                    let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
                    let p_node = p_parsed.node();
                    match matcher::pattern_root(&p_node) {
                        None => Ok(None),
                        Some(p_root) => {
                            // Criteria are applied *inside* the search now, so a
                            // constraint rejection drives backtracking to a
                            // different binding rather than discarding the match
                            // (Q13).
                            let criteria = matcher::Criteria {
                                constraints: &rule.constraints,
                                scope: &rule.scope,
                                hierarchy: &hierarchy,
                                sigs: &sigs,
                            };
                            let mut hits =
                                matcher::search(&p_root, &parsed.node(), prepared, &criteria);
                            if let Some(changed) = changed.as_ref() {
                                hits.retain(|m| {
                                    let (start, end) = rewrite::effective_range(&m.node);
                                    let (first, _) = source::line_col(&current, start);
                                    let (last, _) = source::line_col(&current, end);
                                    changed.touches(&absolute, first, last)
                                });
                            }
                            // A rule that does not say which class it means may
                            // be renaming across several. Recorded here, warned
                            // about once at the end (Q10).
                            if unnarrowed {
                                for hit in &hits {
                                    if let Some(class) = matcher::receiver_class(hit, &sigs) {
                                        spread.push(class);
                                    }
                                }
                            }
                            if hits.is_empty() {
                                Ok(None)
                            } else {
                                let template = rule.rewrite.as_deref().unwrap_or_default();
                                match rewrite::plan(
                                    &hits,
                                    &p_root,
                                    prepared,
                                    template,
                                    &current,
                                    &rule.constant_captures(),
                                ) {
                                    Err(r) => Err(format!("{r:?}")),
                                    Ok(planned) => {
                                        let text = rewrite::apply(&current, &planned.edits);
                                        match rewrite::verify(&text) {
                                            Err(r) => Err(format!("{r:?}")),
                                            Ok(()) => {
                                                Ok(Some((text, planned.sites, planned.dropped)))
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                };

                match step {
                    Err(refusal) => {
                        return Some(Outcome {
                            file,
                            sites: 0,
                            spread: Vec::new(),
                            by_rule: Vec::new(),
                            rewritten: None,
                            refusal: Some(refusal),
                            residue: Vec::new(),
                            deferred: 0,
                        });
                    }
                    Ok(None) => {}
                    Ok(Some((text, n, skipped))) => {
                        total += n;
                        by_rule[index] += n;
                        deferred += skipped;
                        current = text.into_bytes();
                    }
                }
            }

            // Residue is computed whether or not this file changed. It used to
            // sit behind an early return for `total == 0`, so a file containing
            // *only* dynamic reaches -- a serializer full of `delegate` and
            // `validates`, which is the dangerous case exactly -- was never
            // looked at. Measured on the testbed: recall 4 of 7 to 7 of 7.
            //
            // Reported against the *rewritten* source, so an occurrence a rule
            // already handled is not counted twice -- and so a subclass call
            // site left behind by a rename is visible rather than silently
            // broken.
            let residue = {
                let parsed = ruby_prism::parse(&current);
                let mut found = Vec::new();
                for prepared in &prepareds {
                    let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
                    let p_node = p_parsed.node();
                    let Some(p_root) = matcher::pattern_root(&p_node) else {
                        continue;
                    };
                    let anchors = residue::anchors(&p_root, prepared);
                    if anchors.is_empty() {
                        continue;
                    }
                    let mut occurrences = residue::find(&parsed.node(), &anchors, &[], &current);
                    if let Some(class) = &class_anchor {
                        occurrences = residue::scoped_to(occurrences, class, &hierarchy);
                    }
                    found.extend(occurrences.into_iter().map(|o| {
                        let (line, col) = source::line_col(&current, o.byte_start);
                        Residue {
                            file: file.clone(),
                            line,
                            col,
                            context: o.context,
                            text: source::line_at(&current, o.byte_start),
                        }
                    }));
                }
                found.sort_by_key(|r| (r.line, r.col));
                found.dedup_by_key(|r| (r.line, r.col));
                found
            };

            if total == 0 && residue.is_empty() {
                return None;
            }

            Some(Outcome {
                file,
                sites: total,
                spread,
                by_rule,
                // Nothing to write when nothing changed, so the write path
                // skips a file that only contributed residue.
                rewritten: (total > 0).then(|| String::from_utf8_lossy(&current).into_owned()),
                refusal: None,
                residue,
                deferred,
            })
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
            && let Some(text) = &outcome.rewritten
            && let Err(e) = std::fs::write(&outcome.file, text)
        {
            eprintln!("rwr: cannot write {}: {e}", outcome.file);
            return Exit::Error.into();
        }
        // A file that only contributed residue is not a changed file.
        if outcome.sites == 0 {
            continue;
        }
        changed.push(Changed {
            file: outcome.file.clone(),
            sites: outcome.sites,
            rules: outcome
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

    let mut left_over: Vec<Residue> = outcomes
        .iter()
        .flat_map(|o| o.residue.iter().cloned())
        .collect();
    left_over.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    changed.sort_by(|a, b| a.file.cmp(&b.file));

    match out {
        Output::Text => {
            for c in &changed {
                let verb = if write { "rewrote" } else { "would rewrite" };
                println!("{}: {verb} {} site(s)", c.file, c.sites);
            }
            report_by_rule(&changed);
            report_spread(
                &outcomes
                    .iter()
                    .flat_map(|o| o.spread.iter())
                    .collect::<Vec<_>>(),
            );
            report_unsafe(&changed, &rules);
            report_residue(&left_over, if claims_completeness { templates } else { 0 });
        }
        _ => {
            // Residue is the product, not a diagnostic, so it cannot be text-only:
            // an agent runs `-j` and was getting the edits with no account of what
            // they missed at all (D7, principle 3).
            let report = Report {
                changed: &changed,
                residue: &left_over,
                templates_skipped: if claims_completeness { templates } else { 0 },
            };
            if emit_rows(out, std::slice::from_ref(&report)).is_some() {
                return Exit::Error.into();
            }
        }
    }

    profile::report();
    let deferred: usize = outcomes.iter().map(|o| o.deferred).sum();
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
    if write || changed.is_empty() {
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
            diff: None,
            explain: false,
            ruby: None,
            unsafe_rules: false,
            profile: false,
        };
        assert_eq!(c.output(), Output::Ndjson);
    }
}
