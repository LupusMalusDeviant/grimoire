//! Crate-wide error type for the content and pool APIs (contract §11.2/§11.3).

use crate::behavior::BehaviorId;
use crate::unit::{UnitError, UnitId};

/// Errors reported by the content and `BulletPool` APIs of this crate.
///
/// `#[non_exhaustive]`: the full runtime contract (§11) also defines `RegistryMismatch`,
/// `AlreadyInstalled`, `NotInstalled`, `SwapLimit` and `ContentEpochMismatch`, all of which belong
/// to `install()`/hot-swap (§11.6/§11.8) and are out of scope for WP1.3. They are omitted here
/// rather than stubbed, and a later work package can add them without breaking existing matches.
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
}
