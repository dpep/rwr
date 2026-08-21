//! Loading a rule: either a YAML file or a bare pattern plus `-r`.
//!
//! The one-liner form reaches sequences but not `where:` constraints, which is
//! the intended progressive disclosure (D30): simple cases stay terse, and a
//! rule file arrives exactly when precision is needed.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub(crate) struct Rule {
    /// The structural pattern, in Ruby with `$METAVAR` placeholders.
    #[serde(rename = "match")]
    pub pattern: String,
    /// The replacement template. Absent for a find-only rule.
    #[serde(default)]
    pub rewrite: Option<String>,
    /// Constraints source syntax cannot express, keyed by metavariable.
    #[serde(default, rename = "where")]
    pub constraints: HashMap<String, Constraint>,
}

/// What a capture must satisfy beyond matching structurally.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Constraint {
    /// The capture must be one of these identifiers.
    ///
    /// Ranked first in the backlog because it unblocks `Performance/Detect` and
    /// `Performance/Count` at once: `select` and `find_all` are synonyms and a
    /// rule has to match both. ast-grep needs a separate pass per name.
    #[serde(default)]
    pub name: Option<Vec<String>>,
}

#[derive(Debug)]
pub(crate) enum RuleError {
    Unreadable {
        path: String,
        message: String,
    },
    Malformed {
        path: String,
        message: String,
    },
    /// A bare pattern was given with no replacement, for a command that needs one.
    NoTemplate,
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleError::Unreadable { path, message } => write!(f, "cannot read {path}: {message}"),
            RuleError::Malformed { path, message } => write!(f, "{path} is not a rule: {message}"),
            RuleError::NoTemplate => write!(
                f,
                "a replacement is required — pass -r/--replace, or give a rule file with a `rewrite:` key"
            ),
        }
    }
}

/// Resolve the `RULE` argument, which is a file path when one exists and a bare
/// pattern otherwise.
pub(crate) fn load(rule: &str, replace: Option<&str>) -> Result<Rule, RuleError> {
    if let Some(template) = replace {
        return Ok(Rule {
            pattern: rule.to_string(),
            rewrite: Some(template.to_string()),
            constraints: HashMap::new(),
        });
    }

    if !Path::new(rule).is_file() {
        return Err(RuleError::NoTemplate);
    }

    let raw = std::fs::read_to_string(rule).map_err(|e| RuleError::Unreadable {
        path: rule.to_string(),
        message: e.to_string(),
    })?;
    serde_yaml::from_str(&raw).map_err(|e| RuleError::Malformed {
        path: rule.to_string(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_replacement_makes_a_rule() {
        let rule = load("foo($A)", Some("bar($A)")).expect("inline rule");
        assert_eq!(rule.pattern, "foo($A)");
        assert_eq!(rule.rewrite.as_deref(), Some("bar($A)"));
    }

    #[test]
    fn a_bare_pattern_without_a_template_is_an_error() {
        assert!(matches!(load("foo($A)", None), Err(RuleError::NoTemplate)));
    }

    #[test]
    fn constraints_are_parsed() {
        let dir = std::env::temp_dir().join("rwr-rule-where");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("w.yml");
        std::fs::write(
            &path,
            "match: $R.$SEL { |$P| $B }.first\nwhere:\n  $SEL:\n    name: [select, find_all]\nrewrite: $R.detect { |$P| $B }\n",
        )
        .expect("write");
        let rule = load(path.to_str().expect("utf8"), None).expect("rule");
        let names = rule.constraints["$SEL"].name.as_ref().expect("names");
        assert_eq!(names, &["select", "find_all"]);
    }

    #[test]
    fn a_rule_file_is_parsed() {
        let dir = std::env::temp_dir().join("rwr-rule-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("r.yml");
        std::fs::write(&path, "match: return nil\nrewrite: return\n").expect("write");
        let rule = load(path.to_str().expect("utf8"), None).expect("file rule");
        assert_eq!(rule.pattern, "return nil");
        assert_eq!(rule.rewrite.as_deref(), Some("return"));
    }
}
