//! Edit computation and splicing.
//!
//! The conflict unit is the *edit* range, not the match range (D15). Because
//! edits are minimal, nested matches usually produce disjoint edits and both
//! apply cleanly; only genuine overlap aborts.
//!
//! Splicing goes through [`effective_range`] only (D14). A heredoc body lives
//! far from its `<<~FOO` token, and detaching one still *parses*, so no
//! downstream check would catch the mistake.

use crate::pattern::generated;
use crate::pattern::matcher::{Bound, Env, Match};
use crate::pattern::metavar::{self, Arity};
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
    /// Two edits overlap partially, so neither can be applied without
    /// corrupting the other (D15).
    Overlap { first: Edit, second: Edit },
    /// The rewritten source no longer parses, or parses to something other than
    /// intended. The whole transformation is discarded (DESIGN.md §7).
    VerifyFailed { message: String },
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
pub(crate) fn render(template: &str, env: &Env<'_>, source: &[u8]) -> Result<String, Refusal> {
    let vars = metavar::scan(template);
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0;

    for var in &vars {
        let mut start = var.start;
        let replacement: Option<String> = match var.name.as_ref().and_then(|n| env.get(n)) {
            None => None,
            Some(Bound::Name(bytes)) => Some(String::from_utf8_lossy(bytes).into_owned()),
            Some(bound) => {
                captured_text(bound, source)?.map(|b| String::from_utf8_lossy(b).into_owned())
            }
        };

        let text = match replacement {
            Some(text) => text,
            // An empty sequence: drop a separator that would otherwise dangle,
            // so `foo($A, *$REST)` with nothing left renders `foo(a)`.
            None if var.arity == Arity::Many => {
                let head = &template[..var.start];
                let trimmed = head.trim_end();
                if trimmed.ends_with(',') {
                    start = trimmed.len() - 1;
                }
                String::new()
            }
            None => String::new(),
        };

        out.push_str(&template[cursor..start.max(cursor)]);
        out.push_str(&text);
        cursor = var.end;
    }
    out.push_str(&template[cursor..]);
    Ok(out)
}

/// Turn matches into edits, keeping outermost on conflict and refusing on
/// partial overlap (D15).
pub(crate) fn plan(
    matches: &[Match<'_>],
    template: &str,
    source: &[u8],
) -> Result<Vec<Edit>, Refusal> {
    let mut edits: Vec<Edit> = matches
        .iter()
        .map(|m| {
            let (start, end) = effective_range(&m.node);
            Ok(Edit {
                start,
                end,
                text: render(template, &m.env, source)?,
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;

    // Outermost first: a wider edit that contains a narrower one wins, and the
    // contained match is dropped rather than applied against stale offsets.
    edits.sort_by_key(|e| (e.start, std::cmp::Reverse(e.end)));

    let mut kept: Vec<Edit> = Vec::new();
    for edit in edits {
        match kept.last() {
            Some(previous) if edit.start < previous.end => {
                if edit.end <= previous.end {
                    // Fully contained: the outer edit already covers it.
                    continue;
                }
                return Err(Refusal::Overlap {
                    first: previous.clone(),
                    second: edit,
                });
            }
            _ => kept.push(edit),
        }
    }
    Ok(kept)
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
        let hits = matcher::search(&p_root, &parsed.node(), &prepared);

        let edits = plan(&hits, template, source.as_bytes())?;
        let out = apply(source.as_bytes(), &edits);
        verify(&out)?;
        Ok(out)
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
        // The padding inside the parens belongs to the call, not the argument,
        // so it is the template's to decide. What survives untouched is the
        // capture's own internal spacing.
        let out = rewrite("foo($A)", "bar($A)", "foo(  x  +  1  )\n").unwrap();
        assert_eq!(out, "bar(x  +  1)\n");
    }

    /// A heredoc's content is discontiguous -- the `<<~SQL` token sits inline
    /// and the body follows the enclosing line -- so splicing a capture that
    /// contains one would drag along text belonging to the enclosing call.
    /// rwr refuses rather than producing a subtly wrong file (principle 2).
    ///
    /// The right long-term fix is computing edits as a structural diff, where
    /// a capture that does not move is never spliced at all. Until then this
    /// declines work rather than doing it wrongly.
    #[test]
    fn heredoc_captures_are_refused_not_corrupted() {
        let src = "foo(<<~SQL)\n  SELECT 1\nSQL\n";
        assert!(matches!(
            rewrite("foo($A)", "bar($A)", src),
            Err(Refusal::DiscontiguousCapture { .. })
        ));
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

    /// Nested matches produce disjoint edits only when the inner one is not
    /// contained; a contained match is dropped, not applied against stale
    /// offsets (D15).
    #[test]
    fn contained_matches_are_dropped_not_misapplied() {
        let out = rewrite("foo($A)", "bar($A)", "foo(foo(1))\n").unwrap();
        assert_eq!(out, "bar(foo(1))\n");
        verify(&out).expect("still parses");
    }
}
