//! # grimoire_sigil
//!
//! Sigil (`.sigil`), the declarative bullet-pattern language of Grimoire: the compact binary unit
//! format and the deterministic runtime that executes it (PRD-0004, contract §11).
//!
//! **Status (WP5.5):** the binary unit format [`SigilUnit`] (WP4.2, with the `Transforms` section
//! since WP5.2), the content/pool/emitter/behavior data types (WP1.3) and [`install`] with the
//! five `sigil.*` tick-phase systems (`crate::systems`) run compiled patterns end to end: the
//! seven placement blocks and six stackable modifiers (`crate::blocks`, `crate::runtime`), each
//! bullet type's bound `BulletBehavior`, and the transforms `reverse`, `change_type`, `burst` and
//! `become_emitter` with `time`, `distance` and `event` ([`EventRequest`]) triggers under the
//! cascade cap. Since WP5.4 spawns and despawns allocate nothing and the budget benches live in
//! `grimoire_bench`; since WP5.5 [`replace_unit`] swaps a unit at a tick boundary and
//! [`restore_checked`] rejects snapshots of a foreign content epoch (§11.8).

mod behavior;
mod blocks;
mod content;
mod emitter;
mod error;
mod pool;
mod runtime;
mod swap;
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
pub use emitter::{AimTarget, ClearFilter, ClearRequest, Emitter, EventId, EventRequest};
pub use error::SigilError;
pub use pool::{
    BulletColumns, BulletEvent, BulletId, BulletPool, BulletRef, BulletSpawn, DespawnCause,
    PoolBlock,
};
pub use swap::{SwapReport, replace_unit, restore_checked};
pub use systems::{POOL_BLOCK_SIZE, SigilConfig, install, stream, system_names};
pub use unit::{SigilUnit, UnitError, UnitId};
