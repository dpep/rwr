//! Well-formedness of the Phase 0 corpus.
//!
//! The corpus is the gate that decides whether rwr should exist, so a fixture
//! that is silently malformed would invalidate the measurement it is supposed
//! to make. These checks run before any rule does.

use std::fs;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

/// Every `NNN-name/` directory in the corpus.
fn entries() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(corpus_dir())
        .expect("corpus/ exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.as_bytes().first().is_some_and(u8::is_ascii_digit))
        })
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "corpus has no entries");
    dirs
}

fn ruby_files(dir: &Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return vec![];
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .expect("readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rb"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_entry_declares_a_rule_and_metadata() {
    for entry in entries() {
        for required in ["meta.yml", "rule.yml"] {
            assert!(
                entry.join(required).is_file(),
                "{} is missing {required}",
                entry.display()
            );
        }
    }
}

/// The partitions are scored separately and must not be mixed: ast-grep
/// definitionally cannot pass the semantic partition, so counting those wins
/// toward rwr's survival would be scoring a race against an absent runner.
#[test]
fn every_entry_declares_a_valid_family() {
    for entry in entries() {
        let raw = fs::read_to_string(entry.join("meta.yml")).expect("meta.yml readable");
        let meta: serde_yaml::Value = serde_yaml::from_str(&raw).expect("meta.yml parses");
        let family = meta
            .get("family")
            .and_then(|f| f.as_str())
            .unwrap_or_else(|| panic!("{} declares no family", entry.display()));
        assert!(
            matches!(family, "syntactic" | "semantic"),
            "{} has unknown family {family:?}",
            entry.display()
        );
    }
}

/// A malformed fixture would make the gate measure the wrong thing, and an
/// expected-output file that does not parse is a bug in the expectation rather
/// than in the tool.
#[test]
fn every_fixture_is_valid_ruby() {
    for entry in entries() {
        for dir in ["in", "out"] {
            for file in ruby_files(&entry.join(dir)) {
                let src = fs::read(&file).expect("fixture readable");
                let result = ruby_prism::parse(&src);
                assert_eq!(
                    result.errors().count(),
                    0,
                    "{} is not valid Ruby",
                    file.display()
                );
            }
        }
    }
}

/// `in/refuses-*.rb` asserts a refusal and therefore has no expected output;
/// everything else must have one. Refusing correctly is behaviour worth
/// pinning, not an absence of behaviour.
#[test]
fn inputs_pair_with_outputs_unless_they_assert_a_refusal() {
    for entry in entries() {
        for input in ruby_files(&entry.join("in")) {
            let name = input.file_name().expect("named");
            let expected = entry.join("out").join(name);
            let is_refusal = name.to_string_lossy().starts_with("refuses-");

            if is_refusal {
                assert!(
                    !expected.exists(),
                    "{} asserts a refusal but also declares an expected output",
                    input.display()
                );
            } else {
                assert!(
                    expected.is_file(),
                    "{} has no expected output at {}",
                    input.display(),
                    expected.display()
                );
            }
        }
    }
}

/// An expected output identical to its input records "this rule changes
/// nothing here", which is almost always a fixture written by mistake.
#[test]
fn expected_outputs_differ_from_their_inputs() {
    for entry in entries() {
        for input in ruby_files(&entry.join("in")) {
            let expected = entry.join("out").join(input.file_name().expect("named"));
            if !expected.is_file() {
                continue;
            }
            assert_ne!(
                fs::read(&input).expect("readable"),
                fs::read(&expected).expect("readable"),
                "{} is unchanged by its rule",
                input.display()
            );
        }
    }
}
