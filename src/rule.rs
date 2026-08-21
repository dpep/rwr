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
    /// Constraints on the match as a whole.
    #[serde(default)]
    pub scope: Scope,
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

    /// The capture's receiver must resolve to this class.
    ///
    /// The narrowing no other Ruby structural tool offers: `node_pattern` has
    /// no notion of a receiver, ast-grep's FAQ disclaims type analysis, and Ruby
    /// LSP matches methods by bare name. Resolution is conservative -- a
    /// receiver rwr cannot resolve does **not** match, so a `type:` constraint
    /// can only ever narrow. Missed sites surface as residue rather than being
    /// silently rewritten.
    #[serde(default, rename = "type")]
    pub receiver_type: Option<String>,
}

/// Constraints on the match as a whole rather than on one capture.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Scope {
    /// The match must sit lexically inside this class or module.
    ///
    /// Ships in v1 (D19) because the corpus demanded it at entry one: the first
    /// realistic rename needs it to reach implicit-self call sites, which are
    /// 43.5% of all calls in rails.
    #[serde(default)]
    pub inside: Option<String>,
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

/// Resolve the `RULE` argument into one or more rules.
///
/// A rule file may hold a single rule or a list of them. A complete rename
/// genuinely needs several -- the definition and the call sites are different
/// shapes -- so a rule set is the unit of work, not a single pattern.
pub(crate) fn load_all(rule: &str, replace: Option<&str>) -> Result<Vec<Rule>, RuleError> {
    if let Some(template) = replace {
        return Ok(vec![Rule {
            pattern: rule.to_string(),
            rewrite: Some(template.to_string()),
            constraints: HashMap::new(),
            scope: Scope::default(),
        }]);
    }

    if !Path::new(rule).is_file() {
        return Err(RuleError::NoTemplate);
    }
    let raw = std::fs::read_to_string(rule).map_err(|e| RuleError::Unreadable {
        path: rule.to_string(),
        message: e.to_string(),
    })?;

    // A sequence is a rule set; a mapping is one rule. Trying the sequence
    // first keeps the single-rule spelling unchanged.
    if let Ok(rules) = serde_yaml::from_str::<Vec<Rule>>(&raw) {
        return Ok(rules);
    }
    serde_yaml::from_str::<Rule>(&raw)
        .map(|r| vec![r])
        .map_err(|e| RuleError::Malformed {
            path: rule.to_string(),
            message: e.to_string(),
        })
}

/// Resolve the `RULE` argument, which is a file path when one exists and a bare
/// pattern otherwise.
#[allow(dead_code)]
pub(crate) fn load(rule: &str, replace: Option<&str>) -> Result<Rule, RuleError> {
    if let Some(template) = replace {
        return Ok(Rule {
            pattern: rule.to_string(),
            rewrite: Some(template.to_string()),
            constraints: HashMap::new(),
            scope: Scope::default(),
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
    fn a_rule_file_may_hold_a_set() {
        let dir = std::env::temp_dir().join("rwr-rule-set");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("set.yml");
        std::fs::write(
            &path,
            "- match: def display_name; $B; end\n  rewrite: def full_name; $B; end\n- match: $R.display_name\n  rewrite: $R.full_name\n",
        )
        .expect("write");
        let rules = load_all(path.to_str().expect("utf8"), None).expect("rule set");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1].pattern, "$R.display_name");
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
