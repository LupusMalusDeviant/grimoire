//! Stage frame, layer order and the bullet channel (contract §6, "Bühnen-Frame,
//! Ebenenreihenfolge und Bullet-Kanal", P1 addition).
//!
//! This module is the single owner of [`BulletInstance`], the palette-space constants, the
//! [`RenderLayer`] order and the extension path of [`StageFrame`]. It is purely additive: the P0
//! types [`RenderFrame`], [`RenderStats`] and [`SpriteInstance`] are reused unchanged.
//!
//! [`StageFrame`] and [`StageStats`] also carry the WP2.2 camera, mesh, material and light
//! channels defined in the sibling `stage3d` module; this module wires them into the frame, its
//! `clear()` and the shared extraction/counting logic, alongside the P1 bullet channel.

use crate::stage3d::{
    AmbientLight, BulletLightCap, Camera25D, DirectionalLight, MeshHandle, MeshInstance,
    PbrMaterial, PointLight,
};
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
/// Later work packages grow this type with further channels without breaking existing callers,
/// which is why it is `#[non_exhaustive]` and offers [`StageFrame::new`] instead of requiring a
/// struct literal. WP2.2 is the first to use that growth path: it adds the camera, mesh, material
/// and light channels below.
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
    /// Tilted 2.5D camera for the mesh and light channels below (WP2.2). `None` until a camera
    /// exists (contract §9.3): the facade's mouse-aim sampling (`sample_aim`, contract §9.4) only
    /// replaces its input axes once a [`Camera25D`] is present here. Persists across
    /// [`StageFrame::clear`], like [`RenderFrame::camera`].
    pub camera_25d: Option<Camera25D>,
    /// Mesh instances drawn on [`RenderLayer::World`] (WP2.2; geometry itself and its GPU upload
    /// are WP2.3).
    pub meshes: Vec<MeshInstance>,
    /// Material table [`crate::MaterialHandle`] indexes into (WP2.2).
    pub materials: Vec<PbrMaterial>,
    /// Point lights in view (WP2.2; the clustered forward+ pass and its count budget are WP3.4).
    pub point_lights: Vec<PointLight>,
    /// Directional "key light" for this frame, at most one (WP2.2). Persists across
    /// [`StageFrame::clear`], like [`StageFrame::camera_25d`].
    pub key_light: Option<DirectionalLight>,
    /// Ambient/environment term for this frame, always present (WP2.2). Persists across
    /// [`StageFrame::clear`], like [`StageFrame::camera_25d`].
    pub ambient: AmbientLight,
    /// Bullet-light cap for this frame (PRD-0003 rule 5 / FR-15, WP2.2). Persists across
    /// [`StageFrame::clear`], like [`StageFrame::camera_25d`].
    pub bullet_light_cap: BulletLightCap,
}

impl StageFrame {
    /// Creates an empty stage frame. Identical to [`StageFrame::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes all instances from every channel but keeps their allocations, the camera(s), the
    /// clear colour and the lighting fields ([`StageFrame::key_light`], [`StageFrame::ambient`],
    /// [`StageFrame::bullet_light_cap`]) — like [`StageFrame::camera_25d`], these describe the
    /// current scene rather than a per-frame instance list, so the extraction step overwrites them
    /// directly instead of re-adding them after a clear.
    pub fn clear(&mut self) {
        self.base.clear();
        self.bullets.clear();
        self.marker_sprites.clear();
        self.debug_sprites.clear();
        self.meshes.clear();
        self.materials.clear();
        self.point_lights.clear();
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
    /// Number of mesh instances accepted: finite `transform`, `material` in range and pointing at
    /// a valid [`PbrMaterial`], `layer == RenderLayer::World`, and — in a renderer that owns a
    /// mesh registry — `mesh` registered with it (contract §6, PO decision V-20, 2026-09-16; see
    /// [`StageStats::meshes_rejected_unregistered`]). A renderer without a registry
    /// ([`crate::NullRenderer`]) applies only the structural checks, as before WP2.3.
    pub meshes_drawn: u32,
    /// Number of mesh instances rejected because `layer` was not [`RenderLayer::World`], the only
    /// layer P1 accepts for meshes.
    pub meshes_rejected_layer: u32,
    /// Number of mesh instances rejected because they were structurally invalid: a non-finite
    /// `transform`, or `material` out of range or pointing at an invalid [`PbrMaterial`].
    pub meshes_rejected_invalid: u32,
    /// Number of mesh instances that passed every structural check above (finite `transform`,
    /// valid `material`, `layer == RenderLayer::World`) but whose `mesh` handle was never
    /// registered with the renderer (contract §6, PO decision V-20, 2026-09-16). Not counted in
    /// [`StageStats::meshes_drawn`].
    ///
    /// The structural checks are shared code, applied identically by every renderer; only the
    /// registry check itself is specific to renderers that own a mesh registry.
    /// [`crate::WgpuRenderer`] fills this counter from its registry (plan 0002 WP2.3).
    /// [`crate::NullRenderer`] has no registry of its own (a frozen P0 contract type, see its doc
    /// comment) and therefore never rejects a mesh for this reason: this field stays `0` for it.
    pub meshes_rejected_unregistered: u32,
    /// Number of materials in [`StageFrame::materials`] that failed [`PbrMaterial::is_valid`].
    pub materials_rejected_invalid: u32,
    /// Number of point lights accepted (see [`PointLight::is_valid`]).
    pub point_lights_drawn: u32,
    /// Number of point lights rejected because [`PointLight::is_valid`] returned `false`.
    pub point_lights_rejected_invalid: u32,
    /// Of [`StageStats::point_lights_drawn`], how many had [`PointLight::is_bullet_light`] set
    /// (PRD-0003 rule 5 / FR-15 visibility, no cap is applied by this crate yet).
    pub bullet_point_lights_drawn: u32,
    /// Whether [`StageFrame::key_light`] was present but failed
    /// [`DirectionalLight::is_valid`].
    pub key_light_rejected_invalid: bool,
    /// Whether [`StageFrame::ambient`] failed [`AmbientLight::is_valid`].
    pub ambient_rejected_invalid: bool,
    /// Whether [`StageFrame::bullet_light_cap`] failed [`BulletLightCap::is_valid`]; shading falls
    /// back to [`BulletLightCap::clamped_floor_contribution`] in that case.
    pub bullet_light_cap_invalid: bool,
}

/// Outcome of classifying a [`StageFrame::bullets`] channel against the bullet pass rules.
struct BulletExtraction {
    drawn: u32,
    rejected_palette_space: u32,
    rejected_invalid: u32,
}

/// Combines a `base` [`RenderStats`] — from drawing `frame.base` through
/// [`crate::Renderer::render`] — with the extraction and counting of `frame`'s bullet, mesh,
/// material, light, marker and debug channels into a full [`StageStats`].
///
/// Shared by [`crate::NullRenderer::render_stage`] and [`crate::WgpuRenderer::render_stage`] so
/// both apply contract §6 identically by construction, whether or not the renderer actually
/// rasterises the additional channels yet: `sprites_drawn` counts every sprite channel (world,
/// marker and debug sprites), `draw_calls` stays whatever `render` reported (no additional pass
/// exists yet in P1), the bullet counters come from [`extract_bullets`], and the mesh/material/
/// light counters come from [`extract_stage3d`].
///
/// `is_mesh_registered` is the one part of this shared function that is deliberately *not*
/// identical for every caller (contract §6, PO decision V-20, 2026-09-16): the structural mesh
/// checks in [`extract_stage3d`] are shared code, but only a renderer with its own mesh registry
/// can say whether a `mesh` handle is actually known. Pass `None` for a renderer without a
/// registry ([`crate::NullRenderer`]) — every structurally valid mesh instance then counts as
/// drawn, exactly as before WP2.3 — or `Some(&is_registered)` for one that owns a registry
/// ([`crate::WgpuRenderer`]), which moves an otherwise-drawable but unregistered instance from
/// [`StageStats::meshes_drawn`] into [`StageStats::meshes_rejected_unregistered`].
pub(crate) fn stage_stats_from_base(
    base: RenderStats,
    frame: &StageFrame,
    is_mesh_registered: Option<&dyn Fn(MeshHandle) -> bool>,
) -> StageStats {
    let bullets = extract_bullets(&frame.bullets);
    let stage3d = extract_stage3d(frame, is_mesh_registered);
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
        meshes_drawn: stage3d.meshes_drawn,
        meshes_rejected_layer: stage3d.meshes_rejected_layer,
        meshes_rejected_invalid: stage3d.meshes_rejected_invalid,
        meshes_rejected_unregistered: stage3d.meshes_rejected_unregistered,
        materials_rejected_invalid: stage3d.materials_rejected_invalid,
        point_lights_drawn: stage3d.point_lights_drawn,
        point_lights_rejected_invalid: stage3d.point_lights_rejected_invalid,
        bullet_point_lights_drawn: stage3d.bullet_point_lights_drawn,
        key_light_rejected_invalid: stage3d.key_light_rejected_invalid,
        ambient_rejected_invalid: stage3d.ambient_rejected_invalid,
        bullet_light_cap_invalid: stage3d.bullet_light_cap_invalid,
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

/// Outcome of classifying a [`StageFrame`]'s WP2.2 mesh, material and light channels.
struct Stage3dExtraction {
    meshes_drawn: u32,
    meshes_rejected_layer: u32,
    meshes_rejected_invalid: u32,
    meshes_rejected_unregistered: u32,
    materials_rejected_invalid: u32,
    point_lights_drawn: u32,
    point_lights_rejected_invalid: u32,
    bullet_point_lights_drawn: u32,
    key_light_rejected_invalid: bool,
    ambient_rejected_invalid: bool,
    bullet_light_cap_invalid: bool,
}

/// Applies the mesh, material and light validation rules (contract §6, WP2.2) to `frame`; used by
/// [`stage_stats_from_base`]. Never panics: every rejection is deterministic data classification,
/// with no debug-only assertion analogous to the bullet pass's palette-space check — there is no
/// pre-existing hard invariant here for one to guard, only this WP's own new validation.
///
/// A mesh instance is drawn only if all of: its own fields are valid (finite `transform`),
/// `material` indexes a present, valid [`PbrMaterial`] in `frame.materials`, and — when
/// `is_mesh_registered` is `Some`, i.e. the caller owns a mesh registry — `mesh` is registered
/// with it (contract §6, PO decision V-20, 2026-09-16). An out-of-range or invalid material
/// rejects the mesh (`meshes_rejected_invalid`) without double-counting into
/// `materials_rejected_invalid`, which counts invalid *materials* themselves regardless of whether
/// any mesh references them. An otherwise-drawable mesh with an unregistered handle is counted
/// separately (`meshes_rejected_unregistered`), never folded into `meshes_rejected_invalid`. With
/// `is_mesh_registered == None` every structurally valid mesh counts as drawn — the behaviour
/// every caller had before a registry existed (contract §6, still [`crate::NullRenderer`]'s
/// behaviour, which has no registry of its own).
fn extract_stage3d(
    frame: &StageFrame,
    is_mesh_registered: Option<&dyn Fn(MeshHandle) -> bool>,
) -> Stage3dExtraction {
    let materials_rejected_invalid = frame
        .materials
        .iter()
        .filter(|material| !material.is_valid())
        .count();

    let mut meshes_drawn = 0u32;
    let mut meshes_rejected_layer = 0u32;
    let mut meshes_rejected_invalid = 0u32;
    let mut meshes_rejected_unregistered = 0u32;
    for mesh in &frame.meshes {
        if mesh.layer != RenderLayer::World {
            meshes_rejected_layer += 1;
            continue;
        }
        let transform_finite = mesh.transform.iter().flatten().all(|c| c.is_finite());
        let material = frame.materials.get(mesh.material.0 as usize);
        let material_valid = material.is_some_and(PbrMaterial::is_valid);
        if !(transform_finite && material_valid) {
            meshes_rejected_invalid += 1;
            continue;
        }
        let registered = is_mesh_registered.is_none_or(|is_registered| is_registered(mesh.mesh));
        if registered {
            meshes_drawn += 1;
        } else {
            meshes_rejected_unregistered += 1;
        }
    }

    let mut point_lights_drawn = 0u32;
    let mut point_lights_rejected_invalid = 0u32;
    let mut bullet_point_lights_drawn = 0u32;
    for light in &frame.point_lights {
        if light.is_valid() {
            point_lights_drawn += 1;
            if light.is_bullet_light {
                bullet_point_lights_drawn += 1;
            }
        } else {
            point_lights_rejected_invalid += 1;
        }
    }

    Stage3dExtraction {
        meshes_drawn,
        meshes_rejected_layer,
        meshes_rejected_invalid,
        meshes_rejected_unregistered,
        materials_rejected_invalid: u32::try_from(materials_rejected_invalid).unwrap_or(u32::MAX),
        point_lights_drawn,
        point_lights_rejected_invalid,
        bullet_point_lights_drawn,
        key_light_rejected_invalid: frame.key_light.is_some_and(|light| !light.is_valid()),
        ambient_rejected_invalid: !frame.ambient.is_valid(),
        bullet_light_cap_invalid: !frame.bullet_light_cap.is_valid(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage3d::MaterialHandle;

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
        frame.meshes.push(MeshInstance::default());
        frame.materials.push(PbrMaterial::default());
        frame.point_lights.push(PointLight::default());
        frame.base.camera.world_height = 42.0;
        frame.base.clear_color = [0.1, 0.2, 0.3, 1.0];
        frame.camera_25d = Some(Camera25D::default());
        frame.key_light = Some(DirectionalLight::default());
        let cap = BulletLightCap {
            floor_contribution: 0.3,
        };
        frame.bullet_light_cap = cap;

        frame.clear();

        assert!(frame.base.sprites.is_empty());
        assert!(frame.bullets.is_empty());
        assert!(frame.marker_sprites.is_empty());
        assert!(frame.debug_sprites.is_empty());
        assert!(frame.meshes.is_empty());
        assert!(frame.materials.is_empty());
        assert!(frame.point_lights.is_empty());
        assert!((frame.base.camera.world_height - 42.0).abs() < f32::EPSILON);
        assert_eq!(frame.base.clear_color, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(frame.camera_25d, Some(Camera25D::default()));
        assert_eq!(frame.key_light, Some(DirectionalLight::default()));
        assert_eq!(frame.bullet_light_cap, cap);
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

    // --- extract_stage3d: meshes, materials, lights (WP2.2) ----------------------------------

    #[test]
    fn extract_stage3d_counts_valid_mesh_material_and_light() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });
        frame.point_lights.push(PointLight::default());
        frame.point_lights.push(PointLight {
            is_bullet_light: true,
            ..PointLight::default()
        });
        frame.key_light = Some(DirectionalLight::default());

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 1);
        assert_eq!(result.meshes_rejected_layer, 0);
        assert_eq!(result.meshes_rejected_invalid, 0);
        assert_eq!(result.materials_rejected_invalid, 0);
        assert_eq!(result.point_lights_drawn, 2);
        assert_eq!(result.point_lights_rejected_invalid, 0);
        assert_eq!(result.bullet_point_lights_drawn, 1);
        assert!(!result.key_light_rejected_invalid);
        assert!(!result.ambient_rejected_invalid);
        assert!(!result.bullet_light_cap_invalid);
    }

    #[test]
    fn extract_stage3d_rejects_mesh_on_a_foreign_layer() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            layer: RenderLayer::Telegraphy,
            ..MeshInstance::default()
        });

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 0);
        assert_eq!(result.meshes_rejected_layer, 1);
        assert_eq!(result.meshes_rejected_invalid, 0);
    }

    #[test]
    fn extract_stage3d_rejects_mesh_with_non_finite_transform() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        let mut mesh = MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        };
        mesh.transform[0][0] = f32::NAN;
        frame.meshes.push(mesh);

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 0);
        assert_eq!(result.meshes_rejected_layer, 0);
        assert_eq!(result.meshes_rejected_invalid, 1);
    }

    #[test]
    fn extract_stage3d_rejects_mesh_with_out_of_range_or_invalid_material() {
        let mut frame = StageFrame::new();
        // No materials at all: index 0 is out of range.
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });
        // One invalid material at index 0.
        frame.materials.push(PbrMaterial {
            metallic_factor: 2.0,
            ..PbrMaterial::default()
        });
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 0);
        assert_eq!(result.meshes_rejected_invalid, 2);
        assert_eq!(
            result.materials_rejected_invalid, 1,
            "the invalid material itself is counted once, independent of how many meshes reference it"
        );
    }

    // --- extract_stage3d: mesh registry check (WP2.3, PO decision V-20, 2026-09-16) -----------

    #[test]
    fn extract_stage3d_rejects_a_structurally_valid_but_unregistered_mesh_without_panic() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });

        let never_registered: &dyn Fn(MeshHandle) -> bool = &|_| false;
        let result = extract_stage3d(&frame, Some(never_registered));
        assert_eq!(
            result.meshes_drawn, 0,
            "an unregistered handle must not be counted as drawn"
        );
        assert_eq!(result.meshes_rejected_layer, 0);
        assert_eq!(
            result.meshes_rejected_invalid, 0,
            "an unregistered handle is not the same rejection as a structurally invalid mesh"
        );
        assert_eq!(result.meshes_rejected_unregistered, 1);
    }

    #[test]
    fn extract_stage3d_counts_a_registered_mesh_as_drawn() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });

        let always_registered: &dyn Fn(MeshHandle) -> bool = &|_| true;
        let result = extract_stage3d(&frame, Some(always_registered));
        assert_eq!(result.meshes_drawn, 1);
        assert_eq!(result.meshes_rejected_unregistered, 0);
    }

    #[test]
    fn extract_stage3d_without_a_registry_never_rejects_for_being_unregistered() {
        // `None` is what a renderer without a mesh registry (`NullRenderer`) passes: every
        // structurally valid mesh counts as drawn, the same behaviour every caller had before a
        // registry existed.
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 1);
        assert_eq!(result.meshes_rejected_unregistered, 0);
    }

    #[test]
    fn extract_stage3d_rejects_invalid_point_light_key_light_ambient_and_cap() {
        let mut frame = StageFrame::new();
        frame.point_lights.push(PointLight {
            intensity: f32::NAN,
            ..PointLight::default()
        });
        frame.key_light = Some(DirectionalLight {
            direction: [0.0, 0.0, 0.0],
            ..DirectionalLight::default()
        });
        frame.ambient = AmbientLight::Flat {
            color: [1.0, 1.0, 1.0],
            intensity: -1.0,
        };
        frame.bullet_light_cap = BulletLightCap {
            floor_contribution: 2.0,
        };

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.point_lights_drawn, 0);
        assert_eq!(result.point_lights_rejected_invalid, 1);
        assert_eq!(result.bullet_point_lights_drawn, 0);
        assert!(result.key_light_rejected_invalid);
        assert!(result.ambient_rejected_invalid);
        assert!(result.bullet_light_cap_invalid);
    }

    #[test]
    fn extract_stage3d_default_frame_has_no_rejections() {
        let frame = StageFrame::new();
        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 0);
        assert_eq!(result.meshes_rejected_layer, 0);
        assert_eq!(result.meshes_rejected_invalid, 0);
        assert_eq!(result.meshes_rejected_unregistered, 0);
        assert_eq!(result.materials_rejected_invalid, 0);
        assert_eq!(result.point_lights_drawn, 0);
        assert_eq!(result.point_lights_rejected_invalid, 0);
        assert!(
            !result.key_light_rejected_invalid,
            "no key light: nothing to reject"
        );
        assert!(!result.ambient_rejected_invalid, "default ambient is valid");
        assert!(!result.bullet_light_cap_invalid, "default cap is valid");
    }
}
