//! Content types: bullet types, the unit library, and the installable content resource
//! (contract §11.2).

use std::sync::Arc;

use grimoire_core::{StableHash, StableHasher, impl_stable_hash};
use grimoire_sim::ContentManifestHash;

use crate::behavior::BehaviorRegistry;
use crate::error::SigilError;
use crate::unit::{SigilUnit, UnitId};

/// Neutral visual identity of a bullet type: silhouette, palette and glow ids.
///
/// These are opaque ids with no meaning inside this crate; a facade adapter (not yet implemented)
/// maps them onto `grimoire_render`'s `BulletInstance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BulletVisual {
    /// Silhouette id.
    pub silhouette: u16,
    /// Palette id.
    pub palette: u16,
    /// Palette space id.
    pub palette_space: u8,
    /// Glow id.
    pub glow: u8,
}
impl_stable_hash!(BulletVisual {
    silhouette,
    palette,
    palette_space,
    glow
});

/// Bit flags describing bullet behaviour toward the rest of the game (contract §11.2).
///
/// Only the four low bits are defined in v1; every other bit must be zero (enforced by
/// [`crate::SigilUnit::from_bytes`] for decoded bullet types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BulletFlags(pub u8);

impl BulletFlags {
    /// The bullet can be destroyed by a smash-type player action.
    pub const SMASHABLE: Self = Self(1);
    /// The bullet can be reflected.
    pub const REFLECTABLE: Self = Self(2);
    /// The bullet interacts with the environment while active.
    pub const ENV_ACTIVE: Self = Self(4);
    /// The bullet can be grazed for score/meter.
    pub const GRAZEABLE: Self = Self(8);

    /// Whether every bit set in `other` is also set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl StableHash for BulletFlags {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u8(self.0);
    }
}

/// One bullet type of a [`SigilUnit`]: visuals, collision and lifetime.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BulletType {
    /// Visual identity, mapped to render data by a facade adapter.
    pub visual: BulletVisual,
    /// Visible radius (`BulletInstance::radius`).
    pub radius: f32,
    /// Collision radius for `grimoire_collide`.
    pub collision_radius: f32,
    /// Lifetime in ticks; `0` means unbounded (subject only to bounds, transforms and clears).
    pub lifetime_ticks: u32,
    /// Behaviour flags.
    pub flags: BulletFlags,
}
impl_stable_hash!(BulletType {
    visual,
    radius,
    collision_radius,
    lifetime_ticks,
    flags
});

/// Swap/manifest identity of a loaded [`SigilLibrary`] (contract §11.2/§11.8).
///
/// `#[non_exhaustive]`: constructed only by this crate; a later work package may add fields once
/// hot-swap (§11.8) is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ContentEpoch {
    /// Number of successful `replace_unit` calls since installation (always `0` for a library that
    /// has never been swapped, which is every library WP1.3 can build).
    pub swaps: u32,
    /// Manifest hash identifying the loaded units and behavior registry.
    ///
    /// Typed as `grimoire_sim::ContentManifestHash` (contract §8.1), now that the type exists and
    /// the crate map already allows the `grimoire_sigil -> grimoire_sim` edge this uses
    /// (Engine-ADR-0008, contract §1; the same edge already carries `Simulation`,
    /// `derive_block_rng`, `Tick`, `SimSeed` and `stream`). The value is still computed with
    /// exactly the §11.8 formula (`StableHasher` over the registry fingerprint, the unit count,
    /// then each unit's id and content hash in ascending `UnitId` order).
    pub manifest_hash: ContentManifestHash,
}

impl ContentEpoch {
    /// Builds an epoch from its fields.
    #[must_use]
    pub const fn new(swaps: u32, manifest_hash: ContentManifestHash) -> Self {
        Self {
            swaps,
            manifest_hash,
        }
    }
}
impl_stable_hash!(ContentEpoch {
    swaps,
    manifest_hash
});

/// Computes the §11.8 manifest hash for a freshly built library: the registry fingerprint, the
/// unit count, then each unit's id and content hash, in ascending `UnitId` order.
fn manifest_hash(registry_fingerprint: u64, units: &[SigilUnit]) -> ContentManifestHash {
    let mut hasher = StableHasher::new();
    hasher.write_u64(registry_fingerprint);
    hasher.write_usize(units.len());
    for unit in units {
        hasher.write_u64(unit.id().0);
        hasher.write_u64(unit.content_hash());
    }
    ContentManifestHash(hasher.finish())
}

/// Immutable set of loaded Sigil units plus the behavior registry they were validated against.
#[derive(Debug)]
pub struct SigilLibrary {
    units: Vec<SigilUnit>,
    registry: Arc<BehaviorRegistry>,
    epoch: ContentEpoch,
}

impl SigilLibrary {
    /// Builds a library from `units` (sorted internally by [`UnitId`]) and `registry`.
    ///
    /// # Errors
    ///
    /// - [`SigilError::DuplicateUnit`] if two units share an id.
    /// - [`SigilError::TooManyUnits`] if there are more than 65535 units.
    /// - [`SigilError::UnknownBehavior`] if a unit references a behavior id that `registry` does
    ///   not define.
    pub fn new(
        mut units: Vec<SigilUnit>,
        registry: Arc<BehaviorRegistry>,
    ) -> Result<Self, SigilError> {
        units.sort_by_key(SigilUnit::id);
        for pair in units.windows(2) {
            if pair[0].id() == pair[1].id() {
                return Err(SigilError::DuplicateUnit(pair[0].id()));
            }
        }
        if units.len() > usize::from(u16::MAX) {
            return Err(SigilError::TooManyUnits);
        }
        for unit in &units {
            for &behavior in unit.behavior_refs() {
                if registry.get(behavior).is_none() {
                    return Err(SigilError::UnknownBehavior {
                        unit: unit.id(),
                        behavior,
                    });
                }
            }
        }

        let registry_fingerprint = registry.fingerprint();
        let epoch = ContentEpoch::new(0, manifest_hash(registry_fingerprint, &units));

        Ok(Self {
            units,
            registry,
            epoch,
        })
    }

    /// Loaded units, ascending by [`UnitId`].
    #[must_use]
    pub fn units(&self) -> &[SigilUnit] {
        &self.units
    }

    /// Looks up a unit by id.
    #[must_use]
    pub fn unit(&self, id: UnitId) -> Option<&SigilUnit> {
        self.unit_index(id).map(|index| &self.units[index as usize])
    }

    /// Position of a unit inside [`SigilLibrary::units`], stable until the next swap.
    #[must_use]
    pub fn unit_index(&self, id: UnitId) -> Option<u16> {
        self.units
            .binary_search_by_key(&id, SigilUnit::id)
            .ok()
            // `SigilLibrary::new` rejects more than `u16::MAX` units, so every valid index fits.
            .map(|index| index as u16)
    }

    /// Current content epoch.
    #[must_use]
    pub const fn epoch(&self) -> ContentEpoch {
        self.epoch
    }

    /// Fingerprint of the behavior registry this library was built against.
    #[must_use]
    pub fn registry_fingerprint(&self) -> u64 {
        self.registry.fingerprint()
    }

    /// Behavior registry this library was built against.
    #[must_use]
    pub const fn registry(&self) -> &Arc<BehaviorRegistry> {
        &self.registry
    }
}

/// Installable content handle sharing an [`Arc<SigilLibrary>`].
///
/// A concrete, `Clone + StableHash` type by deliberate design (contract "Trait-Entscheid", §11):
/// trait objects would be neither hashable nor snapshottable.
#[derive(Debug, Clone)]
pub struct SigilContent {
    library: Arc<SigilLibrary>,
}

impl SigilContent {
    /// Builds content directly from a library, with a fresh epoch (`swaps = 0`).
    ///
    /// ADDITION/contract-change candidate (WP1.3): the contract's method list for `SigilContent`
    /// names only `library()`/`epoch()` — a constructor is not listed because the full contract
    /// expects `install()` (§11.6, out of scope here) to be the sole producer, wiring a library
    /// into a running `Simulation`. WP1.3 needs `BulletPool::spawn` testable against a
    /// `&SigilContent` without a `Simulation`, so this minimal, side-effect-free constructor is
    /// added. `install()` can wrap or replace it later without changing this type's shape.
    #[must_use]
    pub fn new(library: SigilLibrary) -> Self {
        Self {
            library: Arc::new(library),
        }
    }

    /// The wrapped library.
    #[must_use]
    pub fn library(&self) -> &SigilLibrary {
        &self.library
    }

    /// Current content epoch (forwarded from the wrapped library).
    #[must_use]
    pub fn epoch(&self) -> ContentEpoch {
        self.library.epoch()
    }
}

impl StableHash for SigilContent {
    /// Feeds only the epoch (`swaps`, `manifest_hash`), never unit bytes or addresses (contract
    /// §11.2): content identity flows through the manifest hash.
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.epoch().stable_hash(hasher);
    }
}

#[cfg(test)]
mod tests {
    use grimoire_core::hash_of;

    use super::*;
    use crate::behavior::{BehaviorId, BehaviorRegistryBuilder};
    use crate::test_support::{build_content, build_unit, empty_registry, plain_bullet_type};

    #[test]
    fn bullet_flags_contains() {
        let flags = BulletFlags(BulletFlags::SMASHABLE.0 | BulletFlags::GRAZEABLE.0);
        assert!(flags.contains(BulletFlags::SMASHABLE));
        assert!(flags.contains(BulletFlags::GRAZEABLE));
        assert!(!flags.contains(BulletFlags::REFLECTABLE));
    }

    #[test]
    fn library_sorts_units_by_id() {
        let bt = [plain_bullet_type()];
        let a = build_unit(5, &bt, 0);
        let b = build_unit(1, &bt, 0);
        let c = build_unit(3, &bt, 0);
        let library = SigilLibrary::new(vec![a, b, c], empty_registry()).expect("must build");
        let ids: Vec<UnitId> = library.units().iter().map(SigilUnit::id).collect();
        assert_eq!(ids, vec![UnitId(1), UnitId(3), UnitId(5)]);
        assert_eq!(library.unit_index(UnitId(3)), Some(1));
        assert_eq!(library.unit(UnitId(5)).map(SigilUnit::id), Some(UnitId(5)));
        assert_eq!(library.unit_index(UnitId(99)), None);
    }

    #[test]
    fn library_rejects_duplicate_unit_ids() {
        let bt = [plain_bullet_type()];
        let a = build_unit(1, &bt, 0);
        let b = build_unit(1, &bt, 0);
        assert_eq!(
            SigilLibrary::new(vec![a, b], empty_registry()).unwrap_err(),
            SigilError::DuplicateUnit(UnitId(1))
        );
    }

    #[test]
    fn library_rejects_too_many_units() {
        let bt = [plain_bullet_type()];
        let units: Vec<SigilUnit> = (1..=u32::from(u16::MAX) + 1)
            .map(|id| build_unit(u64::from(id), &bt, 0))
            .collect();
        assert_eq!(
            SigilLibrary::new(units, empty_registry()).unwrap_err(),
            SigilError::TooManyUnits
        );
    }

    #[test]
    fn library_rejects_unknown_behavior_reference() {
        let bytes = crate::test_support::build_unit_bytes(
            1,
            &[plain_bullet_type()],
            0,
            None,
            Some(&[BehaviorId(42)]),
        );
        let unit = SigilUnit::from_bytes(&bytes).expect("must decode");
        assert_eq!(
            SigilLibrary::new(vec![unit], empty_registry()).unwrap_err(),
            SigilError::UnknownBehavior {
                unit: UnitId(1),
                behavior: BehaviorId(42),
            }
        );
    }

    #[test]
    fn library_accepts_a_known_behavior_reference() {
        fn behavior(
            _input: &crate::BehaviorInput<'_>,
            _motion: &mut crate::BulletMotion,
            _rng: &mut grimoire_sim::SimRng,
        ) -> crate::BehaviorOutcome {
            crate::BehaviorOutcome::Keep
        }
        let mut builder = BehaviorRegistryBuilder::new(1);
        builder.register(BehaviorId(42), "noop", behavior).unwrap();
        let registry = builder.build();

        let bytes = crate::test_support::build_unit_bytes(
            1,
            &[plain_bullet_type()],
            0,
            None,
            Some(&[BehaviorId(42)]),
        );
        let unit = SigilUnit::from_bytes(&bytes).expect("must decode");
        assert!(SigilLibrary::new(vec![unit], registry).is_ok());
    }

    #[test]
    fn epoch_starts_at_zero_swaps_and_matches_the_manifest_formula() {
        let bt = [plain_bullet_type()];
        let unit = build_unit(7, &bt, 0);
        let registry = empty_registry();
        let registry_fingerprint = registry.fingerprint();
        let expected_manifest = manifest_hash(registry_fingerprint, std::slice::from_ref(&unit));
        let library = SigilLibrary::new(vec![unit], registry).expect("must build");
        assert_eq!(library.epoch().swaps, 0);
        assert_eq!(library.epoch().manifest_hash, expected_manifest);
        assert_eq!(library.registry_fingerprint(), registry_fingerprint);
    }

    #[test]
    fn content_hash_feeds_only_the_epoch() {
        let bt = [plain_bullet_type()];
        let unit_a = build_unit(1, &bt, 0);
        let unit_b = build_unit(1, &bt, 5); // different emitter_count -> different content_hash
        let content_a = build_content(vec![unit_a]);
        let content_b = build_content(vec![unit_b]);
        // The two units differ, but since `SigilContent::stable_hash` only feeds the epoch, and
        // the epoch's manifest hash is itself sensitive to each unit's `content_hash`, these two
        // contents are in fact expected to hash *differently* here. This test instead checks the
        // documented positive claim: hashing a content twice gives the same value, and cloning
        // shares state without changing the hash.
        assert_eq!(hash_of(&content_a), hash_of(&content_a.clone()));
        assert_ne!(hash_of(&content_a), hash_of(&content_b));
    }
}
