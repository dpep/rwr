//! ERB templates, translated to Ruby with a map back.
//!
//! rwr could count templates and search them for text, and that was all. This
//! reads them: the tag bodies are stitched into one Ruby program, matched
//! structurally like any other source, and each edit is mapped back to the
//! template it came from.
//!
//! **Stitching is what makes it work.** A tag on its own often does not parse --
//! `<% @posts.each do |p| %>` opens a block that `<% end %>` closes three tags
//! later, and 40% of tags in a real corpus fail alone. Joined in document order
//! they are one program, and 95% of templates parse that way (159 of 168 across
//! discourse and mastodon).
//!
//! A template that does not parse is reported and left alone, exactly as an
//! unparseable `.rb` file is.

/// One tag body, and where it came from.
#[derive(Debug, Clone, Copy)]
struct Fragment {
    /// Byte offset of the body within the generated Ruby.
    ruby: usize,
    /// Byte offset of the body within the template.
    template: usize,
    len: usize,
}

/// A template as Ruby, with the map needed to write back to it.
#[derive(Debug)]
pub(crate) struct Translated {
    pub ruby: Vec<u8>,
    fragments: Vec<Fragment>,
}

impl Translated {
    /// Where a Ruby byte range sits in the template.
    ///
    /// `None` when the range crosses a fragment boundary: those bytes include
    /// template text that is not Ruby at all, and rewriting through them would
    /// splice HTML into a Ruby expression. Refusing is the only safe answer,
    /// and it is rare -- an expression rarely spans two tags.
    pub(crate) fn to_template(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        let holding = |at: usize| {
            self.fragments
                .iter()
                .find(|f| at >= f.ruby && at <= f.ruby + f.len)
        };
        let first = holding(start)?;
        let last = holding(end.max(start))?;
        if first.ruby != last.ruby {
            return None;
        }
        Some((
            first.template + (start - first.ruby),
            first.template + (end - first.ruby),
        ))
    }
}

/// Pull the Ruby out of an ERB template.
///
/// Returns `None` when the template holds no Ruby at all, so a page of static
/// HTML costs nothing beyond the scan.
pub(crate) fn translate(template: &[u8]) -> Option<Translated> {
    let mut ruby: Vec<u8> = Vec::new();
    let mut fragments = Vec::new();
    let mut at = 0;

    while let Some(open) = find(template, at, b"<%") {
        let after = open + 2;
        // `<%%` is an escaped literal `<%`, not a tag.
        if template.get(after) == Some(&b'%') {
            at = after + 1;
            continue;
        }
        let Some(close) = find(template, after, b"%>") else {
            break;
        };
        // A comment tag holds prose, not Ruby.
        let comment = template.get(after) == Some(&b'#');
        // `<%=`, `<%==` and `<%-` all introduce Ruby; the sigil is not part of it.
        let mut body = after;
        while matches!(template.get(body), Some(b'=' | b'-' | b'#')) {
            body += 1;
        }
        // `-%>` trims trailing whitespace; the dash is not Ruby either.
        let mut end = close;
        if end > body && template[end - 1] == b'-' {
            end -= 1;
        }

        if !comment && end > body {
            let text = &template[body..end];
            let trimmed_start = text.len() - text.trim_ascii_start().len();
            let trimmed = text.trim_ascii();
            if !trimmed.is_empty() {
                fragments.push(Fragment {
                    ruby: ruby.len(),
                    template: body + trimmed_start,
                    len: trimmed.len(),
                });
                ruby.extend_from_slice(trimmed);
                ruby.push(b'\n');
            }
        }
        at = close + 2;
    }

    (!fragments.is_empty()).then_some(Translated { ruby, fragments })
}

/// Apply Ruby-coordinate edits to the template they came from.
///
/// Every edit must sit inside one fragment; one that spans two covers template
/// text that is not Ruby, and splicing through it would put HTML inside an
/// expression. Refusing is the only safe answer.
pub(crate) fn splice(
    translated: &Translated,
    template: &[u8],
    edits: &[crate::rewrite::Edit],
) -> Option<Vec<u8>> {
    let mut mapped: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for edit in edits {
        let (start, end) = translated.to_template(edit.start, edit.end)?;
        mapped.push((start, end, &edit.text));
    }
    mapped.sort_by_key(|(start, _, _)| *start);

    let mut out = Vec::with_capacity(template.len());
    let mut cursor = 0;
    for (start, end, text) in mapped {
        if start < cursor {
            return None;
        }
        out.extend_from_slice(template.get(cursor..start)?);
        out.extend_from_slice(text.as_bytes());
        cursor = end;
    }
    out.extend_from_slice(template.get(cursor..)?);
    Some(out)
}

/// Where a Ruby offset sits in the template, for reporting a line number.
pub(crate) fn template_offset(translated: &Translated, ruby: usize) -> Option<usize> {
    translated.to_template(ruby, ruby).map(|(start, _)| start)
}

fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(haystack.get(from..)?, needle).map(|i| i + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_body_maps_back_to_the_template() {
        let template = b"<h1><%= account.display_name %></h1>\n";
        let t = translate(template).expect("has ruby");
        assert_eq!(t.ruby, b"account.display_name\n");

        // `display_name` starts at 8 in the Ruby, and the same text in the
        // template is at 8 + the `<h1><%= ` prefix.
        let (start, end) = t.to_template(8, 20).expect("maps");
        assert_eq!(&template[start..end], b"display_name");
    }

    /// The case that makes stitching necessary: neither tag parses alone.
    #[test]
    fn control_flow_across_tags_becomes_one_program() {
        let t = translate(b"<% posts.each do |p| %>\n<%= p.title %>\n<% end %>").expect("ruby");
        assert_eq!(t.ruby, b"posts.each do |p|\np.title\nend\n");
        assert_eq!(ruby_prism::parse(&t.ruby).errors().count(), 0);
    }

    /// A range spanning two tags covers template text that is not Ruby, so
    /// there is nothing to rewrite through.
    #[test]
    fn a_range_crossing_two_fragments_refuses() {
        let t = translate(b"<%= a %> and <%= b %>").expect("ruby");
        assert_eq!(t.ruby, b"a\nb\n");
        assert!(t.to_template(0, 1).is_some());
        assert!(t.to_template(0, 3).is_none());
    }

    /// The whole point: an edit computed against the Ruby lands in the template.
    #[test]
    fn an_edit_lands_in_the_template() {
        let template = b"<h1><%= account.display_name %></h1>\n".to_vec();
        let t = translate(&template).expect("ruby");
        let edits = vec![crate::rewrite::Edit {
            start: 8,
            end: 20,
            text: "full_name".to_string(),
        }];
        let out = splice(&t, &template, &edits).expect("splices");
        assert_eq!(out, b"<h1><%= account.full_name %></h1>\n");
    }

    #[test]
    fn sigils_and_trims_are_not_ruby() {
        assert_eq!(translate(b"<%= a %>").expect("ruby").ruby, b"a\n");
        assert_eq!(translate(b"<%- a -%>").expect("ruby").ruby, b"a\n");
        assert_eq!(translate(b"<%== a %>").expect("ruby").ruby, b"a\n");
        // A comment holds prose, and `<%%` is an escaped literal.
        assert!(translate(b"<%# just a note %>").is_none());
        assert!(translate(b"<h1>plain</h1>").is_none());
    }
}
