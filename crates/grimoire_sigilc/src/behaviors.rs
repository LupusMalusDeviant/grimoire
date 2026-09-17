//! The behaviour manifest: the name-to-`BehaviorId` table `sigilc check`/`build` compile against
//! (Plan 0002 WP4.3; `docs/formats/sigil.md` §11.4 point 4 and §13.3).
//!
//! `behaviour = <name>` in Sigil source resolves against a table supplied by the caller of
//! [`crate::compiler::compile`], because behaviour ids are assigned by whichever Rust code calls
//! `BehaviorRegistryBuilder::register` (contract §11.5) and cannot be computed from text. The CLI
//! reads that table from a small JSON file given with `--behaviors <file>`; this module is its
//! decoder. Like every decoder of foreign bytes it returns an error for any malformed input and
//! never panics (contract §2 rule 9), and it bounds the input size and the entry count before
//! trusting either.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Value of the manifest's `schema` field.
pub const BEHAVIOR_MANIFEST_SCHEMA: &str = "grimoire.sigilc.behaviors";

/// The only `schema_version` this decoder reads.
pub const BEHAVIOR_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Largest manifest accepted, in bytes.
pub const MAX_BEHAVIOR_MANIFEST_BYTES: usize = 1024 * 1024;

/// Most behaviours one manifest may list.
pub const MAX_BEHAVIORS: usize = 4096;

/// Longest behaviour name, in bytes.
pub const MAX_BEHAVIOR_NAME_BYTES: usize = 64;

/// Why a behaviour manifest was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BehaviorManifestError {
    /// The manifest is larger than [`MAX_BEHAVIOR_MANIFEST_BYTES`].
    #[error("the behaviour manifest is {len} bytes, more than the limit of {max}")]
    TooLarge {
        /// Actual size in bytes.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// The manifest is not JSON of the documented shape (unknown or missing keys, wrong types).
    #[error("the behaviour manifest is malformed: {0}")]
    Malformed(String),
    /// `schema` or `schema_version` names a different document.
    #[error(
        "the behaviour manifest has schema `{schema}` version {schema_version}, expected `grimoire.sigilc.behaviors` version 1"
    )]
    WrongSchema {
        /// The `schema` found.
        schema: String,
        /// The `schema_version` found.
        schema_version: u32,
    },
    /// More than [`MAX_BEHAVIORS`] entries.
    #[error("the behaviour manifest lists {count} behaviours, more than the limit of {max}")]
    TooManyEntries {
        /// Entries found.
        count: usize,
        /// The limit.
        max: usize,
    },
    /// A name is not a Sigil identifier of at most [`MAX_BEHAVIOR_NAME_BYTES`] bytes.
    #[error("`{name}` is not a valid behaviour name (a Sigil identifier of at most 64 bytes)")]
    InvalidName {
        /// The rejected name.
        name: String,
    },
    /// Two entries share a name.
    #[error("the behaviour name `{name}` is listed twice")]
    DuplicateName {
        /// The repeated name.
        name: String,
    },
    /// Two entries share an id.
    #[error("the behaviour id {id} is listed twice")]
    DuplicateId {
        /// The repeated id.
        id: u32,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDocument {
    schema: String,
    schema_version: u32,
    behaviors: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEntry {
    name: String,
    id: u32,
}

/// Decodes a behaviour manifest into the table [`crate::compiler::compile`] takes.
///
/// # Errors
/// A [`BehaviorManifestError`] for input that is too large, malformed, of another schema, too
/// long, or has an invalid or repeated name or a repeated id.
pub fn parse_behavior_manifest(json: &str) -> Result<BTreeMap<String, u32>, BehaviorManifestError> {
    if json.len() > MAX_BEHAVIOR_MANIFEST_BYTES {
        return Err(BehaviorManifestError::TooLarge {
            len: json.len(),
            max: MAX_BEHAVIOR_MANIFEST_BYTES,
        });
    }
    let document: ManifestDocument = serde_json::from_str(json)
        .map_err(|error| BehaviorManifestError::Malformed(error.to_string()))?;
    if document.schema != BEHAVIOR_MANIFEST_SCHEMA
        || document.schema_version != BEHAVIOR_MANIFEST_SCHEMA_VERSION
    {
        return Err(BehaviorManifestError::WrongSchema {
            schema: document.schema,
            schema_version: document.schema_version,
        });
    }
    if document.behaviors.len() > MAX_BEHAVIORS {
        return Err(BehaviorManifestError::TooManyEntries {
            count: document.behaviors.len(),
            max: MAX_BEHAVIORS,
        });
    }
    let mut table = BTreeMap::new();
    let mut ids = std::collections::BTreeSet::new();
    for entry in document.behaviors {
        if !is_behavior_name(&entry.name) {
            return Err(BehaviorManifestError::InvalidName { name: entry.name });
        }
        if !ids.insert(entry.id) {
            return Err(BehaviorManifestError::DuplicateId { id: entry.id });
        }
        if table.contains_key(&entry.name) {
            return Err(BehaviorManifestError::DuplicateName { name: entry.name });
        }
        table.insert(entry.name, entry.id);
    }
    Ok(table)
}

/// A Sigil identifier (`docs/formats/sigil.md` §2) of at most [`MAX_BEHAVIOR_NAME_BYTES`] bytes.
fn is_behavior_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    name.len() <= MAX_BEHAVIOR_NAME_BYTES
        && (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn decodes_a_valid_manifest() {
        let json = r#"{
            "schema": "grimoire.sigilc.behaviors",
            "schema_version": 1,
            "behaviors": [
                { "name": "orbit_parent", "id": 1 },
                { "name": "Homing_2", "id": 4294967295 }
            ]
        }"#;
        let table = parse_behavior_manifest(json).unwrap();
        assert_eq!(table.get("orbit_parent"), Some(&1));
        assert_eq!(table.get("Homing_2"), Some(&u32::MAX));
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn rejects_every_documented_problem() {
        let wrap = |behaviors: &str| {
            format!(
                r#"{{"schema":"grimoire.sigilc.behaviors","schema_version":1,"behaviors":[{behaviors}]}}"#
            )
        };
        let cases: Vec<(String, &str)> = vec![
            ("not json".to_string(), "malformed"),
            (
                r#"{"schema":"grimoire.sigilc.behaviors","schema_version":1}"#.to_string(),
                "malformed",
            ),
            (wrap(r#"{"name":"a","id":1,"extra":true}"#), "malformed"),
            (wrap(r#"{"name":"a","id":-1}"#), "malformed"),
            (wrap(r#"{"name":"a","id":4294967296}"#), "malformed"),
            (
                r#"{"schema":"other","schema_version":1,"behaviors":[]}"#.to_string(),
                "schema",
            ),
            (
                r#"{"schema":"grimoire.sigilc.behaviors","schema_version":2,"behaviors":[]}"#
                    .to_string(),
                "schema",
            ),
            (wrap(r#"{"name":"1a","id":1}"#), "name"),
            (wrap(r#"{"name":"","id":1}"#), "name"),
            (wrap(r#"{"name":"a-b","id":1}"#), "name"),
            (
                wrap(&format!(r#"{{"name":"{}","id":1}}"#, "a".repeat(65))),
                "name",
            ),
            (
                wrap(r#"{"name":"a","id":1},{"name":"a","id":2}"#),
                "dup_name",
            ),
            (wrap(r#"{"name":"a","id":1},{"name":"b","id":1}"#), "dup_id"),
        ];
        for (json, expected) in cases {
            let error = parse_behavior_manifest(&json).unwrap_err();
            let kind = match error {
                BehaviorManifestError::Malformed(_) => "malformed",
                BehaviorManifestError::WrongSchema { .. } => "schema",
                BehaviorManifestError::InvalidName { .. } => "name",
                BehaviorManifestError::DuplicateName { .. } => "dup_name",
                BehaviorManifestError::DuplicateId { .. } => "dup_id",
                other => panic!("unexpected {other:?} for {json}"),
            };
            assert_eq!(kind, expected, "{json}");
        }
    }

    #[test]
    fn bounds_size_and_entry_count() {
        let big = " ".repeat(MAX_BEHAVIOR_MANIFEST_BYTES + 1);
        assert!(matches!(
            parse_behavior_manifest(&big),
            Err(BehaviorManifestError::TooLarge { .. })
        ));
        let entries: Vec<String> = (0..=MAX_BEHAVIORS)
            .map(|i| format!(r#"{{"name":"b{i}","id":{i}}}"#))
            .collect();
        let json = format!(
            r#"{{"schema":"grimoire.sigilc.behaviors","schema_version":1,"behaviors":[{}]}}"#,
            entries.join(",")
        );
        assert!(matches!(
            parse_behavior_manifest(&json),
            Err(BehaviorManifestError::TooManyEntries { .. })
        ));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Contract §2 rule 9: arbitrary text never panics the decoder.
        #[test]
        fn arbitrary_text_never_panics(json in "\\PC{0,300}") {
            let _ = parse_behavior_manifest(&json);
        }

        /// Truncations and single-byte edits of a valid manifest never panic the decoder.
        #[test]
        fn edited_valid_manifests_never_panic(cut in 0usize..120, index in 0usize..120, byte in any::<u8>()) {
            let valid = r#"{"schema":"grimoire.sigilc.behaviors","schema_version":1,"behaviors":[{"name":"orbit_parent","id":1}]}"#;
            let mut bytes = valid.as_bytes().to_vec();
            if index < bytes.len() {
                bytes[index] = byte;
            }
            bytes.truncate(cut.min(bytes.len()));
            let text = String::from_utf8_lossy(&bytes);
            let _ = parse_behavior_manifest(&text);
        }
    }
}
