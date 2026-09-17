//! Assets → Sigil adapter (contract §9.1 table row `grimoire::adapters::assets`, §9.11, §11.2,
//! §12; Plan 0002 WP8.3): turns the Sigil entries of an [`AssetSource`] (a pack v1 file through
//! [`grimoire_assets::PackReader`], or a [`grimoire_assets::MemorySource`]) into the
//! [`SigilLibrary`] that [`grimoire_sigil::install`] takes.
//!
//! `grimoire_assets` and `grimoire_sigil` have no edge to each other (engine crate map §1): the
//! pack knows entries of kind [`AssetKind::SIGIL`] only as bytes, the runtime knows units only as
//! [`SigilUnit`]s. The mapping, with its checks, lives here:
//!
//! - every entry of kind `SIGIL` is read (the source verifies its SHA-256) and decoded with
//!   [`SigilUnit::from_bytes`]; entries of any other kind are left alone;
//! - its kind version must be [`SigilUnit::FORMAT_VERSION`] (contract §12: the kind version of a
//!   Sigil entry is the unit's binary format version);
//! - the unit's [`UnitId`] must equal the entry's [`AssetId`]: both derive from the same canonical
//!   content path (contract §11.1, §12), so a disagreement means the entry was packed under
//!   another path than the one `sigilc` compiled it for, and the id a swap message names would
//!   not find it.

use std::sync::Arc;

use grimoire_assets::{AssetError, AssetId, AssetKind, AssetSource};
use grimoire_sigil::{BehaviorRegistry, SigilError, SigilLibrary, SigilUnit, UnitError, UnitId};

/// Why the Sigil entries of an asset source could not become a library. `#[non_exhaustive]`: new
/// variants are additive (contract §2 rule 13).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum SigilAssetsError {
    /// Reading an entry failed (for example a SHA-256 mismatch).
    #[error("reading Sigil entry {id}: {source}")]
    Read {
        /// The entry.
        id: AssetId,
        /// The source's error.
        source: AssetError,
    },
    /// The entry's kind version is not the unit format version this build reads.
    #[error(
        "Sigil entry {id} has kind version {kind_version}, this build reads SigilUnit v{expected}"
    )]
    UnsupportedUnitVersion {
        /// The entry.
        id: AssetId,
        /// Its kind version.
        kind_version: u32,
        /// [`SigilUnit::FORMAT_VERSION`].
        expected: u32,
    },
    /// The entry's bytes are not a valid unit.
    #[error("Sigil entry {id} is not a valid unit: {source}")]
    Unit {
        /// The entry.
        id: AssetId,
        /// The decoder's error.
        source: UnitError,
    },
    /// The unit's id is not the entry's id (contract §11.1: both derive from one canonical path).
    #[error("Sigil entry {id} contains unit {unit}; the ids must be equal")]
    UnitIdMismatch {
        /// The entry.
        id: AssetId,
        /// The id inside the unit.
        unit: UnitId,
    },
    /// The units do not form a library (for example a behaviour the registry does not define).
    #[error("building the Sigil library: {0}")]
    Library(#[from] SigilError),
}

/// Decodes every entry of kind [`AssetKind::SIGIL`] in `source`, in the source's entry order
/// (ascending [`AssetId`]).
///
/// # Errors
/// The first entry that cannot be read, has another kind version than
/// [`SigilUnit::FORMAT_VERSION`], does not decode, or carries a unit id other than its own entry
/// id ([`SigilAssetsError`]).
pub fn sigil_units(source: &dyn AssetSource) -> Result<Vec<SigilUnit>, SigilAssetsError> {
    let mut units = Vec::new();
    for entry in source
        .entries()
        .iter()
        .filter(|entry| entry.kind == AssetKind::SIGIL)
    {
        let id = entry.id;
        if entry.kind_version != SigilUnit::FORMAT_VERSION {
            return Err(SigilAssetsError::UnsupportedUnitVersion {
                id,
                kind_version: entry.kind_version,
                expected: SigilUnit::FORMAT_VERSION,
            });
        }
        let bytes = source
            .read(id)
            .map_err(|source| SigilAssetsError::Read { id, source })?;
        let unit = SigilUnit::from_bytes(&bytes)
            .map_err(|source| SigilAssetsError::Unit { id, source })?;
        if unit.id().0 != id.0 {
            return Err(SigilAssetsError::UnitIdMismatch {
                id,
                unit: unit.id(),
            });
        }
        units.push(unit);
    }
    Ok(units)
}

/// [`sigil_units`] of `source`, built into a [`SigilLibrary`] against `registry`, ready for
/// [`grimoire_sigil::install`].
///
/// # Errors
/// Everything [`sigil_units`] reports, and [`SigilAssetsError::Library`] if
/// [`SigilLibrary::new`] rejects the units.
pub fn sigil_library(
    source: &dyn AssetSource,
    registry: Arc<BehaviorRegistry>,
) -> Result<SigilLibrary, SigilAssetsError> {
    Ok(SigilLibrary::new(sigil_units(source)?, registry)?)
}
