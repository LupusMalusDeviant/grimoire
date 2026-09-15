//! # grimoire_sigil
//!
//! Sigil (`.sigil`), the declarative bullet-pattern language of Grimoire: the compact binary unit
//! format and the deterministic runtime data types (PRD-0004, contract §11).
//!
//! **Status (WP1.3):** the binary header/section-table layer of [`SigilUnit`], the content types
//! ([`BulletType`], [`SigilLibrary`], [`SigilContent`]), [`BulletPool`] (implemented and fully
//! behaviour-tested), the emitter/aim/clear data types and the [`BehaviorRegistry`] are
//! implemented. Installing content into a `Simulation`, the five per-tick systems
//! (`sigil.begin`/`update`/`resolve`/`emit`/`clear`), hot-swap and Sigil pattern-language execution
//! are later work (WP4.x/WP5.x) and are not part of this crate yet.
//!
//! Several places in this crate invent a minimal, explicitly labelled *provisional* encoding for
//! section content whose real format is deferred to `docs/formats/sigil.md` (WP4.1); each is
//! marked with a `PROVISIONAL` doc comment in the `unit` module.

mod behavior;
mod content;
mod emitter;
mod error;
mod pool;
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
pub use unit::{SigilUnit, UnitError, UnitId};
