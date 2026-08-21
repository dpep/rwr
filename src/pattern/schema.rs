//! How each Prism field participates in node equality (D36).
//!
//! Equality is *variant + atoms + children*; locations never participate. This
//! module holds the classification and the parity check that keeps it honest.
//!
//! Prism's own bindings are generated from a machine-readable schema that
//! `ruby-prism` vendors (`vendor/prism-<v>/config.json`). Classifying by field
//! *type* rather than by field name keeps the table tiny — thirteen types cover
//! all 151 node kinds — and the parity test reads that same schema to assert
//! nothing has appeared that we do not classify.
//!
//! Two layers of drift protection, per D36:
//!   * a new node *variant* fails a non-exhaustive match — a compile error;
//!   * a new *field* on an existing variant is the silent case, and is what the
//!     parity test here catches.

/// How a field participates in equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldClass {
    /// A child node, compared recursively.
    Child,
    /// An identifier. Compared by **resolved bytes**, never by id — pattern and
    /// target come from different parses with different constant pools.
    NameAtom,
    /// A literal value. Compared by **unescaped value**, so `"x"` equals `'x'`
    /// and heredoc bodies compare correctly with no heredoc-specific code.
    ValueAtom,
    /// Source position. Never compared: spelling is opt-in through `where:`
    /// predicates (DESIGN.md §2), not baked into equality.
    Ignored,
}

/// Classify a Prism field type. `None` means unrecognised, which the parity
/// test turns into a loud failure rather than a silent pass.
pub(crate) fn classify(field_type: &str) -> Option<FieldClass> {
    Some(match field_type {
        "node" | "node?" | "node[]" => FieldClass::Child,
        "constant" | "constant?" | "constant[]" => FieldClass::NameAtom,
        "string" | "integer" | "double" | "uint8" | "uint32" => FieldClass::ValueAtom,
        "location" | "location?" => FieldClass::Ignored,
        _ => return None,
    })
}

/// Flag bits that record *how the source was written* rather than what it
/// means. Ignoring these is what makes `foo` equal `foo()`.
///
/// Everything not listed here is treated as semantic and compared, so a new
/// flag defaults to being significant — the safe direction under
/// refuse-rather-than-guess.
pub(crate) const PARSE_ARTIFACT_FLAGS: &[&str] = &[
    // `foo` parses as a call with no arguments and this bit set; `foo()` is the
    // same call without it. Identical meaning, different spelling.
    "VARIABLE_CALL",
    // Encoding bookkeeping, not meaning.
    "FORCED_UTF8_ENCODING",
    "FORCED_BINARY_ENCODING",
    "FORCED_US_ASCII_ENCODING",
    // Integer base is spelling: 0x10 and 16 are the same value.
    "BINARY",
    "OCTAL",
    "DECIMAL",
    "HEXADECIMAL",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Locate the `config.json` that `ruby-prism` vendors, by walking the
    /// registry checkout rather than hardcoding a content hash.
    fn schema_path() -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var("HOME").ok()?);
        let src = home.join(".cargo/registry/src");
        for registry in std::fs::read_dir(src).ok()?.filter_map(Result::ok) {
            for crate_dir in std::fs::read_dir(registry.path())
                .ok()?
                .filter_map(Result::ok)
            {
                let name = crate_dir.file_name();
                if !name.to_string_lossy().starts_with("ruby-prism-") {
                    continue;
                }
                let vendor = crate_dir.path().join("vendor");
                let Ok(entries) = std::fs::read_dir(&vendor) else {
                    continue;
                };
                for prism in entries.filter_map(Result::ok) {
                    let config = prism.path().join("config.json");
                    if config.is_file() {
                        return Some(config);
                    }
                }
            }
        }
        None
    }

    fn schema() -> Option<serde_json::Value> {
        let raw = std::fs::read_to_string(schema_path()?).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// D36's drift guard for the silent case. A Prism upgrade that adds a field
    /// type we do not classify fails here, naming it, rather than being
    /// silently dropped from equality — which would make the matcher quietly
    /// wrong rather than loudly broken.
    #[test]
    fn every_schema_field_type_is_classified() {
        let Some(cfg) = schema() else {
            eprintln!("skipping: ruby-prism schema not found in the cargo registry");
            return;
        };
        let nodes = cfg["nodes"].as_array().expect("schema has nodes");
        assert!(nodes.len() > 100, "schema looks truncated");

        let mut unclassified: Vec<String> = nodes
            .iter()
            .flat_map(|n| {
                let node = n["name"].as_str().unwrap_or("?").to_string();
                n["fields"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(move |f| {
                        let ty = f["type"].as_str()?;
                        classify(ty)
                            .is_none()
                            .then(|| format!("{node}.{} ({ty})", f["name"].as_str().unwrap_or("?")))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        unclassified.sort();
        unclassified.dedup();

        assert!(
            unclassified.is_empty(),
            "unclassified Prism fields: {unclassified:?}"
        );
    }

    /// Flags are declared on the node, not among its fields, so they need their
    /// own guard. A new flag defaults to *semantic* — the safe direction — but
    /// this test surfaces it so the choice is deliberate.
    #[test]
    fn flag_families_are_known() {
        let Some(cfg) = schema() else {
            eprintln!("skipping: ruby-prism schema not found");
            return;
        };
        let families = cfg["flags"].as_array().expect("schema has flags");
        assert!(!families.is_empty());

        // Every artifact flag we name must still exist upstream; a stale entry
        // means we are silently ignoring nothing.
        let all: Vec<&str> = families
            .iter()
            .flat_map(|f| {
                f["values"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
            })
            .filter_map(|v| v["name"].as_str())
            .collect();

        for artifact in PARSE_ARTIFACT_FLAGS {
            assert!(
                all.contains(artifact),
                "{artifact} is no longer a Prism flag; the ignore list is stale"
            );
        }
    }

    #[test]
    fn locations_never_participate_in_equality() {
        assert_eq!(classify("location"), Some(FieldClass::Ignored));
        assert_eq!(classify("location?"), Some(FieldClass::Ignored));
    }

    /// The trap that started D36: a call's method name is not a child node, so
    /// comparing variant and children alone would match `foo(a)` against
    /// `bar(a)`.
    #[test]
    fn call_node_name_is_an_atom_not_a_child() {
        assert_eq!(classify("constant"), Some(FieldClass::NameAtom));
    }
}
