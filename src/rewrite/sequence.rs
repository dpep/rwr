//! Sequence transforms in rewrite templates (D33), with comment attachment (D35).
//!
//! `[*$ITEMS]` -> `[*$ITEMS.sort]` needs something a substituting template
//! cannot express, because sorting is *computation*. The set is closed by
//! definition rather than by listing: **zero-argument, deterministic, total,
//! sequence-to-sequence**. That admits `sort`, `uniq` and `reverse` and
//! excludes `sort_by` by construction, since a block is user code -- the line
//! principle 8 draws.
//!
//! Reordering is where comment attachment becomes load-bearing (D35): a
//! comment on an element's own line travels with it, and a comment above it on
//! its own line does too.

use crate::rewrite::effective_range;
use ruby_prism::Node;

/// A transform a template may apply to a sequence capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transform {
    Sort,
    Uniq,
    Reverse,
}

impl Transform {
    /// Recognise a transform suffix, e.g. the `.sort` in `*$ITEMS.sort`.
    ///
    /// An unrecognised suffix is *not* silently treated as literal output --
    /// the caller reports it, because a typo'd transform that emitted
    /// `items.srot` would be a silent wrong rewrite.
    pub(crate) fn parse(suffix: &str) -> Option<Self> {
        match suffix {
            "sort" => Some(Transform::Sort),
            "uniq" => Some(Transform::Uniq),
            "reverse" => Some(Transform::Reverse),
            _ => None,
        }
    }
}

/// One element together with the comments that belong to it.
#[derive(Debug, Clone)]
struct Unit {
    /// Full span including attached comments.
    start: usize,
    end: usize,
    /// The element's own text, which is what ordering compares.
    key: Vec<u8>,
}

/// Whether every byte in `range` is blank or part of a `#` comment.
fn is_comment_line(source: &[u8], start: usize, end: usize) -> bool {
    let line = &source[start..end];
    let trimmed: Vec<u8> = line
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect();
    trimmed.first() == Some(&b'#')
}

fn line_start(source: &[u8], at: usize) -> usize {
    source[..at]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1)
}

fn line_end(source: &[u8], at: usize) -> usize {
    source[at..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(source.len(), |i| at + i)
}

/// Extend an element's span over the comments that belong to it (D35).
///
/// Trailing: a comment after the element on its own line. Leading: whole-line
/// comments directly above, with no blank line between. A comment sharing a
/// line with another element has no unambiguous owner, so the caller refuses.
fn unit_for(source: &[u8], element: &Node<'_>, floor: usize) -> Unit {
    let (start, end) = effective_range(element);
    let key = source[start..end].to_vec();

    let mut unit_start = line_start(source, start);
    // Only claim the line if the element begins it; otherwise a preceding
    // element shares the line and the span must not swallow it.
    if unit_start < floor
        || source[unit_start..start]
            .iter()
            .any(|b| !b.is_ascii_whitespace())
    {
        return Unit { start, end, key };
    }

    // Walk upward over contiguous whole-line comments.
    while unit_start > floor {
        let previous_end = unit_start.saturating_sub(1);
        let previous_start = line_start(source, previous_end);
        if previous_start < floor || !is_comment_line(source, previous_start, previous_end) {
            break;
        }
        unit_start = previous_start;
    }

    Unit {
        start: unit_start,
        end: line_end(source, end),
        key,
    }
}

/// Whether any comment sits on a line shared by more than one element.
///
/// Such a comment has no unambiguous owner, and silently reattaching it to a
/// neighbour is the quiet wrongness the design exists to prevent (D35).
fn has_ambiguous_comment(source: &[u8], elements: &[Node<'_>]) -> bool {
    for pair in elements.windows(2) {
        let (_, first_end) = effective_range(&pair[0]);
        let (second_start, _) = effective_range(&pair[1]);
        if line_start(source, first_end) == line_start(source, second_start) {
            // Two elements share a line; a `#` anywhere on it is ambiguous.
            let end = line_end(source, second_start);
            if source[first_end..end].contains(&b'#') {
                return true;
            }
        }
    }
    false
}

/// Apply `transform` to a captured sequence, returning the replacement text.
///
/// Returns `None` when a comment cannot be unambiguously attached.
/// `floor` is the offset just past the container's opening delimiter. Comments
/// are claimed only above it, so a leading comment on the *first* element is
/// still reachable -- using the first element's own line as the floor would
/// silently drop it.
pub(crate) fn render(
    source: &[u8],
    elements: &[Node<'_>],
    transform: Transform,
    floor: usize,
) -> Option<String> {
    if elements.is_empty() {
        return Some(String::new());
    }
    if has_ambiguous_comment(source, elements) {
        return None;
    }

    let mut units: Vec<Unit> = elements
        .iter()
        .map(|e| unit_for(source, e, floor))
        .collect();

    // The separator between elements as the author wrote it, so a one-per-line
    // array stays one-per-line and a single-line one stays single-line.
    let separator = if units.len() > 1 {
        String::from_utf8_lossy(&source[units[0].end..units[1].start]).into_owned()
    } else {
        String::new()
    };

    match transform {
        Transform::Sort => units.sort_by(|a, b| a.key.cmp(&b.key)),
        Transform::Reverse => units.reverse(),
        Transform::Uniq => {
            let mut seen = Vec::new();
            units.retain(|u| {
                let fresh = !seen.contains(&u.key);
                if fresh {
                    seen.push(u.key.clone());
                }
                fresh
            });
        }
    }

    let rendered: Vec<String> = units
        .iter()
        .map(|u| String::from_utf8_lossy(&source[u.start..u.end]).into_owned())
        .collect();
    Some(rendered.join(&separator))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elements_of<'a>(parsed: &'a ruby_prism::ParseResult<'a>) -> Vec<Node<'a>> {
        let node = parsed.node();
        let program = node.as_program_node().expect("program");
        let first = program
            .statements()
            .body()
            .iter()
            .next()
            .expect("one statement");
        let array = first.as_array_node().expect("an array");
        array.elements().iter().collect()
    }

    /// The offset just past the array's opening bracket.
    fn floor_of<'a>(parsed: &'a ruby_prism::ParseResult<'a>) -> usize {
        let node = parsed.node();
        let program = node.as_program_node().expect("program");
        let first = program
            .statements()
            .body()
            .iter()
            .next()
            .expect("one statement");
        let array = first.as_array_node().expect("an array");
        array.opening_loc().map_or(0, |l| l.end_offset())
    }

    fn sorted(src: &str) -> Option<String> {
        let parsed = ruby_prism::parse(src.as_bytes());
        let elements = elements_of(&parsed);
        render(
            src.as_bytes(),
            &elements,
            Transform::Sort,
            floor_of(&parsed),
        )
    }

    #[test]
    fn sorts_a_single_line_array() {
        assert_eq!(
            sorted("[:zebra, :apple, :mango]").as_deref(),
            Some(":apple, :mango, :zebra")
        );
    }

    /// The case D35 exists for: a leading comment on its own line and a
    /// trailing comment on the element's line both travel with their element.
    #[test]
    fn comments_travel_with_their_element() {
        let src = "[\n  # about zebra\n  :zebra,\n  :apple, # about apple\n  :mango,\n]";
        let out = sorted(src).expect("not ambiguous");
        let apple = out.find("about apple").expect("apple comment kept");
        let zebra = out.find("about zebra").expect("zebra comment kept");
        assert!(
            apple < zebra,
            "comments did not move with their elements:\n{out}"
        );
    }

    /// A comment sharing a line with two elements has no unambiguous owner, so
    /// reordering refuses rather than reattaching it to a neighbour.
    #[test]
    fn an_ambiguous_comment_refuses() {
        let src = "[\n  :zebra, :apple, # which one?\n  :mango,\n]";
        assert!(sorted(src).is_none());
    }

    #[test]
    fn uniq_and_reverse() {
        let parsed = ruby_prism::parse(b"[:a, :b, :a]");
        let elements = elements_of(&parsed);
        let floor = floor_of(&parsed);
        assert_eq!(
            render(b"[:a, :b, :a]", &elements, Transform::Uniq, floor).as_deref(),
            Some(":a, :b")
        );
        assert_eq!(
            render(b"[:a, :b, :a]", &elements, Transform::Reverse, floor).as_deref(),
            Some(":a, :b, :a")
        );
    }

    /// The closed set is defined, not listed: a block is user code, which is
    /// the line principle 8 draws.
    #[test]
    fn only_the_closed_set_is_recognised() {
        assert!(Transform::parse("sort").is_some());
        assert!(Transform::parse("uniq").is_some());
        assert!(Transform::parse("reverse").is_some());
        assert!(Transform::parse("sort_by").is_none());
        assert!(Transform::parse("map").is_none());
    }
}
