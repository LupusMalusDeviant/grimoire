//! Sigil → Render extraction adapter (contract §6 "Kennungen"/"Position", §9.1, plan 0002 WP5.3).
//!
//! `grimoire_sigil` and `grimoire_render` do not know each other (engine ADR-0008); this module is
//! where a live [`BulletPool`] becomes the renderer's bullet channel,
//! [`StageFrame::bullets`](grimoire_render::StageFrame::bullets):
//!
//! 1. **Pool.** Every live slot, in ascending slot order, read straight from the pool's
//!    structure-of-arrays columns ([`BulletPool::columns`]) — no per-bullet struct in between.
//! 2. **Visual ids.** The bullet's [`BulletType`] (unit and type index of the slot, looked up in
//!    the loaded [`SigilContent`]) carries a neutral [`BulletVisual`]; [`map_visual`] maps it onto
//!    the bullet pass's tables. In P1 that mapping is the identity with a range check
//!    ([`bullet_silhouette::COUNT`], [`bullet_palette::COUNT`], the three
//!    [`palette_space`] constants); a bullet whose visual does not map is counted in
//!    [`BulletExtractionStats::unmapped_visual`] and not handed to the renderer.
//! 3. **[`BulletInstance`].** Position interpolated between the previous and the current tick,
//!    `previous + (current - previous) * alpha`; `rotation` from the flight direction (the pool's
//!    heading column, kept in sync with the velocity by the interpreter without per-tick
//!    trigonometry, contract §11.6); `radius` from [`BulletType::radius`]; `palette_space`
//!    passed through, so the renderer's palette-space check (PRD-0003 rule 4) stays the one place
//!    that rejects foreign spaces.
//!
//! **Read-only and deterministic.** The adapter reads `&World` outside the ticks and never writes
//! simulation state (contract §9.1); nothing it computes flows back. It allocates nothing once the
//! output vector's capacity has grown (the main loop clears, never shrinks, the stage frame).
//!
//! **Cost.** One pass over the pool's slot range with a one-entry cache for the (unit, type)
//! lookup, because pattern content spawns bullets of the same type into neighbouring slots. The
//! plan's budget is 0.5 ms for 10,000 bullets; `grimoire_bench`'s `sigil_extract_10k` scenario
//! measures it on the CI runner.

use grimoire_core::math::dmath;
use grimoire_ecs::World;
use grimoire_render::{
    BulletInstance, StageFrame, bullet_palette, bullet_silhouette, palette_space,
};
use grimoire_sigil::{BulletPool, BulletType, BulletVisual, SigilContent};

use crate::GamePlugin;

/// [`GamePlugin::name`] of [`SigilRenderPlugin`].
pub const SIGIL_RENDER_PLUGIN_NAME: &str = "grimoire.sigil_render";

/// Counters of one extraction.
///
/// Output type only built by this module ([`extract_bullets`], [`extract_pool`]); growable
/// (contract §2 rule 13).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BulletExtractionStats {
    /// Bullets appended to the output as [`BulletInstance`]s.
    pub extracted: u32,
    /// Live bullets not handed to the renderer because their [`BulletVisual`] has no mapping
    /// ([`map_visual`]) or their unit or type is missing from the loaded content.
    pub unmapped_visual: u32,
}

/// The render-side ids of one [`BulletVisual`], as [`map_visual`] maps them.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MappedVisual {
    /// [`BulletInstance::silhouette`].
    pub silhouette: u16,
    /// [`BulletInstance::palette`].
    pub palette: u16,
    /// [`BulletInstance::palette_space`].
    pub palette_space: u8,
    /// [`BulletInstance::glow`].
    pub glow: u8,
}

/// Maps Sigil's neutral visual ids onto the bullet pass's tables (contract §6 "Kennungen").
///
/// P1: the identity, with a range check — `silhouette` below [`bullet_silhouette::COUNT`],
/// `palette` below [`bullet_palette::COUNT`], `palette_space` one of the [`palette_space`]
/// constants. `glow` maps one to one. `None` for anything outside those ranges.
///
/// The palette space is range-checked, not filtered: a bullet in a known but foreign space (for
/// example [`palette_space::FRIENDLY`]) maps and reaches the renderer, whose bullet pass rejects
/// and counts it (PRD-0003 rule 4 lives in one place).
#[must_use]
pub fn map_visual(visual: BulletVisual) -> Option<MappedVisual> {
    let known_space = matches!(
        visual.palette_space,
        palette_space::UNASSIGNED | palette_space::HOSTILE | palette_space::FRIENDLY
    );
    (visual.silhouette < bullet_silhouette::COUNT
        && visual.palette < bullet_palette::COUNT
        && known_space)
        .then_some(MappedVisual {
            silhouette: visual.silhouette,
            palette: visual.palette,
            palette_space: visual.palette_space,
            glow: visual.glow,
        })
}

/// Extracts every live bullet of `world`'s [`BulletPool`] into `out` (appending; see the module
/// documentation for the three steps). Returns zero counters and leaves `out` untouched if the
/// world has no pool or no [`SigilContent`] (Sigil not installed).
///
/// `alpha` is the main loop's interpolation factor in `[0, 1]` ([`GamePlugin::extract_stage`]);
/// it is clamped into that range, and a non-finite `alpha` shows the current tick.
pub fn extract_bullets(
    world: &World,
    alpha: f32,
    out: &mut Vec<BulletInstance>,
) -> BulletExtractionStats {
    match (
        world.resource::<BulletPool>(),
        world.resource::<SigilContent>(),
    ) {
        (Some(pool), Some(content)) => extract_pool(pool, content, alpha, out),
        _ => BulletExtractionStats::default(),
    }
}

/// [`extract_bullets`] for an explicit pool and content, without a [`World`] (tests, benches, and
/// tools that hold a pool outside a simulation).
pub fn extract_pool(
    pool: &BulletPool,
    content: &SigilContent,
    alpha: f32,
    out: &mut Vec<BulletInstance>,
) -> BulletExtractionStats {
    let alpha = if alpha.is_finite() {
        dmath::max(0.0, dmath::min(alpha, 1.0))
    } else {
        1.0
    };
    let units = content.library().units();
    let columns = pool.columns();
    out.reserve(pool.len() as usize);

    let mut stats = BulletExtractionStats::default();
    // One-entry cache: `(unit, type)` of the previous live bullet and what it mapped to.
    let mut cached_key: Option<(u16, u16)> = None;
    let mut cached: Option<(f32, MappedVisual)> = None;
    let slots = columns
        .alive
        .iter()
        .zip(columns.unit)
        .zip(columns.bullet_type)
        .zip(columns.position)
        .zip(columns.previous_position)
        .zip(columns.angle);
    for (((((&alive, &unit), &bullet_type), &position), &previous), &angle) in slots {
        if !alive {
            continue;
        }
        if cached_key != Some((unit, bullet_type)) {
            cached_key = Some((unit, bullet_type));
            cached = units
                .get(usize::from(unit))
                .and_then(|unit| unit.bullet_types().get(usize::from(bullet_type)))
                .and_then(|bullet_type: &BulletType| {
                    map_visual(bullet_type.visual).map(|visual| (bullet_type.radius, visual))
                });
        }
        let Some((radius, visual)) = cached else {
            stats.unmapped_visual += 1;
            continue;
        };
        let interpolated = previous + (position - previous) * alpha;
        out.push(BulletInstance {
            position: interpolated.to_array(),
            radius,
            rotation: angle,
            silhouette: visual.silhouette,
            palette: visual.palette,
            palette_space: visual.palette_space,
            glow: visual.glow,
            flags: 0,
        });
        stats.extracted += 1;
    }
    stats
}

/// Plugin that fills [`StageFrame::bullets`] from the simulation's [`BulletPool`] every frame
/// ([`extract_bullets`] from [`GamePlugin::extract_stage`]). Register it after the plugin that
/// installs Sigil; it adds nothing to the simulation (no systems, resources or entities), so
/// registering it changes no state hash.
#[derive(Debug, Clone, Default)]
pub struct SigilRenderPlugin {
    last: BulletExtractionStats,
}

impl SigilRenderPlugin {
    /// A plugin with zeroed counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counters of the most recent extraction.
    #[must_use]
    pub fn last_stats(&self) -> BulletExtractionStats {
        self.last
    }
}

impl GamePlugin for SigilRenderPlugin {
    fn name(&self) -> &str {
        SIGIL_RENDER_PLUGIN_NAME
    }

    fn extract_stage(&mut self, world: &World, alpha: f32, stage: &mut StageFrame) {
        self.last = extract_bullets(world, alpha, &mut stage.bullets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visual(silhouette: u16, palette: u16, palette_space: u8) -> BulletVisual {
        BulletVisual {
            silhouette,
            palette,
            palette_space,
            glow: 77,
        }
    }

    #[test]
    fn map_visual_is_the_identity_inside_the_tables() {
        let mapped = map_visual(visual(
            bullet_silhouette::DIAMOND,
            bullet_palette::POISON_LIME,
            palette_space::HOSTILE,
        ))
        .expect("in range");
        assert_eq!(mapped.silhouette, bullet_silhouette::DIAMOND);
        assert_eq!(mapped.palette, bullet_palette::POISON_LIME);
        assert_eq!(mapped.palette_space, palette_space::HOSTILE);
        assert_eq!(mapped.glow, 77);
    }

    #[test]
    fn map_visual_rejects_ids_outside_the_tables() {
        assert_eq!(
            map_visual(visual(bullet_silhouette::COUNT, 0, palette_space::HOSTILE)),
            None
        );
        assert_eq!(
            map_visual(visual(0, bullet_palette::COUNT, palette_space::HOSTILE)),
            None
        );
        assert_eq!(map_visual(visual(0, 0, 3)), None, "unknown palette space");
    }

    #[test]
    fn map_visual_passes_known_foreign_palette_spaces_on_to_the_renderer() {
        // Rule 4 is the renderer's check, not the adapter's (module docs).
        assert!(map_visual(visual(0, 0, palette_space::FRIENDLY)).is_some());
        assert!(map_visual(visual(0, 0, palette_space::UNASSIGNED)).is_some());
    }

    #[test]
    fn extract_bullets_without_sigil_extracts_nothing() {
        let world = World::new();
        let mut out = vec![BulletInstance::default()];
        let stats = extract_bullets(&world, 0.5, &mut out);
        assert_eq!(stats, BulletExtractionStats::default());
        assert_eq!(out.len(), 1, "appends only, never clears");
    }
}
