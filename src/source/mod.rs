//! Source discovery and parsing.
//!
//! Walks the repo honoring `.gitignore`, skipping generated and vendored code
//! by default — a rewrite that "succeeds" by editing `db/schema.rb` is a bug.
//! Parses with Prism (decision D1) in parallel.
//!
//! Phase 1 holds no persistent state: parse, answer, exit (decision D5 as
//! amended). Whether that is fast enough at 1M LOC is Phase 0 measurement (d).

#[cfg(test)]
mod tests {
    /// Decision D1 rests on Prism *reporting* what it could not parse, rather
    /// than recovering silently into a plausible-but-wrong tree. Pin both
    /// halves: clean source parses with no diagnostics, broken source yields
    /// diagnostics instead of a confident answer.
    #[test]
    fn prism_reports_parse_errors_rather_than_guessing() {
        let ok = ruby_prism::parse(b"foo(a, b)");
        assert_eq!(ok.errors().count(), 0);

        let broken = ruby_prism::parse(b"def foo(");
        assert!(broken.errors().count() > 0);
    }

    /// The heredoc hazard behind decision D14, pinned as an executable fact:
    /// the string node's own location stops at its opening token, nowhere near
    /// the body three lines down. Splicing from raw node locations would
    /// silently detach that body — and the result still parses, so nothing
    /// downstream catches it. Hence `effective_range()`.
    #[test]
    fn heredoc_location_excludes_its_body() {
        let src: &[u8] = b"foo(<<~SQL, b)\n  SELECT 1\nSQL\n";
        let result = ruby_prism::parse(src);
        assert_eq!(result.errors().count(), 0);

        let body_offset = src
            .windows(6)
            .position(|w| w == b"SELECT")
            .expect("fixture contains the heredoc body");

        let node = result.node();
        let program = node.as_program_node().expect("root is a program");
        let statements = program.statements();
        let first = statements.body().iter().next().expect("one statement");
        let call = first.as_call_node().expect("statement is a call");
        let args = call.arguments().expect("call has arguments");
        let heredoc = args.arguments().iter().next().expect("first argument");

        assert!(
            heredoc.location().end_offset() < body_offset,
            "heredoc node location unexpectedly reached its body"
        );
    }

    /// Refutes an earlier design claim that concrete-syntax transformations
    /// (`and` -> `&&`, hash rockets) are impossible because the trees are
    /// identical. The *node types* match, but Prism retains operator locations,
    /// so the spelling is recoverable — which puts a family the design had
    /// written off back within reach (see Q12).
    #[test]
    fn operator_spelling_survives_parsing() {
        for (src, expected) in [("a and b", "and"), ("a && b", "&&")] {
            let result = ruby_prism::parse(src.as_bytes());
            let node = result.node();
            let program = node.as_program_node().expect("program");
            let stmt = program
                .statements()
                .body()
                .iter()
                .next()
                .expect("one statement");
            let and = stmt.as_and_node().expect("an `and` node");
            let loc = and.operator_loc();
            assert_eq!(&src[loc.start_offset()..loc.end_offset()], expected);
        }
    }

    /// Same for hash syntax: the shorthand carries no operator location at all,
    /// the rocket carries one. A `where:` predicate can therefore distinguish
    /// them without rwr needing to read raw source text.
    #[test]
    fn hash_rocket_is_distinguishable_from_shorthand() {
        let spellings = ["{ a: 1 }", "{ :a => 1 }"];
        let found: Vec<bool> = spellings
            .iter()
            .map(|src| {
                let result = ruby_prism::parse(src.as_bytes());
                let node = result.node();
                let program = node.as_program_node().expect("program");
                let stmt = program
                    .statements()
                    .body()
                    .iter()
                    .next()
                    .expect("one statement");
                let hash = stmt.as_hash_node().expect("a hash");
                let first = hash.elements().iter().next().expect("one element");
                first
                    .as_assoc_node()
                    .expect("an assoc")
                    .operator_loc()
                    .is_some()
            })
            .collect();

        assert_eq!(
            found,
            vec![false, true],
            "hash spellings were indistinguishable"
        );
    }
}
