//! Canonical content paths of Sigil sources (contract §11.1, §12; Plan 0002 WP4.3).
//!
//! A compiled unit's `UnitId` derives from its **canonical content path**: an `AssetPath`
//! (contract §12) relative to the caller's content root, ending in `.sigil`. `sigilc` receives a
//! root plus a file and forms that relative path itself, but it **never normalises**: a file
//! outside the root, or a path that breaks §12's rules (a backslash, an uppercase letter, a
//! non-ASCII byte, a `.`/`..` segment), is a compile error, not something to repair (contract
//! §11.1). Normalising is the asset compiler's job (project ADR-0010).
//!
//! The `AssetPath` rules are restated here from contract §12 rather than imported:
//! `grimoire_sigilc` has no edge to `grimoire_assets`, not even a dev one (engine ADR-0008), exactly
//! as [`crate::derive_unit_id`] restates the id derivation.

use std::path::{Component, Path};

/// Longest canonical content path, in bytes; equal to `grimoire_assets::MAX_PATH_LEN` (contract
/// §12).
pub const MAX_CONTENT_PATH_LEN: usize = 255;

/// The extension every canonical content path of a Sigil source ends in.
pub const SIGIL_EXTENSION: &str = ".sigil";

/// Why a path is not a valid canonical content path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContentPathError {
    /// The path breaks one of contract §12's `AssetPath` rules or does not end in `.sigil`.
    #[error("`{path}` is not a canonical content path: {reason}")]
    Invalid {
        /// The offending path (relative, `/`-separated where it could be formed).
        path: String,
        /// Which rule it breaks.
        reason: &'static str,
    },
    /// The file does not lie inside the content root.
    #[error("`{file}` is not inside the content root `{root}`")]
    OutsideRoot {
        /// The file as given.
        file: String,
        /// The root as given.
        root: String,
    },
    /// The root or the file could not be resolved on disk (for example, it does not exist).
    #[error("cannot resolve `{path}`: {reason}")]
    Unresolvable {
        /// The path that could not be resolved.
        path: String,
        /// The operating system's reason.
        reason: String,
    },
}

/// Checks `path` against contract §12's `AssetPath` rules plus the `.sigil` extension.
///
/// # Errors
/// [`ContentPathError::Invalid`] naming the first broken rule.
pub fn validate_content_path(path: &str) -> Result<(), ContentPathError> {
    let invalid = |reason: &'static str| ContentPathError::Invalid {
        path: path.to_string(),
        reason,
    };
    if path.is_empty() {
        return Err(invalid("the path is empty"));
    }
    if path.len() > MAX_CONTENT_PATH_LEN {
        return Err(invalid("the path is longer than 255 bytes"));
    }
    if path.starts_with('/') || path.ends_with('/') {
        return Err(invalid("the path has a leading or trailing '/'"));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(invalid("the path has an empty segment"));
        }
        if segment == "." || segment == ".." {
            return Err(invalid("the path has a '.' or '..' segment"));
        }
    }
    let allowed = |byte: u8| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'.' | b'-' | b'/')
    };
    if !path.bytes().all(allowed) {
        return Err(invalid(
            "the path contains a byte outside ASCII [a-z0-9_.-] and '/'",
        ));
    }
    if !path.ends_with(SIGIL_EXTENSION) || path.len() == SIGIL_EXTENSION.len() {
        return Err(invalid("the path does not end in `<name>.sigil`"));
    }
    Ok(())
}

/// Forms the canonical content path of `file` relative to `root` and validates it.
///
/// Both are resolved on disk first (`std::fs::canonicalize`: symbolic links followed, `.`/`..`
/// resolved, the platform's own spelling of the path), so `content/./sigil/a.sigil` and an
/// absolute spelling of the same file agree; the relative path is then joined with `/` and checked
/// by [`validate_content_path`] exactly as it is, never lower-cased or otherwise repaired.
///
/// # Errors
/// [`ContentPathError::Unresolvable`] if `root` or `file` cannot be resolved,
/// [`ContentPathError::OutsideRoot`] if `file` does not lie under `root`, and
/// [`ContentPathError::Invalid`] if the relative path breaks a rule.
pub fn canonical_content_path(root: &Path, file: &Path) -> Result<String, ContentPathError> {
    let resolve = |path: &Path| {
        std::fs::canonicalize(path).map_err(|error| ContentPathError::Unresolvable {
            path: path.display().to_string(),
            reason: error.to_string(),
        })
    };
    let resolved_root = resolve(root)?;
    let resolved_file = resolve(file)?;
    let Ok(relative) = resolved_file.strip_prefix(&resolved_root) else {
        return Err(ContentPathError::OutsideRoot {
            file: file.display().to_string(),
            root: root.display().to_string(),
        });
    };
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => match segment.to_str() {
                Some(segment) => segments.push(segment),
                None => {
                    return Err(ContentPathError::Invalid {
                        path: relative.display().to_string(),
                        reason: "the path is not valid Unicode",
                    });
                }
            },
            _ => {
                return Err(ContentPathError::OutsideRoot {
                    file: file.display().to_string(),
                    root: root.display().to_string(),
                });
            }
        }
    }
    let path = segments.join("/");
    validate_content_path(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_asset_paths_ending_in_sigil() {
        for path in [
            "a.sigil",
            "sigil/imp_volley.sigil",
            "bosses/act-1/finale.v2.sigil",
        ] {
            assert_eq!(validate_content_path(path), Ok(()), "{path}");
        }
    }

    #[test]
    fn rejects_every_broken_rule_without_normalising() {
        for path in [
            "",
            "/a.sigil",
            "a.sigil/",
            "a//b.sigil",
            "./a.sigil",
            "a/../b.sigil",
            "A.sigil",
            "a\\b.sigil",
            "caf\u{e9}.sigil",
            "a b.sigil",
            "a.txt",
            ".sigil",
            "a.SIGIL",
        ] {
            assert!(
                matches!(
                    validate_content_path(path),
                    Err(ContentPathError::Invalid { .. })
                ),
                "{path:?} should be rejected"
            );
        }
        let long = format!("{}.sigil", "a".repeat(250));
        assert_eq!(long.len(), 256);
        assert!(validate_content_path(&long).is_err());
        let longest = format!("{}.sigil", "a".repeat(249));
        assert_eq!(validate_content_path(&longest), Ok(()));
    }
}
