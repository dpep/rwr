//! Tidying the whitespace at the seam an edit leaves (D128).
//!
//! An edit that removes or inserts text joins two pieces of source that were
//! not written to meet. rwr takes licence to tidy the whitespace *at that seam*
//! -- never beyond it, so untouched code keeps its bytes -- and to stop wherever
//! a comment could be joined onto code.

use super::Edit;

/// Widen `edit` over the whitespace its seam would leave wrong, staying within
/// `floor..ceil` so it never reaches a neighbouring edit.
///
/// - An emptied container closes up: `(\n  status: 200\n)` becomes `()`.
/// - A line the deletion empties goes, newline and all.
/// - A deletion whose text began its own line takes the spaces and comma left
///   before a closer on that line; on a shared line they are the author's
///   padding.
/// - No trailing whitespace and no doubled space is created.
pub(super) fn tidy(source: &[u8], edit: &Edit, floor: usize, ceil: usize) -> Edit {
    let (mut start, mut end) = (edit.start, edit.end);
    let text = edit.text.as_bytes();
    let deletion = text.is_empty();

    let spaces_before = |at: usize| back_over(source, at, floor, |b| matches!(b, b' ' | b'\t'));
    let spaces_after = |at: usize| ahead_over(source, at, ceil, |b| matches!(b, b' ' | b'\t'));

    if deletion {
        // An emptied container.
        let l = back_over(source, start, floor, |b| b.is_ascii_whitespace());
        let r = ahead_over(source, end, ceil, |b| b.is_ascii_whitespace());
        if l > 0
            && matches!(source[l - 1], b'(' | b'[' | b'{')
            && matches!(source.get(r), Some(b')' | b']' | b'}'))
            && joinable(source, l, start)
        {
            return Edit {
                start: l,
                end: r,
                text: String::new(),
            };
        }

        // A line left empty.
        let line_start = spaces_before(start);
        let line_end = spaces_after(end);
        let opens_line = line_start == 0 || source[line_start - 1] == b'\n';
        if opens_line && source.get(line_end) == Some(&b'\n') && line_end < ceil {
            return Edit {
                start: line_start,
                end: line_end + 1,
                text: String::new(),
            };
        }

        // Closer padding owned by a deleted line.
        let lead = &source[start..end];
        let began_a_line = lead
            .iter()
            .take_while(|b| b.is_ascii_whitespace() || **b == b',')
            .any(|b| *b == b'\n');
        if began_a_line {
            let r = ahead_over(source, end, ceil, |b| matches!(b, b' ' | b'\t' | b','));
            if matches!(source.get(r), Some(b')' | b']' | b'}')) {
                end = r;
            }
        }
    }

    // Trailing whitespace: a deletion leaves only spaces before the line ends.
    let r = spaces_after(end);
    if deletion && matches!(source.get(r), None | Some(b'\n')) && r <= ceil {
        let l = spaces_before(start);
        if l > 0 && source[l - 1] != b'\n' {
            start = l;
        }
        end = r;
    }

    // A doubled space, where the seam is mid-line rather than indentation.
    let mid_line = {
        let l = spaces_before(start);
        l > 0 && source[l - 1] != b'\n'
    };
    if mid_line && start > floor && source[start - 1] == b' ' {
        if deletion && source.get(end) == Some(&b' ') {
            end = spaces_after(end);
        } else if text.first() == Some(&b' ') {
            start = spaces_before(start);
        }
    }
    if !deletion && text.last() == Some(&b' ') && source.get(end) == Some(&b' ') {
        end = spaces_after(end);
    }

    Edit {
        start,
        end,
        text: edit.text.clone(),
    }
}

/// Whether removing `from..to` (whitespace that may span lines) keeps every
/// comment where it was: joining a line onto one ending in `# ...` would put
/// the code inside the comment.
fn joinable(source: &[u8], from: usize, to: usize) -> bool {
    if !source[from..to].contains(&b'\n') {
        return true;
    }
    let line_start = source[..from]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |n| n + 1);
    !source[line_start..from].contains(&b'#')
}

fn back_over(source: &[u8], mut at: usize, floor: usize, skip: impl Fn(u8) -> bool) -> usize {
    while at > floor && skip(source[at - 1]) {
        at -= 1;
    }
    at
}

fn ahead_over(source: &[u8], mut at: usize, ceil: usize, skip: impl Fn(u8) -> bool) -> usize {
    while at < ceil.min(source.len()) && skip(source[at]) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply an edit, spelled as the text it removes marked `[...]` and what it
    /// inserts after `=>`, with and without the tidy.
    fn tidied(marked: &str, insert: &str) -> String {
        let start = marked.find('[').expect("a [ marks the edit");
        let end = marked.find(']').expect("a ] closes it") - 1;
        let source = marked.replacen('[', "", 1).replacen(']', "", 1);
        let edit = Edit {
            start,
            end,
            text: insert.to_string(),
        };
        let t = tidy(source.as_bytes(), &edit, 0, source.len());
        format!("{}{}{}", &source[..t.start], t.text, &source[t.end..])
    }

    #[test]
    fn the_seam_an_edit_leaves_is_tidied() {
        let cases: &[(&str, &str, &str)] = &[
            // Closer padding from a deleted line goes; shared-line padding stays.
            (
                "to_return(body: \"abc\"[,\n          status: 200] )\n",
                "",
                "to_return(body: \"abc\")\n",
            ),
            (
                "to_return(body: \"abc\"[,\n          status: 200], )\n",
                "",
                "to_return(body: \"abc\")\n",
            ),
            ("foo( a: 1[, b: 2] )\n", "", "foo( a: 1 )\n"),
            // An emptied container closes, but never onto a comment.
            ("x.foo(\n  [status: 200]\n)\n", "", "x.foo()\n"),
            (
                "x.foo( # why\n  [status: 200]\n)\n",
                "",
                "x.foo( # why\n)\n",
            ),
            // A line the deletion empties goes, newline and all.
            ("a\n  [b]\nc\n", "", "a\nc\n"),
            // No trailing whitespace, no doubled space -- deleting or inserting.
            ("x = a [+ b]\ny\n", "", "x = a\ny\n"),
            ("f(a, [b,] c)\n", "", "f(a, c)\n"),
            ("x = [y] + 1\n", " z ", "x = z + 1\n"),
            // Indentation is not a doubled space.
            ("  [y] + 1\n", " z", "   z + 1\n"),
            // A plain replacement is left alone.
            ("foo.[bar](1)\n", "baz", "foo.baz(1)\n"),
        ];
        for (marked, insert, want) in cases {
            assert_eq!(tidied(marked, insert), *want, "{marked:?} => {insert:?}");
        }
    }
}
