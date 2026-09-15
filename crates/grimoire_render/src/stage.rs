//! Stage frame, layer order and the bullet channel (contract §6, "Bühnen-Frame,
//! Ebenenreihenfolge und Bullet-Kanal", P1 addition).
//!
//! This module is the single owner of [`BulletInstance`], the palette-space constants, the
//! [`RenderLayer`] order and the extension path of [`StageFrame`]. It is purely additive: the P0
//! types [`RenderFrame`], [`RenderStats`] and [`SpriteInstance`] are reused unchanged.

use crate::{RenderFrame, RenderStats, SpriteInstance};

mod bullet_instance {
    // bytemuck's derive macros expand to `unsafe impl` blocks.
    #![allow(unsafe_code)]

    /// One instanced bullet for the bullet pass. Layout is `#[repr(C)]`, 24 bytes, no padding,
    /// uploaded verbatim (field byte offsets 0, 8, 12, 16, 18, 20, 21, 22).
    ///
    /// The layout is provisional until the OF-3.3 ADR (WP3.2); after that it changes only through
    /// the contract-change protocol (§2b) and never after the render extraction of the sigil
    /// runtime starts (WP5.3).
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct BulletInstance {
        /// Already-interpolated centre on the play plane, in world units. The renderer never
        /// interpolates this itself; the caller (the facade) has already applied
        /// `previous + (current - previous) * alpha`.
        pub position: [f32; 2],
        /// Visible radius, in world units.
        pub radius: f32,
        /// Counter-clockwise rotation in radians; `0` points along `+X`.
        pub rotation: f32,
        /// Index into the bullet pass's silhouette table. P1 has no silhouette table yet, so no
        /// range is enforced.
        pub silhouette: u16,
        /// Index into the palette of `palette_space`. P1 has no palette table yet, so no range is
        /// enforced.
        pub palette: u16,
        /// One of the constants in [`crate::palette_space`]. The bullet pass only draws instances
        /// with [`crate::BULLET_PASS_PALETTE_SPACE`]; every other value is rejected.
        pub palette_space: u8,
        /// Glow intensity, linear: `0` is no glow, `255` is full glow. Drawn by the bullet pass
        /// itself, never by a post-processing effect.
        pub glow: u8,
        /// Reserved for future use; always `0` in P1.
        pub flags: u16,
    }
}

pub use bullet_instance::BulletInstance;

/// Named identifiers for [`BulletInstance::palette_space`].
///
/// A palette space groups the colour tables of one part of the game (enemy projectiles vs.
/// player projectiles); the bullet pass accepts only [`HOSTILE`](palette_space::HOSTILE).
pub mod palette_space {
    /// No palette space has been assigned yet.
    pub const UNASSIGNED: u8 = 0;
    /// Enemy projectiles: the only palette space the bullet pass accepts.
    pub const HOSTILE: u8 = 1;
    /// Player projectiles, drawn through the sprite or (from WP2.2) mesh channels instead of the
    /// bullet pass.
    pub const FRIENDLY: u8 = 2;
}

/// The only [`palette_space`] value the bullet pass accepts. Every [`BulletInstance`] with a
/// different `palette_space` is rejected and counted in
/// [`StageStats::bullets_rejected_palette_space`].
pub const BULLET_PASS_PALETTE_SPACE: u8 = palette_space::HOSTILE;

/// Fixed draw order of the stage renderer (contract §6). Not configurable: [`RenderLayer::ORDER`]
/// lists layers earliest first, later layers are drawn on top of earlier ones, and within one
/// layer's channel, later list entries are drawn on top. No effect after
/// [`RenderLayer::PostFxResolve`] may tint, blur or occlude [`RenderLayer::Telegraphy`] or
/// [`RenderLayer::Bullets`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RenderLayer {
    /// Layers 1-3: [`StageFrame::base`] sprites (from WP2.2 also meshes).
    World,
    /// Post-processing input layer; empty in P1.
    Vfx,
    /// Post-processing resolve; empty in P1.
    PostFxResolve,
    /// Layer 4, reserved for telegraphed enemy attacks; P1 has no channel for it.
    Telegraphy,
    /// Layer 6: [`StageFrame::bullets`].
    Bullets,
    /// Layer 7: [`StageFrame::marker_sprites`] (player marker, nearby HUD rings).
    PlayerMarker,
    /// Debug and UI overlays, for example [`StageFrame::debug_sprites`].
    DebugUi,
}

impl RenderLayer {
    /// The fixed draw order, earliest first.
    pub const ORDER: [RenderLayer; 7] = [
        RenderLayer::World,
        RenderLayer::Vfx,
        RenderLayer::PostFxResolve,
        RenderLayer::Telegraphy,
        RenderLayer::Bullets,
        RenderLayer::PlayerMarker,
        RenderLayer::DebugUi,
    ];
}

/// Everything the stage renderer needs for one frame, in addition to the plain [`RenderFrame`]
/// that P0 renderers already understand.
///
/// Later work packages grow this type with further channels (WP2.2 adds camera, mesh and light
/// channels) without breaking existing callers, which is why it is `#[non_exhaustive]` and offers
/// [`StageFrame::new`] instead of requiring a struct literal.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StageFrame {
    /// Clear colour, camera and world sprites, drawn on [`RenderLayer::World`]. P0 semantics are
    /// unchanged: a renderer that only implements [`crate::Renderer::render`] draws exactly this
    /// part.
    pub base: RenderFrame,
    /// Enemy bullet instances, drawn on [`RenderLayer::Bullets`] (layer 6).
    pub bullets: Vec<BulletInstance>,
    /// Sprites drawn on [`RenderLayer::PlayerMarker`] (layer 7): the player marker and nearby HUD
    /// rings.
    pub marker_sprites: Vec<SpriteInstance>,
    /// Sprites drawn on [`RenderLayer::DebugUi`], for example a stats overlay (a later work
    /// package).
    pub debug_sprites: Vec<SpriteInstance>,
}

impl StageFrame {
    /// Creates an empty stage frame. Identical to [`StageFrame::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes all instances from every channel but keeps their allocations, the camera and the
    /// clear colour.
    pub fn clear(&mut self) {
        self.base.clear();
        self.bullets.clear();
        self.marker_sprites.clear();
        self.debug_sprites.clear();
    }
}

/// Measurements of one [`crate::Renderer::render_stage`] call.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageStats {
    /// Base render statistics: `sprites_drawn` counts every sprite channel (world, marker and
    /// debug sprites combined) and `draw_calls` counts every pass.
    pub base: RenderStats,
    /// Number of bullet instances accepted by the bullet pass.
    pub bullets_drawn: u32,
    /// Number of bullet instances rejected because their `palette_space` was not
    /// [`BULLET_PASS_PALETTE_SPACE`].
    pub bullets_rejected_palette_space: u32,
    /// Number of bullet instances rejected because they were structurally invalid: non-finite
    /// `position`, `radius` or `rotation`, or `radius <= 0`.
    pub bullets_rejected_invalid: u32,
}

/// Outcome of classifying a [`StageFrame::bullets`] channel against the bullet pass rules.
struct BulletExtraction {
    drawn: u32,
    rejected_palette_space: u32,
    rejected_invalid: u32,
}

/// Combines a `base` [`RenderStats`] — from drawing `frame.base` through
/// [`crate::Renderer::render`] — with the extraction and counting of `frame`'s bullet, marker and
/// debug channels into a full [`StageStats`].
///
/// Shared by [`crate::NullRenderer::render_stage`] and [`crate::WgpuRenderer::render_stage`] so
/// both apply contract §6 identically by construction, whether or not the renderer actually
/// rasterises the additional channels yet: `sprites_drawn` counts every sprite channel (world,
/// marker and debug sprites), `draw_calls` stays whatever `render` reported (no additional pass
/// exists yet in P1), and the bullet counters come from [`extract_bullets`].
pub(crate) fn stage_stats_from_base(base: RenderStats, frame: &StageFrame) -> StageStats {
    let bullets = extract_bullets(&frame.bullets);
    let sprite_channels = frame.marker_sprites.len() + frame.debug_sprites.len();
    StageStats {
        base: RenderStats {
            sprites_drawn: base.sprites_drawn + u32::try_from(sprite_channels).unwrap_or(u32::MAX),
            draw_calls: base.draw_calls,
            cpu_time: base.cpu_time,
        },
        bullets_drawn: bullets.drawn,
        bullets_rejected_palette_space: bullets.rejected_palette_space,
        bullets_rejected_invalid: bullets.rejected_invalid,
    }
}

/// Applies the bullet pass's palette-space and validity rules (contract §6) to `bullets`; used by
/// [`stage_stats_from_base`].
///
/// The palette-space check runs first: an instance in a foreign palette space is rejected (and,
/// in debug builds, additionally triggers a `debug_assert!` — the one documented exception to
/// "null implementations never panic", contract §2a) without being checked for validity. Only
/// instances in the accepted palette space are checked for finiteness and a positive radius.
fn extract_bullets(bullets: &[BulletInstance]) -> BulletExtraction {
    let mut drawn = 0u32;
    let mut rejected_palette_space = 0u32;
    let mut rejected_invalid = 0u32;
    for (index, bullet) in bullets.iter().enumerate() {
        if bullet.palette_space != BULLET_PASS_PALETTE_SPACE {
            debug_assert!(
                false,
                "bullet instance {index} uses palette space {}; the bullet pass accepts only palette space 1",
                bullet.palette_space
            );
            rejected_palette_space += 1;
            continue;
        }
        let valid = bullet.position[0].is_finite()
            && bullet.position[1].is_finite()
            && bullet.radius.is_finite()
            && bullet.rotation.is_finite()
            && bullet.radius > 0.0;
        if valid {
            drawn += 1;
        } else {
            rejected_invalid += 1;
        }
    }
    BulletExtraction {
        drawn,
        rejected_palette_space,
        rejected_invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullet_instance_layout() {
        assert_eq!(std::mem::size_of::<BulletInstance>(), 24);
        assert_eq!(std::mem::offset_of!(BulletInstance, position), 0);
        assert_eq!(std::mem::offset_of!(BulletInstance, radius), 8);
        assert_eq!(std::mem::offset_of!(BulletInstance, rotation), 12);
        assert_eq!(std::mem::offset_of!(BulletInstance, silhouette), 16);
        assert_eq!(std::mem::offset_of!(BulletInstance, palette), 18);
        assert_eq!(std::mem::offset_of!(BulletInstance, palette_space), 20);
        assert_eq!(std::mem::offset_of!(BulletInstance, glow), 21);
        assert_eq!(std::mem::offset_of!(BulletInstance, flags), 22);
    }

    #[test]
    fn render_layer_order_matches_declaration() {
        use RenderLayer::{Bullets, DebugUi, PlayerMarker, PostFxResolve, Telegraphy, Vfx, World};
        assert_eq!(
            RenderLayer::ORDER,
            [
                World,
                Vfx,
                PostFxResolve,
                Telegraphy,
                Bullets,
                PlayerMarker,
                DebugUi
            ]
        );
        for pair in RenderLayer::ORDER.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{pair:?} must be strictly increasing (= draw order, later on top)"
            );
        }
    }

    #[test]
    fn stage_frame_new_is_default() {
        assert_eq!(StageFrame::new(), StageFrame::default());
    }

    #[test]
    fn stage_frame_clear_empties_channels_but_keeps_camera_and_clear_color() {
        let mut frame = StageFrame::new();
        frame.base.sprites.push(SpriteInstance::default());
        frame.bullets.push(BulletInstance::default());
        frame.marker_sprites.push(SpriteInstance::default());
        frame.debug_sprites.push(SpriteInstance::default());
        frame.base.camera.world_height = 42.0;
        frame.base.clear_color = [0.1, 0.2, 0.3, 1.0];

        frame.clear();

        assert!(frame.base.sprites.is_empty());
        assert!(frame.bullets.is_empty());
        assert!(frame.marker_sprites.is_empty());
        assert!(frame.debug_sprites.is_empty());
        assert!((frame.base.camera.world_height - 42.0).abs() < f32::EPSILON);
        assert_eq!(frame.base.clear_color, [0.1, 0.2, 0.3, 1.0]);
    }

    fn valid_bullet() -> BulletInstance {
        BulletInstance {
            position: [1.0, 2.0],
            radius: 3.0,
            rotation: 0.5,
            silhouette: 0,
            palette: 0,
            palette_space: BULLET_PASS_PALETTE_SPACE,
            glow: 0,
            flags: 0,
        }
    }

    #[test]
    fn extract_bullets_counts_valid_instances() {
        let bullets = vec![valid_bullet(), valid_bullet()];
        let result = extract_bullets(&bullets);
        assert_eq!(result.drawn, 2);
        assert_eq!(result.rejected_palette_space, 0);
        assert_eq!(result.rejected_invalid, 0);
    }

    #[test]
    fn extract_bullets_rejects_invalid_instances_without_panic() {
        let bullets = vec![
            BulletInstance {
                radius: 0.0,
                ..valid_bullet()
            },
            BulletInstance {
                radius: f32::NAN,
                ..valid_bullet()
            },
            BulletInstance {
                rotation: f32::INFINITY,
                ..valid_bullet()
            },
            BulletInstance {
                position: [f32::NAN, 0.0],
                ..valid_bullet()
            },
        ];
        let result = extract_bullets(&bullets);
        assert_eq!(result.drawn, 0);
        assert_eq!(result.rejected_invalid, 4);
        assert_eq!(result.rejected_palette_space, 0);
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "the bullet pass accepts only palette space 1")
    )]
    fn extract_bullets_rejects_foreign_palette_space() {
        let bullets = vec![BulletInstance {
            palette_space: palette_space::FRIENDLY,
            ..valid_bullet()
        }];
        let result = extract_bullets(&bullets);
        assert_eq!(result.drawn, 0);
        assert_eq!(result.rejected_palette_space, 1);
        assert_eq!(result.rejected_invalid, 0);
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "the bullet pass accepts only palette space 1")
    )]
    fn extract_bullets_checks_palette_space_before_validity() {
        // Invalid AND wrong palette space: the palette-space rejection (with its debug_assert)
        // takes precedence, so this is never double-counted.
        let bullets = vec![BulletInstance {
            radius: -1.0,
            palette_space: palette_space::UNASSIGNED,
            ..valid_bullet()
        }];
        let result = extract_bullets(&bullets);
        assert_eq!(result.rejected_palette_space, 1);
        assert_eq!(result.rejected_invalid, 0);
    }
}
