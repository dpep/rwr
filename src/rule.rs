//! Loading a rule: either a YAML file or a bare pattern plus `-r`.
//!
//! The one-liner form reaches sequences but not `where:` constraints, which is
//! the intended progressive disclosure (D30): simple cases stay terse, and a
//! rule file arrives exactly when precision is needed.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rule {
    /// What to call this rule when reporting which one fired.
    ///
    /// Absent for the inline `-r` form, which has no name to report. A rule
    /// loaded from a pack takes its path within the pack when it declares
    /// nothing, so `rules/performance/detect.yml` reports as
    /// `performance/detect`.
    #[serde(default)]
    pub id: Option<String>,
    /// One line on what the rule does, for humans reading the pack.
    #[serde(default)]
    #[allow(dead_code)]
    pub description: Option<String>,
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

    /// The lowest Ruby version this rule's *output* parses on.
    ///
    /// `{foo:}` is a syntax error before 3.1 and `filter_map` does not exist
    /// before 2.7, and rwr's own `verify` cannot catch either -- Prism parses
    /// modern Ruby, so the output is valid there. The check has to come from the
    /// codebase's declared version instead (Q6).
    #[serde(default)]
    pub ruby: Option<String>,

    /// Why this rule can change behaviour, when it can.
    ///
    /// Present means unsafe, and the value is the reason -- there is no boolean
    /// to set without saying what for. Ruby is dynamically typed, so most
    /// interesting rewrites have an input that breaks them: `inject(:+)` returns
    /// nil on an empty collection where `sum` returns 0, and `select` on an
    /// ActiveRecord relation names columns rather than filtering rows.
    ///
    /// RuboCop carries the same information as `SafeAutoCorrect: false`, in a
    /// config file nobody reads at the moment of the edit. Here it is a
    /// sentence, printed when the rule fires.
    #[serde(default, rename = "unsafe")]
    pub unsafe_because: Option<String>,

    /// Fixtures pinning what this rule does, run by `rwr test`.
    ///
    /// Not an option (D57): a fixture parameterizes nothing and cannot change
    /// what the rule does to any file. It is a falsifiable claim *about* the
    /// declared behaviour, living beside the declaration for the same reason
    /// `unsafe:`'s reason does -- the information is worthless anywhere else.
    #[serde(default)]
    pub tests: Vec<Case>,
}

/// One fixture: a snippet, and what the rule set should do to it.
///
/// A case that asserts nothing is refused at load rather than passing vacuously,
/// which is the failure a fixture suite exists to prevent.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
    /// The snippet, evaluated as a whole Ruby file.
    ///
    /// A snippet *is* a file: a rule constrained by `scope: inside:` or a
    /// `type:` receiver writes the class or the assignment it needs into its own
    /// input, rather than being handed a synthetic wrapper nobody asked for.
    pub input: String,
    /// The expected source afterwards, compared byte for byte.
    ///
    /// No normalization, the trailing newline included: this is the
    /// identity-rewrite philosophy applied to fixtures, and a forgiving
    /// comparison would hide what the tool actually writes.
    #[serde(default)]
    pub output: Option<String>,
    /// The snippet must come back byte-identical.
    #[serde(default)]
    pub unchanged: Option<bool>,
    /// How many findings a rule proposing no edit should report.
    #[serde(default)]
    pub finds: Option<usize>,
    /// How many occurrences the rule should be unable to account for.
    ///
    /// A rename's report is the product, not a diagnostic -- and a fixture could
    /// pin what the rule *rewrote* while saying nothing about what it *reported*,
    /// which is the half that decides whether the change is safe to ship. Only
    /// meaningful for a rule set that moves a name; asserting it on one that
    /// does not is refused, the same way `finds:` is on a rewriting set.
    #[serde(default)]
    pub residue: Option<usize>,
}

impl Case {
    /// What this case claims, or why it claims nothing.
    ///
    /// `rewrites` and `reports` describe the rule *set*, since a case runs the
    /// whole document (D54 makes the file the unit of identity, and a
    /// `method:`/`rename:` pair expands to several rules that only mean
    /// anything together).
    fn check(&self, rewrites: bool, reports: bool) -> Result<(), String> {
        if self.output.is_some() && self.unchanged.is_some() {
            return Err("a case cannot claim both `output:` and `unchanged:`".into());
        }
        if self.unchanged == Some(false) {
            return Err("`unchanged: false` asserts nothing -- say what the output is".into());
        }
        if self.output.is_none()
            && self.unchanged.is_none()
            && self.finds.is_none()
            && self.residue.is_none()
        {
            return Err(
                "this case asserts nothing -- add `output:`, `unchanged: true`, `finds:` or \
                 `residue:`"
                    .into(),
            );
        }
        if self.finds.is_some() && !reports {
            return Err("`finds:` needs a rule that proposes no edit; this set rewrites".into());
        }
        if (self.output.is_some() || self.unchanged.is_some()) && !rewrites {
            return Err(
                "`output:`/`unchanged:` need a rule with `rewrite:`; this set only reports".into(),
            );
        }
        Ok(())
    }
}

/// Every fixture in a rule set, checked against what the set can actually do.
///
/// Returns the cases in document order. An empty result means the set declares
/// no fixtures at all, which `rwr test` reports rather than calling a green
/// nothing.
pub(crate) fn cases(rules: &[Rule]) -> Result<Vec<Case>, String> {
    let rewrites = rules.iter().any(|r| r.rewrite.is_some());
    let reports = rules.iter().any(|r| r.rewrite.is_none());
    let mut found = Vec::new();
    for case in rules.iter().flat_map(|r| &r.tests) {
        case.check(rewrites, reports)?;
        found.push(case.clone());
    }
    Ok(found)
}

/// What a capture must satisfy beyond matching structurally.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Constraint {
    /// The capture must be one of these identifiers.
    ///
    /// Ranked first in the backlog because it unblocks `Performance/Detect` and
    /// `Performance/Count` at once: `select` and `find_all` are synonyms and a
    /// rule has to match both. ast-grep needs a separate pass per name.
    #[serde(default)]
    pub name: Option<Vec<String>>,

    /// The capture must be none of these identifiers.
    ///
    /// A predicate, not an option (D57): it narrows what the pattern *means*,
    /// in the rule file, indistinguishable in kind from `name:`. The ignore-list
    /// alternative -- a flag or a sidecar naming exceptions -- would configure a
    /// rule from outside it, which is the thing D57 refuses.
    ///
    /// Asymmetric with `name:` on purpose: `name:` needs an identifier and fails
    /// without one, while this *passes* when the capture has none. Nothing that
    /// is not an identifier can be one of the excluded ones, and a constraint
    /// that widened on missing data would be a guess.
    #[serde(default)]
    pub name_not: Option<Vec<String>>,

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

    /// The capture's receiver must resolve, and to none of these classes.
    ///
    /// **Not the mirror of `name_not:`, on purpose.** `name_not:` passes when
    /// the capture has no identifier, because nothing that is not an identifier
    /// can be one of the excluded ones. A type exclusion cannot pass on missing
    /// data the same way: `type:` under-matches when it cannot resolve, which is
    /// the safe direction, and a negation that inherited that would *widen* --
    /// every unresolved receiver would sail through an exclusion meant to hold
    /// it back. So this requires resolution and then excludes, and narrowing
    /// still only ever narrows.
    ///
    /// Descent is always honoured, with no flag to set: "not an
    /// `ActiveRecord::Base`" plainly means not an `Account` either, and there is
    /// no reading of an exclusion where admitting the subclass is what the
    /// author wanted.
    ///
    /// The case that motivated it is a nilable boolean, where a rewrite has to
    /// know what it is *not* looking at:
    ///
    /// ```yaml
    /// where:
    ///   $X: { type_not: [TrueClass, FalseClass, Boolean] }
    /// ```
    ///
    /// `Boolean` is in that list because `T::Boolean` is a constant path and
    /// resolves by its last segment, so a Sorbet signature returning one arrives
    /// under that name rather than as the two classes it aliases.
    #[serde(default)]
    pub type_not: Option<Vec<String>>,

    /// Whether `type:` means an instance or the class object.
    ///
    /// `Account.display_name` and `account.display_name` are different methods,
    /// so a rule has to say which. Defaults to `instance`, matching Ruby's own
    /// `Account#display_name` notation and the commoner case.
    #[serde(default)]
    pub kind: Option<Kind>,

    /// Admit receivers whose class descends from `type:` (D51).
    ///
    /// Off by default, because narrowing must only ever narrow. On for a
    /// rename, where a subclass call site left behind is a `NoMethodError`.
    #[serde(default)]
    pub subclasses: Option<bool>,

    /// The capture must be this kind of node.
    ///
    /// The predicate a *literal* rule needs. Sorting array elements is only
    /// wanted where order is presentation rather than meaning, which in practice
    /// means a constant; `gsub` -> `tr` is only valid for string literals. Both
    /// are shape questions the pattern language cannot ask, because the same
    /// syntax position accepts several node kinds.
    #[serde(default)]
    pub is: Option<NodeKind>,

    /// The capture's literal content must be exactly this many characters.
    ///
    /// `tr` maps character by character, so `gsub("ab", "cd")` is not `tr` -- the
    /// rewrite is valid only when both arguments are one character long.
    #[serde(default)]
    pub length: Option<usize>,

    /// The capture's subtree must contain a match of this pattern.
    ///
    /// A pattern matches a *shape*, and until now there was no way to say "and
    /// somewhere inside it, this". Metavariables shared with the outer pattern
    /// must bind to the same thing, which is what lets a block's body be tied
    /// to the block's own parameter:
    ///
    /// ```yaml
    /// match: $R.each { |$X| $B }
    /// where:
    ///   $B: { contains: $X.$INNER }
    /// ```
    ///
    /// The inline `{ ... }` is YAML's *flow mapping*, and it reads better than
    /// three indented lines. It has one trap: inside it, `,` `{` `}` `[` and
    /// `]` are structural, so a pattern containing any of them is silently
    /// truncated. Quote such a pattern, or write the constraint in block
    /// style. rwr refuses loudly when this happens rather than running a rule
    /// that quietly matches nothing.
    #[serde(default)]
    pub contains: Option<String>,

    /// This capture must name the same identifier as another.
    ///
    /// `{foo: foo}` -> `{foo:}` needs a symbol key compared against a
    /// local-variable or method-call value: the same *name*, but different node
    /// kinds, so D16's AST equality does not apply.
    #[serde(default)]
    pub same_name_as: Option<String>,
}

/// A node kind a capture may be constrained to.
///
/// Deliberately a closed set: an unknown value is a rule bug and must be a
/// diagnostic rather than a constraint that quietly never matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NodeKind {
    /// `FOO`, `Foo::Bar` -- a constant read or its enclosing path.
    Constant,
    /// `:foo`
    Symbol,
    /// `"foo"`, `'foo'` -- a plain literal, not an interpolated one.
    String,
    /// `1`, `0xff`
    Integer,
    /// `[1, 2]`
    Array,
    /// `{a: 1}`
    Hash,
}

/// Which of a class's two method tables a constraint means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    /// `Account#display_name`
    Instance,
    /// `Account.display_name`
    Class,
}

impl Constraint {
    /// Whether this constraint wants an instance receiver.
    pub(crate) fn wants_instance(&self) -> bool {
        !matches!(self.kind, Some(Kind::Class))
    }

    /// Whether this constraint resolves a receiver, and so needs the signature
    /// index and the hierarchy to have been built.
    ///
    /// Both are built lazily, and a predicate left out of this answer does not
    /// fail loudly -- it resolves nothing, declines every candidate, and reports
    /// "receiver did not resolve" about a receiver that would have resolved
    /// perfectly well had the index been built. `type_not:` shipped that way for
    /// the length of one test run.
    pub(crate) fn narrows_by_receiver(&self) -> bool {
        self.receiver_type.is_some() || self.type_not.is_some()
    }

    /// Class names whose descendants this constraint needs to know about.
    ///
    /// `type_not:` consults descent too -- "not an `ActiveRecord::Base`" has to
    /// rule out `Account` -- so its classes are hierarchy roots exactly as
    /// `type:`'s one is.
    pub(crate) fn hierarchy_roots(&self) -> Vec<String> {
        let mut out: Vec<String> = self.receiver_type.iter().cloned().collect();
        out.extend(self.type_not.iter().flatten().cloned());
        out
    }
}

/// Constraints on the match as a whole rather than on one capture.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scope {
    /// The match must sit lexically inside this class or module.
    ///
    /// Ships in v1 (D19) because the corpus demanded it at entry one: the first
    /// realistic rename needs it to reach implicit-self call sites, which are
    /// 43.5% of all calls in rails.
    #[serde(default)]
    pub inside: Option<String>,

    /// Restrict to singleton context -- inside `def self.x` or `class << self`.
    ///
    /// Without this, a bare `display_name` inside a class-method body is
    /// indistinguishable from one inside an instance method, so a class-method
    /// rename could not reach its implicit-self calls.
    #[serde(default)]
    pub singleton: Option<bool>,

    /// Admit classes descending from `inside:` (D51), so a rename reaches an
    /// override's definition as well as the original's.
    #[serde(default)]
    pub subclasses: Option<bool>,
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
    /// A directory was given as a rule pack but holds no rule files.
    EmptyPack {
        path: String,
    },
    /// Neither a path nor anything in the built-in pack.
    NoSuchRule {
        name: String,
        known: Vec<String>,
    },
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleError::Unreadable { path, message } => write!(f, "cannot read {path}: {message}"),
            RuleError::Malformed { path, message } => {
                write!(f, "{path} is not a rule: {message}")?;
                // A pattern cut in half by a YAML flow mapping arrives as an
                // extra key, and serde's report of it -- "unknown field `$B)`"
                // -- describes the symptom without naming the cause.
                if message.contains("unknown field") && message.contains('$') {
                    write!(
                        f,
                        "\n  A pattern inside `{{ ... }}` cannot hold a comma, brace or \
                         bracket: YAML claims those. Quote it, or use indented keys."
                    )?;
                }
                Ok(())
            }
            RuleError::EmptyPack { path } => {
                write!(f, "no .yml or .yaml rule files under {path}")
            }
            RuleError::NoSuchRule { name, known } => write!(
                f,
                "no such file, directory, or built-in rule: {name}\n  try one of: {}",
                known.join(", ")
            ),
            RuleError::NoTemplate => write!(
                f,
                "a replacement is required — pass -r/--replace, or give a rule file with a `rewrite:` key"
            ),
        }
    }
}

/// Every field above is spelled out and unknown ones are rejected, because the
/// alternative is silent and dangerous: `wher:` for `where:` produced a rule
/// that ran happily *without its constraint*, turning a narrowed rename into an
/// unnarrowed one. serde ignores unknown fields by default, which is the wrong
/// default for a file that decides what gets rewritten.
///
/// A rename written in Ruby's own method notation.
///
/// `Account#display_name` and `Account.display_name` are how Ruby developers
/// name methods, and they carry the instance-versus-class distinction that a
/// rename must respect. One line expands to the rule set a complete rename
/// needs -- the definition, the explicit-receiver calls, and the implicit-self
/// calls -- which is otherwise three hand-written rules.
#[derive(Debug, Deserialize)]
pub(crate) struct MethodRename {
    /// `Account#display_name`, `Account.display_name`, or a bare `display_name`.
    pub method: String,
    /// The new name.
    pub rename: String,
}

impl MethodRename {
    /// Split `Account#foo` / `Account.foo` into its class, name, and kind.
    fn parts(&self) -> (Option<&str>, &str, Kind) {
        if let Some((class, name)) = self.method.split_once('#') {
            (Some(class), name, Kind::Instance)
        } else if let Some((class, name)) = self.method.split_once('.') {
            (Some(class), name, Kind::Class)
        } else {
            (None, self.method.as_str(), Kind::Instance)
        }
    }

    /// The rules this notation stands for.
    pub(crate) fn expand(&self) -> Vec<Rule> {
        let (class, name, kind) = self.parts();
        let new = &self.rename;
        // A rename covers the hierarchy: reaching subclass call sites without
        // renaming an override's definition would ship a NoMethodError, and
        // renaming the definition without the call sites would too.
        let scope = || Scope {
            inside: class.map(str::to_string),
            singleton: None,
            subclasses: Some(true),
        };
        let receiver = || {
            let mut c = HashMap::new();
            c.insert(
                "$R".to_string(),
                Constraint {
                    receiver_type: class.map(str::to_string),
                    kind: Some(kind),
                    subclasses: Some(true),
                    ..Default::default()
                },
            );
            c
        };

        // `(*$P)` rather than a bare `def name`: an override whose parameter
        // list has drifted from its parent's is the ordinary shape of legacy
        // inheritance, and a pattern with no parameter list matches only a
        // definition that has none. Leaving those to the residue report meant a
        // rename declined the one occurrence guaranteed to break.
        //
        // `class << self` puts an ordinary-looking `def` in singleton context,
        // so the definition rules have to say which context they mean. Left
        // unconstrained, a `#` rename rewrote a class method defined that way --
        // a wrong rewrite from a node identical to the one it was looking for --
        // and a `.` rename missed the same definition entirely.
        let in_singleton = |singleton: bool| Scope {
            inside: class.map(str::to_string),
            singleton: Some(singleton),
            subclasses: Some(true),
        };
        let definitions = match kind {
            Kind::Instance => vec![Rule {
                pattern: format!("def {name}(*$P); $B; end"),
                rewrite: Some(format!("def {new}(*$P); $B; end")),
                scope: in_singleton(false),
                ..Default::default()
            }],
            Kind::Class => vec![
                Rule {
                    pattern: format!("def self.{name}(*$P); $B; end"),
                    rewrite: Some(format!("def self.{new}(*$P); $B; end")),
                    scope: scope(),
                    ..Default::default()
                },
                // The same method, spelled the other way.
                Rule {
                    pattern: format!("def {name}(*$P); $B; end"),
                    rewrite: Some(format!("def {new}(*$P); $B; end")),
                    scope: in_singleton(true),
                    ..Default::default()
                },
            ],
        };

        // Explicit receivers, narrowed by class *and* kind. `self.foo` inside an
        // ordinary method body resolves as an instance receiver, so this covers
        // it too.
        let calls = Rule {
            pattern: format!("$R.{name}"),
            rewrite: Some(format!("$R.{new}")),
            constraints: receiver(),
            ..Default::default()
        };

        // A literal name handed to a dispatcher, on a receiver that resolves.
        //
        // `account.send(:display_name)` is as unambiguous as `account.display_name`
        // once the receiver is known -- the same narrowing decides both -- so
        // reporting it and leaving the user to edit it by hand was declining work
        // rwr had already proved it could do safely. The receiver constraint is
        // what makes it safe: `unknown.send(:display_name)` does not resolve and
        // is reported, exactly as the plain call would be.
        let dispatchers = || {
            let mut c = receiver();
            c.insert(
                "$SEND".to_string(),
                Constraint {
                    name: Some(
                        ["send", "public_send", "__send__", "try", "try!"]
                            .iter()
                            .map(|s| (*s).to_string())
                            .collect(),
                    ),
                    ..Default::default()
                },
            );
            c
        };

        let mut rules = definitions;
        rules.push(calls);
        rules.push(Rule {
            pattern: format!("$R.$SEND(:{name})"),
            rewrite: Some(format!("$R.$SEND(:{new})")),
            constraints: dispatchers(),
            ..Default::default()
        });
        rules.push(Rule {
            pattern: format!("$R.$SEND(\"{name}\")"),
            rewrite: Some(format!("$R.$SEND(\"{new}\")")),
            constraints: dispatchers(),
            ..Default::default()
        });

        // Implicit self, the largest receiver bucket -- reachable only through
        // lexical scope, so it needs a class to be anchored to. The singleton
        // flag is what keeps a class-method rename from touching an instance
        // method's body, and vice versa.
        if class.is_some() {
            rules.push(Rule {
                pattern: name.to_string(),
                rewrite: Some(new.to_string()),
                scope: Scope {
                    inside: class.map(str::to_string),
                    singleton: Some(kind == Kind::Class),
                    subclasses: Some(true),
                },
                ..Default::default()
            });
        }
        rules
    }
}

impl Rule {
    /// The class this rule is about, if it names one.
    ///
    /// Used to scope the rule's own residue report. Per rule rather than per
    /// set: a pack containing two renames has two classes, and scoping both by
    /// the first drops the second's account entirely.
    pub(crate) fn class_anchor(&self) -> Option<String> {
        self.scope.inside.clone().or_else(|| {
            self.constraints
                .values()
                .find_map(|c| c.receiver_type.clone())
        })
    }

    /// Everything about a rule that can only be checked once its pattern is
    /// prepared, checked before a single file is read.
    ///
    /// Each of these used to degrade silently, and silently in the same
    /// direction: a rule that ran clean and did the wrong amount of work. A
    /// constraint on a capture the pattern never binds matched *nothing*; a
    /// metavariable in the template that the pattern never binds rendered as
    /// *empty*, turning `log($A, $B)` into `log(a, )`. Neither said a word.
    pub(crate) fn validate(
        &self,
        prepared: &crate::pattern::prepare::Prepared,
    ) -> Result<(), RuleError> {
        let bound: Vec<&str> = prepared
            .bindings
            .values()
            .filter_map(|b| b.name.as_deref())
            .collect();
        let complain = |what: String| RuleError::Malformed {
            path: self.id.clone().unwrap_or_else(|| "rule".to_string()),
            message: what,
        };
        let known = |name: &str| bound.iter().any(|b| *b == name.trim_start_matches('$'));

        for (key, constraint) in &self.constraints {
            if !known(key) {
                return Err(complain(format!(
                    "`where:` constrains {key}, which `match:` never captures"
                )));
            }
            if let Some(other) = &constraint.same_name_as
                && !known(other)
            {
                return Err(complain(format!(
                    "`same_name_as: {other}` names something `match:` never captures"
                )));
            }
            // With an allowlist in hand the exclusion is either redundant or
            // contradictory. Refuse rather than define an intersection nobody
            // will remember reading.
            if constraint.name.is_some() && constraint.name_not.is_some() {
                return Err(complain(format!(
                    "{key} has both `name:` and `name_not:` -- an allowlist already says which"
                )));
            }
        }

        // A metavariable the template introduces has nothing to be replaced
        // with, and renders as empty rather than announcing itself.
        if let Some(template) = &self.rewrite {
            for var in crate::pattern::metavar::scan(template) {
                if let Some(name) = &var.name
                    && !known(name)
                {
                    return Err(complain(format!(
                        "`rewrite:` uses ${name}, which `match:` never captures"
                    )));
                }
            }
        }

        if let Some(text) = &self.ruby
            && crate::ruby::Version::parse(text).is_none()
        {
            return Err(complain(format!(
                "`ruby: {text}` is not a version like 3.1"
            )));
        }
        Ok(())
    }

    /// Sub-patterns this rule's `contains:` constraints need, prepared once.
    ///
    /// Preparing is a parse-and-retry loop, so doing it per candidate match
    /// would put it in the hot path for the sake of a pattern that never
    /// changes.
    pub(crate) fn contained(
        &self,
    ) -> Result<HashMap<String, crate::pattern::prepare::Prepared>, RuleError> {
        self.constraints
            .iter()
            .filter(|(_, c)| c.contains.is_some())
            .map(|(key, c)| {
                let text = c.contains.as_deref().unwrap_or_default();
                // Loudly. Swallowing this left a rule that silently matched
                // nothing, which is how an unquoted comma inside a YAML flow
                // mapping -- `{ contains: log($A, $B) }`, truncated by YAML to
                // `log($A` -- turned into a rule that ran clean and found zero.
                let prepared =
                    crate::pattern::prepare::prepare(text).map_err(|e| RuleError::Malformed {
                        path: format!("{key} contains:"),
                        message: format!(
                            "{e}. If this pattern holds a comma, a brace or a \
                                          bracket inside a `{{ ... }}` mapping, YAML has \
                                          eaten part of it -- quote it."
                        ),
                    })?;
                Ok((key.trim_start_matches('$').to_string(), prepared))
            })
            .collect()
    }

    /// Captures the rule declares to be constants.
    ///
    /// Fed to the substitution, which cannot otherwise tell `FOO = 1` from
    /// `foo = 1`: both casings parse, so the case-repair loop never fires.
    pub(crate) fn constant_captures(&self) -> Vec<String> {
        self.constraints
            .iter()
            .filter(|(_, c)| c.is == Some(NodeKind::Constant))
            .map(|(k, _)| k.clone())
            .collect()
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
            ..Default::default()
        }]);
    }

    let path = Path::new(rule);
    if path.is_dir() {
        return load_pack(path);
    }
    if !path.is_file() {
        // Not a path, so it may name part of the built-in pack. A real path
        // always wins: resolving the other way round would mean a rule shipped
        // in a later version could quietly shadow the caller's own directory.
        return builtin(rule);
    }
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rule.to_string());
    load_file(path, &id)
}

/// The rule pack compiled into the binary, as `(id, yaml)` in path order.
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/builtin_rules.rs"));
}

/// Rules from the built-in pack selected by `name`.
///
/// `all` is everything; a family name is every rule under it (`performance`);
/// a full id is one rule (`performance/detect`). The pack has to be compiled in
/// rather than read from disk: `cargo install` copies the binary alone, so a
/// pack that lives only in the repo is not shipped.
fn builtin(name: &str) -> Result<Vec<Rule>, RuleError> {
    let selected: Vec<&(&str, &str)> = embedded::BUILTIN
        .iter()
        .filter(|(id, _)| {
            name == "all"
                || *id == name
                || id.strip_prefix(name).is_some_and(|r| r.starts_with('/'))
        })
        .collect();

    if selected.is_empty() {
        return Err(RuleError::NoSuchRule {
            name: name.to_string(),
            known: families(),
        });
    }

    let mut rules = Vec::new();
    for (id, yaml) in selected {
        rules.extend(parse(yaml, id, id)?);
    }
    Ok(rules)
}

/// What a caller may ask for: `all`, each family, and each full id.
fn families() -> Vec<String> {
    let mut out = vec!["all".to_string()];
    for (id, _) in embedded::BUILTIN {
        if let Some((family, _)) = id.split_once('/')
            && !out.iter().any(|k| k == family)
        {
            out.push(family.to_string());
        }
    }
    out.extend(embedded::BUILTIN.iter().map(|(id, _)| (*id).to_string()));
    out
}

/// Every rule in one file, each stamped with `id` unless it named itself.
fn load_file(path: &Path, id: &str) -> Result<Vec<Rule>, RuleError> {
    let name = path.display().to_string();
    let raw = std::fs::read_to_string(path).map_err(|e| RuleError::Unreadable {
        path: name.clone(),
        message: e.to_string(),
    })?;
    parse(&raw, id, &name)
}

/// Rules from one YAML document, each stamped with `id` unless it named itself.
///
/// `origin` names the source in an error, which is a path for a file on disk and
/// a rule id for the built-in pack.
fn parse(raw: &str, id: &str, origin: &str) -> Result<Vec<Rule>, RuleError> {
    // The method-notation shorthand expands to a rule set.
    let mut rules = if let Ok(rename) = serde_yaml::from_str::<MethodRename>(raw) {
        rename.expand()
    } else if let Ok(rules) = serde_yaml::from_str::<Vec<Rule>>(raw) {
        // A sequence is a rule set; a mapping is one rule. Trying the sequence
        // first keeps the single-rule spelling unchanged.
        rules
    } else {
        vec![
            serde_yaml::from_str::<Rule>(raw).map_err(|e| RuleError::Malformed {
                path: origin.to_string(),
                message: e.to_string(),
            })?,
        ]
    };

    // The file is the unit of identity: a rename expands to several rules but
    // is one thing a user turned on, and reports as one.
    for rule in &mut rules {
        if rule.id.is_none() {
            rule.id = Some(id.to_string());
        }
    }
    Ok(rules)
}

/// Every rule under a directory, in path order.
///
/// A pack is a directory because that is how a user turns a subset on: point at
/// `rules/performance` and get those rules, point at `rules` and get all of
/// them. A file that fails to parse is an error rather than a skip -- silently
/// dropping a rule is the same failure as silently dropping an edit.
fn load_pack(dir: &Path) -> Result<Vec<Rule>, RuleError> {
    let mut files = Vec::new();
    collect(dir, &mut files)?;
    // Sorted so a pack applies in the same order everywhere. Rules run in
    // sequence, each seeing the last one's output, so the order is observable.
    files.sort();
    if files.is_empty() {
        return Err(RuleError::EmptyPack {
            path: dir.display().to_string(),
        });
    }

    let mut rules = Vec::new();
    for file in &files {
        // The path within the pack, so `performance/detect.yml` reports as
        // `performance/detect` and stays unambiguous across subdirectories.
        let id = file
            .strip_prefix(dir)
            .unwrap_or(file)
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        rules.extend(load_file(file, &id)?);
    }
    Ok(rules)
}

/// Every `.yml`/`.yaml` file under `dir`, recursively.
fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), RuleError> {
    let entries = std::fs::read_dir(dir).map_err(|e| RuleError::Unreadable {
        path: dir.display().to_string(),
        message: e.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| RuleError::Unreadable {
            path: dir.display().to_string(),
            message: e.to_string(),
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml" | "yaml")
        ) {
            out.push(path);
        }
    }
    Ok(())
}

/// Resolve the `RULE` argument, which is a file path when one exists and a bare
/// pattern otherwise.
#[allow(dead_code)]
pub(crate) fn load(rule: &str, replace: Option<&str>) -> Result<Rule, RuleError> {
    if let Some(template) = replace {
        return Ok(Rule {
            pattern: rule.to_string(),
            rewrite: Some(template.to_string()),
            ..Default::default()
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

    /// Both the signature index and the hierarchy are built lazily, gated on
    /// whether any rule narrows by receiver. A predicate missing from that gate
    /// does not fail loudly -- it resolves nothing and then reports "receiver
    /// did not resolve" about receivers that would have resolved. `type_not:`
    /// shipped that way until a test run caught it.
    #[test]
    fn every_receiver_predicate_is_in_the_lazy_gate() {
        let by_type = Constraint {
            receiver_type: Some("Account".into()),
            ..Default::default()
        };
        let by_exclusion = Constraint {
            type_not: Some(vec!["TrueClass".into(), "FalseClass".into()]),
            ..Default::default()
        };
        assert!(by_type.narrows_by_receiver());
        assert!(by_exclusion.narrows_by_receiver());
        assert!(!Constraint::default().narrows_by_receiver());

        // Descent is consulted for every excluded class, so each is a root.
        assert_eq!(by_type.hierarchy_roots(), vec!["Account".to_string()]);
        assert_eq!(
            by_exclusion.hierarchy_roots(),
            vec!["TrueClass".to_string(), "FalseClass".to_string()]
        );
    }
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

    /// Ruby's own notation, expanded.
    #[test]
    fn method_notation_expands_to_a_rule_set() {
        let rename = MethodRename {
            method: "Account#display_name".into(),
            rename: "full_name".into(),
        };
        let rules = rename.expand();
        assert_eq!(rules[0].pattern, "def display_name(*$P); $B; end");
        assert_eq!(rules[0].scope.inside.as_deref(), Some("Account"));
        assert_eq!(rules[1].pattern, "$R.display_name");
        assert_eq!(rules[1].constraints["$R"].kind, Some(Kind::Instance));

        // A literal name handed to a dispatcher is the same call in another
        // spelling, and the same receiver constraint decides it.
        let patterns: Vec<&str> = rules.iter().map(|r| r.pattern.as_str()).collect();
        assert!(
            patterns.contains(&"$R.$SEND(:display_name)"),
            "{patterns:?}"
        );
        assert!(
            patterns.contains(&"$R.$SEND(\"display_name\")"),
            "{patterns:?}"
        );
        for rule in rules.iter().filter(|r| r.pattern.contains("$SEND")) {
            assert!(
                rule.constraints["$SEND"]
                    .name
                    .as_ref()
                    .is_some_and(|n| n.iter().any(|m| m == "public_send")),
                "a dispatcher rule must say which dispatchers it means"
            );
            assert_eq!(rule.constraints["$R"].kind, Some(Kind::Instance));
        }

        // Implicit self is last, and reaches only inside the class.
        let implicit = rules.last().expect("an implicit-self rule");
        assert_eq!(implicit.pattern, "display_name");
        assert_eq!(implicit.scope.inside.as_deref(), Some("Account"));
    }

    /// The dot form names a class method, and its definition is on the
    /// singleton -- a different method from the `#` form entirely.
    #[test]
    fn the_dot_form_targets_class_methods() {
        let rename = MethodRename {
            method: "Account.display_name".into(),
            rename: "full_name".into(),
        };
        let rules = rename.expand();
        assert_eq!(rules[0].pattern, "def self.display_name(*$P); $B; end");
        // The same method spelled the other way: `class << self` puts an
        // ordinary-looking `def` in singleton context, and without this rule a
        // `.` rename missed the definition while a `#` rename rewrote it.
        assert_eq!(rules[1].pattern, "def display_name(*$P); $B; end");
        assert_eq!(rules[1].scope.singleton, Some(true));

        let calls = rules
            .iter()
            .find(|r| r.pattern.starts_with("$R."))
            .expect("a call-site rule");
        assert_eq!(calls.constraints["$R"].kind, Some(Kind::Class));

        let implicit = rules.last().expect("an implicit-self rule");
        assert_eq!(
            implicit.scope.singleton,
            Some(true),
            "a class-method rename reaches implicit self only in singleton context"
        );
    }

    /// The `#` form must not reach a definition inside `class << self`.
    ///
    /// Left unconstrained, the definition rule matched it -- an instance rename
    /// rewriting a class method, from a node identical to the one it wanted.
    #[test]
    fn the_hash_form_stays_out_of_the_singleton() {
        let rename = MethodRename {
            method: "Account#display_name".into(),
            rename: "full_name".into(),
        };
        let rules = rename.expand();
        assert_eq!(rules[0].pattern, "def display_name(*$P); $B; end");
        assert_eq!(
            rules[0].scope.singleton,
            Some(false),
            "an instance rename must decline a `class << self` definition"
        );
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
    fn a_directory_loads_as_a_pack() {
        let dir = std::env::temp_dir().join("rwr-pack");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("style")).expect("temp dir");
        std::fs::write(
            dir.join("style/return-nil.yml"),
            "match: return nil\nrewrite: return\n",
        )
        .expect("write");
        std::fs::write(dir.join("a.yaml"), "match: foo\nrewrite: bar\n").expect("write");
        // Not a rule file, and must be ignored rather than fail the load.
        std::fs::write(dir.join("README.md"), "notes\n").expect("write");

        let rules = load_all(dir.to_str().expect("utf8"), None).expect("pack");
        let ids: Vec<&str> = rules.iter().filter_map(|r| r.id.as_deref()).collect();
        // Path order, so a pack applies the same way everywhere.
        assert_eq!(ids, ["a", "style/return-nil"]);
    }

    /// A rule that names itself keeps that name, whatever file it came from.
    #[test]
    fn a_declared_id_wins_over_the_path() {
        let dir = std::env::temp_dir().join("rwr-pack-id");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("whatever.yml"),
            "id: style/return-nil\nmatch: return nil\nrewrite: return\n",
        )
        .expect("write");
        let rules = load_all(dir.to_str().expect("utf8"), None).expect("pack");
        assert_eq!(rules[0].id.as_deref(), Some("style/return-nil"));
    }

    /// Silently skipping an unparseable rule is the same failure as silently
    /// dropping an edit: the pack would look like it ran.
    #[test]
    fn a_malformed_rule_fails_the_pack() {
        let dir = std::env::temp_dir().join("rwr-pack-bad");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("ok.yml"), "match: return nil\nrewrite: return\n").expect("write");
        std::fs::write(dir.join("bad.yml"), "nonsense: true\n").expect("write");
        assert!(load_all(dir.to_str().expect("utf8"), None).is_err());
    }

    #[test]
    fn an_empty_directory_says_so() {
        let dir = std::env::temp_dir().join("rwr-pack-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let err = load_all(dir.to_str().expect("utf8"), None).expect_err("empty");
        assert!(matches!(err, RuleError::EmptyPack { .. }));
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
