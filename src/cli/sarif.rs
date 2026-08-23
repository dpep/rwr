//! SARIF 2.1.0 output, for GitHub Code Scanning and anything else that reads it.
//!
//! Why a third output format in a tool that already has two: SARIF is how a
//! finding becomes an annotation on a pull request without anyone writing a
//! translator. `github/codeql-action/upload-sarif` takes this file and puts each
//! result on the line it names. That is the whole GitHub integration, and it is
//! a serializer rather than an app -- no hosting, no OAuth, no webhook (see
//! DESIGN.md section 8).
//!
//! What rwr has that SARIF does not model well is the *account* -- residue, the
//! files it could not read, the templates it could not parse. Those are not
//! defects in the code and must not read as if they were, so they land at
//! `note` level and, for the ones with no location, as tool-execution
//! notifications rather than results. A blind spot reported as an error would
//! train people to ignore the report, which is the failure this tool exists to
//! avoid.

use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Sarif {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: Vec<Run>,
}

#[derive(Serialize)]
struct Run {
    tool: Tool,
    results: Vec<SarifResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    invocations: Vec<Invocation>,
}

#[derive(Serialize)]
struct Tool {
    driver: Driver,
}

#[derive(Serialize)]
struct Driver {
    name: &'static str,
    version: &'static str,
    #[serde(rename = "informationUri")]
    information_uri: &'static str,
    rules: Vec<ReportingDescriptor>,
}

/// One rule, described once and referenced by id from every result.
#[derive(Serialize)]
struct ReportingDescriptor {
    id: String,
    #[serde(rename = "shortDescription")]
    short_description: Message,
}

#[derive(Serialize)]
struct SarifResult {
    #[serde(rename = "ruleId")]
    rule_id: String,
    level: &'static str,
    message: Message,
    locations: Vec<Location>,
}

#[derive(Serialize)]
struct Message {
    text: String,
}

#[derive(Serialize)]
struct Location {
    #[serde(rename = "physicalLocation")]
    physical_location: PhysicalLocation,
}

#[derive(Serialize)]
struct PhysicalLocation {
    #[serde(rename = "artifactLocation")]
    artifact_location: ArtifactLocation,
    region: Region,
}

#[derive(Serialize)]
struct ArtifactLocation {
    uri: String,
}

#[derive(Serialize)]
struct Region {
    #[serde(rename = "startLine")]
    start_line: usize,
    #[serde(rename = "startColumn")]
    start_column: usize,
}

#[derive(Serialize)]
struct Invocation {
    #[serde(rename = "executionSuccessful")]
    execution_successful: bool,
    #[serde(rename = "toolExecutionNotifications")]
    tool_execution_notifications: Vec<Notification>,
}

#[derive(Serialize)]
struct Notification {
    level: &'static str,
    message: Message,
}

/// A result, before it knows what level it is.
pub(crate) struct Entry {
    pub rule: String,
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub text: String,
    pub level: &'static str,
}

impl Sarif {
    /// Build a run from what rwr found.
    ///
    /// `notes` are the things with no line to point at -- files that would not
    /// parse, templates that were only text-searched. SARIF has a place for
    /// exactly that, and putting them in `results` with a made-up location would
    /// be inventing evidence.
    pub(crate) fn new(entries: Vec<Entry>, notes: Vec<String>) -> Self {
        let mut ids: Vec<String> = entries.iter().map(|e| e.rule.clone()).collect();
        ids.sort();
        ids.dedup();

        Sarif {
            schema: "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
            version: "2.1.0",
            runs: vec![Run {
                tool: Tool {
                    driver: Driver {
                        name: "rwr",
                        version: env!("CARGO_PKG_VERSION"),
                        information_uri: "https://github.com/dpep/rwr",
                        rules: ids
                            .into_iter()
                            .map(|id| ReportingDescriptor {
                                short_description: Message { text: id.clone() },
                                id,
                            })
                            .collect(),
                    },
                },
                results: entries
                    .into_iter()
                    .map(|e| SarifResult {
                        rule_id: e.rule,
                        level: e.level,
                        message: Message { text: e.text },
                        locations: vec![Location {
                            physical_location: PhysicalLocation {
                                artifact_location: ArtifactLocation {
                                    // Relative, and without a leading `./`:
                                    // Code Scanning matches these against paths
                                    // in the repository, and a `./` prefix makes
                                    // every annotation land nowhere.
                                    uri: e.file.trim_start_matches("./").to_string(),
                                },
                                region: Region {
                                    start_line: e.line.max(1),
                                    start_column: e.col.max(1),
                                },
                            },
                        }],
                    })
                    .collect(),
                invocations: if notes.is_empty() {
                    Vec::new()
                } else {
                    vec![Invocation {
                        execution_successful: true,
                        tool_execution_notifications: notes
                            .into_iter()
                            .map(|text| Notification {
                                level: "note",
                                message: Message { text },
                            })
                            .collect(),
                    }]
                },
            }],
        }
    }
}
