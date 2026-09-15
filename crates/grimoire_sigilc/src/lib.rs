//! # grimoire_sigilc
//!
//! Library and CLI (`sigilc`) for the offline compiler of Sigil (`.sigil`), the declarative
//! bullet-pattern language of Grimoire (PRD-0004).
//!
//! Sigil source is compiled **offline**; the simulation runtime never parses source text and no
//! runtime crate ever depends on this crate, not even as a dev-dependency (engine ADR-0008,
//! contract §1). This crate is the sole intended producer of binary Sigil units
//! (`grimoire_sigil::SigilUnit`, contract §11.1); through the future `sigilc simulate`
//! subcommand it is also the only tool that drives the runtime interpreter
//! (`grimoire_sigil::install`) against a `grimoire_sim::Simulation` for offline verification
//! (Plan 0002 WP5.6).
//!
//! It belongs to the determinism set (engine ADR-0008, contract §3) even though it is a compiler,
//! not a runtime crate: its output enters content hashes, golden masters and replays, so it must
//! be byte-identical across Windows, Linux and macOS. It therefore obeys the same float, thread
//! and hash-container rules as the simulation crates (enforced by `clippy.toml`, kept identical
//! to the other six determinism-set crates).
//!
//! **Status:** skeleton (Plan-0002 WP1.3). The actual parser, validator and compiler land in
//! WP1.4 (syntax spike) and WP4 (compiler). [`derive_unit_id`] is implemented now because
//! `grimoire_sigil::UnitId` (contract §11.1) is available as of WP1.3.

use grimoire_core::StableHasher;
use grimoire_sigil::UnitId;

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
    let id = hasher.finish();
    if id == 0 {
        return Err(DeriveUnitIdError::ZeroUnitId {
            path: canonical_content_path.to_owned(),
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
