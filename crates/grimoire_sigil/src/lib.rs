//! # grimoire_sigil
//!
//! Sigil (`.sigil`), the declarative bullet-pattern language of Grimoire: the compact binary unit
//! format and the deterministic runtime that executes it (PRD-0004, contract §11).
//!
//! **Status (WP5.1):** the binary header/section-table layer of [`SigilUnit`] (WP4.2) and the
//! content/pool/emitter/behavior data types (WP1.3) are joined by [`install`] and the five
//! `sigil.*` tick-phase systems (`crate::systems`), which actually run compiled patterns: the
//! seven placement blocks and six stackable modifiers (`crate::blocks`, `crate::runtime`) against
//! a [`BulletPool`] taken as an ECS resource. Sub-spawns, per-bullet-type transforms
//! (`Transforms`/§10.4's `change_type`/`become_emitter`/`burst`/`reverse`) and calling a
//! `BulletBehavior` from a tick phase are not: `docs/formats/sigil.md` §11.4's open points 1 and 4
//! record that nothing in the wire format yet ties a bullet type to either, so this work package
//! stops at the boundary the format already draws. Hot-swap (§11.8) and the leader performance
//! target (10k active bullets, §11.6) are separate, later work packages.

mod behavior;
mod blocks;
mod content;
mod emitter;
mod error;
mod pool;
mod runtime;
mod systems;
mod unit;

#[cfg(test)]
mod test_support;

pub use behavior::{
    BehaviorFn, BehaviorId, BehaviorInput, BehaviorOutcome, BehaviorRegistry,
    BehaviorRegistryBuilder, BulletMotion,
};
pub use content::{
    BulletFlags, BulletType, BulletVisual, ContentEpoch, SigilContent, SigilLibrary,
};
pub use emitter::{AimTarget, ClearFilter, ClearRequest, Emitter};
pub use error::SigilError;
pub use pool::{
    BulletColumns, BulletEvent, BulletId, BulletPool, BulletRef, BulletSpawn, DespawnCause,
    PoolBlock,
};
pub use systems::{POOL_BLOCK_SIZE, SigilConfig, install, stream, system_names};
pub use unit::{SigilUnit, UnitError, UnitId};
