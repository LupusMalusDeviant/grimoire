//! Crate-wide error type for the content and pool APIs (contract §11.2/§11.3).

use crate::behavior::BehaviorId;
use crate::content::ContentEpoch;
use crate::unit::{UnitError, UnitId};

/// Errors reported by the content and `BulletPool` APIs of this crate.
///
/// `#[non_exhaustive]`: `AlreadyInstalled` and `RegistryMismatch` (`install()`, contract §11.6)
/// arrived with WP5.1; `NotInstalled`, `SwapLimit` and `ContentEpochMismatch` (hot-swap, §11.8)
/// with WP5.5.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SigilError {
    /// A [`SigilUnit`](crate::SigilUnit) failed to decode.
    #[error(transparent)]
    Unit(#[from] UnitError),
    /// [`BulletPool::spawn`](crate::BulletPool::spawn) found no free or fresh slot.
    #[error("bullet pool is full")]
    PoolFull,
    /// [`SigilLibrary::new`](crate::SigilLibrary::new) was given two units with the same id.
    #[error("unit {0} appears more than once")]
    DuplicateUnit(UnitId),
    /// A unit id was not found in the loaded library.
    #[error("unit {0} is not loaded")]
    UnknownUnit(UnitId),
    /// [`SigilLibrary::new`](crate::SigilLibrary::new) was given more than 65535 units.
    #[error("more than 65535 units in one library")]
    TooManyUnits,
    /// A `BulletSpawn::bullet_type` index has no matching record in the unit.
    #[error("unit {unit} has no bullet type {bullet_type}")]
    BulletTypeOutOfRange {
        /// Unit that was addressed.
        unit: UnitId,
        /// Out-of-range bullet-type index.
        bullet_type: u16,
    },
    /// A `BulletSpawn::program` index has no matching program in the unit.
    #[error("unit {unit} has no program {program}")]
    ProgramOutOfRange {
        /// Unit that was addressed.
        unit: UnitId,
        /// Out-of-range program index.
        program: u16,
    },
    /// A cascade depth exceeded [`SigilUnit::MAX_CASCADE_DEPTH`](crate::SigilUnit::MAX_CASCADE_DEPTH).
    #[error("cascade depth {depth} exceeds the maximum")]
    CascadeTooDeep {
        /// The offending cascade depth.
        depth: u8,
    },
    /// A spawn position, angle or speed was not finite.
    #[error("spawn value is not finite")]
    NonFinite,
    /// [`BehaviorRegistryBuilder::register`](crate::BehaviorRegistryBuilder::register) was given a
    /// duplicate id.
    #[error("behavior {0:?} is already registered")]
    DuplicateBehavior(BehaviorId),
    /// A unit references a behavior id that is not present in the registry it was built against.
    #[error("unit {unit} references unknown behavior {behavior:?}")]
    UnknownBehavior {
        /// Unit that references the missing behavior.
        unit: UnitId,
        /// The missing behavior id.
        behavior: BehaviorId,
    },
    /// [`crate::install`] was called on a [`grimoire_sim::Simulation`] that already has
    /// [`crate::SigilContent`] installed.
    #[error("sigil content is already installed on this simulation")]
    AlreadyInstalled,
    /// [`crate::install`] was given a library built against a different
    /// [`BehaviorRegistry`](crate::BehaviorRegistry) (by fingerprint) than the `registry`
    /// argument.
    #[error(
        "library was built against behavior registry fingerprint {loaded:#018x}, but install \
         was given registry fingerprint {given:#018x}"
    )]
    RegistryMismatch {
        /// Fingerprint the library was validated against ([`crate::SigilLibrary::new`]).
        loaded: u64,
        /// Fingerprint of the registry passed to [`crate::install`].
        given: u64,
    },
    /// [`crate::replace_unit`] was called on a simulation without installed Sigil content.
    #[error("no sigil content is installed on this simulation")]
    NotInstalled,
    /// [`crate::replace_unit`] would exceed `u32::MAX` swaps since installation.
    #[error("the content epoch has reached the maximum number of swaps")]
    SwapLimit,
    /// [`crate::restore_checked`] was given a snapshot whose content epoch differs from the
    /// loaded one; nothing was restored.
    #[error(
        "snapshot content epoch {snapshot:?} does not match the loaded content epoch {loaded:?}"
    )]
    ContentEpochMismatch {
        /// Epoch stored in the snapshot, `None` if it holds no Sigil content.
        snapshot: Option<ContentEpoch>,
        /// Epoch of the loaded content, `None` if none is installed.
        loaded: Option<ContentEpoch>,
    },
}
