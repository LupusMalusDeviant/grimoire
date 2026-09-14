//! # grimoire
//!
//! Facade of the Grimoire engine. Games depend on this crate only: it re-exports the public
//! contracts of the engine crates and owns the application lifecycle (`App`, `GamePlugin`,
//! fixed-timestep main loop).
//!
//! **Status:** re-exports only; the app lifecycle is integrated after the P0 subsystem streams.

pub use grimoire_core as core;
pub use grimoire_ecs as ecs;
pub use grimoire_platform as platform;
pub use grimoire_render as render;
pub use grimoire_sim as sim;
