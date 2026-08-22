//! A rule is a config file that decides what gets rewritten, so a broken one
//! must fail loudly rather than run and do the wrong amount of work.
//!
//! Every case here degraded silently at some point: a misspelled `where:` ran
//! the rule *without its constraint*, turning a narrowed rename into an
//! unnarrowed one; a constraint on a capture the pattern never binds matched
//! nothing; a metavariable the template introduced rendered as empty, turning
//! `log($A, $B)` into `log(a, )`. None of them said a word.

use std::process::{Command, Output};

fn rwr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rwr"))
        .args(args)
        .output()
        .expect("binary runs")
}

/// Run `check` with a rule file holding `yaml`, over one trivial Ruby file.
fn check(yaml: &str) -> (Option<i32>, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.rb"), "log(a, b)\nx = y.display_name\n").expect("write");
    let rule = dir.path().join("r.yml");
    std::fs::write(&rule, yaml).expect("write");

    let out = rwr(&[
        "check",
        rule.to_str().expect("utf8"),
        dir.path().to_str().expect("utf8"),
    ]);
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Exit 3 is "the rule is wrong", and it must be reachable for every way a rule
/// can be wrong -- not just the ones that happen to fail at parse time.
#[test]
fn every_broken_rule_is_refused_with_a_reason() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "a misspelled key",
            "match: $R.display_name\nwher:\n  $R:\n    type: Account\nrewrite: $R.x\n",
            "unknown field",
        ),
        (
            "a constraint on a capture that does not exist",
            "match: $R.display_name\nwhere:\n  $NOPE:\n    name: [a]\nrewrite: $R.x\n",
            "never captures",
        ),
        (
            "same_name_as naming a capture that does not exist",
            "match: $R.display_name\nwhere:\n  $R:\n    same_name_as: $GONE\nrewrite: $R.x\n",
            "never captures",
        ),
        (
            "a template metavariable the pattern never binds",
            "match: log($A, $B)\nrewrite: log($A, $TYPO)\n",
            "never captures",
        ),
        (
            "a version that is not a version",
            "match: $R.display_name\nruby: banana\nrewrite: $R.x\n",
            "not a version",
        ),
        (
            "a contains: pattern YAML cut in half",
            "match: $R\nwhere:\n  $R: { contains: log($A, $B) }\n",
            "Quote it",
        ),
        (
            "an unknown constraint",
            "match: $R.display_name\nwhere:\n  $R:\n    colour: blue\nrewrite: $R.x\n",
            "unknown field",
        ),
        (
            "a pattern that is not Ruby",
            "match: def (((\nrewrite: x\n",
            "not valid Ruby",
        ),
    ];

    for (what, yaml, expected) in cases {
        let (code, err) = check(yaml);
        assert_eq!(
            code,
            Some(3),
            "{what}: expected exit 3, got {code:?}\n{err}"
        );
        assert!(
            err.contains(expected),
            "{what}: message should mention {expected:?}, got:\n{err}"
        );
    }
}

/// A rule that is merely *unusual* is not a broken one. Refusing these would
/// make the checks above worthless, because people would stop writing rules.
#[test]
fn a_valid_rule_is_not_refused() {
    let cases: &[(&str, &str)] = &[
        ("plain", "match: $R.display_name\nrewrite: $R.full_name\n"),
        (
            "a lint, with no rewrite at all",
            "id: x\ndescription: d\nmatch: $R.display_name\n",
        ),
        ("a deletion", "match: $R.display_name\nrewrite: ''\n"),
        (
            "flow style with no YAML metacharacters",
            "match: $R.$M\nwhere:\n  $M: { name: [display_name] }\nrewrite: $R.full_name\n",
        ),
        (
            "a quoted pattern that does hold them",
            "match: $R\nwhere:\n  $R: { contains: \"log($A, $B)\" }\n",
        ),
        (
            "every constraint at once",
            "match: $R.$M($A)\nwhere:\n  $R: { type: Account, kind: instance, subclasses: true }\n  \
             $M: { name: [display_name] }\n  $A: { is: string, length: 1 }\nrewrite: $R.full_name($A)\n",
        ),
    ];

    for (what, yaml) in cases {
        let (code, err) = check(yaml);
        assert_ne!(code, Some(3), "{what} should be accepted:\n{err}");
    }
}

/// The checks apply to *your* rules, not just the shipped ones.
///
/// Nothing in the pipeline distinguishes them: a rule file, a directory of
/// them, and the built-in pack all resolve through one loader and one
/// validation. Worth pinning, because the shipped rules are the ones under test
/// and a check that only guarded those would be worth very little.
#[test]
fn a_rule_directory_of_your_own_is_validated_too() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("a.rb"),
        "x = 1
",
    )
    .expect("write");
    let pack = dir.path().join("my-rules");
    std::fs::create_dir_all(&pack).expect("mkdir");
    std::fs::write(
        pack.join("fine.yml"),
        "match: $R.display_name
rewrite: $R.full_name
",
    )
    .expect("write");
    std::fs::write(
        pack.join("broken.yml"),
        "match: $R.display_name
where:
  $NOPE:
    name: [a]
rewrite: $R.x
",
    )
    .expect("write");

    let out = rwr(&[
        "check",
        pack.to_str().expect("utf8"),
        dir.path().to_str().expect("utf8"),
    ]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("never captures"), "{err}");
    // And it names which of your rules, since a directory may hold many.
    assert!(err.contains("broken"), "names the offending rule: {err}");
}

/// The pack ships compiled into the binary, so a rule broken in the repo is a
/// rule broken for every user. This is the guard that catches it before they do.
#[test]
fn every_shipped_rule_is_valid() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.rb"), "x = 1\n").expect("write");

    let out = rwr(&[
        "check",
        "all",
        dir.path().to_str().expect("utf8"),
        "--unsafe",
        "--ruby",
        "3.4",
    ]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(
        out.status.code(),
        Some(3),
        "a shipped rule does not load:\n{err}"
    );
}

/// What each shipped rule actually does, written down.
///
/// `every_shipped_rule_is_valid` proves they load; this proves they are *right*.
/// The expected outputs here were written from what each rule is supposed to
/// do, then checked -- not captured from what it did, which would only confirm
/// the behaviour rather than test it. A corpus fixture in this project once
/// recorded a bug as its expected output for exactly that reason.
#[test]
fn every_shipped_rule_does_what_it_says() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "style/return-nil",
            "def a
  return nil
end
",
            "def a
  return
end
",
        ),
        (
            "style/hash-shorthand",
            "h = { name: name }
",
            "h = { name: }
",
        ),
        (
            "style/redundant-self-assign",
            "x = x + 1
",
            "x += 1
",
        ),
        (
            "style/sorted-constant-array",
            "PERMS = [:zebra, :apple]
",
            "PERMS = [:apple, :zebra]
",
        ),
        (
            "performance/detect",
            "a = xs.select { |x| x.ok? }.first
",
            "a = xs.detect { |x| x.ok? }
",
        ),
        (
            "performance/count",
            "a = xs.select { |x| x.ok? }.size
",
            "a = xs.count { |x| x.ok? }
",
        ),
        (
            "performance/filter-map",
            "a = xs.map { |x| x.y }.compact
",
            "a = xs.filter_map { |x| x.y }
",
        ),
        (
            "performance/reverse-each",
            "xs.reverse.each { |x| p x }
",
            "xs.reverse_each { |x| p x }
",
        ),
        (
            "performance/sum",
            "a = xs.inject(:+)
",
            "a = xs.sum
",
        ),
        (
            "performance/string-replacement",
            "a = s.gsub(\"-\", \"_\")
",
            "a = s.tr(\"-\", \"_\")
",
        ),
        (
            "performance/exists",
            "a = Model.where(x: 1).count > 0
",
            "a = Model.where(x: 1).exists?
",
        ),
        (
            "performance/find-by",
            "a = Model.where(x: 1).first
",
            "a = Model.find_by(x: 1)
",
        ),
        (
            "performance/pluck",
            "a = Model.all.map(&:name)
",
            "a = Model.all.pluck(:name)
",
        ),
        (
            "performance/relation-count",
            "a = Model.all.to_a.size
",
            "a = Model.all.count
",
        ),
    ];

    for (rule, before, expected) in cases {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("a.rb");
        std::fs::write(&file, before).expect("write");

        let out = rwr(&[
            "rewrite",
            rule,
            dir.path().to_str().expect("utf8"),
            "--unsafe",
            "--ruby",
            "3.4",
        ]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_ne!(out.status.code(), Some(3), "{rule}: {err}");
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            *expected,
            "{rule} did not produce what it promises"
        );
    }
}

/// A rule must not fire on the shape it is *not* about. These are the near
/// misses each one has to leave alone.
#[test]
fn shipped_rules_leave_near_misses_alone() {
    let cases: &[(&str, &str)] = &[
        // `tr` maps character by character, so a multi-character argument is a
        // different operation entirely.
        (
            "performance/string-replacement",
            "a = s.gsub(\"ab\", \"cd\")
",
        ),
        // A regex is not a string literal.
        (
            "performance/string-replacement",
            "a = s.gsub(/x/, \"y\")
",
        ),
        // Array order is meaning unless the array is a constant.
        (
            "style/sorted-constant-array",
            "order = [:zebra, :apple]
",
        ),
        // `{foo: bar}` is not shorthand for anything.
        (
            "style/hash-shorthand",
            "h = { name: other }
",
        ),
        // A different aggregate is a different rule.
        (
            "performance/detect",
            "a = xs.select { |x| x.ok? }.last
",
        ),
    ];

    for (rule, source) in cases {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("a.rb");
        std::fs::write(&file, source).expect("write");

        let out = rwr(&[
            "rewrite",
            rule,
            dir.path().to_str().expect("utf8"),
            "--unsafe",
            "--ruby",
            "3.4",
        ]);
        assert_ne!(out.status.code(), Some(3), "{rule}");
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            *source,
            "{rule} fired on a shape it is not about"
        );
    }
}

/// Selecting one family must not quietly select nothing, which is what a
/// renamed or misfiled rule would look like.
#[test]
fn each_shipped_family_holds_rules() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.rb"), "x = 1\n").expect("write");

    for family in ["style", "performance"] {
        let out = rwr(&[
            "check",
            family,
            dir.path().to_str().expect("utf8"),
            "--unsafe",
            "--ruby",
            "3.4",
        ]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            !err.contains("no such file"),
            "family {family} resolves to nothing:\n{err}"
        );
        assert_ne!(out.status.code(), Some(3), "family {family}:\n{err}");
    }
}
