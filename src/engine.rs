//! The evaluator: a rule set applied to one source.
//!
//! Split out of `cli::cmd_apply` so that `check`, `rewrite` and `test` share
//! one code path rather than growing parallel ones. A fixture that ran through
//! a second, simpler evaluator would test something other than what ships --
//! which is the drift the fixtures exist to catch, arriving through the back
//! door.
//!
//! What stays in `cli`: walking, reading, `--diff` resolution, templates,
//! unsafe/ruby gating, writing, reporting, exit codes. What lives here is the
//! part that would be identical if the source came from a file, a fixture, or
//! a pipe.

use crate::pattern::{matcher, prefilter, prepare};
use crate::profile;
use crate::residue;
use crate::rewrite;
use crate::rule;
use crate::rule::Constraint;
use crate::source;
use serde::Serialize;
use std::collections::HashMap;

/// An occurrence a rule could not account for.
#[derive(Debug, Serialize, Clone)]
pub(crate) struct Residue {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) context: residue::Context,
    /// The rule whose name this occurrence is, when the run has named rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rule: Option<String>,
    pub(crate) text: String,
}

/// A match of a rule that proposes no edit.
#[derive(Debug, Serialize, Clone)]
pub(crate) struct Finding {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) rule: String,
    pub(crate) note: String,
    pub(crate) text: String,
}

/// A site a rule would rewrite, with somewhere to point.
///
/// The per-file count answers "how much"; a CI annotation needs "where", and
/// `changed` carried only the first.
#[derive(Debug, Serialize, Clone)]
pub(crate) struct Rewrite {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
    /// The last line the site occupies. A suggestion replaces whole lines, and a
    /// multi-line rewrite needs both ends.
    pub(crate) end_line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rule: Option<String>,
    /// The rule's own one-liner. A reviewer reading an annotation wants to know
    /// what the rule is *for*, which the rule already says and the report was
    /// throwing away.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    /// What lines `line..=end_line` become -- the body of a `suggestion` block.
    pub(crate) replacement: String,
}

/// A site the pattern matched, then a constraint declined -- as reported.
///
/// Field names follow the standing contract: `file`, `line`, `rule` mean what
/// they mean everywhere else.
#[derive(Debug, Serialize, Clone)]
pub(crate) struct Rejection {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rule: Option<String>,
    /// Which capture was refused. Absent for a scope miss, which is about the
    /// match as a whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) capture: Option<String>,
    /// Which predicate declined it: `name`, `type`, `is`, `contains`, `length`,
    /// `same_name_as`, `inside`, `singleton`, or `rule-bug`.
    pub(crate) constraint: &'static str,
    /// What it wanted, and what it saw.
    pub(crate) detail: String,
    /// The source text of the refused binding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bound: Option<String>,
}

/// A prepared rule set, ready to apply to any number of sources.
///
/// Everything checkable before a source is read is checked in [`Engine::new`]:
/// a rule that is wrong should say so once, not run clean and do the wrong
/// amount of work on every file.
pub(crate) struct Engine {
    rules: Vec<rule::Rule>,
    prepareds: Vec<prepare::Prepared>,
    /// Sub-patterns for `contains:`, prepared once per rule rather than per
    /// candidate match -- preparing is a parse-and-retry loop.
    contained: Vec<HashMap<String, prepare::Prepared>>,
    /// One filter per rule. Checked disjunctively to decide whether to read a
    /// file at all, and individually inside the scan so a ten-rule pack does
    /// not walk every file's tree ten times.
    filters: Vec<prefilter::Filter>,
    /// Whether this run claims completeness at all. Residue applies only where
    /// the rule set moves a *definition* (D7 as amended twice): a set that only
    /// rewrites call sites leaves every name it did not touch working, so it
    /// has nothing to be incomplete about.
    claims_completeness: bool,
    /// A rule set that names no class cannot tell `Account#display_name` from
    /// `Company#display_name`, so its matches are tallied by resolved receiver.
    unnarrowed: bool,
    /// The class the rule set is about, when it names one.
    anchor: Option<String>,
    /// Per rule, the identifiers its template introduces.
    introduced: Vec<Vec<String>>,
}

/// The local variables a scope declares, with the byte range it covers.
///
/// Prism keeps a local table on every node that opens a scope. It is derived
/// rather than written, so it is deliberately not part of equality (D73) -- but
/// it is exactly the right answer to "is this name already taken here".
fn scopes(parsed: &ruby_prism::ParseResult<'_>) -> Vec<(usize, usize, Vec<String>)> {
    let mut out = Vec::new();
    let mut stack = vec![crate::pattern::generated::dup(&parsed.node())];
    while let Some(node) = stack.pop() {
        let locals = match &node {
            ruby_prism::Node::DefNode { .. } => node.as_def_node().map(|n| {
                n.locals()
                    .iter()
                    .map(|l| String::from_utf8_lossy(l.as_slice()).into_owned())
                    .collect::<Vec<_>>()
            }),
            ruby_prism::Node::BlockNode { .. } => node.as_block_node().map(|n| {
                n.locals()
                    .iter()
                    .map(|l| String::from_utf8_lossy(l.as_slice()).into_owned())
                    .collect::<Vec<_>>()
            }),
            ruby_prism::Node::ProgramNode { .. } => node.as_program_node().map(|n| {
                n.locals()
                    .iter()
                    .map(|l| String::from_utf8_lossy(l.as_slice()).into_owned())
                    .collect::<Vec<_>>()
            }),
            _ => None,
        };
        if let Some(locals) = locals
            && !locals.is_empty()
        {
            let at = node.location();
            out.push((at.start_offset(), at.end_offset(), locals));
        }
        stack.extend(crate::pattern::generated::children(&node));
    }
    // Innermost first, so the first range containing an offset is its scope.
    out.sort_by_key(|(start, end, _)| (end - start, *start));
    out
}

/// Whether `name` is already a local where `offset` sits.
fn shadowed(scopes: &[(usize, usize, Vec<String>)], offset: usize, name: &str) -> bool {
    scopes
        .iter()
        .filter(|(start, end, _)| offset >= *start && offset < *end)
        .any(|(_, _, locals)| locals.iter().any(|l| l == name))
}

/// Whether a source activates any of `modules` with `using`.
///
/// A refinement is inert until a file says so, which is exactly what makes it
/// different from an `include`: the same call means different things in two
/// files, and only this call tells them apart.
fn activates(parsed: &ruby_prism::ParseResult<'_>, modules: &[String]) -> Option<String> {
    let mut stack = vec![crate::pattern::generated::dup(&parsed.node())];
    while let Some(node) = stack.pop() {
        if let Some(call) = node.as_call_node()
            && call.name().as_slice() == b"using"
            && let Some(arguments) = call.arguments()
            && let Some(named) = arguments.arguments().iter().next()
            && let Some(name) = crate::hierarchy::constant_name(&named)
            && modules.contains(&name)
        {
            return Some(name);
        }
        stack.extend(crate::pattern::generated::children(&node));
    }
    None
}

/// What the rules need to know about the wider program.
///
/// Built from whatever sources it is handed: for `check` that is the walked
/// repository, for a fixture it is the snippet itself. A snippet is a file, so
/// a rule needing a class or a signature says so in its own source rather than
/// being handed a synthetic one.
#[derive(Default)]
pub(crate) struct Context {
    hierarchy: crate::hierarchy::Hierarchy,
    sigs: crate::sigs::Signatures,
}

/// Restricting a scan to the lines a change touched.
#[derive(Clone, Copy)]
pub(crate) struct Only<'a> {
    pub(crate) changed: &'a crate::diff::Changed,
    pub(crate) absolute: &'a std::path::Path,
}

/// What one source had to say.
pub(crate) enum ScanOutcome {
    /// The source did not parse. `check` skips it; a fixture must fail rather
    /// than pass every negative assertion vacuously.
    Unparseable,
    /// Nothing matched and nothing was left unaccounted for.
    Quiet,
    /// An edit could not be made safely. One refusal declines this source and
    /// is reported, rather than aborting work already proven safe elsewhere
    /// (DESIGN.md section 4).
    Refused(String),
    Scanned(Box<Scanned>),
}

/// A source the rule set had something to say about.
#[derive(Default)]
pub(crate) struct Scanned {
    pub(crate) sites: usize,
    /// Why candidates were declined. Empty unless `-e` asked.
    pub(crate) rejections: Vec<Rejection>,
    /// Findings a `# rwr:ignore` directive accepted.
    pub(crate) suppressed: Vec<crate::suppress::Suppressed>,
    /// Directives that accepted nothing -- stale debt, reported unconditionally.
    pub(crate) stale: Vec<crate::suppress::Stale>,
    /// Directives naming no rule, which cannot be audited.
    pub(crate) malformed: Vec<crate::suppress::Malformed>,
    /// Classes this source's matched receivers resolved to, for the
    /// cross-class warning. Empty unless the rule set narrows by none.
    pub(crate) spread: Vec<String>,
    pub(crate) flagged: Vec<Finding>,
    /// Edits per rule, positionally. Attribution is per source because that is
    /// where the work happened; totals aggregate from it.
    pub(crate) by_rule: Vec<usize>,
    /// Where each rewritten site sits.
    pub(crate) rewrites: Vec<Rewrite>,
    pub(crate) rewritten: Option<String>,
    pub(crate) residue: Vec<Residue>,
    /// Matches a wider edit covered. Non-zero means a rerun makes further
    /// progress, which is the retryable outcome rather than a failure (D15).
    pub(crate) deferred: usize,
}

impl Engine {
    pub(crate) fn new(rules: Vec<rule::Rule>) -> Result<Self, String> {
        let contained: Vec<HashMap<String, prepare::Prepared>> = rules
            .iter()
            .map(rule::Rule::contained)
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;

        let mut prepareds = Vec::with_capacity(rules.len());
        for r in &rules {
            let p = prepare::prepare_with(&r.pattern, &r.constant_captures())
                .map_err(|e| e.to_string())?;
            // Scoped so the parse's borrow of `p` ends before it moves.
            let single = {
                let parsed = ruby_prism::parse(p.source.as_bytes());
                matcher::pattern_root(&parsed.node()).is_some()
            };
            if !single {
                return Err(format!(
                    "a pattern must be a single expression: {}",
                    r.pattern
                ));
            }
            r.validate(&p).map_err(|e| e.to_string())?;
            prepareds.push(p);
        }

        let claims_completeness = prepareds.iter().any(|prepared| {
            let parsed = ruby_prism::parse(prepared.source.as_bytes());
            matcher::pattern_root(&parsed.node())
                .is_some_and(|root| residue::defines_a_method(&root, prepared))
        });
        let unnarrowed = !rules
            .iter()
            .any(|r| r.constraints.values().any(Constraint::narrows_by_receiver));
        // Identifiers a rewrite brings in that the pattern did not have. If one
        // of them is already a local where the edit lands, the rewrite produces
        // a name collision -- `full_name = full_name` -- which parses, runs, and
        // means something else entirely.
        let introduced: Vec<Vec<String>> = rules
            .iter()
            .map(|r| {
                let Some(template) = r.rewrite.as_deref() else {
                    return Vec::new();
                };
                let had = prefilter::required(&r.pattern);
                prefilter::required(template)
                    .into_iter()
                    .filter(|name| !had.contains(name))
                    .collect()
            })
            .collect();

        let filters: Vec<prefilter::Filter> = rules
            .iter()
            .map(|r| prefilter::Filter::new(&prefilter::required(&r.pattern), &[]))
            .collect();

        let anchor = rules.iter().find_map(rule::Rule::class_anchor);
        Ok(Engine {
            anchor,
            introduced,
            rules,
            prepareds,
            contained,
            filters,
            claims_completeness,
            unnarrowed,
        })
    }

    pub(crate) fn rules(&self) -> &[rule::Rule] {
        &self.rules
    }

    /// Each rule beside its prepared pattern, for a caller that drives its own
    /// scan -- the ERB pass, whose per-rule re-translation and offset mapping
    /// make it a different data flow rather than another `scan` caller.
    pub(crate) fn prepared(&self) -> impl Iterator<Item = (&rule::Rule, &prepare::Prepared)> + '_ {
        self.rules.iter().zip(&self.prepareds)
    }

    /// The match criteria for one rule, built in one place so a caller cannot
    /// assemble them differently from the way `scan` does.
    pub(crate) fn criteria<'a>(&'a self, index: usize, ctx: &'a Context) -> matcher::Criteria<'a> {
        matcher::Criteria {
            explain: false,
            constraints: &self.rules[index].constraints,
            contained: &self.contained[index],
            scope: &self.rules[index].scope,
            hierarchy: &ctx.hierarchy,
            sigs: &ctx.sigs,
        }
    }

    pub(crate) fn claims_completeness(&self) -> bool {
        self.claims_completeness
    }

    /// Whether any rule needs this source read at all.
    ///
    /// A rule set's literals are checked disjunctively -- any one rule matching
    /// is enough to need the file.
    pub(crate) fn may_contribute(&self, bytes: &[u8]) -> bool {
        self.filters.iter().any(|f| f.may_contribute(bytes))
    }

    /// Build the class hierarchy and signature table these rules need.
    ///
    /// Both are built per run rather than cached: a full rails parse is under
    /// 200ms (Phase 0 measurement (d)), so there is no staleness to manage.
    /// Neither is built at all unless a rule asks for it.
    pub(crate) fn context(&self, sources: &[source::Source]) -> Context {
        let hierarchy = if self.rules.iter().any(|r| {
            r.scope.subclasses.unwrap_or(false)
                || r.constraints
                    .values()
                    .any(|c| c.subclasses.unwrap_or(false))
        }) {
            let started = profile::now();
            // Only the part of the hierarchy reachable from the classes the
            // rules name is needed, which is a handful rather than all of them.
            // Every named class, not the first one found: a constraint can name
            // several (`type_not: [TrueClass, FalseClass]`), and descent is
            // consulted for each.
            let roots: Vec<String> = self
                .rules
                .iter()
                .flat_map(|r| {
                    r.scope
                        .inside
                        .clone()
                        .into_iter()
                        .chain(r.constraints.values().flat_map(Constraint::hierarchy_roots))
                })
                .collect();
            let (h, parsed) = crate::hierarchy::Hierarchy::reachable_from(sources, &roots);
            let total = sources.len();
            profile::mark("hierarchy", started, || {
                format!("{parsed} parsed, {} skipped", total.saturating_sub(parsed))
            });
            h
        } else {
            crate::hierarchy::Hierarchy::default()
        };

        // Return types stated by Sorbet signatures. Built only when a rule
        // narrows by receiver, and costing a single substring search per file
        // in a repository that has none (D62).
        let sigs = if self
            .rules
            .iter()
            .any(|r| r.constraints.values().any(Constraint::narrows_by_receiver))
        {
            let started = profile::now();
            let (found, parsed) = crate::sigs::Signatures::from_sources(sources);
            profile::mark("signatures", started, || {
                format!("{} signature(s) from {parsed} file(s)", found.len())
            });
            found
        } else {
            crate::sigs::Signatures::default()
        };

        Context { hierarchy, sigs }
    }

    /// Apply the rule set to one source.
    ///
    /// `label` is what the source is called in the report -- a path for a file,
    /// a case name for a fixture.
    pub(crate) fn scan(
        &self,
        label: &str,
        bytes: &[u8],
        ctx: &Context,
        only: Option<Only<'_>>,
        explain: bool,
    ) -> ScanOutcome {
        let mut current = bytes.to_vec();
        let mut total = 0usize;
        let mut deferred = 0usize;
        let mut by_rule = vec![0usize; self.rules.len()];
        let mut spread: Vec<String> = Vec::new();
        let mut flagged: Vec<Finding> = Vec::new();
        let mut rewrites: Vec<Rewrite> = Vec::new();
        let mut rejections: Vec<Rejection> = Vec::new();
        let mut suppressed: Vec<crate::suppress::Suppressed> = Vec::new();
        // Keyed by document order, which survives the rewrites of a run where a
        // line number does not.
        let mut used: std::collections::HashSet<(usize, String)> = std::collections::HashSet::new();
        let mut directives: Vec<crate::suppress::Directive> = Vec::new();
        let mut malformed: Vec<crate::suppress::Malformed> = Vec::new();

        // One parse serves every rule until a rule actually rewrites something.
        // It used to be one parse *per rule*, so a ten-rule pack parsed each
        // candidate file ten times whether or not any rule matched -- measured
        // at ~85 ms per additional rule on discourse, for rules that matched
        // nothing at all.
        let mut next = 0;
        'generation: while next < self.rules.len() {
            // What a rule changed, carried out of the parse's scope so
            // `current` can be replaced once the borrow ends.
            let mut applied: Option<(String, usize, usize, usize)> = None;
            let step: Result<(), String> = {
                let parsed = ruby_prism::parse(&current);
                if parsed.errors().count() > 0 {
                    return ScanOutcome::Unparseable;
                }
                // A refinement active in this file intercepts the very call
                // the rename would rewrite. Rewriting it routes around the
                // refinement -- the refined behaviour silently stops happening,
                // with no error and nothing that fails to parse. Refuse the
                // file: a loud refusal is recoverable, and this rewrite is not.
                if let Some(anchor) = &self.anchor {
                    let refining = ctx.hierarchy.refined_by(anchor);
                    if !refining.is_empty()
                        && let Some(module) = activates(&parsed, refining)
                    {
                        return ScanOutcome::Refused(format!(
                            "`using {module}` refines {anchor} here, so a call may be \
                             dispatching to the refinement rather than the class"
                        ));
                    }
                }

                let (here, bad) = crate::suppress::directives(&parsed, &current);
                directives = here;
                malformed = bad
                    .into_iter()
                    .map(|(line, why)| crate::suppress::Malformed {
                        file: label.to_string(),
                        line,
                        why,
                    })
                    .collect();
                let mut outcome = Ok(());
                for (index, (rule, prepared)) in self
                    .rules
                    .iter()
                    .zip(&self.prepareds)
                    .enumerate()
                    .skip(next)
                {
                    // Each rule's own literals, not just the set's union.
                    // Without a per-rule gate every rule walked the whole tree
                    // of every file any rule wanted, which for a ten-rule pack
                    // is nine wasted walks per file.
                    if !self.filters[index].may_contribute(&current) {
                        continue;
                    }
                    let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
                    let p_node = p_parsed.node();
                    type Applied = (String, usize, usize, Vec<rewrite::Site>);
                    let found: Result<Option<Applied>, String> =
                        match matcher::pattern_root(&p_node) {
                            None => Ok(None),
                            Some(p_root) => {
                                // Criteria are applied *inside* the search, so a
                                // constraint rejection drives backtracking to a
                                // different binding rather than discarding the
                                // match (Q13).
                                let criteria = matcher::Criteria {
                                    explain,
                                    constraints: &rule.constraints,
                                    contained: &self.contained[index],
                                    scope: &rule.scope,
                                    hierarchy: &ctx.hierarchy,
                                    sigs: &ctx.sigs,
                                };
                                let (mut hits, declined) = matcher::search_explaining(
                                    &p_root,
                                    &parsed.node(),
                                    prepared,
                                    &criteria,
                                );
                                for r in declined {
                                    let (line, col) = source::line_col(&current, r.start);
                                    rejections.push(Rejection {
                                        file: label.to_string(),
                                        line,
                                        col,
                                        rule: rule.id.clone(),
                                        capture: r.verdict.capture(),
                                        constraint: r.verdict.constraint(),
                                        detail: r.verdict.detail(!ctx.sigs.is_empty()),
                                        bound: r.bound.map(|(a, b)| {
                                            String::from_utf8_lossy(&current[a..b]).into_owned()
                                        }),
                                    });
                                }
                                // Accepted findings drop out before anything
                                // else looks at them, so `check` and `rewrite`
                                // cannot disagree about which sites exist.
                                if !directives.is_empty() {
                                    hits.retain(|m| {
                                        let (start, _) = rewrite::effective_range(&m.node);
                                        let (line, _) = source::line_col(&current, start);
                                        let id = rule.id.as_deref();
                                        match directives.iter().find(|d| d.covers(id, start)) {
                                            None => true,
                                            Some(d) => {
                                                used.insert((
                                                    d.index,
                                                    id.unwrap_or_default().to_string(),
                                                ));
                                                suppressed.push(crate::suppress::Suppressed {
                                                    file: label.to_string(),
                                                    line,
                                                    rule: rule.id.clone(),
                                                    source: "directive",
                                                });
                                                false
                                            }
                                        }
                                    });
                                }
                                // A rewrite that brings in a name already
                                // bound as a local here would produce
                                // `full_name = full_name`: valid Ruby, quietly
                                // meaning something else, and `verify` passes
                                // it. Refuse rather than write it.
                                if !self.introduced[index].is_empty() {
                                    let here = scopes(&parsed);
                                    for hit in &hits {
                                        let (start, _) = rewrite::effective_range(&hit.node);
                                        if let Some(name) = self.introduced[index]
                                            .iter()
                                            .find(|name| shadowed(&here, start, name))
                                        {
                                            let (line, _) = source::line_col(&current, start);
                                            return ScanOutcome::Refused(format!(
                                                "line {line}: `{name}` is already a local \
                                                 variable here, so the rewrite would collide \
                                                 with it"
                                            ));
                                        }
                                    }
                                }
                                if let Some(only) = only {
                                    hits.retain(|m| {
                                        let (start, end) = rewrite::effective_range(&m.node);
                                        let (first, _) = source::line_col(&current, start);
                                        let (last, _) = source::line_col(&current, end);
                                        only.changed.touches(only.absolute, first, last)
                                    });
                                }
                                // A rule that does not say which class it means
                                // may be renaming across several. Recorded here,
                                // warned about once at the end (Q10).
                                if self.unnarrowed {
                                    for hit in &hits {
                                        if let Some(class) = matcher::receiver_class(hit, &ctx.sigs)
                                        {
                                            spread.push(class);
                                        }
                                    }
                                }
                                // A finding rule proposes no edit: its matches
                                // are reported and the source left alone, so the
                                // parse still describes it.
                                if rule.rewrite.is_none() {
                                    for hit in &hits {
                                        let (start, _) = rewrite::effective_range(&hit.node);
                                        let (line, col) = source::line_col(&current, start);
                                        flagged.push(Finding {
                                            file: label.to_string(),
                                            line,
                                            col,
                                            rule: rule.id.clone().unwrap_or_default(),
                                            note: rule.description.clone().unwrap_or_default(),
                                            text: source::line_at(&current, start),
                                        });
                                    }
                                    Ok(None)
                                } else if hits.is_empty() {
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
                                        Err(r) => Err(r.to_string()),
                                        Ok(planned) => {
                                            let text = rewrite::apply(&current, &planned.edits);
                                            // Parses, *and* says what the rule
                                            // said it would. The first alone
                                            // passed `any?xs`.
                                            match rewrite::verify(&text).and_then(|()| {
                                                rewrite::verify_template(
                                                    &text,
                                                    &planned.matched,
                                                    &planned.edits,
                                                    template,
                                                    &rule.constant_captures(),
                                                )
                                            }) {
                                                Err(r) => Err(r.to_string()),
                                                Ok(()) => Ok(Some((
                                                    text,
                                                    planned.sites,
                                                    planned.dropped,
                                                    planned.at.clone(),
                                                ))),
                                            }
                                        }
                                    }
                                }
                            }
                        };

                    match found {
                        Err(refusal) => {
                            outcome = Err(refusal);
                            break;
                        }
                        // Nothing changed, so the parse still describes the
                        // source and the next rule can reuse it.
                        Ok(None) => {}
                        Ok(Some((text, sites, dropped, at))) => {
                            for site in &at {
                                let (line, col) = source::line_col(&current, site.start);
                                let (end_line, _) = source::line_col(&current, site.end);
                                rewrites.push(Rewrite {
                                    file: label.to_string(),
                                    line,
                                    col,
                                    end_line,
                                    rule: rule.id.clone(),
                                    note: rule.description.clone(),
                                    replacement: site.replacement.clone(),
                                });
                            }
                            applied = Some((text, sites, dropped, index));
                            break;
                        }
                    }
                }
                outcome
            };

            if let Err(refusal) = step {
                return ScanOutcome::Refused(refusal);
            }
            match applied {
                // Every remaining rule left the source alone.
                None => break 'generation,
                Some((text, sites, dropped, index)) => {
                    total += sites;
                    by_rule[index] += sites;
                    deferred += dropped;
                    current = text.into_bytes();
                    next = index + 1;
                }
            }
        }

        // A directive that accepted nothing is stale debt -- the symmetry that
        // keeps this from becoming rubocop_todo: a suppression whose finding is
        // gone is itself a finding. Only asserted for rules this run actually
        // evaluated; a directive naming a rule from another pack is left alone.
        let mine: Vec<&str> = self.rules.iter().filter_map(|r| r.id.as_deref()).collect();
        let stale: Vec<crate::suppress::Stale> = directives
            .iter()
            .flat_map(|d| {
                d.rules
                    .iter()
                    .filter(|r| mine.contains(&r.as_str()))
                    .filter(|r| !used.contains(&(d.index, (*r).clone())))
                    .map(|r| crate::suppress::Stale {
                        file: label.to_string(),
                        line: d.line,
                        rule: r.clone(),
                        source: "directive",
                    })
            })
            .collect();

        let residue = self.residue(label, &current, ctx);
        if total == 0
            && residue.is_empty()
            && flagged.is_empty()
            && rejections.is_empty()
            && suppressed.is_empty()
            && stale.is_empty()
            && malformed.is_empty()
        {
            return ScanOutcome::Quiet;
        }
        ScanOutcome::Scanned(Box::new(Scanned {
            sites: total,
            rewrites,
            rejections,
            suppressed,
            stale,
            malformed,
            spread,
            flagged,
            by_rule,
            // Nothing to write when nothing changed, so the write path skips a
            // source that only contributed residue.
            rewritten: (total > 0).then(|| String::from_utf8_lossy(&current).into_owned()),
            residue,
            deferred,
        }))
    }

    /// What the rule set could not account for in this source.
    ///
    /// Computed whether or not the source changed. It used to sit behind an
    /// early return for `total == 0`, so a file containing *only* dynamic
    /// reaches -- a serializer full of `delegate` and `validates`, which is the
    /// dangerous case exactly -- was never looked at. Measured on the testbed:
    /// recall 4 of 7 to 7 of 7.
    ///
    /// Reported against the *rewritten* source, so an occurrence a rule already
    /// handled is not counted twice -- and so a subclass call site left behind
    /// by a rename is visible rather than silently broken.
    fn residue(&self, label: &str, current: &[u8], ctx: &Context) -> Vec<Residue> {
        if !self.claims_completeness {
            return Vec::new();
        }
        let parsed = ruby_prism::parse(current);
        let mut found = Vec::new();
        for (rule, prepared) in self.rules.iter().zip(&self.prepareds) {
            let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
            let p_node = p_parsed.node();
            let Some(p_root) = matcher::pattern_root(&p_node) else {
                continue;
            };
            let anchors = residue::anchors(&p_root, prepared);
            if anchors.is_empty() {
                continue;
            }
            let mut occurrences = residue::find(&parsed.node(), &anchors, &[], current);
            // Comments live beside the tree, not in it, so they need their own
            // pass -- and a rename that leaves `# returns the display_name`
            // behind has left something stale that this report should name.
            occurrences.extend(residue::in_comments(&parsed, &anchors, current));
            // Each rule scopes by *its own* class. Taking the set's first meant
            // a pack of two renames reported everything against the first one's
            // class and dropped the second's entirely.
            if let Some(class) = rule.class_anchor() {
                occurrences = residue::scoped_to(occurrences, &class, &ctx.hierarchy);
            }
            found.extend(occurrences.into_iter().map(|o| {
                let (line, col) = source::line_col(current, o.byte_start);
                Residue {
                    file: label.to_string(),
                    line,
                    col,
                    context: o.context,
                    // Which rule's name this is. A pack can run several renames,
                    // and an unlabelled block leaves the reader guessing which
                    // one an occurrence belongs to.
                    rule: rule.id.clone(),
                    text: source::line_at(current, o.byte_start),
                }
            }));
        }
        found.sort_by_key(|r| (r.line, r.col));
        found.dedup_by_key(|r| (r.line, r.col));
        found
    }
}
