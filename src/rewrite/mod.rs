//! Edit computation and splicing.
//!
//! The conflict unit is the *edit* range, not the match range (D15). Because
//! edits are minimal, nested matches usually produce disjoint edits and both
//! apply cleanly; only genuine overlap aborts.
//!
//! Splicing goes through [`effective_range`] only (D14). A heredoc body lives
//! far from its `<<~FOO` token, and detaching one still *parses*, so no
//! downstream check would catch the mistake.

// Reachable only from its own tests until `render` wires transforms into
// template substitution. Drop this allow then.
#[allow(dead_code)]
pub(crate) mod sequence;

use crate::pattern::compare::Atom;
use crate::pattern::generated;
use crate::pattern::matcher::{self, Bound, Env, Match};
use crate::pattern::metavar::{self, Arity};
use crate::pattern::prepare::{self, Prepared};
use ruby_prism::Node;

/// One replacement of a source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Why a rewrite was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A deletion whose match does not occupy whole lines of its own.
    ///
    /// Removing it would leave a hole in an expression, and the result can
    /// still parse -- `x = a.name` becomes `x = ` and swallows the next line.
    PartialDeletion { text: String },
    /// Two edits overlap partially, so neither can be applied without
    /// corrupting the other (D15).
    Overlap { first: Edit, second: Edit },
    /// The rewritten source no longer parses, or parses to something other than
    /// intended. The whole transformation is discarded (DESIGN.md §7).
    VerifyFailed { message: String },
    /// A rewrite template applies an unknown transform to a sequence, e.g.
    /// `*$ITEMS.srot`. Emitting it as literal text would be a silent wrong
    /// rewrite, so it is reported (D33).
    UnknownTransform { name: String },
    /// Reordering a sequence would move a comment that shares a line with
    /// several elements, so it has no unambiguous owner (D35).
    AmbiguousComment,
    /// The rewritten site does not match the template it was written from.
    ///
    /// The splice produced valid Ruby that means something other than the rule
    /// asked for, which [`verify`] cannot see.
    TemplateMismatch { text: String, template: String },
    /// A metavariable holds different source text after the rewrite than it did
    /// before. The shape is right and the wrong bytes are inside it.
    CaptureMoved {
        capture: String,
        was: String,
        now: String,
    },
    /// A capture being spliced contains a heredoc, whose content is
    /// *discontiguous*: the `<<~FOO` token sits inline while the body lives
    /// after the enclosing line. Splicing it by range would drag along text
    /// belonging to the enclosing expression.
    ///
    /// `effective_range` stops an edit being truncated; it cannot make a
    /// heredoc movable. Until edits are computed as a structural diff -- where
    /// a capture that does not move is never spliced at all -- refusing is the
    /// honest answer (principle 2).
    DiscontiguousCapture { at: usize },
}

/// A refusal is a *product*, not a diagnostic: refusing rather than guessing is
/// the whole contract, and it costs the caller a round trip -- so it has to say
/// what happened in a sentence they can act on. These were being printed with
/// `{:?}`, so a typo'd sequence transform read `UnknownTransform { name: "srot" }`.
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::PartialDeletion { text } => write!(
                f,
                "deleting `{text}` would leave a hole in the expression around it, \
                 and the result can still parse"
            ),
            Refusal::Overlap { first, second } => write!(
                f,
                "two edits overlap and neither can apply without corrupting the other \
                 ({}..{} and {}..{})",
                first.start, first.end, second.start, second.end
            ),
            Refusal::VerifyFailed { message } => {
                write!(f, "the rewritten source did not verify: {message}")
            }
            Refusal::UnknownTransform { name } => write!(
                f,
                "`.{name}` is not a sequence transform -- expected `.sort`, `.uniq` or \
                 `.reverse`. Emitting it as literal text would write `.{name}` into your source"
            ),
            Refusal::AmbiguousComment => write!(
                f,
                "a comment shares a line with elements being reordered and could describe \
                 either neighbour, so there is no way to say which it belongs to"
            ),
            Refusal::TemplateMismatch { text, template } => write!(
                f,
                "the rewrite produced `{text}`, which is not what `{template}` describes -- valid \
                 Ruby, and not the transformation the rule asked for"
            ),
            Refusal::CaptureMoved { capture, was, now } => write!(
                f,
                "the rewrite moved ${capture}: it captured `{was}` before and `{now}` after, so \
                 the result has the right shape around the wrong code"
            ),
            Refusal::DiscontiguousCapture { at } => write!(
                f,
                "a capture at byte {at} contains a heredoc, whose body sits outside the \
                 expression -- splicing it by range would drag along surrounding text"
            ),
        }
    }
}

/// The span a node truly occupies, including content its own location excludes.
///
/// A heredoc's `closing_loc` sits past its body, so unioning every location the
/// node and its descendants carry extends the range over content that
/// `node.location()` stops short of. Only the end extends: heredoc bodies
/// always follow their opening token, never precede it.
pub(crate) fn effective_range(node: &Node<'_>) -> (usize, usize) {
    let base = node.location();
    let start = base.start_offset();
    let mut end = base.end_offset();

    let mut stack = vec![generated::dup(node)];
    while let Some(current) = stack.pop() {
        end = end.max(current.location().end_offset());
        for (_, e) in generated::locations(&current) {
            end = end.max(e);
        }
        stack.extend(generated::children(&current));
    }
    (start, end)
}

/// Whether a node's content runs past its own location -- true exactly when it
/// contains a heredoc, whose body sits outside the span the node reports.
fn is_discontiguous(node: &Node<'_>) -> bool {
    effective_range(node).1 > node.location().end_offset()
}

/// The source text a capture stands for, preserving its original formatting.
fn captured_text<'a>(bound: &Bound<'_>, source: &'a [u8]) -> Result<Option<&'a [u8]>, Refusal> {
    match bound {
        Bound::One(node) => {
            if is_discontiguous(node) {
                return Err(Refusal::DiscontiguousCapture {
                    at: node.location().start_offset(),
                });
            }
            let (s, e) = effective_range(node);
            Ok(Some(&source[s..e]))
        }
        Bound::Many(nodes) => {
            if nodes.iter().any(is_discontiguous) {
                let at = nodes
                    .iter()
                    .find(|n| is_discontiguous(n))
                    .map_or(0, |n| n.location().start_offset());
                return Err(Refusal::DiscontiguousCapture { at });
            }
            let (Some(first), Some(last)) = (nodes.first(), nodes.last()) else {
                return Ok(None);
            };
            // Span the whole run rather than re-joining the elements, so the
            // separators and layout the author wrote survive untouched.
            Ok(Some(
                &source[effective_range(first).0..effective_range(last).1],
            ))
        }
        Bound::Name(_) => Ok(None),
    }
}

/// Render a rewrite template, substituting captures with their original source.
///
/// The captured subtrees keep their exact formatting; the template governs
/// everything around them.
/// Whether two nodes' atoms mean the same thing.
///
/// A metavariable substitutes to a *different placeholder identifier* in the
/// pattern and the template -- `$P` may be `rwr_mv_2` in one and `rwr_mv_1` in
/// the other -- so comparing atom bytes reports a difference where there is
/// none, and alignment gives up on every pattern with a metavariable in a name
/// position. Placeholders are compared by the metavariable they stand for.
fn atoms_correspond(
    pattern: &Node<'_>,
    template: &Node<'_>,
    prepared: &Prepared,
    t_prepared: &Prepared,
) -> bool {
    let (p_atoms, t_atoms) = (generated::atoms(pattern), generated::atoms(template));
    if p_atoms.len() != t_atoms.len() {
        return false;
    }
    p_atoms.iter().zip(&t_atoms).all(|(p, t)| match (p, t) {
        (Atom::Name(pn), Atom::Name(tn)) => {
            let meta = |bytes: &[u8], prep: &Prepared| {
                std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|s| prep.bindings.get(s))
                    .and_then(|b| b.name.clone())
            };
            match (meta(pn, prepared), meta(tn, t_prepared)) {
                (Some(a), Some(b)) => a == b,
                (None, None) => pn == tn,
                // A placeholder on one side only is a genuine difference.
                _ => false,
            }
        }
        _ => p == t,
    })
}

/// Align a template against one of the pattern's children, deleting whatever
/// surrounded it in the target.
///
/// Conservative: it takes the first child that aligns, and alignment already
/// requires matching shape and corresponding metavariables, so an accidental
/// match is unlikely. Failing returns `None` and the caller replaces the whole
/// span -- correct, merely non-minimal.
#[allow(clippy::too_many_arguments)]
fn unwrap_to_subtree(
    p_kids: &[Node<'_>],
    template: &Node<'_>,
    x_kids: &[Node<'_>],
    target: &Node<'_>,
    prepared: &Prepared,
    t_prepared: &Prepared,
    template_src: &[u8],
    env: &Env<'_>,
    source: &[u8],
) -> Option<Vec<Edit>> {
    if p_kids.len() != x_kids.len() {
        return None;
    }
    for (p_child, x_child) in p_kids.iter().zip(x_kids) {
        let Some(mut edits) = structural_diff(
            p_child,
            template,
            x_child,
            prepared,
            t_prepared,
            template_src,
            env,
            source,
        ) else {
            continue;
        };
        let (outer_start, outer_end) = effective_range(target);
        let (inner_start, inner_end) = effective_range(x_child);
        if outer_start > inner_start || inner_end > outer_end {
            continue;
        }
        if outer_start < inner_start {
            edits.push(Edit {
                start: outer_start,
                end: inner_start,
                text: String::new(),
            });
        }
        if inner_end < outer_end {
            edits.push(Edit {
                start: inner_end,
                end: outer_end,
                text: String::new(),
            });
        }
        return Some(edits);
    }
    None
}

/// A container's span between its delimiters, for sequence transforms.
fn inner_span(node: &Node<'_>) -> Option<(usize, usize)> {
    let (open, close) = match node {
        Node::ArrayNode { .. } => {
            let a = node.as_array_node()?;
            (a.opening_loc()?, a.closing_loc()?)
        }
        Node::HashNode { .. } => {
            let h = node.as_hash_node()?;
            (h.opening_loc(), h.closing_loc())
        }
        _ => return None,
    };
    Some((open.end_offset(), close.start_offset()))
}

/// A `.name` suffix directly after a metavariable in a template.
fn transform_suffix(template: &str, at: usize) -> Option<(&str, usize)> {
    let rest = template.get(at..)?.strip_prefix('.')?;
    let len = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    (len > 0).then(|| (&rest[..len], at + 1 + len))
}

pub(crate) fn render(
    template: &str,
    env: &Env<'_>,
    source: &[u8],
    inner: (usize, usize),
) -> Result<String, Refusal> {
    let vars = metavar::scan(template);
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0;

    for var in &vars {
        let mut start = var.start;
        let mut end = var.end;

        // A transform is recognised only on a *sequence* capture: `$X.sort` on a
        // single capture is legitimate literal output, so arity disambiguates
        // (D33).
        let mut transformed: Option<String> = None;
        if var.arity == Arity::Many
            && let Some((name, after)) = transform_suffix(template, var.end)
        {
            let Some(transform) = sequence::Transform::parse(name) else {
                return Err(Refusal::UnknownTransform {
                    name: name.to_string(),
                });
            };
            let nodes = match var.name.as_ref().and_then(|n| env.get(n)) {
                Some(Bound::Many(nodes)) => nodes.as_slice(),
                _ => &[],
            };
            let Some(text) = sequence::render(source, nodes, transform, inner) else {
                return Err(Refusal::AmbiguousComment);
            };
            transformed = Some(text);
            end = after;
        }
        let replacement: Option<String> = match var.name.as_ref().and_then(|n| env.get(n)) {
            None => None,
            Some(Bound::Name(bytes)) => Some(String::from_utf8_lossy(bytes).into_owned()),
            Some(bound) => {
                captured_text(bound, source)?.map(|b| String::from_utf8_lossy(b).into_owned())
            }
        };

        let text = match transformed.or(replacement) {
            Some(text) => text,
            // An empty sequence: drop a separator that would otherwise dangle,
            // so `foo($A, *$REST)` with nothing left renders `foo(a)`.
            None if var.arity == Arity::Many => {
                // An empty sequence must not leave a dangling separator on
                // either side: `foo($A, *$R)` renders `foo(a)`, and
                // `{**$B, $K:}` renders `{k:}` rather than `{, k:}`.
                let head = &template[..var.start];
                let trimmed = head.trim_end();
                if trimmed.ends_with(',') {
                    start = trimmed.len() - 1;
                } else {
                    let tail = &template[end..];
                    let stripped = tail.trim_start();
                    if let Some(rest) = stripped.strip_prefix(',') {
                        end = template.len() - rest.trim_start().len();
                    }
                }
                String::new()
            }
            None => String::new(),
        };

        out.push_str(&template[cursor..start.max(cursor)]);
        out.push_str(&text);
        cursor = end;
    }
    out.push_str(&template[cursor..]);
    Ok(out)
}

/// Turn matches into edits, keeping outermost on conflict and refusing on
/// partial overlap (D15).
pub(crate) fn plan(
    matches: &[Match<'_>],
    pattern_root: &Node<'_>,
    pattern_prepared: &Prepared,
    template: &str,
    source: &[u8],
    constants: &[String],
) -> Result<Planned, Refusal> {
    // The template is prepared and parsed once so its tree can be aligned
    // against the pattern's. Placeholder ids differ between the two, but both
    // resolve to the same metavariable names, which is what alignment keys on.
    // Seeded identically to the pattern, or the two trees would not align:
    // `$C = [...]` is a constant write on one side and a local write on the
    // other, and the diff would see a shape change that is not there.
    let t_prepared = prepare::prepare_with(template, constants).ok();
    let t_parsed = t_prepared
        .as_ref()
        .map(|p| ruby_prism::parse(p.source.as_bytes()));
    let t_node = t_parsed.as_ref().map(ruby_prism::ParseResult::node);
    let t_root = t_node.as_ref().and_then(matcher::pattern_root);

    // Each edit remembers which match produced it: a shape-changing rewrite
    // emits several edits for one site, so edits cannot stand in for sites.
    let mut edits: Vec<(usize, Edit)> = Vec::new();
    for (index, m) in matches.iter().enumerate() {
        // Minimal first: where pattern and template agree in shape, edit only
        // what differs and leave every untouched subtree's bytes alone.
        let minimal = match (&t_root, &t_prepared) {
            (Some(root), Some(tp)) => structural_diff(
                pattern_root,
                root,
                &m.node,
                pattern_prepared,
                tp,
                tp.source.as_bytes(),
                &m.env,
                source,
            ),
            _ => None,
        };

        match minimal {
            Some(found) if !found.is_empty() => {
                edits.extend(found.into_iter().map(|e| (index, e)));
            }
            // No difference at all: the rule is a no-op here.
            Some(_) => {}
            // An empty template is a deletion, and deletion means the *unit*:
            // the node, the comments written directly above it, and the line it
            // ends on. Replacing only the node's own bytes leaves its comment
            // stranded above a blank gap, which is not what anyone means by
            // deleting a method (D66).
            _ if template.trim().is_empty() => {
                let Some((start, end)) = sequence::unit_range(source, &m.node) else {
                    return Err(Refusal::PartialDeletion {
                        text: String::from_utf8_lossy(
                            &source[effective_range(&m.node).0..effective_range(&m.node).1],
                        )
                        .into_owned(),
                    });
                };
                edits.push((
                    index,
                    Edit {
                        start,
                        end,
                        text: String::new(),
                    },
                ));
            }
            // Shapes diverge, so fall back to replacing the whole span. Correct,
            // merely non-minimal.
            None => {
                let (start, end) = effective_range(&m.node);
                edits.push((
                    index,
                    Edit {
                        start,
                        end,
                        text: render(
                            template,
                            &m.env,
                            source,
                            inner_span(&m.node).unwrap_or_else(|| {
                                let l = m.node.location();
                                (l.start_offset(), l.end_offset())
                            }),
                        )?,
                    },
                ));
            }
        }
    }

    // Outermost first: a wider edit that contains a narrower one wins, and the
    // contained match is dropped rather than applied against stale offsets.
    edits.sort_by_key(|(_, e)| (e.start, std::cmp::Reverse(e.end)));
    let mut dropped = 0usize;

    let mut kept: Vec<(usize, Edit)> = Vec::new();
    for (index, edit) in edits {
        match kept.last().map(|(_, e)| e) {
            Some(previous) if edit.start < previous.end => {
                if edit.end <= previous.end {
                    // Contained in a wider edit. Dropping it is correct -- its
                    // offsets are stale the moment the outer edit applies -- but
                    // the caller must be told, or a rule that needed two passes
                    // looks like one that finished (D15).
                    dropped += 1;
                    continue;
                }
                return Err(Refusal::Overlap {
                    first: previous.clone(),
                    second: edit,
                });
            }
            _ => kept.push((index, edit)),
        }
    }

    // A site counts once however many edits it took, and only if one survived.
    let mut sites: Vec<usize> = kept.iter().map(|(i, _)| *i).collect();
    sites.sort_unstable();
    sites.dedup();
    // Where each surviving site starts, and what its lines become.
    //
    // A count tells a reader how much there is; a location tells a CI annotation
    // where to point; and the replacement text is what lets a review comment
    // carry an *applicable* suggestion rather than a description of one. All
    // three were being computed and discarded.
    //
    // Expanded to whole lines because that is the unit a suggestion replaces --
    // GitHub's `suggestion` block substitutes the commented lines entire, so a
    // byte-range replacement would need the rest of the line reconstructed
    // anyway.
    let mut at: Vec<Site> = sites
        .iter()
        .filter_map(|index| {
            let mine: Vec<&Edit> = kept
                .iter()
                .filter(|(i, _)| i == index)
                .map(|(_, e)| e)
                .collect();
            let start = mine.iter().map(|e| e.start).min()?;
            let end = mine.iter().map(|e| e.end).max()?;
            let from = source[..start]
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |n| n + 1);
            let to = source[end..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(source.len(), |n| end + n);

            // The site's own edits applied to its own lines, so the result is
            // what those lines become and nothing else moves.
            let mut text = source[from..to].to_vec();
            let mut ordered: Vec<&&Edit> = mine.iter().collect();
            ordered.sort_by_key(|e| std::cmp::Reverse(e.start));
            for edit in ordered {
                let (a, b) = (edit.start - from, edit.end - from);
                text.splice(a..b, edit.text.bytes());
            }
            Some(Site {
                start: from,
                end: to,
                replacement: String::from_utf8_lossy(&text).into_owned(),
            })
        })
        .collect();
    at.sort_by_key(|s| s.start);
    let mut paired: Vec<SiteCaptures> = sites
        .iter()
        .filter_map(|index| {
            let m = matches.get(*index)?;
            Some((effective_range(&m.node), captured_texts(&m.env, source)))
        })
        .collect();
    paired.sort_by_key(|(span, _)| *span);
    let (matched, captures): (Vec<_>, Vec<_>) = paired.into_iter().unzip();
    Ok(Planned {
        sites: sites.len(),
        at,
        edits: kept.into_iter().map(|(_, e)| e).collect(),
        dropped,
        matched,
        captures,
    })
}

/// One changed site, as whole lines and their replacement.
#[derive(Debug, Clone)]
pub(crate) struct Site {
    pub start: usize,
    pub end: usize,
    /// What `source[start..end]` becomes -- directly usable as the body of a
    /// GitHub `suggestion` block.
    pub replacement: String,
}

/// Edits to apply, and how many matches were dropped as contained.
#[derive(Debug)]
pub(crate) struct Planned {
    pub edits: Vec<Edit>,
    /// Matched sites that changed. A shape-changing rewrite emits several edits
    /// for one site, so reporting `edits.len()` overstates what a reader sees
    /// in the diff.
    pub sites: usize,
    /// Each changed site: the lines it occupies and what they become.
    pub at: Vec<Site>,
    /// Matches skipped because a wider edit covered them. Non-zero means a
    /// rerun will make further progress -- the retryable outcome (exit 4).
    pub dropped: usize,
    /// Each changed site's matched node, as a span in the *pre-edit* source.
    ///
    /// Kept so the result can be checked against the template it came from:
    /// shifted by the edits, this is where the rewritten node lands.
    pub matched: Vec<(usize, usize)>,
    /// What each site's metavariables captured, as source text, aligned with
    /// `matched`. Compared against what they capture in the *result*, which is
    /// how a correct shape wrapped around the wrong capture is caught.
    pub captures: Vec<Vec<Capture>>,
}

/// Apply edits to source. Edits must be disjoint and sorted, as [`plan`] leaves
/// them.
pub(crate) fn apply(source: &[u8], edits: &[Edit]) -> String {
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for edit in edits {
        out.push_str(&String::from_utf8_lossy(&source[cursor..edit.start]));
        out.push_str(&edit.text);
        cursor = edit.end;
    }
    out.push_str(&String::from_utf8_lossy(&source[cursor..]));
    out
}

/// Reparse the rewritten source and discard the whole transformation if it no
/// longer parses (DESIGN.md §7).
///
/// This is the backstop for range arithmetic: a mistake that produces invalid
/// Ruby is caught before it reaches a file. It cannot catch a mistake that
/// happens to stay valid -- which is why `effective_range` exists rather than
/// relying on this.
pub(crate) fn verify(rewritten: &str) -> Result<(), Refusal> {
    let parsed = ruby_prism::parse(rewritten.as_bytes());
    match parsed.errors().next() {
        None => Ok(()),
        Some(e) => Err(Refusal::VerifyFailed {
            message: e.message().to_string(),
        }),
    }
}

/// One site's matched span paired with what its metavariables captured.
type SiteCaptures = ((usize, usize), Vec<Capture>);

/// What one metavariable held, and where it held it.
///
/// The span is in pre-edit coordinates and is what makes a *nested* rewrite
/// distinguishable from a corrupted splice: another site sitting inside this
/// span rewrote the capture legitimately, and comparing text across that is
/// comparing two different questions.
#[derive(Debug, Clone)]
pub(crate) struct Capture {
    pub name: String,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// What each metavariable captured, as source text and span.
///
/// A capture rwr cannot render contiguously -- a heredoc -- yields nothing and
/// is simply absent, so it is never compared rather than compared wrongly. A
/// bare name binding has no span and is likewise absent.
fn captured_texts(env: &Env<'_>, source: &[u8]) -> Vec<Capture> {
    let mut out: Vec<Capture> = env
        .iter()
        .filter_map(|(name, bound)| {
            let text = captured_text(bound, source).ok()??;
            let (start, end) = match bound {
                Bound::One(node) => effective_range(node),
                Bound::Many(nodes) => (
                    effective_range(nodes.first()?).0,
                    effective_range(nodes.last()?).1,
                ),
                Bound::Name(_) => return None,
            };
            Some(Capture {
                name: name.clone(),
                text: String::from_utf8_lossy(text).into_owned(),
                start,
                end,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Check that each rewritten site is now what the template said it would be.
///
/// [`verify`] reparses and catches a splice that produces invalid Ruby. It
/// cannot catch one that produces *valid* Ruby meaning something else, and its
/// own comment says so: `!$X.empty?` -> `$X.any?` once wrote `any?xs`, which
/// Ruby reads as `any?(xs)`, and every check passed. This is the one that
/// notices, by asking the obvious question afterwards -- does the node we just
/// wrote match the template we wrote it from?
///
/// **Conservative by construction.** Anything it cannot check it skips, and a
/// skip is never a failure: refusing a correct rewrite is worse than missing an
/// incorrect one, because the first breaks a working run and the second leaves
/// things exactly as they were before this existed. It declines to judge an
/// empty template (a deletion has no shape to check), a template carrying a
/// sequence transform (`*$ITEMS.sort` is an instruction, not output), one that
/// is not a single expression, and any site whose node it cannot locate again.
///
/// Only the *shape* is checked, not the bindings: metavariables match freely
/// here. A splice that puts the right shape around the wrong capture is a
/// narrower bug than one that mangles the shape, and this is the cheap half.
pub(crate) fn verify_template(
    rewritten: &str,
    matched: &[(usize, usize)],
    captures: &[Vec<Capture>],
    edits: &[Edit],
    template: &str,
    constants: &[String],
) -> Result<(), Refusal> {
    if template.trim().is_empty()
        || metavar::scan(template)
            .iter()
            .any(|v| v.arity != Arity::One)
    {
        return Ok(());
    }
    let Some(prepared) = prepare::prepare_with(template, constants).ok() else {
        return Ok(());
    };
    let parsed_pattern = ruby_prism::parse(prepared.source.as_bytes());
    let pattern_node = parsed_pattern.node();
    let Some(root) = matcher::pattern_root(&pattern_node) else {
        return Ok(());
    };

    let parsed = ruby_prism::parse(rewritten.as_bytes());
    let tree = parsed.node();
    for (index, (start, end)) in matched.iter().enumerate() {
        let (from, to) = (shifted(edits, *start), shifted(edits, *end));
        let Some(node) = node_at(&tree, from, to) else {
            continue;
        };
        let found = matcher::search(&root, &node, &prepared, &matcher::Criteria::none());
        let text = || {
            rewritten
                .get(from..to)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        // The match rooted at the node itself, not one nested inside it: a
        // template that also matches a sub-expression would otherwise be checked
        // against the wrong thing.
        let Some(whole) = found
            .iter()
            .find(|m| effective_range(&m.node) == (from, to))
        else {
            return Err(Refusal::TemplateMismatch {
                text: text(),
                template: template.trim().to_string(),
            });
        };

        // Shape agrees. Now the captures: a metavariable the template carries
        // over from the pattern must hold the same source text afterwards,
        // because a capture is spliced verbatim. This is what catches the right
        // shape wrapped around the wrong bytes -- `foo($A, $B)` emitted with the
        // arguments swapped matches its own template perfectly.
        let after = captured_texts(&whole.env, rewritten.as_bytes());
        let Some(before) = captures.get(index) else {
            continue;
        };
        for capture in before {
            // A *different* site sitting inside this capture rewrote it, and
            // legitimately: `$R.freeze` over `x.freeze.freeze` captures
            // `x.freeze` on the outer match and rewrites it on the inner one, so
            // the text differs for a reason that is the rule working. Comparing
            // across that refused a correct rewrite outright.
            //
            // The site's own edits never land inside its own captures -- a
            // capture is carried over verbatim -- so an edit there is still a
            // corrupted splice and is still caught.
            if matched
                .iter()
                .enumerate()
                .any(|(other, (s, e))| other != index && *s >= capture.start && *e <= capture.end)
            {
                continue;
            }
            let Some(now) = after.iter().find(|c| c.name == capture.name) else {
                // Dropped by the template, or not renderable on one side. Either
                // way there is nothing to compare, and inventing a comparison is
                // how a checker starts refusing correct work.
                continue;
            };
            if now.text != capture.text {
                return Err(Refusal::CaptureMoved {
                    capture: capture.name.clone(),
                    was: capture.text.clone(),
                    now: now.text.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Where an offset lands once `edits` are applied.
///
/// Edits are disjoint and sorted, so an offset is displaced by every edit that
/// finishes at or before it -- the site's own edits included, which is what puts
/// its end in the right place.
fn shifted(edits: &[Edit], offset: usize) -> usize {
    let delta: isize = edits
        .iter()
        .filter(|e| e.end <= offset)
        .map(|e| e.text.len() as isize - (e.end - e.start) as isize)
        .sum();
    (offset as isize + delta).max(0) as usize
}

/// The node occupying exactly `from..to`, if one does.
///
/// Exact rather than containing: a node that merely spans the range is the
/// enclosing statement, and matching a template against that asks a different
/// question than the one intended.
fn node_at<'pr>(tree: &Node<'pr>, from: usize, to: usize) -> Option<Node<'pr>> {
    let mut stack = vec![generated::dup(tree)];
    while let Some(node) = stack.pop() {
        let (s, e) = effective_range(&node);
        if s == from && e == to {
            return Some(node);
        }
        if s <= from && e >= to {
            stack.extend(generated::children(&node));
        }
    }
    None
}

/// Structural-diff editing: emit edits only where pattern and template differ.
///
/// Rendering a whole template re-imposes its layout on everything it covers,
/// which is why a multiline chain collapses and a `do ... end` block comes back
/// as braces. Aligning the two trees instead means an unchanged subtree is
/// never spliced, so its formatting -- and any heredoc inside it -- survives.
///
/// Deliberately conservative: it descends only while the two trees agree in
/// shape and gives up the moment they diverge, leaving the caller to replace the
/// whole span. Giving up is always *correct*, merely non-minimal, which is the
/// right direction to be wrong in.
///
/// **Scope today:** same-shape rewrites, where pattern and template differ only
/// in atoms. That covers renames, the largest rule family. A shape-changing
/// rewrite -- `$R.select { .. }.first` -> `$R.detect { .. }`, which drops an
/// enclosing call -- still falls back to full replacement.
/// The metavariable a body-position lone placeholder stands for.
///
/// `def foo; $B; end` parses its body as a `StatementsNode` holding one
/// placeholder. The matcher binds that to the target's whole body whatever shape
/// it has (D73), so the diff has to recognise the same thing or it will call the
/// body diverged.
fn lone_placeholder(node: &Node<'_>, prepared: &Prepared) -> Option<String> {
    if !matches!(node, Node::StatementsNode { .. }) {
        return None;
    }
    let kids = generated::children(node);
    let [only] = kids.as_slice() else {
        return None;
    };
    matcher::placeholder_name(only, prepared)
}

#[allow(clippy::too_many_arguments)]
fn structural_diff(
    pattern: &Node<'_>,
    template: &Node<'_>,
    target: &Node<'_>,
    prepared: &Prepared,
    t_prepared: &Prepared,
    template_src: &[u8],
    env: &Env<'_>,
    source: &[u8],
) -> Option<Vec<Edit>> {
    // Each side resolves placeholders through its *own* mapping: the pattern
    // and template are prepared separately, so `rwr_mv_1` may be `$SEL` in one
    // and `$P` in the other. Sharing one mapping silently misaligned them.
    let p_meta = matcher::placeholder_name(pattern, prepared);
    let t_meta = matcher::placeholder_name(template, t_prepared);

    // The same metavariable on both sides: this subtree is carried across
    // untouched, so emit nothing and let the original bytes stand.
    if let (Some(p), Some(t)) = (&p_meta, &t_meta) {
        return (p == t).then(Vec::new);
    }
    // A placeholder on one side only means the subtree is introduced or
    // dropped; there is no correspondence to edit through.
    if p_meta.is_some() || t_meta.is_some() {
        return None;
    }

    let (p_kids, t_kids, x_kids) = (
        generated::children(pattern),
        generated::children(template),
        generated::children(target),
    );

    // Shapes differ. Before giving up, check whether the template corresponds to
    // a *subtree* of the pattern -- which is what a rewrite that unwraps looks
    // like. `$R.select { .. }.first -> $R.detect { .. }` drops an enclosing
    // call, so the template aligns with the pattern's receiver, and the edit is
    // that subtree's own differences plus deleting what surrounded it.
    if std::mem::discriminant(pattern) != std::mem::discriminant(template)
        || p_kids.len() != t_kids.len()
        || name_slot(pattern) != name_slot(template)
    {
        return unwrap_to_subtree(
            &p_kids,
            template,
            &x_kids,
            target,
            prepared,
            t_prepared,
            template_src,
            env,
            source,
        );
    }

    // A body that is the same lone metavariable on both sides is carried across
    // untouched, whatever shape the target's body has. A `def` carrying `rescue`
    // has a `BeginNode` body rather than a `StatementsNode`, so without this the
    // diff called the body diverged, localized to the whole `def`, and emitted a
    // second edit that *contained* the correct one -- which was then dropped as
    // nested, leaving the file unchanged while the run claimed a rewrite and
    // asked to be run again forever.
    if let (Some(p_body), Some(t_body)) = (
        lone_placeholder(pattern, prepared),
        lone_placeholder(template, t_prepared),
    ) && p_body == t_body
    {
        return Some(Vec::new());
    }

    if std::mem::discriminant(pattern) != std::mem::discriminant(target) {
        return None;
    }

    // A sequence placeholder stands for a *run* of target children, so the two
    // child lists need not be the same length. Without this, every rule using
    // `*$REST` or `**$REST` fell straight through to whole-node replacement --
    // which is how hash shorthand reflowed multiline hashes onto one line.
    let aligned = align(&p_kids, &t_kids, &x_kids, prepared, t_prepared, env)?;

    let mut edits = Vec::new();

    // Atoms differing between pattern and template are the interesting case: a
    // method rename is exactly this, and it is one small edit over the name.
    if !atoms_correspond(pattern, template, prepared, t_prepared) {
        let (from, to) = (name_span(target)?, name_text(template, template_src)?);
        edits.push(Edit {
            start: from.0,
            end: from.1,
            text: to,
        });
    }

    for (p, t, x) in aligned {
        match structural_diff(p, t, x, prepared, t_prepared, template_src, env, source) {
            Some(found) => edits.extend(found),
            // This child diverges. Replacing *it* is still minimal -- every
            // sibling keeps its bytes -- so localize here rather than propagate
            // the failure up and re-render the whole node.
            None => edits.push(localized(t, x, t_prepared, template_src, env, source)?),
        }
    }
    Some(edits)
}

/// Pair pattern children with template and target children, one triple per
/// position that needs diffing.
///
/// A sequence placeholder matched on both sides is carried across untouched and
/// yields no triple; it only advances the target cursor by however many nodes it
/// captured. Anything else is one-to-one.
#[allow(clippy::type_complexity)]
fn align<'a, 'pr>(
    p_kids: &'a [Node<'pr>],
    t_kids: &'a [Node<'pr>],
    x_kids: &'a [Node<'pr>],
    prepared: &Prepared,
    t_prepared: &Prepared,
    env: &Env<'_>,
) -> Option<Vec<(&'a Node<'pr>, &'a Node<'pr>, &'a Node<'pr>)>> {
    if p_kids.len() != t_kids.len() {
        return None;
    }
    let mut triples = Vec::with_capacity(p_kids.len());
    let mut cursor = 0;
    for (p, t) in p_kids.iter().zip(t_kids) {
        // `def foo(*$P)` carries the parameter list across untouched, and when
        // the target has none it accounts for zero target children -- the same
        // treatment a sequence placeholder gets, which is what it is here.
        // Without this the alignment overran, the diff gave up, and the whole
        // `def` was re-rendered: `def full_name()` with the body reflowed.
        if let Some(name) = matcher::lone_rest_placeholder(p, prepared) {
            if matcher::lone_rest_placeholder(t, t_prepared).as_deref() != Some(&name) {
                return None;
            }
            match env.get(&name) {
                Some(Bound::Many(nodes)) => cursor += nodes.len(),
                Some(Bound::One(_)) => cursor += 1,
                _ => return None,
            }
            continue;
        }
        match matcher::splat_placeholder_name(p, prepared) {
            Some(name) => {
                // The same sequence must sit at the same position in the
                // template, or the rule is reordering and there is no
                // correspondence to edit through.
                if matcher::splat_placeholder_name(t, t_prepared).as_deref() != Some(&name) {
                    return None;
                }
                match env.get(&name) {
                    Some(Bound::Many(nodes)) => cursor += nodes.len(),
                    _ => return None,
                }
            }
            None => {
                triples.push((p, t, x_kids.get(cursor)?));
                cursor += 1;
            }
        }
    }
    // Every target child must be accounted for; a leftover means the alignment
    // is a guess rather than a correspondence.
    (cursor == x_kids.len()).then_some(triples)
}

/// Replace one target node with its corresponding template node, rendered.
///
/// The template is held in *prepared* form -- metavariables already substituted
/// for placeholder identifiers -- so it is restored to `$NAME` spelling before
/// rendering, which is what [`render`] understands.
fn localized(
    template: &Node<'_>,
    target: &Node<'_>,
    t_prepared: &Prepared,
    template_src: &[u8],
    env: &Env<'_>,
    source: &[u8],
) -> Option<Edit> {
    // The value half of `{foo:}` is an ImplicitNode, which borrows the *key's*
    // location rather than having one of its own -- so it has no text to
    // localize to, and rendering its span writes the key over the value. The
    // parent handles the assoc as a whole instead.
    if matches!(template, Node::ImplicitNode { .. }) {
        return None;
    }
    let loc = template.location();
    let fragment = template_src.get(loc.start_offset()..loc.end_offset())?;
    if fragment.is_empty() {
        return None;
    }
    let restored = restore(std::str::from_utf8(fragment).ok()?, t_prepared)?;
    let (start, end) = effective_range(target);
    let text = render(
        &restored,
        env,
        source,
        inner_span(target).unwrap_or((start, end)),
    )
    .ok()?;
    Some(Edit { start, end, text })
}

/// Turn placeholder identifiers back into `$NAME` metavariable spelling.
///
/// `None` when a placeholder is anonymous (`_`), since there is no name to write
/// and nothing in the environment to substitute for it.
fn restore(fragment: &str, prepared: &Prepared) -> Option<String> {
    let mut out = String::with_capacity(fragment.len());
    let bytes = fragment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_ident_start(bytes[i]) {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_ident_byte(bytes[i]) {
            i += 1;
        }
        let word = &fragment[start..i];
        match prepared.bindings.get(word) {
            None => out.push_str(word),
            Some(binding) => {
                out.push('$');
                out.push_str(binding.name.as_ref()?);
            }
        }
    }
    Some(out)
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The span of the identifier a node is named by, when it has one that can be
/// edited in isolation.
///
/// Covers the shapes a rename touches: a call's message and a definition's
/// name. Without the definition case, renaming `def foo` falls back to
/// replacing the whole method and collapses its body onto one line.
fn name_span(node: &Node<'_>) -> Option<(usize, usize)> {
    let loc = match node {
        Node::CallNode { .. } => node.as_call_node()?.message_loc()?,
        Node::DefNode { .. } => node.as_def_node()?.name_loc(),
        _ => return None,
    };
    Some((loc.start_offset(), loc.end_offset()))
}

/// Where a call writes its name, relative to its receiver.
///
/// `!x` and `x.any?` both hold their name in `message_loc`, so the rename
/// branch of [`structural_diff`] would write `any?` over the `!` of
/// `!xs.empty?` and leave `xs.empty?` standing -- `any?xs`, which Ruby parses
/// as `any?(xs)`. Valid, silent, and the opposite of what the rule asked for.
/// The two names are not the same slot, and only a slot-for-slot pair is a
/// rename; anything else has to unwrap or re-render.
///
/// `None` is "no receiver-relative slot at all" -- a bare `foo($X)`, or a node
/// that is not a call -- which likewise does not correspond to one that has one.
fn name_slot(node: &Node<'_>) -> Option<Slot> {
    let call = node.as_call_node()?;
    let message = call.message_loc()?;
    let receiver = call.receiver()?;
    Some(
        if message.start_offset() < receiver.location().start_offset() {
            Slot::BeforeReceiver
        } else {
            Slot::AfterReceiver
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `!x`, `-x`, `not x`
    BeforeReceiver,
    /// `x.any?`, `a + b`, `xs[0]`
    AfterReceiver,
}

/// The identifier the template calls for.
fn name_text(node: &Node<'_>, src: &[u8]) -> Option<String> {
    let (start, end) = name_span(node)?;
    Some(String::from_utf8_lossy(&src[start..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{matcher, prepare};

    /// Rewrite `source` with `pattern` -> `template`, or report the refusal.
    fn rewrite(pattern: &str, template: &str, source: &str) -> Result<String, Refusal> {
        let prepared = prepare::prepare(pattern).expect("pattern prepares");
        let p_parsed = ruby_prism::parse(prepared.source.as_bytes());
        let p_node = p_parsed.node();
        let p_root = matcher::pattern_root(&p_node).expect("single expression");

        let parsed = ruby_prism::parse(source.as_bytes());
        assert_eq!(parsed.errors().count(), 0, "source does not parse");
        let hits = matcher::search(
            &p_root,
            &parsed.node(),
            &prepared,
            &matcher::Criteria::none(),
        );

        let planned = plan(&hits, &p_root, &prepared, template, source.as_bytes(), &[])?;
        let out = apply(source.as_bytes(), &planned.edits);
        verify(&out)?;
        Ok(out)
    }

    /// A prefix operator's name and a `.method` name are both `message_loc`,
    /// and treating them as one slot wrote `any?` over the `!` and left the
    /// receiver's own call standing: `!xs.empty?` became `any?xs`, which parses
    /// as `any?(xs)`. Valid Ruby, so `verify` passed it; the wrong program, so
    /// nothing else would have.
    /// The check that notices a splice which parses and still means something
    /// else. Reconstructed from the real failure: `!$X.empty?` -> `$X.any?`
    /// once wrote `any?xs`, which Ruby reads as `any?(xs)` -- so `verify`
    /// passed it, and every other check was silent.
    #[test]
    fn a_valid_but_wrong_splice_is_caught() {
        // `a = !xs.empty?` with the `!` overwritten by `any?` and the receiver's
        // own call replaced by `xs` -- exactly the edits the bug emitted.
        let edits = [
            Edit {
                start: 4,
                end: 5,
                text: "any?".into(),
            },
            Edit {
                start: 5,
                end: 14,
                text: "xs".into(),
            },
        ];
        let rewritten = apply(b"a = !xs.empty?\n", &edits);
        assert_eq!(rewritten, "a = any?xs\n");
        // It parses, which is why this needed a second check at all.
        assert!(verify(&rewritten).is_ok());

        let matched = [(4usize, 14usize)];
        let out = verify_template(&rewritten, &matched, &[], &edits, "$X.any?", &[]);
        assert!(
            matches!(out, Err(Refusal::TemplateMismatch { .. })),
            "{out:?}"
        );

        // And the correct rewrite passes, so this is not simply always refusing.
        let good = [Edit {
            start: 4,
            end: 14,
            text: "xs.any?".into(),
        }];
        let fixed = apply(b"a = !xs.empty?\n", &good);
        assert_eq!(fixed, "a = xs.any?\n");
        assert!(verify_template(&fixed, &matched, &[], &good, "$X.any?", &[]).is_ok());
    }

    /// The half shape-checking cannot see: the right shape around the wrong
    /// bytes. A splice that swaps two captures produces something that matches
    /// its own template perfectly, so only the captures themselves say so.
    #[test]
    fn a_capture_holding_different_code_is_caught() {
        let source = b"foo(a, b)\n";
        // `a` and `b` exchanged -- same shape, same length, different program.
        let swapped = [
            Edit {
                start: 4,
                end: 5,
                text: "b".into(),
            },
            Edit {
                start: 7,
                end: 8,
                text: "a".into(),
            },
        ];
        let rewritten = apply(source, &swapped);
        assert_eq!(rewritten, "foo(b, a)\n");
        // Parses, and matches its own template: both earlier checks pass.
        assert!(verify(&rewritten).is_ok());

        let matched = [(0usize, 9usize)];
        let cap = |name: &str, text: &str, start, end| Capture {
            name: name.into(),
            text: text.into(),
            start,
            end,
        };
        // Spans of `a` and `b` in the original, which is how a nested rewrite is
        // told apart from a corrupted one.
        let before = vec![vec![cap("A", "a", 4, 5), cap("B", "b", 7, 8)]];
        let out = verify_template(&rewritten, &matched, &before, &swapped, "foo($A, $B)", &[]);
        match out {
            Err(Refusal::CaptureMoved {
                ref capture,
                ref was,
                ref now,
            }) => {
                assert_eq!(
                    (capture.as_str(), was.as_str(), now.as_str()),
                    ("A", "a", "b")
                );
            }
            other => panic!("expected CaptureMoved, got {other:?}"),
        }

        // The same captures left where they were pass, so this is not simply
        // refusing anything that moved bytes.
        let renamed = [Edit {
            start: 0,
            end: 3,
            text: "bar".into(),
        }];
        let ok = apply(source, &renamed);
        assert_eq!(ok, "bar(a, b)\n");
        assert!(verify_template(&ok, &matched, &before, &renamed, "bar($A, $B)", &[]).is_ok());
    }

    /// Skipping is never failing. A template it cannot reason about must let the
    /// rewrite through: refusing a correct rewrite breaks a working run, where
    /// missing an incorrect one only leaves things as they were.
    #[test]
    fn an_uncheckable_template_is_skipped_not_refused() {
        let edits = [Edit {
            start: 0,
            end: 1,
            text: "x".into(),
        }];
        let text = "x = [1]\n";
        // A deletion has no shape to check.
        assert!(verify_template(text, &[(0, 7)], &[], &edits, "", &[]).is_ok());
        // A sequence transform is an instruction, not output.
        assert!(verify_template(text, &[(0, 7)], &[], &edits, "$C = [*$I.sort]", &[]).is_ok());
        // A span that no longer names a node is not evidence of anything.
        assert!(verify_template(text, &[(3, 4)], &[], &edits, "$X.any?", &[]).is_ok());
    }

    #[test]
    fn a_prefix_operator_is_not_a_rename_of_a_method_name() {
        let out = rewrite("!$X.empty?", "$X.any?", "a = !xs.empty?\n").unwrap();
        assert_eq!(out, "a = xs.any?\n");
    }

    /// `not` and `!` are the same node, so the same rule reaches both.
    #[test]
    fn the_word_not_rewrites_like_the_bang() {
        let out = rewrite("!$X.empty?", "$X.any?", "a = not xs.empty?\n").unwrap();
        assert_eq!(out, "a = xs.any?\n");
    }

    /// The same slot mismatch the other way round: a receiver becoming an
    /// argument. This one failed silently -- the name matched, the alignment
    /// paired the receiver against the argument list, and the file came back
    /// unchanged while the run reported a rewritten site.
    #[test]
    fn a_receiver_moving_into_an_argument_is_re_rendered() {
        let out = rewrite("$X.foo", "foo($X)", "d = xs.foo\n").unwrap();
        assert_eq!(out, "d = foo(xs)\n");
    }

    /// Prefix on both sides still corresponds, and still edits only the name.
    #[test]
    fn a_prefix_operator_kept_on_both_sides_stays_minimal() {
        let out = rewrite("!$X.empty?", "!$X.any?", "a = !xs.empty?\n").unwrap();
        assert_eq!(out, "a = !xs.any?\n");
    }

    #[test]
    fn replaces_only_the_matched_span() {
        let out = rewrite("return nil", "return", "def a\n  return nil if x\nend\n").unwrap();
        assert_eq!(out, "def a\n  return if x\nend\n");
    }

    /// Comments, strings and heredoc bodies are not code, so they are not
    /// rewritten -- the failure that makes comby produce different working
    /// programs.
    #[test]
    fn prose_is_never_rewritten() {
        let src = "def a\n  # return nil\n  s = \"return nil\"\n  return nil\nend\n";
        let out = rewrite("return nil", "return", src).unwrap();
        assert!(out.contains("# return nil"), "comment was rewritten");
        assert!(out.contains("\"return nil\""), "string was rewritten");
        assert!(out.contains("  return\n"), "code was not rewritten");
    }

    /// Captured subtrees keep their own formatting; the template governs only
    /// what surrounds them.
    #[test]
    fn captures_preserve_their_source() {
        // Structural diffing edits only the method name, so every byte of the
        // argument -- including the padding inside the parens -- is left alone.
        let out = rewrite("foo($A)", "bar($A)", "foo(  x  +  1  )\n").unwrap();
        assert_eq!(out, "bar(  x  +  1  )\n");
    }

    /// A heredoc's content is discontiguous -- the `<<~SQL` token sits inline
    /// and the body follows the enclosing line -- so splicing a capture holding
    /// one would drag along text belonging to the enclosing call.
    ///
    /// Structural diffing removes the hazard rather than guarding it: the
    /// capture does not move, so it is never spliced, and only the method name
    /// is edited. The refusal this test once asserted no longer fires.
    #[test]
    fn heredoc_captures_survive_a_rename() {
        let src = "foo(<<~SQL)\n  SELECT 1\nSQL\n";
        let out = rewrite("foo($A)", "bar($A)", src).unwrap();
        assert_eq!(out, "bar(<<~SQL)\n  SELECT 1\nSQL\n");
        verify(&out).expect("rewritten source still parses");
    }

    /// A heredoc that is not captured is untouched, so rewriting around one
    /// still works.
    #[test]
    fn heredocs_outside_a_capture_are_unaffected() {
        let src = "def a\n  x = <<~SQL\n    SELECT 1\n  SQL\n  return nil\nend\n";
        let out = rewrite("return nil", "return", src).unwrap();
        assert!(out.contains("SELECT 1"), "{out}");
        assert!(out.contains("  return\n"), "{out}");
        verify(&out).expect("still parses");
    }

    #[test]
    fn sequence_captures_span_their_run() {
        let out = rewrite("foo(*$R)", "bar(*$R)", "foo(1, 2, 3)\n").unwrap();
        assert_eq!(out, "bar(1, 2, 3)\n");
    }

    /// An empty sequence must not leave a dangling separator.
    #[test]
    fn empty_sequences_drop_their_separator() {
        let out = rewrite("foo($A, *$R)", "bar($A, *$R)", "foo(1)\n").unwrap();
        assert_eq!(out, "bar(1)\n");
    }

    /// An empty sequence must not leave a dangling separator on *either* side.
    /// `{**$B, $K: $V}` with nothing before the pair renders `{a: 1}`, not
    /// `{, a: 1}` -- which reparse-verify would refuse, correctly but late.
    #[test]
    fn an_empty_leading_sequence_drops_its_separator() {
        let out = rewrite("{**$B, $K: $V}", "{**$B, $K: $V}", "x = {a: 1}\n").unwrap();
        assert_eq!(out, "x = {a: 1}\n");
    }

    /// D15's conflict unit is the *edit* range, not the match range. Minimal
    /// edits over nested matches are genuinely disjoint -- two method names in
    /// different places -- so both apply in one pass rather than the inner one
    /// needing a rerun.
    #[test]
    fn nested_matches_both_apply_when_their_edits_are_disjoint() {
        let out = rewrite("foo($A)", "bar($A)", "foo(foo(1))\n").unwrap();
        assert_eq!(out, "bar(bar(1))\n");
        verify(&out).expect("still parses");
    }

    /// A rewrite that *unwraps* -- the template corresponds to a subtree of the
    /// pattern -- still edits minimally. `.first` is deleted and `select`
    /// renamed, so the chain's layout and the block's spelling survive.
    #[test]
    fn a_shape_changing_rewrite_keeps_its_layout() {
        let src = "x = accounts\n  .select { |a| a.b }\n  .first\n";
        let out = rewrite("$R.select { |$P| $B }.first", "$R.detect { |$P| $B }", src).unwrap();
        assert_eq!(out, "x = accounts\n  .detect { |a| a.b }\n");
    }

    /// A `do ... end` block survives a rewrite whose template is written with
    /// braces, because an unchanged subtree is never re-rendered.
    #[test]
    fn block_spelling_survives_a_rewrite() {
        let src = "accounts.select do |a|\n  a.b\nend.first\n";
        let out = rewrite("$R.select { |$P| $B }.first", "$R.detect { |$P| $B }", src).unwrap();
        assert!(
            out.contains("do |a|"),
            "block was re-rendered as braces: {out}"
        );
        assert!(
            !out.contains(".first"),
            "the outer call was not dropped: {out}"
        );
    }

    /// The whole point of structural diffing: layout the rule does not mention
    /// is never re-rendered, so a multiline call keeps its shape.
    #[test]
    fn layout_outside_the_change_is_untouched() {
        let src = "foo(\n  a,\n  b,\n)\n";
        let out = rewrite("foo($A, $B)", "bar($A, $B)", src).unwrap();
        assert_eq!(out, "bar(\n  a,\n  b,\n)\n");
    }
}
