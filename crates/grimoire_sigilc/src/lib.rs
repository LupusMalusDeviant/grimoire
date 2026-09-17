//! # grimoire_sigilc
//!
//! Library and CLI (`sigilc`) for the offline compiler of Sigil (`.sigil`), the declarative
//! bullet-pattern language of Grimoire (PRD-0004).
//!
//! Sigil source is compiled **offline**; the simulation runtime never parses source text and no
//! runtime crate ever depends on this crate, not even as a dev-dependency (engine ADR-0008,
//! contract §1). This crate is the sole intended producer of binary Sigil units
//! (`grimoire_sigil::SigilUnit`, contract §11.1); through `sigilc simulate` ([`simulate`], Plan
//! 0002 WP5.6) it is also the only tool that drives the runtime interpreter
//! (`grimoire_sigil::install`) against a `grimoire_sim::Simulation` offline, as the preview source
//! of the editor and of agents.
//!
//! It belongs to the determinism set (engine ADR-0008, contract §3) even though it is a compiler,
//! not a runtime crate: its output enters content hashes, golden masters and replays, so it must
//! be byte-identical across Windows, Linux and macOS. It therefore obeys the same float, thread
//! and hash-container rules as the simulation crates (enforced by `clippy.toml`, kept identical
//! to the other six determinism-set crates).
//!
//! **Status:** lexer, parser and diagnostics land in WP4.1 (this module tree: [`span`],
//! [`syntax`], [`parser`], [`fmt`], [`diagnostics`]); the schema pass, name resolution,
//! `from`-composition, static validation and the `SigilUnit` encoder land in WP4.2 ([`compiler`]);
//! the command-line interface lands in WP4.3 ([`cli`], with value listing and lossless `set` in
//! [`edit`], the canonical layout in [`fmt::format_canonical`], content paths in
//! [`content_path`] and the behaviour manifest in [`behaviors`]); silhouette and palette names
//! compile to rows of the shared visual catalogue ([`catalog`], PO decision 2026-09-17);
//! `sigilc simulate` runs a compiled unit and records every tick ([`simulate`], WP5.6).
//! [`derive_unit_id`] predates WP4.1 (added in WP1.3, once `grimoire_sigil::UnitId`, contract
//! §11.1, existed) and is unrelated to parsing.
//!
//! WP4.1's own entry point is [`parser::parse`]: it lexes and parses one `.sigil` source file
//! into a lossless [`syntax::SyntaxNode`] plus every [`diagnostics::Diagnostic`] found along the
//! way, never panicking regardless of how malformed the input is (contract §2 rule 9). WP4.2's
//! entry point is [`compiler::compile`]: given a loaded entry file and a [`compiler::SourceLoader`]
//! for its imports, it resolves, validates and lowers a `.sigil` source to
//! `grimoire_sigil::SigilUnit` bytes, or every diagnostic ([`diagnostics::Diagnostic`], codes
//! `SIG0012` onward) that stopped it from doing so. See `docs/formats/sigil.md` for the grammar,
//! the binary format and the full diagnostic code table, and
//! `crates/grimoire_sigilc/tests/corpus/` (parser corpus) and
//! `crates/grimoire_sigilc/tests/corpus/schema-invalid/` (compiler corpus) for the conformance
//! corpora both are checked against.

use grimoire_core::StableHasher;
use grimoire_sigil::UnitId;

pub mod behaviors;
pub mod catalog;
pub mod cli;
pub mod compiler;
pub mod content_path;
pub mod diagnostics;
pub mod edit;
pub mod fmt;
mod lexer;
pub mod parser;
pub mod simulate;
pub mod span;
pub mod syntax;

pub use diagnostics::{
    DIAGNOSTICS_SCHEMA_VERSION, Diagnostic, DiagnosticsDocument, RelatedLocation, Severity,
};
pub use fmt::{FormatError, format, format_canonical};
pub use parser::{ParseOutput, SUPPORTED_SIGIL_VERSION, parse};
pub use span::{Position, Span};
pub use syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

/// Domain separator fed into [`derive_unit_id`] before the path itself.
///
/// Identical to `grimoire_assets::AssetId::from_path`'s domain separator (contract §11.1: "leitet
/// `UnitId` nach derselben Regel ab wie `AssetId::from_path`", §12). The string is duplicated
/// here, rather than imported, because `grimoire_sigilc` has no dependency edge to
/// `grimoire_assets` (engine ADR-0008; this crate depends only on crates of the determinism set).
const UNIT_ID_DOMAIN: &str = "grimoire.asset-id.v1";

/// Errors from [`derive_unit_id`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DeriveUnitIdError {
    /// The hash of `canonical_content_path` happened to be exactly `0`, the one value
    /// [`UnitId`] reserves as always invalid (contract §11.1). Astronomically unlikely for a
    /// real path; kept as a real, reportable error rather than a panic (contract §2 rule 9).
    #[error("derived unit id for path {path:?} is 0, the reserved invalid id")]
    ZeroUnitId {
        /// The canonical content path that hashed to `0`.
        path: String,
    },
}

/// Derives the [`UnitId`] of a compiled Sigil unit from its canonical content path.
///
/// Same rule as `grimoire_assets::AssetId::from_path` (contract §11.1, §12): a fresh
/// [`StableHasher`] (algorithm version 1) fed with the domain separator
/// `"grimoire.asset-id.v1"` and then the path string, finished with [`StableHasher::finish`].
/// `sigilc` is the sole producer of `UnitId`s in P1 (PO decision P-2), so pack entry, swap
/// message and runtime always name a unit with the same number as long as they start from the
/// same canonical content path.
///
/// `canonical_content_path` is expected to already be a canonical content path (contract §11.1):
/// a valid `AssetPath` (§12) relative to the caller's content root, ending in `.sigil`. This
/// function does not itself validate that shape — resolving a source file or a root-plus-file
/// pair into that path is a translation concern of the parser/CLI landing in WP4 — it only
/// derives the id from whatever string it is given.
///
/// # Errors
/// Returns [`DeriveUnitIdError::ZeroUnitId`] if the derived id is `0`.
pub fn derive_unit_id(canonical_content_path: &str) -> Result<UnitId, DeriveUnitIdError> {
    let mut hasher = StableHasher::new();
    hasher.write_str(UNIT_ID_DOMAIN);
    hasher.write_str(canonical_content_path);
    reject_zero(hasher.finish(), canonical_content_path)
}

/// The `id == 0` check of [`derive_unit_id`], factored out so a test can exercise
/// [`DeriveUnitIdError::ZeroUnitId`] directly: no known path hashes to `0` (finding one is a
/// roughly 1-in-2^64 search), so the error arm would otherwise have no real test coverage.
fn reject_zero(id: u64, path: &str) -> Result<UnitId, DeriveUnitIdError> {
    if id == 0 {
        return Err(DeriveUnitIdError::ZeroUnitId {
            path: path.to_owned(),
        });
    }
    Ok(UnitId(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_unit_id_is_deterministic_and_domain_separated() {
        let a = derive_unit_id("units/basic.sigil").unwrap();
        let b = derive_unit_id("units/basic.sigil").unwrap();
        assert_eq!(a, b);

        let other = derive_unit_id("units/other.sigil").unwrap();
        assert_ne!(a, other);
    }

    #[test]
    fn derive_unit_id_never_returns_the_reserved_zero_id() {
        // Every id produced by a real path in the golden table below is non-zero; this asserts
        // the invariant the type itself relies on (`UnitId::INVALID = UnitId(0)`).
        for path in ["units/basic.sigil", "bosses/act1/finale.sigil", "a.sigil"] {
            assert_ne!(derive_unit_id(path).unwrap(), UnitId::INVALID);
        }
    }

    #[test]
    fn a_zero_hash_is_reported_as_an_error_not_the_invalid_id() {
        // No known path hashes to exactly 0 (the check above), so this exercises the
        // `DeriveUnitIdError::ZeroUnitId` arm directly against the underlying hash value instead
        // of searching for a colliding path.
        assert_eq!(
            reject_zero(0, "units/basic.sigil"),
            Err(DeriveUnitIdError::ZeroUnitId {
                path: "units/basic.sigil".to_owned(),
            })
        );
        assert_eq!(reject_zero(1, "units/basic.sigil"), Ok(UnitId(1)));
    }

    #[test]
    #[ignore = "prints the current derive_unit_id hashes; used only to (re)freeze the golden consts below"]
    fn print_derive_unit_id_hashes() {
        for path in ["units/basic.sigil", "bosses/act1/finale.sigil", "a.sigil"] {
            println!("{path:?} => {:#018x}", derive_unit_id(path).unwrap().0);
        }
    }

    /// Freezes the whole way from a fixed source path, through the canonical content path (here
    /// already in canonical form — path resolution itself lands in WP4), to the derived
    /// [`UnitId`] (contract §11.1). A change to the domain separator, the hash algorithm, or the
    /// derivation rule breaks this test; that is the point.
    #[test]
    fn golden_unit_ids_for_fixed_canonical_paths() {
        const GOLDEN: &[(&str, u64)] = &[
            ("units/basic.sigil", 0x7213_694b_8ac9_abcc),
            ("bosses/act1/finale.sigil", 0xaf68_c4d9_7aab_250c),
            ("a.sigil", 0x87da_7190_0cbf_c621),
        ];
        for (path, expected) in GOLDEN {
            let id = derive_unit_id(path).unwrap();
            assert_eq!(
                id,
                UnitId(*expected),
                "derive_unit_id({path:?}) = {id}, expected {expected:#018x}"
            );
        }
    }
}
