//! `BulletBehavior` registry: plain function pointers, registered once and never mutated
//! (contract §11.5). Execution (calling a [`BehaviorFn`] from a tick-phase system) is out of scope
//! for WP1.3; this module only defines the data types and the registry.

use std::collections::BTreeMap;
use std::sync::Arc;

use grimoire_core::{StableHash, StableHasher, Vec2};
use grimoire_sim::SimRng;

use crate::error::SigilError;

/// Stable identifier of a registered [`BehaviorFn`].
///
/// IDs are permanent constants chosen by whoever registers a behavior; `0x8000_0000..=u32::MAX` is
/// reserved for future engine-provided behaviors (none exist in P1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BehaviorId(pub u32);

impl StableHash for BehaviorId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(self.0);
    }
}

/// Read-only input to a [`BehaviorFn`] call for one bullet at one tick.
#[derive(Debug, Clone, Copy)]
pub struct BehaviorInput<'a> {
    /// Current simulation tick.
    pub tick: u64,
    /// Number of ticks since the bullet was spawned.
    pub age: u32,
    /// Constant parameters from the behavior reference in the unit.
    pub params: &'a [f32],
    /// Current aim target, if any.
    pub target: Option<Vec2>,
}

/// Mutable per-bullet motion state a [`BehaviorFn`] reads and updates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BulletMotion {
    /// Current position.
    pub position: Vec2,
    /// Current velocity.
    pub velocity: Vec2,
    /// Current heading, in radians.
    pub angle: f32,
    /// Current speed.
    pub speed: f32,
    /// Free-form per-bullet state a behavior may use across ticks.
    pub state: [f32; 4],
}

/// Result of one [`BehaviorFn`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehaviorOutcome {
    /// The bullet stays alive.
    Keep,
    /// The bullet is despawned with [`crate::DespawnCause::Behavior`].
    Despawn,
}

/// A pure per-bullet behaviour function.
///
/// A plain function pointer, never a closure, so it cannot capture mutable state: every bullet's
/// state lives in [`BulletMotion::state`] in the pool. `rng` is the caller's block generator,
/// advanced in slot order (contract §11.5/§11.7).
pub type BehaviorFn = fn(&BehaviorInput<'_>, &mut BulletMotion, &mut SimRng) -> BehaviorOutcome;

/// Builder for a [`BehaviorRegistry`].
///
/// Kept a plain ordered map (`BTreeMap`, never a hash map: contract §3 determinism rules) so
/// registration order never affects the built registry's contents or fingerprint.
#[derive(Debug)]
pub struct BehaviorRegistryBuilder {
    version: u32,
    entries: BTreeMap<BehaviorId, (&'static str, BehaviorFn)>,
}

impl BehaviorRegistryBuilder {
    /// Starts a builder for registry format `version`.
    #[must_use]
    pub fn new(version: u32) -> Self {
        Self {
            version,
            entries: BTreeMap::new(),
        }
    }

    /// Registers one behavior under `id`.
    ///
    /// # Errors
    ///
    /// Returns [`SigilError::DuplicateBehavior`] if `id` was already registered.
    pub fn register(
        &mut self,
        id: BehaviorId,
        name: &'static str,
        f: BehaviorFn,
    ) -> Result<&mut Self, SigilError> {
        if self.entries.contains_key(&id) {
            return Err(SigilError::DuplicateBehavior(id));
        }
        self.entries.insert(id, (name, f));
        Ok(self)
    }

    /// Finalises the registry. After this, it is immutable and shared by `Arc`.
    #[must_use]
    pub fn build(self) -> Arc<BehaviorRegistry> {
        Arc::new(BehaviorRegistry {
            version: self.version,
            entries: self.entries,
        })
    }
}

/// Immutable set of registered behaviors, shared by every [`crate::SigilLibrary`] built against it.
#[derive(Debug)]
pub struct BehaviorRegistry {
    version: u32,
    entries: BTreeMap<BehaviorId, (&'static str, BehaviorFn)>,
}

impl BehaviorRegistry {
    /// Registry format version, as given to [`BehaviorRegistryBuilder::new`].
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Number of registered behaviors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry has no registered behaviors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Registered ids, ascending.
    pub fn ids(&self) -> impl Iterator<Item = BehaviorId> + '_ {
        self.entries.keys().copied()
    }

    /// Diagnostic name a behavior was registered under.
    #[must_use]
    pub fn name(&self, id: BehaviorId) -> Option<&'static str> {
        self.entries.get(&id).map(|&(name, _)| name)
    }

    /// The function pointer registered under `id`.
    #[must_use]
    pub fn get(&self, id: BehaviorId) -> Option<BehaviorFn> {
        self.entries.get(&id).map(|&(_, f)| f)
    }

    /// `StableHasher` fingerprint over `version`, the entry count, then each id and name in
    /// ascending id order — never over function addresses, so two registries built with the same
    /// entries in different `register` order produce the same fingerprint (contract §11.5).
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = StableHasher::new();
        hasher.write_u32(self.version);
        hasher.write_usize(self.entries.len());
        for (id, &(name, _)) in &self.entries {
            hasher.write_u32(id.0);
            hasher.write_str(name);
        }
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep(
        _input: &BehaviorInput<'_>,
        _motion: &mut BulletMotion,
        _rng: &mut SimRng,
    ) -> BehaviorOutcome {
        BehaviorOutcome::Keep
    }

    fn despawn(
        _input: &BehaviorInput<'_>,
        _motion: &mut BulletMotion,
        _rng: &mut SimRng,
    ) -> BehaviorOutcome {
        BehaviorOutcome::Despawn
    }

    #[test]
    fn register_rejects_duplicate_ids() {
        let mut builder = BehaviorRegistryBuilder::new(1);
        builder.register(BehaviorId(1), "keep", keep).unwrap();
        assert_eq!(
            builder
                .register(BehaviorId(1), "despawn", despawn)
                .unwrap_err(),
            SigilError::DuplicateBehavior(BehaviorId(1))
        );
    }

    #[test]
    fn registry_exposes_registered_entries() {
        let mut builder = BehaviorRegistryBuilder::new(3);
        builder.register(BehaviorId(2), "despawn", despawn).unwrap();
        builder.register(BehaviorId(1), "keep", keep).unwrap();
        let registry = builder.build();

        assert_eq!(registry.version(), 3);
        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
        assert_eq!(
            registry.ids().collect::<Vec<_>>(),
            vec![BehaviorId(1), BehaviorId(2)]
        );
        assert_eq!(registry.name(BehaviorId(1)), Some("keep"));
        assert_eq!(registry.name(BehaviorId(99)), None);
        assert!(registry.get(BehaviorId(2)).is_some());
        assert!(registry.get(BehaviorId(99)).is_none());
    }

    #[test]
    fn empty_registry_reports_empty() {
        let registry = BehaviorRegistryBuilder::new(1).build();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.ids().count(), 0);
    }

    #[test]
    fn fingerprint_is_independent_of_registration_order() {
        let mut a = BehaviorRegistryBuilder::new(1);
        a.register(BehaviorId(1), "keep", keep).unwrap();
        a.register(BehaviorId(2), "despawn", despawn).unwrap();

        let mut b = BehaviorRegistryBuilder::new(1);
        b.register(BehaviorId(2), "despawn", despawn).unwrap();
        b.register(BehaviorId(1), "keep", keep).unwrap();

        assert_eq!(a.build().fingerprint(), b.build().fingerprint());
    }

    #[test]
    fn fingerprint_differs_when_contents_differ() {
        let mut a = BehaviorRegistryBuilder::new(1);
        a.register(BehaviorId(1), "keep", keep).unwrap();
        let mut b = BehaviorRegistryBuilder::new(1);
        b.register(BehaviorId(1), "despawn", despawn).unwrap();

        assert_ne!(a.build().fingerprint(), b.build().fingerprint());
    }
}
