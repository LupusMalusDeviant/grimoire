//! Facade adapters between subsystem crates that do not know each other (contract §9.1).
//!
//! `grimoire_sigil`, `grimoire_collide`, `grimoire_render`, `grimoire_assets` and `grimoire_debug`
//! have no edges to one another (engine ADR-0008); every mapping between them lives here, in the
//! facade that depends on all of them.
//!
//! | Module | Direction | Status |
//! |--------|-----------|--------|
//! | [`sigil_collide`] | Sigil → Collision | [`sigil_collide::GrazeProbe`] and [`sigil_collide::GrazeHits`] ship with WP1.3 because the player proxy (§9.5) writes `GrazeProbe`; the broadphase/graze systems and `SigilCollideConfig`/`SigilCollidePlugin` follow in WP11.2 |
//! | `sigil_render` | Sigil → Render | WP5.3 |
//! | `assets` | Assets → Sigil | §11.2, §12, once `grimoire_sigil::install` lands |
//! | `debug` | Debug ↔ Sim/Render | §9.7, §13, WP8.2 |

pub mod sigil_collide;
