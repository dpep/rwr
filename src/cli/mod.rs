//! CLI surface: argument parsing, structured output, exit codes.
//!
//! The contract here is public from v0.1 (decision D17) and is specified in
//! `docs/cli-conventions.md`. Conventions are inherited from `rq` so that an
//! agent which has learned one of these tools has learned the others.

use crate::pattern::{matcher, prepare};
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
        Command::Check { .. } => not_yet("check", out),
        Command::Rewrite { .. } => not_yet("rewrite", out),
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

/// One structural match, as reported.
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

    let mut scoped: Vec<String> = paths.to_vec();
    scoped.extend(common.path.iter().cloned());
    let files = source::ruby_files(&scoped, common.include_vendored);

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

    match out {
        Output::Text => {
            for f in &found {
                println!("{}:{}:{}: {}", f.file, f.line, f.col, f.text);
            }
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

fn not_yet(what: &str, _out: Output) -> ExitCode {
    eprintln!("rwr: `{what}` is not implemented yet — see DESIGN.md for the plan");
    Exit::Error.into()
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
