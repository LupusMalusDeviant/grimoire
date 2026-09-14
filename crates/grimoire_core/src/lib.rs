//! # grimoire_core
//!
//! Leaf crate of the Grimoire engine: deterministic, platform-independent primitives shared by
//! every simulation-side crate (`grimoire_ecs`, `grimoire_sim`, `grimoire_collide`, ...).
//!
//! - [`hash`]: [`StableHasher`] and the [`StableHash`] trait for golden masters and replay checks.
//! - [`math`]: floating-point rules for simulation code, pure-Rust [`math::dmath`] and [`Vec2`].
//!
//! Layer rule: this crate must not depend on any other `grimoire_*` crate.

pub mod hash;
pub mod math;

pub use hash::{StableHash, StableHasher, hash_of};
pub use math::Vec2;
