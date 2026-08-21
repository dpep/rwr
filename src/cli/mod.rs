//! CLI surface: argument parsing, structured output, exit codes.
//!
//! The contract here is public from v0.1 (decision D17) and is specified in
//! `docs/cli-conventions.md`. Conventions are inherited from `rq` so that an
//! agent which has learned one of these tools has learned the others.

use crate::pattern::{matcher, prepare};
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
}

impl Common {
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

/// Print the account of what the rule could not see, grouped by class.
///
/// Grouping matters as much as the total: the classes mean different things.
/// Symbols and strings are metaprogramming reaches -- genuine blind spots.
/// Calls and definitions are usually a different method that happens to share
/// the name, which only receiver resolution can rule out.
fn report_residue(residues: &[Residue]) {
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
        eprintln!(
            "  ... and {} more. Narrow the rule with a `where:` receiver \
             constraint to scope this report.",
            residues.len() - RESIDUE_DETAIL_CAP
        );
    }
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

    let mut scoped: Vec<String> = paths.to_vec();
    scoped.extend(common.path.iter().cloned());
    let files = source::ruby_files(&scoped, common.include_vendored);

    // Residue is collected across the parallel walk, so it needs a shared sink.
    let residues: std::sync::Mutex<Vec<Residue>> = std::sync::Mutex::new(Vec::new());

    let mut found: Vec<Found> = files
        .par_iter()
        .flat_map_iter(|path| {
            let Ok(src) = std::fs::read(path) else {
                return Vec::new().into_iter();
            };
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
            let hits = matcher::search(&p_root, &parsed.node(), &prepared);

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
                    Some(class) => residue::scoped_to(extra, class),
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
            if emit_rows(out, &found).is_some() {
                return Exit::Error.into();
            }
        }
    }

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
    edits: usize,
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
    let rules = match rule::load_all(rule_arg, replace) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("rwr: {e}");
            return Exit::PatternError.into();
        }
    };
    if rules.iter().any(|r| r.rewrite.is_none()) {
        eprintln!("rwr: {}", rule::RuleError::NoTemplate);
        return Exit::PatternError.into();
    }
    // Each rule is prepared once; they apply in order, each seeing the
    // previous one's output. A rename genuinely needs a set, since the
    // definition and the call sites are different shapes.
    let mut prepareds = Vec::with_capacity(rules.len());
    for r in &rules {
        match prepare::prepare(&r.pattern) {
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

    let mut scoped: Vec<String> = paths.to_vec();
    scoped.extend(common.path.iter().cloned());
    let files = source::ruby_files(&scoped, common.include_vendored);

    // Each file is independent: one refusal declines that file and is reported,
    // rather than aborting work already proven safe elsewhere (DESIGN.md §4).
    struct Outcome {
        file: String,
        edits: usize,
        rewritten: Option<String>,
        refusal: Option<String>,
        residue: Vec<Residue>,
    }

    // The class a rule set is about, used to scope its own residue report.
    let class_anchor: Option<String> = rules.iter().find_map(|r| {
        r.scope
            .inside
            .clone()
            .or_else(|| r.constraints.values().find_map(|c| c.receiver_type.clone()))
    });

    let outcomes: Vec<Outcome> = files
        .par_iter()
        .filter_map(|path| {
            let original = std::fs::read(path).ok()?;
            let file = path.display().to_string();
            let mut current = original.clone();
            let mut total = 0usize;

            for (rule, prepared) in rules.iter().zip(&prepareds) {
                // Scoped so every borrow of `current` ends before it is
                // replaced with this rule's output.
                let step: Result<Option<(String, usize)>, String> = {
                    let parsed = ruby_prism::parse(&current);
                    if parsed.errors().count() > 0 {
                        return None;
                    }
                    let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
                    let p_node = p_parsed.node();
                    match matcher::pattern_root(&p_node) {
                        None => Ok(None),
                        Some(p_root) => {
                            let hits: Vec<_> = matcher::search(&p_root, &parsed.node(), prepared)
                                .into_iter()
                                .filter(|m| matcher::satisfies(m, &rule.constraints, &rule.scope))
                                .collect();
                            if hits.is_empty() {
                                Ok(None)
                            } else {
                                let template = rule.rewrite.as_deref().unwrap_or_default();
                                match rewrite::plan(&hits, &p_root, prepared, template, &current) {
                                    Err(r) => Err(format!("{r:?}")),
                                    Ok(edits) => {
                                        let text = rewrite::apply(&current, &edits);
                                        match rewrite::verify(&text) {
                                            Err(r) => Err(format!("{r:?}")),
                                            Ok(()) => Ok(Some((text, edits.len()))),
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
                            edits: 0,
                            rewritten: None,
                            refusal: Some(refusal),
                            residue: Vec::new(),
                        });
                    }
                    Ok(None) => {}
                    Ok(Some((text, n))) => {
                        total += n;
                        current = text.into_bytes();
                    }
                }
            }

            if total == 0 {
                return None;
            }

            // What the rule set could not account for (D7). Reported against
            // the *rewritten* source, so an occurrence a rule already handled
            // is not counted twice -- and so a subclass call site left behind
            // by a rename is visible rather than silently broken.
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
                        occurrences = residue::scoped_to(occurrences, class);
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

            Some(Outcome {
                file,
                edits: total,
                rewritten: Some(String::from_utf8_lossy(&current).into_owned()),
                refusal: None,
                residue,
            })
        })
        .collect();

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
        changed.push(Changed {
            file: outcome.file.clone(),
            edits: outcome.edits,
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
                println!("{}: {verb} {} site(s)", c.file, c.edits);
            }
            report_residue(&left_over);
        }
        _ => {
            if emit_rows(out, &changed).is_some() {
                return Exit::Error.into();
            }
        }
    }

    if refused {
        return Exit::Refused.into();
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
            explain: false,
        };
        assert_eq!(c.output(), Output::Ndjson);
    }
}
