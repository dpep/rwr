//! Accepting a finding: `# rwr:ignore <rule-id>`.
//!
//! One concept with (eventually) two authoring surfaces -- a directive at the
//! site, and a baseline file for bulk -- sharing one rule: **a suppression whose
//! finding is gone is itself a finding.** A mechanism that can silence a run
//! must never be able to silence itself, which is what turned RuboCop's todo
//! file into a permanent monument.
//!
//! Reading a directive is not matching or rewriting a comment, so D67 stands:
//! directives are a third category of comment -- instructions addressed to rwr
//! rather than prose about the code. They suppress findings and edits, never
//! residue. The account of blind spots is the product.

use ruby_prism::Node;

/// What a `# rwr:ignore` comment says.
#[derive(Debug, Clone)]
pub(crate) struct Directive {
    /// Byte range this covers: the outermost node starting on the attached
    /// line. A line-scoped directive would be the wrong unit for a structural
    /// tool -- `# rwr:ignore` above a `def` means the method, and rwr is the one
    /// tool that can say so, because it has the tree.
    pub(crate) covers: (usize, usize),
    /// Where the comment itself sits, for reporting it stale.
    pub(crate) line: usize,
    /// Rule ids named. Never empty -- a bare directive is malformed, because a
    /// blanket suppression cannot be checked for staleness and so is
    /// unaccountable.
    pub(crate) rules: Vec<String>,
    /// Document order, which is stable across the rewrites of a run where a
    /// line number is not.
    pub(crate) index: usize,
}

/// A finding a directive accepted.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Suppressed {
    pub(crate) file: String,
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rule: Option<String>,
    /// Where the acceptance was written: `directive` today, `baseline` later.
    pub(crate) source: &'static str,
}

/// A suppression that no longer suppresses anything.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Stale {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) rule: String,
    pub(crate) source: &'static str,
}

/// A directive naming no rules, which cannot be checked for staleness.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Malformed {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) why: &'static str,
}

const MARKER: &str = "rwr:ignore";

/// Read every directive in a source.
///
/// A directive attaches to the line it sits on when there is code before it, and
/// otherwise to the next line carrying code -- comment lines in between are
/// skipped so a directive stacked above a doc block still reaches its subject,
/// but a blank line ends the search, since attachment does not cross one (D35).
///
/// It then covers the **outermost node starting on that line**, so a directive
/// above a `def` covers the method rather than only its first line. Bounded by
/// the syntax, which is why this carries none of the hazard of a `disable`/
/// `enable` block: there is nothing to forget to terminate.
pub(crate) fn directives(
    parsed: &ruby_prism::ParseResult<'_>,
    source: &[u8],
) -> (Vec<Directive>, Vec<(usize, &'static str)>) {
    let mut found = Vec::new();
    let mut malformed = Vec::new();
    let lines: Vec<&[u8]> = source.split(|b| *b == b'\n').collect();
    let widest = widest_by_line(parsed, source);

    for comment in parsed.comments() {
        let location = comment.location();
        let text = String::from_utf8_lossy(
            &source[location.start_offset()..location.end_offset().min(source.len())],
        )
        .into_owned();
        let Some(rest) = text.split_once(MARKER).map(|(_, r)| r) else {
            continue;
        };
        let line = crate::source::line_col(source, location.start_offset()).0;

        // A reason is the natural thing to write next to a suppression, and
        // taking it as part of a rule name meant the directive silently named a
        // rule nothing has -- neither honoured nor reported stale, because an
        // unknown id is assumed to belong to another pack.
        let named = rest.split(" -- ").next().unwrap_or(rest);
        let rules: Vec<String> = named
            .split([',', ' ', '\t'])
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .collect();
        if rules.is_empty() {
            // A blanket ignore is a blind spot nothing can audit, so it is an
            // error rather than a very effective directive.
            malformed.push((line, "names no rule; `# rwr:ignore <rule-id>`"));
            continue;
        }

        // Trailing when code precedes it on the line, leading otherwise.
        let before = lines
            .get(line - 1)
            .map(|l| &l[..(location.start_offset() - line_start(source, location.start_offset()))])
            .unwrap_or(&[]);
        let covers = if before.iter().any(|b| !b.is_ascii_whitespace()) {
            line
        } else {
            let mut probe = line;
            loop {
                probe += 1;
                match lines.get(probe - 1) {
                    None => break line,
                    Some(text) => {
                        let trimmed: Vec<u8> = text
                            .iter()
                            .copied()
                            .skip_while(u8::is_ascii_whitespace)
                            .collect();
                        // Blank ends the search: attachment does not cross one.
                        if trimmed.is_empty() {
                            break line;
                        }
                        if !trimmed.starts_with(b"#") {
                            break probe;
                        }
                    }
                }
            }
        };

        found.push(Directive {
            // No node starting there means the directive covers nothing, which
            // is exactly what a stale report should say.
            covers: widest.get(&covers).copied().unwrap_or((0, 0)),
            line,
            rules,
            index: found.len(),
        });
    }
    (found, malformed)
}

/// The widest *statement* starting on each line.
///
/// Statements, not nodes: the widest node starting on a line is the program
/// itself whenever a directive sits above the first statement in a file, since
/// a comment is not in the tree and the root therefore begins at the same line.
/// That made one directive cover the whole file. A unit is a child of a
/// `StatementsNode` -- which is exactly "a thing you could put a comment above".
fn widest_by_line(
    parsed: &ruby_prism::ParseResult<'_>,
    source: &[u8],
) -> std::collections::HashMap<usize, (usize, usize)> {
    let mut out: std::collections::HashMap<usize, (usize, usize)> =
        std::collections::HashMap::new();
    let mut stack = vec![crate::pattern::generated::dup(&parsed.node())];
    while let Some(node) = stack.pop() {
        if matches!(node, Node::StatementsNode { .. }) {
            for statement in crate::pattern::generated::children(&node) {
                let location = statement.location();
                let (start, end) = (location.start_offset(), location.end_offset());
                let line = crate::source::line_col(source, start).0;
                out.entry(line)
                    .and_modify(|span| {
                        if end - start > span.1 - span.0 {
                            *span = (start, end);
                        }
                    })
                    .or_insert((start, end));
            }
        }
        stack.extend(crate::pattern::generated::children(&node));
    }
    out
}

fn line_start(source: &[u8], offset: usize) -> usize {
    source[..offset]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |n| n + 1)
}

impl Directive {
    /// Whether this directive accepts findings of `rule` at `offset`.
    pub(crate) fn covers(&self, rule: Option<&str>, offset: usize) -> bool {
        if offset < self.covers.0 || offset >= self.covers.1 {
            return false;
        }
        // A bare-pattern run has no id, and suppression is for standing
        // enforcement -- an ad-hoc query is exploration by someone who typed the
        // pattern seconds ago.
        rule.is_some_and(|id| self.rules.iter().any(|r| r == id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(src: &str) -> (Vec<Directive>, Vec<(usize, &'static str)>) {
        let parsed = ruby_prism::parse(src.as_bytes());
        directives(&parsed, src.as_bytes())
    }

    /// Offset of `needle` in `src`, for asserting what a directive reaches.
    fn at(src: &str, needle: &str) -> usize {
        src.find(needle).expect("needle")
    }

    #[test]
    fn a_trailing_directive_covers_its_own_line() {
        let src = "sleep 1  # rwr:ignore style/no-sleep\n";
        let (found, _) = read(src);
        assert!(found[0].covers(Some("style/no-sleep"), at(src, "sleep 1")));
        assert_eq!(found[0].rules, vec!["style/no-sleep"]);
    }

    #[test]
    fn a_leading_directive_covers_the_next_code_line() {
        let src = "# rwr:ignore style/no-sleep\nsleep 1\n";
        let (found, _) = read(src);
        assert!(found[0].covers(Some("style/no-sleep"), at(src, "sleep 1")));
    }

    /// The unit is the node, not the line: above a `def`, it covers the method.
    /// A line-scoped directive would sit above the definition and cover nothing
    /// but its signature, which is never what anyone means.
    #[test]
    fn a_leading_directive_covers_the_whole_definition() {
        let src = "# rwr:ignore a/b\ndef three\n  return nil\nend\n";
        let (found, _) = read(src);
        assert!(found[0].covers(Some("a/b"), at(src, "return nil")));
        // And stops at its end.
        assert!(!found[0].covers(Some("a/b"), src.len()));
    }

    /// A directive above the *first* statement in a file must not take the
    /// file. The widest node starting on that line is the program itself --
    /// a comment is not in the tree, so the root begins on the same line -- and
    /// scoping by node rather than by statement silently swallowed everything
    /// below.
    #[test]
    fn a_directive_stops_at_the_end_of_its_statement() {
        let src =
            "# rwr:ignore a/b\ndef covered\n  return nil\nend\n\ndef after\n  return nil\nend\n";
        let (found, _) = read(src);
        let inside = at(src, "return nil");
        let outside = src.rfind("return nil").expect("second");
        assert!(found[0].covers(Some("a/b"), inside));
        assert!(
            !found[0].covers(Some("a/b"), outside),
            "must not reach the next method"
        );
    }

    /// Everything nested inside the covered statement is covered, including a
    /// block several levels down -- that is what "the whole method" means.
    #[test]
    fn a_directive_reaches_nested_statements() {
        let src = "# rwr:ignore a/b\ndef covered\n  [1].each do\n    return nil\n  end\nend\n";
        let (found, _) = read(src);
        assert!(found[0].covers(Some("a/b"), at(src, "return nil")));
    }

    /// Stacked above a doc comment, it still reaches the code.
    #[test]
    fn a_leading_directive_skips_intervening_comments() {
        let src = "# rwr:ignore a/b\n# Why this waits.\nsleep 1\n";
        let (found, _) = read(src);
        assert!(found[0].covers(Some("a/b"), at(src, "sleep 1")));
    }

    /// Attachment does not cross a blank line, so a stray directive does not
    /// silently reach whatever happens to follow it.
    #[test]
    fn a_blank_line_ends_the_search() {
        let src = "# rwr:ignore a/b\n\nsleep 1\n";
        let (found, _) = read(src);
        assert!(!found[0].covers(Some("a/b"), at(src, "sleep 1")));
    }

    #[test]
    fn several_rules_on_one_directive() {
        let (found, _) = read("sleep 1 # rwr:ignore a/b, c/d\n");
        assert_eq!(found[0].rules, vec!["a/b", "c/d"]);
    }

    /// A reason after `--` is prose, not a rule name. Writing one is the
    /// natural thing to do, and swallowing it made the directive name a rule
    /// nothing has -- which is neither honoured nor reported, since an unknown
    /// id is assumed to belong to another pack.
    #[test]
    fn a_reason_after_a_dash_is_not_a_rule_name() {
        let src = "sleep 1 # rwr:ignore a/b -- flaky in CI, see PIE-4\n";
        let (found, _) = read(src);
        assert_eq!(found[0].rules, vec!["a/b"]);
    }

    /// Space-separated names work too, since that is how people write lists.
    #[test]
    fn names_may_be_separated_by_spaces_or_commas() {
        let src = "sleep 1 # rwr:ignore a/b c/d, e/f\n";
        let (found, _) = read(src);
        assert_eq!(found[0].rules, vec!["a/b", "c/d", "e/f"]);
    }

    /// A blanket ignore cannot be checked for staleness, so it is an error
    /// rather than a very effective directive.
    #[test]
    fn a_bare_directive_is_malformed() {
        let (found, bad) = read("sleep 1 # rwr:ignore\n");
        assert!(found.is_empty());
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn a_directive_only_covers_the_rules_it_names() {
        let src = "sleep 1 # rwr:ignore a/b\n";
        let (found, _) = read(src);
        let here = at(src, "sleep 1");
        assert!(found[0].covers(Some("a/b"), here));
        assert!(!found[0].covers(Some("c/d"), here));
        // A bare pattern has no id to name, and suppression is for standing
        // enforcement rather than an ad-hoc query.
        assert!(!found[0].covers(None, here));
    }
}
