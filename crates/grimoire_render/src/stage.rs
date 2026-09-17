//! Stage frame, layer order and the bullet channel (contract §6, "Bühnen-Frame,
//! Ebenenreihenfolge und Bullet-Kanal", P1 addition).
//!
//! This module is the single owner of [`BulletInstance`], the palette-space constants, the
//! [`RenderLayer`] order and the extension path of [`StageFrame`]. It is purely additive: the P0
//! types [`RenderFrame`], [`RenderStats`] and [`SpriteInstance`] are reused unchanged.
//!
//! [`StageFrame`] and [`StageStats`] also carry the WP2.2 camera, mesh, material and light
//! channels, and (from WP2.6) the shadow channels, defined in the sibling `stage3d` module; this
//! module wires them into the frame, its `clear()` and the shared extraction/counting logic,
//! alongside the P1 bullet channel.

use std::time::Duration;

use crate::stage3d::{
    AmbientLight, BlobShadowInstance, BulletLightCap, Camera25D, DirectionalLight, MAX_SKIN_JOINTS,
    MeshHandle, MeshInstance, PbrMaterial, PointLight, ShadowConfig,
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
        /// Index into the bullet pass's silhouette table, [`crate::bullet_silhouette`] (plan 0002
        /// WP3.5). An index at or beyond [`crate::bullet_silhouette::COUNT`] is rejected and
        /// counted in [`crate::StageStats::bullets_rejected_invalid`].
        pub silhouette: u16,
        /// Index into the palette of `palette_space`, [`crate::bullet_palette`] for the only
        /// space the bullet pass draws (plan 0002 WP3.5). An index at or beyond
        /// [`crate::bullet_palette::COUNT`] is rejected and counted in
        /// [`crate::StageStats::bullets_rejected_invalid`].
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

/// Silhouette table of the bullet pass (plan 0002 WP3.5, engine ADR-0014, stylebook v0
/// "Bullets"): the values of [`BulletInstance::silhouette`].
///
/// PRD-0003 rule 3 requires bullet types to differ in silhouette, never in colour alone. A
/// billboard quad always has the same outline, so the pass reads each silhouette from a signed
/// distance atlas instead of from geometry (engine ADR-0014, "Textur-Atlas statt Geometrie"). The
/// three shapes are the stylebook's v0 base forms; a new bullet type needs a new, clearly
/// distinguishable silhouette here before it gets another colour.
///
/// [`NAMES`](bullet_silhouette::NAMES) gives every row its stable name in the shared visual
/// catalogue (`docs/formats/sigil.md` §10.10): Sigil sources name a silhouette, `sigilc` compiles the
/// name to the catalogue index, and the catalogue's drawn rows are exactly this table, in this
/// order. A new silhouette is added as the next catalogue row, never in between.
pub mod bullet_silhouette {
    /// Round orb: a disc filling the visible radius.
    pub const ORB: u16 = 0;
    /// Rice grain: an elongated capsule along the flight direction (`BulletInstance::rotation`).
    pub const RICE: u16 = 1;
    /// Diamond: a rhombus pointing along the flight direction.
    pub const DIAMOND: u16 = 2;
    /// Number of silhouettes in the table; valid indices are `0..COUNT`.
    pub const COUNT: u16 = 3;
    /// Catalogue name of every row, indexed by the constants above.
    pub const NAMES: [&str; COUNT as usize] = ["orb", "rice", "diamond"];
}

/// Palette table of the bullet pass's palette space, [`BULLET_PASS_PALETTE_SPACE`] (plan 0002
/// WP3.5, stylebook v0 "Bullets", table "Gegnerisch (HOSTILE)"): the values of
/// [`BulletInstance::palette`]. Every entry has a body colour, a white-hot core and the shared dark
/// rim; the colours themselves are provisional until the look review (P-11).
///
/// [`NAMES`](bullet_palette::NAMES) gives every row its stable name in the shared visual catalogue
/// (`docs/formats/sigil.md` §10.10, `enemy.<name>` in Sigil sources), exactly as for
/// [`bullet_silhouette`].
pub mod bullet_palette {
    /// H0 "Hexenmagenta": body `#FF2FB4`, core `#FFE3F4`.
    pub const HEX_MAGENTA: u16 = 0;
    /// H1 "Giftlimette": body `#B6FF2E`, core `#F6FFE0`.
    pub const POISON_LIME: u16 = 1;
    /// Number of palettes in the table; valid indices are `0..COUNT`.
    pub const COUNT: u16 = 2;
    /// Catalogue name of every row, indexed by the constants above.
    pub const NAMES: [&str; COUNT as usize] = ["hex_magenta", "poison_lime"];
}

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
    /// Blob shadow discs (plan 0002 WP2.6, OF-3.2), drawn on the ground under an actor as a cheap
    /// alternative to the key-light shadow map (`docs/art/stilbibel.md`, preset "Low"). Only drawn
    /// when [`StageFrame::shadow_config`]'s [`crate::ShadowMode`] is [`crate::ShadowMode::Blob`];
    /// still validated and counted otherwise, like [`StageFrame::bullets`]'s palette-space check.
    pub blob_shadows: Vec<BlobShadowInstance>,
    /// Shadow technique and its parameters for this frame (plan 0002 WP2.6, OF-3.2). Persists
    /// across [`StageFrame::clear`], like [`StageFrame::camera_25d`].
    pub shadow_config: ShadowConfig,
    /// Flat table of bone matrices every [`MeshInstance::skin`] in this frame indexes into (P1
    /// skinning addendum, contract §6 changelog 2026-09-16), analogous to
    /// [`StageFrame::materials`]. Cleared like the other per-frame instance channels.
    pub joint_matrices: Vec<[[f32; 4]; 4]>,
}

impl StageFrame {
    /// Creates an empty stage frame. Identical to [`StageFrame::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes all instances from every channel but keeps their allocations, the camera(s), the
    /// clear colour and the lighting fields ([`StageFrame::key_light`], [`StageFrame::ambient`],
    /// [`StageFrame::bullet_light_cap`], [`StageFrame::shadow_config`]) — like
    /// [`StageFrame::camera_25d`], these describe the current scene rather than a per-frame
    /// instance list, so the extraction step overwrites them directly instead of re-adding them
    /// after a clear.
    pub fn clear(&mut self) {
        self.base.clear();
        self.bullets.clear();
        self.marker_sprites.clear();
        self.debug_sprites.clear();
        self.meshes.clear();
        self.materials.clear();
        self.point_lights.clear();
        self.blob_shadows.clear();
        self.joint_matrices.clear();
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
    /// `position`, `radius` or `rotation`, `radius <= 0`, or (from WP3.5) `silhouette`/`palette`
    /// outside [`bullet_silhouette`]/[`bullet_palette`].
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
    /// Number of point lights accepted (see [`PointLight::is_valid`]): every valid light of
    /// [`StageFrame::point_lights`] plus, from plan 0002 WP3.5, every bullet-cloud light the
    /// renderer derived from [`StageFrame::bullets`] through [`crate::point_light_from_bullet`].
    pub point_lights_drawn: u32,
    /// Number of point lights rejected because [`PointLight::is_valid`] returned `false`.
    pub point_lights_rejected_invalid: u32,
    /// Of [`StageStats::point_lights_drawn`], how many had [`PointLight::is_bullet_light`] set
    /// (PRD-0003 rule 5 / FR-15 visibility): lights of [`StageFrame::point_lights`] carrying the
    /// flag plus every bullet-cloud light derived from [`StageFrame::bullets`] (WP3.5). Their
    /// contribution to the environment is capped by [`StageFrame::bullet_light_cap`] (WP3.4).
    pub bullet_point_lights_drawn: u32,
    /// Whether [`StageFrame::key_light`] was present but failed
    /// [`DirectionalLight::is_valid`].
    pub key_light_rejected_invalid: bool,
    /// Whether [`StageFrame::ambient`] failed [`AmbientLight::is_valid`].
    pub ambient_rejected_invalid: bool,
    /// Whether [`StageFrame::bullet_light_cap`] failed [`BulletLightCap::is_valid`]; shading falls
    /// back to [`BulletLightCap::clamped_floor_contribution`] in that case.
    pub bullet_light_cap_invalid: bool,
    /// Number of blob shadow discs accepted (see [`BlobShadowInstance::is_valid`]); drawn only
    /// when [`StageFrame::shadow_config`]'s mode is [`crate::ShadowMode::Blob`] (plan 0002 WP2.6).
    pub blob_shadows_drawn: u32,
    /// Number of blob shadow discs rejected because [`BlobShadowInstance::is_valid`] returned
    /// `false`.
    pub blob_shadows_rejected_invalid: u32,
    /// Whether [`StageFrame::shadow_config`] failed [`ShadowConfig::is_valid`]; an invalid config
    /// falls back to no shadows at all for this frame, never to guessed-at clamped values (plan
    /// 0002 WP2.6, the same "reject and count, do not guess" rule contract §6 already applies to
    /// [`PbrMaterial`] and the light types).
    pub shadow_config_invalid: bool,
    /// Number of [`MeshInstance`]s that are, this frame, casters for the key-light shadow map:
    /// exactly [`StageStats::meshes_drawn`] whenever [`StageFrame::shadow_config`] is valid, its
    /// [`crate::ShadowMode`] wants a key-light shadow map
    /// (`crate::ShadowMode::wants_key_light_shadow_map`) and [`StageFrame::key_light`] is present
    /// and valid — `0` otherwise. P1 has no per-instance opt-out (every drawn mesh casts a
    /// shadow); a future work package may add one.
    pub shadow_casters_drawn: u32,
    /// Reserved for point-light shadow casters (plan 0002 WP2.6, deferred — see
    /// [`crate::ShadowMode::KeyLightPlusPoints`]'s doc comment): always `0` in this version,
    /// regardless of how many [`PointLight`]s have [`PointLight::casts_shadow`] set.
    pub point_shadow_casters_drawn: u32,
    /// Of [`StageStats::point_lights_drawn`] (structurally valid lights), how many exceeded the
    /// renderer's configured [`crate::LightBudget`] this frame and were dropped (frame order)
    /// before clustering (plan 0002 WP3.4). Always `0` for a renderer with no configured light
    /// budget ([`crate::NullRenderer`] — it never clusters, the same capability-gated-counter
    /// shape [`StageStats::meshes_rejected_unregistered`] already established).
    pub point_lights_over_budget: u32,
    /// Of [`crate::cluster_layout::CLUSTER_COUNT`] froxels, how many had at least one light
    /// assigned this frame, as of the clustered forward+ pass's most recently completed
    /// (non-blocking) readback (plan 0002 WP3.4, engine ADR-0015; see `cluster_pass.rs`'s module
    /// doc comment for why this lags the true GPU state by roughly a frame under normal load).
    /// Always `0` for a renderer that never clusters ([`crate::NullRenderer`]).
    pub clusters_with_lights: u32,
    /// Total light-cluster assignments across every froxel this frame (sum of each cluster's
    /// light count), same readback caveat as [`StageStats::clusters_with_lights`]. Always
    /// `<= cluster_layout::light_index_list_worst_case_len` for the configured budget, usually far
    /// below it (engine ADR-0013's module doc comment on why the worst case is pessimistic).
    pub light_cluster_index_entries: u32,
    /// GPU time of a recently drawn frame, measured with timestamp queries (plan 0002 WP6.3): the
    /// summed durations of that frame's render and compute passes, from the most recent
    /// measurement whose non-blocking readback had completed when this call started (typically
    /// one to three frames old; see `gpu_timer.rs`). Queue uploads, idle time between submissions
    /// and presentation are not included.
    ///
    /// `None` when the device offers no timestamp queries, before the first measurement completed,
    /// for a frame skipped because of a zero-size target, and always for [`crate::NullRenderer`].
    /// The facade then records a marked estimate instead of a measured zero (contract §9.7).
    pub gpu_time: Option<Duration>,
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
/// `is_mesh_registered` is deliberately *not* identical for every caller (contract §6, PO decision
/// V-20, 2026-09-16): the structural mesh checks in [`extract_stage3d`] are shared code, but only
/// a renderer with its own mesh registry can say whether a `mesh` handle is actually known. Pass
/// `None` for a renderer without a registry ([`crate::NullRenderer`]) — every structurally valid
/// mesh instance then counts as drawn, exactly as before WP2.3 — or `Some(&is_registered)` for one
/// that owns a registry ([`crate::WgpuRenderer`]), which moves an otherwise-drawable but
/// unregistered instance from [`StageStats::meshes_drawn`] into
/// [`StageStats::meshes_rejected_unregistered`].
///
/// `light_budget`/`cluster_stats` (plan 0002 WP3.4) follow the identical capability-gated shape:
/// `None` for a renderer with no configured [`crate::LightBudget`] or clustering pass
/// ([`crate::NullRenderer`]) leaves [`StageStats::point_lights_over_budget`],
/// [`StageStats::clusters_with_lights`] and [`StageStats::light_cluster_index_entries`] at `0`;
/// `Some` for [`crate::WgpuRenderer`] fills them in.
/// [`StageStats::point_lights_over_budget`] is derived here from the already-computed
/// `stage3d.point_lights_drawn` (every structurally valid light, contract §6
/// `PointLight::is_valid`) rather than recomputed by the caller, so the mesh pass's own budget
/// clamp (`mesh_pass::build_light_list`) and this counter can never disagree about which lights
/// count as valid in the first place.
///
/// `derived_bullet_lights` (plan 0002 WP3.5) are the bullet-cloud lights the caller derived from
/// `frame.bullets` with [`crate::bullet_lights::derive_bullet_lights`] — the same slice the
/// caller shades, appended after `frame.point_lights`, so the light counters and the budget clamp
/// see exactly the lights the mesh pass receives. The derivation depends only on `frame`, never on
/// the renderer, so [`crate::NullRenderer`] and [`crate::WgpuRenderer`] count identically.
pub(crate) fn stage_stats_from_base(
    base: RenderStats,
    frame: &StageFrame,
    is_mesh_registered: Option<&dyn Fn(MeshHandle) -> bool>,
    light_budget: Option<usize>,
    cluster_stats: Option<crate::cluster_pass::ClusterFrameStats>,
    derived_bullet_lights: &[PointLight],
) -> StageStats {
    let bullets = extract_bullets(&frame.bullets);
    let mut stage3d = extract_stage3d(frame, is_mesh_registered);
    for light in derived_bullet_lights {
        if light.is_valid() {
            stage3d.point_lights_drawn += 1;
            if light.is_bullet_light {
                stage3d.bullet_point_lights_drawn += 1;
            }
        } else {
            stage3d.point_lights_rejected_invalid += 1;
        }
    }
    let blob_shadows = extract_blob_shadows(&frame.blob_shadows);
    let sprite_channels = frame.marker_sprites.len() + frame.debug_sprites.len();
    let shadow_config_invalid = !frame.shadow_config.is_valid();
    let key_light_castable = frame.key_light.is_some_and(|light| light.is_valid());
    let shadow_casters_drawn = if !shadow_config_invalid
        && frame.shadow_config.mode.wants_key_light_shadow_map()
        && key_light_castable
    {
        stage3d.meshes_drawn
    } else {
        0
    };
    let point_lights_over_budget = light_budget.map_or(0, |budget| {
        let budget = u32::try_from(budget).unwrap_or(u32::MAX);
        stage3d.point_lights_drawn.saturating_sub(budget)
    });
    let cluster_stats = cluster_stats.unwrap_or_default();
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
        blob_shadows_drawn: blob_shadows.drawn,
        blob_shadows_rejected_invalid: blob_shadows.rejected_invalid,
        shadow_config_invalid,
        shadow_casters_drawn,
        point_shadow_casters_drawn: 0,
        point_lights_over_budget,
        clusters_with_lights: cluster_stats.clusters_with_lights,
        light_cluster_index_entries: cluster_stats.light_cluster_index_entries,
        gpu_time: None,
    }
}

/// Outcome of classifying a [`StageFrame::blob_shadows`] channel (plan 0002 WP2.6).
struct BlobShadowExtraction {
    drawn: u32,
    rejected_invalid: u32,
}

/// Applies [`BlobShadowInstance::is_valid`] to `blob_shadows`; used by [`stage_stats_from_base`].
/// Mirrors [`extract_bullets`]'s shape but has no palette-space equivalent to check first.
fn extract_blob_shadows(blob_shadows: &[BlobShadowInstance]) -> BlobShadowExtraction {
    let mut drawn = 0u32;
    let mut rejected_invalid = 0u32;
    for blob in blob_shadows {
        if blob.is_valid() {
            drawn += 1;
        } else {
            rejected_invalid += 1;
        }
    }
    BlobShadowExtraction {
        drawn,
        rejected_invalid,
    }
}

/// Whether `bullet` is structurally drawable by the bullet pass (contract §6 "Gültigkeit"): finite
/// `position`, `radius` and `rotation`, `radius > 0`, and `silhouette`/`palette` inside the pass's
/// tables ([`bullet_silhouette::COUNT`], [`bullet_palette::COUNT`], plan 0002 WP3.5). Says nothing
/// about the palette space, which [`is_accepted_bullet`] checks first.
pub(crate) fn is_structurally_valid_bullet(bullet: &BulletInstance) -> bool {
    bullet.position[0].is_finite()
        && bullet.position[1].is_finite()
        && bullet.radius.is_finite()
        && bullet.rotation.is_finite()
        && bullet.radius > 0.0
        && bullet.silhouette < bullet_silhouette::COUNT
        && bullet.palette < bullet_palette::COUNT
}

/// Whether the bullet pass draws `bullet`: palette space [`BULLET_PASS_PALETTE_SPACE`] and
/// [`is_structurally_valid_bullet`]. Unlike [`extract_bullets`] this never asserts; the GPU
/// staging and the bullet-light derivation use it to filter, while the counting (and the
/// debug-build assertion on a foreign palette space) stays in [`extract_bullets`] alone, so one
/// frame never asserts twice.
pub(crate) fn is_accepted_bullet(bullet: &BulletInstance) -> bool {
    bullet.palette_space == BULLET_PASS_PALETTE_SPACE && is_structurally_valid_bullet(bullet)
}

/// Applies the bullet pass's palette-space and validity rules (contract §6) to `bullets`; used by
/// [`stage_stats_from_base`].
///
/// The palette-space check runs first: an instance in a foreign palette space is rejected (and,
/// in debug builds, additionally triggers a `debug_assert!` — the one documented exception to
/// "null implementations never panic", contract §2a) without being checked for validity. Only
/// instances in the accepted palette space are checked for finiteness, a positive radius and (from
/// WP3.5) silhouette and palette indices inside the pass's tables.
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
        if is_structurally_valid_bullet(bullet) {
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

/// Whether `skin`'s range fits entirely inside `frame.joint_matrices` and `joint_count` is within
/// `1..=MAX_SKIN_JOINTS` (P1 skinning addendum, contract §6 changelog 2026-09-16). A mesh instance
/// with an out-of-range or oversized skin binding is rejected exactly like a non-finite transform
/// or an invalid material — folded into `meshes_rejected_invalid`, not a separate counter, since
/// this is one more structural precondition on the same instance, not a new failure mode a caller
/// needs to distinguish.
fn skin_binding_is_valid(skin: crate::stage3d::SkinBinding, frame: &StageFrame) -> bool {
    skin.joint_count > 0
        && skin.joint_count <= MAX_SKIN_JOINTS
        && skin
            .joint_offset
            .checked_add(skin.joint_count)
            .is_some_and(|end| (end as usize) <= frame.joint_matrices.len())
}

/// Applies the mesh, material and light validation rules (contract §6, WP2.2; skin binding range
/// added P1) to `frame`; used by [`stage_stats_from_base`]. Never panics: every rejection is
/// deterministic data classification, with no debug-only assertion analogous to the bullet pass's
/// palette-space check — there is no pre-existing hard invariant here for one to guard, only this
/// WP's own new validation.
///
/// A mesh instance is drawn only if all of: its own fields are valid (finite `transform`,
/// [`skin_binding_is_valid`] when [`MeshInstance::skin`] is `Some`), `material` indexes a present,
/// valid [`PbrMaterial`] in `frame.materials`, and — when `is_mesh_registered` is `Some`, i.e. the
/// caller owns a mesh registry — `mesh` is registered with it (contract §6, PO decision V-20,
/// 2026-09-16). An out-of-range or invalid material rejects the mesh (`meshes_rejected_invalid`)
/// without double-counting into `materials_rejected_invalid`, which counts invalid *materials*
/// themselves regardless of whether any mesh references them. An otherwise-drawable mesh with an
/// unregistered handle is counted separately (`meshes_rejected_unregistered`), never folded into
/// `meshes_rejected_invalid`. With `is_mesh_registered == None` every structurally valid mesh
/// counts as drawn — the behaviour every caller had before a registry existed (contract §6, still
/// [`crate::NullRenderer`]'s behaviour, which has no registry of its own).
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
        let skin_valid = mesh
            .skin
            .is_none_or(|skin| skin_binding_is_valid(skin, frame));
        if !(transform_finite && material_valid && skin_valid) {
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
    use crate::stage3d::{MaterialHandle, ShadowMode};

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
        frame.blob_shadows.push(BlobShadowInstance::default());
        frame.joint_matrices.push([[0.0; 4]; 4]);
        frame.base.camera.world_height = 42.0;
        frame.base.clear_color = [0.1, 0.2, 0.3, 1.0];
        frame.camera_25d = Some(Camera25D::default());
        frame.key_light = Some(DirectionalLight::default());
        let cap = BulletLightCap {
            floor_contribution: 0.3,
        };
        frame.bullet_light_cap = cap;
        let shadow_config = ShadowConfig {
            mode: ShadowMode::KeyLight,
            ..ShadowConfig::default()
        };
        frame.shadow_config = shadow_config;

        frame.clear();

        assert!(frame.base.sprites.is_empty());
        assert!(frame.bullets.is_empty());
        assert!(frame.marker_sprites.is_empty());
        assert!(frame.debug_sprites.is_empty());
        assert!(frame.meshes.is_empty());
        assert!(frame.materials.is_empty());
        assert!(frame.point_lights.is_empty());
        assert!(frame.blob_shadows.is_empty());
        assert!(frame.joint_matrices.is_empty());
        assert!((frame.base.camera.world_height - 42.0).abs() < f32::EPSILON);
        assert_eq!(frame.base.clear_color, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(frame.camera_25d, Some(Camera25D::default()));
        assert_eq!(frame.key_light, Some(DirectionalLight::default()));
        assert_eq!(frame.bullet_light_cap, cap);
        assert_eq!(frame.shadow_config, shadow_config);
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

    // --- WP3.5: silhouette and palette tables, accepted-bullet classification -----------------

    #[test]
    fn table_names_follow_the_constants_and_are_unique_identifiers() {
        assert_eq!(
            bullet_silhouette::NAMES[usize::from(bullet_silhouette::ORB)],
            "orb"
        );
        assert_eq!(
            bullet_silhouette::NAMES[usize::from(bullet_silhouette::RICE)],
            "rice"
        );
        assert_eq!(
            bullet_silhouette::NAMES[usize::from(bullet_silhouette::DIAMOND)],
            "diamond"
        );
        assert_eq!(
            bullet_palette::NAMES[usize::from(bullet_palette::HEX_MAGENTA)],
            "hex_magenta"
        );
        assert_eq!(
            bullet_palette::NAMES[usize::from(bullet_palette::POISON_LIME)],
            "poison_lime"
        );
        for names in [&bullet_silhouette::NAMES[..], &bullet_palette::NAMES[..]] {
            for (index, name) in names.iter().enumerate() {
                let mut bytes = name.bytes();
                let first = bytes.next().expect("names are not empty");
                assert!(first.is_ascii_lowercase(), "{name}");
                assert!(
                    bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                    "{name} is not a lowercase Sigil identifier"
                );
                assert!(!names[..index].contains(name), "{name} is listed twice");
            }
        }
    }

    #[test]
    fn extract_bullets_accepts_every_table_entry() {
        let mut bullets = Vec::new();
        for silhouette in 0..bullet_silhouette::COUNT {
            for palette in 0..bullet_palette::COUNT {
                bullets.push(BulletInstance {
                    silhouette,
                    palette,
                    ..valid_bullet()
                });
            }
        }
        let result = extract_bullets(&bullets);
        assert_eq!(
            result.drawn as usize,
            usize::from(bullet_silhouette::COUNT) * usize::from(bullet_palette::COUNT)
        );
        assert_eq!(result.rejected_invalid, 0);
    }

    #[test]
    fn extract_bullets_rejects_silhouette_or_palette_outside_the_tables() {
        let bullets = vec![
            BulletInstance {
                silhouette: bullet_silhouette::COUNT,
                ..valid_bullet()
            },
            BulletInstance {
                palette: bullet_palette::COUNT,
                ..valid_bullet()
            },
            BulletInstance {
                silhouette: u16::MAX,
                palette: u16::MAX,
                ..valid_bullet()
            },
        ];
        let result = extract_bullets(&bullets);
        assert_eq!(result.drawn, 0);
        assert_eq!(result.rejected_invalid, 3);
        assert_eq!(result.rejected_palette_space, 0);
    }

    #[test]
    fn is_accepted_bullet_matches_extract_bullets_without_asserting() {
        // A foreign palette space must be classified as "not accepted" silently: the GPU staging
        // and the bullet-light derivation call this, and only `extract_bullets` may assert.
        let foreign = BulletInstance {
            palette_space: palette_space::FRIENDLY,
            ..valid_bullet()
        };
        assert!(!is_accepted_bullet(&foreign));
        assert!(is_structurally_valid_bullet(&foreign));
        assert!(is_accepted_bullet(&valid_bullet()));
        assert!(!is_accepted_bullet(&BulletInstance {
            palette: bullet_palette::COUNT,
            ..valid_bullet()
        }));
    }

    #[test]
    fn derived_bullet_lights_are_counted_as_drawn_bullet_lights() {
        let frame = StageFrame::new();
        let derived = [
            crate::point_light_from_bullet(&BulletInstance {
                glow: 255,
                ..valid_bullet()
            }),
            // Degenerate on purpose: zero glow is still valid (intensity 0), zero radius is not.
            crate::point_light_from_bullet(&BulletInstance {
                radius: 0.0,
                ..valid_bullet()
            }),
        ];
        let stats = stage_stats_from_base(
            RenderStats::default(),
            &frame,
            None,
            Some(1),
            None,
            &derived,
        );
        assert_eq!(stats.point_lights_drawn, 1);
        assert_eq!(stats.bullet_point_lights_drawn, 1);
        assert_eq!(stats.point_lights_rejected_invalid, 1);
        assert_eq!(
            stats.point_lights_over_budget, 0,
            "one valid light, budget 1"
        );
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

    // --- extract_stage3d: skin binding range (P1 skinning addendum, 2026-09-16) --------------

    #[test]
    fn extract_stage3d_accepts_a_mesh_with_a_valid_skin_binding() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.joint_matrices = vec![[[0.0; 4]; 4]; 2];
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            skin: Some(crate::stage3d::SkinBinding {
                joint_offset: 0,
                joint_count: 2,
            }),
            ..MeshInstance::default()
        });

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 1);
        assert_eq!(result.meshes_rejected_invalid, 0);
    }

    #[test]
    fn extract_stage3d_rejects_a_skin_binding_reaching_past_joint_matrices() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.joint_matrices = vec![[[0.0; 4]; 4]; 2];
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            skin: Some(crate::stage3d::SkinBinding {
                joint_offset: 1,
                joint_count: 2, // 1 + 2 = 3 > joint_matrices.len() == 2
            }),
            ..MeshInstance::default()
        });

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 0);
        assert_eq!(result.meshes_rejected_invalid, 1);
    }

    #[test]
    fn extract_stage3d_rejects_a_skin_binding_with_zero_or_oversized_joint_count() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.joint_matrices = vec![[[0.0; 4]; 4]; 300];
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            skin: Some(crate::stage3d::SkinBinding {
                joint_offset: 0,
                joint_count: 0,
            }),
            ..MeshInstance::default()
        });
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            skin: Some(crate::stage3d::SkinBinding {
                joint_offset: 0,
                joint_count: MAX_SKIN_JOINTS + 1,
            }),
            ..MeshInstance::default()
        });

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 0);
        assert_eq!(result.meshes_rejected_invalid, 2);
    }

    #[test]
    fn extract_stage3d_rejects_a_skin_binding_offset_overflow_without_panic() {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            skin: Some(crate::stage3d::SkinBinding {
                joint_offset: u32::MAX,
                joint_count: 1,
            }),
            ..MeshInstance::default()
        });

        let result = extract_stage3d(&frame, None);
        assert_eq!(result.meshes_drawn, 0);
        assert_eq!(result.meshes_rejected_invalid, 1);
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

    // --- WP3.4: light budget / cluster stats capability gating --------------------------------

    #[test]
    fn stage_stats_from_base_without_a_light_budget_never_reports_point_lights_over_budget() {
        // `None` is what `NullRenderer` passes (it never clusters): even with far more valid
        // lights than any real budget, the counter stays 0, the same capability-gated shape as
        // `meshes_rejected_unregistered`.
        let mut frame = StageFrame::new();
        for _ in 0..40 {
            frame.point_lights.push(PointLight::default());
        }
        let stats = stage_stats_from_base(RenderStats::default(), &frame, None, None, None, &[]);
        assert_eq!(stats.point_lights_drawn, 40);
        assert_eq!(stats.point_lights_over_budget, 0);
    }

    #[test]
    fn stage_stats_from_base_derives_point_lights_over_budget_from_the_valid_count() {
        let mut frame = StageFrame::new();
        for _ in 0..40 {
            frame.point_lights.push(PointLight::default());
        }
        // One structurally invalid light: must not count towards the budget either way.
        frame.point_lights.push(PointLight {
            intensity: f32::NAN,
            ..PointLight::default()
        });
        let stats =
            stage_stats_from_base(RenderStats::default(), &frame, None, Some(32), None, &[]);
        assert_eq!(
            stats.point_lights_drawn, 40,
            "the NaN light is invalid, not counted here"
        );
        assert_eq!(
            stats.point_lights_over_budget, 8,
            "40 valid lights, budget 32"
        );
    }

    #[test]
    fn stage_stats_from_base_without_cluster_stats_reports_zero() {
        let frame = StageFrame::new();
        let stats =
            stage_stats_from_base(RenderStats::default(), &frame, None, Some(32), None, &[]);
        assert_eq!(stats.clusters_with_lights, 0);
        assert_eq!(stats.light_cluster_index_entries, 0);
    }

    #[test]
    fn stage_stats_from_base_copies_cluster_stats_through_when_given() {
        let frame = StageFrame::new();
        let cluster_stats = crate::cluster_pass::ClusterFrameStats {
            clusters_with_lights: 12,
            light_cluster_index_entries: 34,
        };
        let stats = stage_stats_from_base(
            RenderStats::default(),
            &frame,
            None,
            Some(32),
            Some(cluster_stats),
            &[],
        );
        assert_eq!(stats.clusters_with_lights, 12);
        assert_eq!(stats.light_cluster_index_entries, 34);
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

    // --- extract_blob_shadows (WP2.6) --------------------------------------------------------

    fn valid_blob() -> BlobShadowInstance {
        BlobShadowInstance {
            position: [0.0, 0.0],
            radius: 1.0,
            softness: 0.4,
            strength: 0.6,
        }
    }

    #[test]
    fn extract_blob_shadows_counts_valid_instances() {
        let blobs = vec![valid_blob(), valid_blob()];
        let result = extract_blob_shadows(&blobs);
        assert_eq!(result.drawn, 2);
        assert_eq!(result.rejected_invalid, 0);
    }

    #[test]
    fn extract_blob_shadows_rejects_invalid_instances_without_panic() {
        let blobs = vec![
            BlobShadowInstance {
                radius: 0.0,
                ..valid_blob()
            },
            BlobShadowInstance {
                softness: 2.0,
                ..valid_blob()
            },
            BlobShadowInstance {
                position: [f32::NAN, 0.0],
                ..valid_blob()
            },
        ];
        let result = extract_blob_shadows(&blobs);
        assert_eq!(result.drawn, 0);
        assert_eq!(result.rejected_invalid, 3);
    }

    // --- stage_stats_from_base: shadow_casters_drawn (WP2.6) ---------------------------------

    fn frame_with_one_valid_mesh_and_key_light(mode: ShadowMode) -> StageFrame {
        let mut frame = StageFrame::new();
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });
        frame.key_light = Some(DirectionalLight::default());
        frame.shadow_config = ShadowConfig {
            mode,
            ..ShadowConfig::default()
        };
        frame
    }

    #[test]
    fn shadow_casters_drawn_matches_meshes_drawn_when_key_light_shadows_are_wanted() {
        let frame = frame_with_one_valid_mesh_and_key_light(ShadowMode::KeyLight);
        let stats = stage_stats_from_base(RenderStats::default(), &frame, None, None, None, &[]);
        assert_eq!(stats.meshes_drawn, 1);
        assert_eq!(stats.shadow_casters_drawn, 1);
        assert_eq!(
            stats.point_shadow_casters_drawn, 0,
            "point-light shadow casters are deferred past WP2.6"
        );
    }

    #[test]
    fn shadow_casters_drawn_is_zero_when_the_mode_does_not_want_key_light_shadows() {
        for mode in [ShadowMode::None, ShadowMode::Blob] {
            let frame = frame_with_one_valid_mesh_and_key_light(mode);
            let stats =
                stage_stats_from_base(RenderStats::default(), &frame, None, None, None, &[]);
            assert_eq!(stats.meshes_drawn, 1);
            assert_eq!(stats.shadow_casters_drawn, 0, "mode {mode:?}");
        }
    }

    #[test]
    fn shadow_casters_drawn_is_zero_without_a_valid_key_light() {
        let mut frame = frame_with_one_valid_mesh_and_key_light(ShadowMode::KeyLight);
        frame.key_light = None;
        let stats = stage_stats_from_base(RenderStats::default(), &frame, None, None, None, &[]);
        assert_eq!(stats.shadow_casters_drawn, 0, "no key light at all");

        let mut frame = frame_with_one_valid_mesh_and_key_light(ShadowMode::KeyLight);
        frame.key_light = Some(DirectionalLight {
            direction: [0.0, 0.0, 0.0], // zero-length: DirectionalLight::is_valid rejects it
            ..DirectionalLight::default()
        });
        let stats = stage_stats_from_base(RenderStats::default(), &frame, None, None, None, &[]);
        assert_eq!(stats.shadow_casters_drawn, 0, "invalid key light");
    }

    #[test]
    fn shadow_casters_drawn_is_zero_with_an_invalid_shadow_config() {
        let mut frame = frame_with_one_valid_mesh_and_key_light(ShadowMode::KeyLight);
        frame.shadow_config.map_size = 0; // ShadowConfig::is_valid rejects a zero map size
        let stats = stage_stats_from_base(RenderStats::default(), &frame, None, None, None, &[]);
        assert!(stats.shadow_config_invalid);
        assert_eq!(
            stats.shadow_casters_drawn, 0,
            "an invalid config falls back to no shadows, never a guessed-at value"
        );
    }

    #[test]
    fn blob_shadows_and_shadow_config_flow_through_stage_stats_from_base() {
        let mut frame = StageFrame::new();
        frame.blob_shadows.push(valid_blob());
        frame.blob_shadows.push(BlobShadowInstance {
            radius: -1.0,
            ..valid_blob()
        });
        let stats = stage_stats_from_base(RenderStats::default(), &frame, None, None, None, &[]);
        assert_eq!(stats.blob_shadows_drawn, 1);
        assert_eq!(stats.blob_shadows_rejected_invalid, 1);
        assert!(!stats.shadow_config_invalid, "default config is valid");
    }
}
