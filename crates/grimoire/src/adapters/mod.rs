//! Facade adapters between subsystem crates that do not know each other (contract §9.1).
//!
//! `grimoire_sigil`, `grimoire_collide`, `grimoire_render`, `grimoire_assets` and `grimoire_debug`
//! have no edges to one another (engine ADR-0008); every mapping between them lives here, in the
//! facade that depends on all of them.
//!
//! | Module | Direction | Status |
//! |--------|-----------|--------|
//! | [`sigil_collide`] | Sigil → Collision | [`sigil_collide::GrazeProbe`] and [`sigil_collide::GrazeHits`] ship with WP1.3 because the player proxy (§9.5) writes `GrazeProbe`; the broadphase/graze systems and `SigilCollideConfig`/`SigilCollidePlugin` follow in WP11.2 |
//! | [`sigil_render`] | Sigil → Render | WP5.3: [`sigil_render::extract_bullets`] turns the live `BulletPool` into interpolated `BulletInstance`s for the WP3.5 bullet pass; [`sigil_render::SigilRenderPlugin`] runs it every frame |
//! | [`assets`] | Assets → Sigil | WP8.3: [`assets::sigil_library`] turns the Sigil entries of an `AssetSource` (a pack v1 file, a `MemorySource`) into the `SigilLibrary` for `grimoire_sigil::install`, checking kind version and unit id |
//! | [`figure_assets`] | Assets → Render | P1 "Figuren in der Engine" package: [`figure_assets::load_figure`] decodes a figure's pack payloads and registers them with a `WgpuRenderer`; posing/animation stays out of scope |
//! | [`debug`] | Debug ↔ Sim/Render | WP6.3: [`debug::Profiler`] measures the main loop and, through [`debug::ProfilerObserver`], every system under its subsystem scope, with budgets and the renderer's GPU time; the debug link (swap queue, `Stats` messages) follows with WP8.4 |

pub mod assets;
pub mod debug;
pub mod figure_assets;
pub mod sigil_collide;
pub mod sigil_render;
