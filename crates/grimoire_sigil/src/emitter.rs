//! Emitter, aim-target and clear-request data types (contract §11.4).
//!
//! These are plain data: the emitter tick-phase system (`sigil.emit`), the aimed-pattern
//! evaluation, and the `sigil.clear` system that applies [`ClearRequest`]s are all out of scope
//! for WP1.3. [`crate::BulletPool::clear`] is the primitive those systems will eventually call.

use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};

use crate::content::BulletFlags;
use crate::unit::UnitId;

/// A stateless bullet emitter attached to an entity.
///
/// What an emitter produces at tick `t` is a pure function of the unit, the emitter index, local
/// time `t - started_at`, `origin`, `rotation`, the current `AimTarget`, the seed, the tick and the
/// entity — there is no other state (contract §11.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Emitter {
    /// Unit the emitter definition comes from.
    pub unit: UnitId,
    /// Emitter index within the unit.
    pub emitter: u16,
    /// World-space origin.
    pub origin: Vec2,
    /// Base rotation, in radians.
    pub rotation: f32,
    /// Tick at which the emitter (re)started; the emitter is inactive before this tick.
    pub started_at: u64,
}
impl_stable_hash!(Emitter {
    unit,
    emitter,
    origin,
    rotation,
    started_at
});

/// Current aim point for `Aimed`-style patterns; `None` falls back to an emitter's own rotation.
///
/// Written by the player-proxy of a sibling work package (or by the game); this crate only defines
/// the resource type.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AimTarget(pub Option<Vec2>);

impl StableHash for AimTarget {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.0.stable_hash(hasher);
    }
}

/// Which live bullets a [`ClearRequest`] or [`crate::BulletPool::clear`] call despawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClearFilter {
    /// Every live bullet.
    All,
    /// Every live bullet spawned from the given unit.
    Unit(UnitId),
    /// Every live bullet of the given bullet type within the given unit.
    Type(UnitId, u16),
    /// Every live bullet with at least one of the given flags set.
    AnyFlags(BulletFlags),
}

impl StableHash for ClearFilter {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        match self {
            Self::All => hasher.write_u8(0),
            Self::Unit(unit) => {
                hasher.write_u8(1);
                unit.stable_hash(hasher);
            }
            Self::Type(unit, bullet_type) => {
                hasher.write_u8(2);
                unit.stable_hash(hasher);
                bullet_type.stable_hash(hasher);
            }
            Self::AnyFlags(flags) => {
                hasher.write_u8(3);
                flags.stable_hash(hasher);
            }
        }
    }
}

/// Structural request for the (not yet implemented) `sigil.clear` phase to despawn matching
/// bullets and then despawn the requesting entity.
///
/// Exclusive systems of this crate or the facade may instead call
/// [`crate::BulletPool::clear`] directly; a game's own systems always request clears through this
/// component (contract §2a/§11.4).
#[derive(Debug, Clone)]
pub struct ClearRequest {
    /// Which bullets to despawn.
    pub filter: ClearFilter,
}
impl_stable_hash!(ClearRequest { filter });
