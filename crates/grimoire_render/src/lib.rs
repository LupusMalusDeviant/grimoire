//! # grimoire_render
//!
//! Renderer of the Grimoire engine. Consumers (the `grimoire` facade and games) describe *what*
//! to draw each frame as plain data ([`RenderFrame`]); *how* it is drawn stays inside this crate.
//!
//! Layer rule: `wgpu` types never appear in this crate's public API.
//!
//! Coordinate convention: world space is a 2D plane with X right and Y up, measured in arbitrary
//! world units. [`Camera2D`] maps world space to the screen. [`Camera25D`] and the mesh/light
//! channels of [`StageFrame`] (WP2.2, contract §6) add a third, "up" axis (Z) on top of that same
//! ground plane, with Y now "away from the viewer" instead of "up" on screen.
//!
//! Colour convention: all colours ([`RenderFrame::clear_color`], [`SpriteInstance::color`]) are
//! linear RGBA. [`WgpuRenderer`] renders into sRGB targets, so the GPU encodes on write and
//! alpha blending happens in linear space; offscreen read-back returns sRGB-encoded bytes.
//!
//! Mesh geometry ([`MeshData`], [`crate::procedural`]) and its GPU upload
//! ([`WgpuRenderer::register_mesh`]) are plan 0002 WP2.3; see the `mesh` and `mesh_pass` modules.
//! PBR shading (GGX, geometric specular anti-aliasing, optional textures) is WP2.5; texture
//! registration ([`TextureData`], [`WgpuRenderer::register_texture`]) mirrors the mesh registry,
//! see the `texture` module. Shadows (OF-3.2, plan 0002 WP2.6) are a key-light shadow map
//! ([`ShadowMode::KeyLight`], `shadow_pass` module) plus cheap blob shadows
//! ([`ShadowMode::Blob`], [`BlobShadowInstance`]), switchable per frame via
//! [`StageFrame::shadow_config`] ([`ShadowConfig`]); see the `stage3d` module's WP2.6 section and
//! this crate's WP2.6 ADR for what is deferred (point-light shadow casters).

use std::time::Duration;

pub mod cluster_layout;
mod mesh;
mod mesh_pass;
pub mod procedural;
mod shadow_pass;
mod sprite_pass;
mod stage;
mod stage3d;
mod texture;
mod wgpu_renderer;

pub use mesh::{MeshData, MeshError, MeshVertex};
pub use stage::{
    BULLET_PASS_PALETTE_SPACE, BulletInstance, RenderLayer, StageFrame, StageStats, palette_space,
};
pub use stage3d::{
    AlphaMode, AmbientLight, BlobShadowInstance, BulletLightCap, Camera25D, CameraFollow,
    DirectionalLight, MaterialHandle, MeshHandle, MeshInstance, PbrMaterial, PointLight,
    ShadowConfig, ShadowMode, TextureHandle,
};
pub use texture::{TextureColorSpace, TextureData, TextureError};

mod instance {
    // bytemuck's derive macros expand to `unsafe impl` blocks.
    #![allow(unsafe_code)]

    /// One instanced sprite. Layout is `#[repr(C)]`, 40 bytes, no padding, uploaded verbatim.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct SpriteInstance {
        /// Centre in world units.
        pub position: [f32; 2],
        /// Half extents in world units (radius for circles).
        pub half_size: [f32; 2],
        /// Counter-clockwise rotation in radians.
        pub rotation: f32,
        /// One of the constants in [`crate::shape`].
        pub shape: u32,
        /// Linear RGBA, not premultiplied.
        pub color: [f32; 4],
    }
}

pub use instance::SpriteInstance;

/// Values for [`SpriteInstance::shape`].
pub mod shape {
    /// Anti-aliased filled circle inscribed in the sprite rectangle.
    pub const CIRCLE: u32 = 0;
    /// Filled rectangle.
    pub const QUAD: u32 = 1;
}

/// Orthographic 2D camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera2D {
    /// World-space point shown at the centre of the screen.
    pub center: [f32; 2],
    /// Visible world height; the visible width follows the aspect ratio.
    pub world_height: f32,
}

impl Default for Camera2D {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            world_height: 100.0,
        }
    }
}

impl Camera2D {
    /// Column-major view-projection matrix mapping world space to clip space for a viewport
    /// with the given `aspect` ratio (width / height).
    #[must_use]
    pub fn view_projection(&self, aspect: f32) -> [[f32; 4]; 4] {
        let half_height = self.world_height * 0.5;
        let half_width = half_height * aspect;
        let scale_x = 1.0 / half_width;
        let scale_y = 1.0 / half_height;
        [
            [scale_x, 0.0, 0.0, 0.0],
            [0.0, scale_y, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [
                -self.center[0] * scale_x,
                -self.center[1] * scale_y,
                0.0,
                1.0,
            ],
        ]
    }

    /// Converts a window position in pixels (origin top-left, Y down) into world coordinates for
    /// a viewport of `viewport` pixels.
    #[must_use]
    pub fn screen_to_world(&self, pixel: [f32; 2], viewport: [f32; 2]) -> [f32; 2] {
        let half_height = self.world_height * 0.5;
        let half_width = half_height * (viewport[0] / viewport[1]);
        let ndc_x = pixel[0] / viewport[0] * 2.0 - 1.0;
        let ndc_y = 1.0 - pixel[1] / viewport[1] * 2.0;
        [
            self.center[0] + ndc_x * half_width,
            self.center[1] + ndc_y * half_height,
        ]
    }
}

/// Everything the renderer needs for one frame.
///
/// Filled by the extraction step from simulation state, consumed by [`Renderer::render`].
/// Reuse one instance across frames ([`RenderFrame::clear`]) to avoid per-frame allocation.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderFrame {
    /// Linear RGBA background colour.
    pub clear_color: [f32; 4],
    /// Camera for this frame.
    pub camera: Camera2D,
    /// Sprites in draw order (later entries are drawn on top).
    pub sprites: Vec<SpriteInstance>,
}

impl Default for RenderFrame {
    fn default() -> Self {
        Self {
            clear_color: [0.0, 0.0, 0.0, 1.0],
            camera: Camera2D::default(),
            sprites: Vec::new(),
        }
    }
}

impl RenderFrame {
    /// Creates an empty frame with a black background.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes all sprites but keeps the allocation, camera and clear colour.
    pub fn clear(&mut self) {
        self.sprites.clear();
    }
}

/// Measurements of one rendered frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderStats {
    /// Number of sprites submitted to the GPU.
    pub sprites_drawn: u32,
    /// Number of draw calls issued.
    pub draw_calls: u32,
    /// CPU time spent preparing and submitting the frame.
    pub cpu_time: Duration,
}

/// Construction parameters of [`WgpuRenderer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererConfig {
    /// Synchronise presentation with the display refresh.
    pub vsync: bool,
    /// Initial capacity of the instance buffer; it grows on demand.
    pub initial_sprite_capacity: u32,
    /// Accept a software adapter (WARP, lavapipe, ...) when no hardware adapter exists.
    pub allow_software_fallback: bool,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            vsync: true,
            initial_sprite_capacity: 16_384,
            allow_software_fallback: false,
        }
    }
}

/// Renderer failures.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// No GPU adapter matching the requirements exists.
    #[error("no suitable GPU adapter found")]
    NoAdapter,
    /// No frame could be presented this time: the window surface was lost or became outdated
    /// (it has been recovered for the next frame), or no frame is available right now because
    /// acquisition timed out or the window is occluded or minimised. Not a failure: skip the
    /// frame and render again; an occluded window may report this every frame, so throttle
    /// rather than log each occurrence.
    #[error("surface lost, outdated or temporarily unavailable")]
    SurfaceLost,
    /// The GPU ran out of memory.
    #[error("GPU out of memory")]
    OutOfMemory,
    /// The operation requires a renderer created with [`WgpuRenderer::new_offscreen`].
    #[error("operation requires an offscreen renderer")]
    NotOffscreen,
    /// Any other backend failure.
    #[error("render backend error: {0}")]
    Backend(String),
}

/// Contract of every renderer implementation. Object-safe.
pub trait Renderer {
    /// Informs the renderer about a new drawable size in physical pixels. Must tolerate zero sizes.
    ///
    /// A non-zero size the renderer cannot use must not turn into silently skipped frames: the
    /// failure is returned from the following [`Renderer::render`] calls.
    fn resize(&mut self, width: u32, height: u32);

    /// Draws `frame`. Never touches simulation state; must accept an empty sprite list.
    ///
    /// # Errors
    /// See [`RenderError`]; [`RenderError::SurfaceLost`] is recoverable by rendering again.
    fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError>;

    /// Backend name for diagnostics, e.g. `"Vulkan"`, `"Metal"`, `"Dx12"` or `"Null"`.
    fn backend_name(&self) -> &str;

    /// Whether this renderer has its own [`Renderer::render_stage`] behaviour (contract §6).
    ///
    /// The default is `false`, which is accurate for any renderer that has not been extended
    /// beyond P0: its `render_stage` only ever draws [`StageFrame::base`] through
    /// [`Renderer::render`] (see the default body below) and never the bullet, mesh, light, marker
    /// or debug channels. That is not a failure; callers that care can check this flag and log it
    /// once.
    fn supports_stage(&self) -> bool {
        false
    }

    /// Draws a full [`StageFrame`] (contract §6: world, bullet, player-marker and debug-UI
    /// layers in [`RenderLayer::ORDER`]; from WP2.2 also meshes and lights on
    /// [`RenderLayer::World`]).
    ///
    /// The provided default only draws [`StageFrame::base`], by forwarding it to
    /// [`Renderer::render`]; every [`StageStats`] counter beyond `base` is `0`. Override this
    /// together with [`Renderer::supports_stage`] to draw the additional channels.
    ///
    /// # Errors
    /// Same as [`Renderer::render`], applied to `frame.base`.
    fn render_stage(&mut self, frame: &StageFrame) -> Result<StageStats, RenderError> {
        let base = self.render(&frame.base)?;
        Ok(StageStats {
            base,
            ..StageStats::default()
        })
    }
}

/// Renderer that draws nothing — for headless runs and tests.
///
/// A frozen P0 contract type (contract §6, "vorhandene Verträge (nicht ändern)"): unlike the P1
/// render types this crate adds from WP2.2 onwards, it is a plain public struct without
/// `#[non_exhaustive]`, so a new field would break every external construction by struct literal.
/// It therefore does **not** grow a mesh registry of its own for WP2.3 — headless mesh-registration
/// tests use [`crate::WgpuRenderer::register_mesh`] (skipped without a GPU adapter, like every
/// other offscreen test) or construct a [`crate::procedural`] mesh and validate it directly via
/// [`MeshData::validate`], which needs no renderer at all. Consequently it never reports
/// [`StageStats::meshes_rejected_unregistered`] (contract §6, PO decision V-20, 2026-09-16): the
/// structural mesh/material/light validation is shared code every renderer applies identically,
/// but the registry check on top of it only runs in a renderer that owns a registry, and this one
/// does not.
#[derive(Debug, Clone, Default)]
pub struct NullRenderer {
    /// Number of `render` calls so far.
    pub frames_rendered: u64,
    /// Sprite count of the most recent frame.
    pub last_sprite_count: usize,
    /// Size passed to the most recent `resize`.
    pub last_size: (u32, u32),
}

impl Renderer for NullRenderer {
    fn resize(&mut self, width: u32, height: u32) {
        self.last_size = (width, height);
    }

    fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError> {
        self.frames_rendered += 1;
        self.last_sprite_count = frame.sprites.len();
        Ok(RenderStats {
            sprites_drawn: u32::try_from(frame.sprites.len()).unwrap_or(u32::MAX),
            draw_calls: 0,
            cpu_time: Duration::ZERO,
        })
    }

    fn backend_name(&self) -> &str {
        "Null"
    }

    fn supports_stage(&self) -> bool {
        true
    }

    /// Draws nothing, like [`Renderer::render`], but applies the full stage semantics of
    /// contract §6: it extracts and counts the bullet, mesh, material, light, marker and debug
    /// channels exactly like [`WgpuRenderer::render_stage`], so headless tests can exercise the
    /// extraction and rejection rules — including the WP2.2 mesh/material/light validation — without
    /// a GPU. `frames_rendered`, `last_sprite_count` (the length of `frame.base.sprites`) and
    /// `last_size` keep being updated by the inner [`Renderer::render`] call.
    ///
    /// Shares the structural mesh/material/light validation with [`WgpuRenderer::render_stage`]
    /// (both go through the same crate-internal extraction step), but passes no mesh-registry
    /// check: this renderer has no registry of its own (see the struct doc comment), so every
    /// structurally valid mesh instance counts as drawn and
    /// [`StageStats::meshes_rejected_unregistered`] stays `0` (contract §6, PO decision V-20,
    /// 2026-09-16).
    ///
    /// # Errors
    /// Same as [`Renderer::render`], applied to `frame.base`.
    fn render_stage(&mut self, frame: &StageFrame) -> Result<StageStats, RenderError> {
        let base = self.render(&frame.base)?;
        Ok(stage::stage_stats_from_base(base, frame, None))
    }
}

pub use wgpu_renderer::WgpuRenderer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_instance_is_40_bytes() {
        assert_eq!(std::mem::size_of::<SpriteInstance>(), 40);
    }

    #[test]
    fn camera_maps_center_to_origin() {
        let camera = Camera2D {
            center: [10.0, -4.0],
            world_height: 20.0,
        };
        let m = camera.view_projection(2.0);
        let clip_x = m[0][0] * 10.0 + m[3][0];
        let clip_y = m[1][1] * -4.0 + m[3][1];
        assert!(clip_x.abs() < 1e-6 && clip_y.abs() < 1e-6);
    }

    #[test]
    fn screen_to_world_corners() {
        let camera = Camera2D {
            center: [0.0, 0.0],
            world_height: 10.0,
        };
        let top_left = camera.screen_to_world([0.0, 0.0], [200.0, 100.0]);
        assert!((top_left[0] + 10.0).abs() < 1e-5 && (top_left[1] - 5.0).abs() < 1e-5);
    }

    #[test]
    fn null_renderer_counts_frames() {
        let mut renderer = NullRenderer::default();
        let mut frame = RenderFrame::new();
        frame.sprites.push(SpriteInstance::default());
        renderer.render(&frame).expect("null renderer never fails");
        assert_eq!(renderer.frames_rendered, 1);
        assert_eq!(renderer.last_sprite_count, 1);
    }

    fn accepted_bullet() -> BulletInstance {
        BulletInstance {
            position: [0.0, 0.0],
            radius: 1.0,
            rotation: 0.0,
            silhouette: 0,
            palette: 0,
            palette_space: BULLET_PASS_PALETTE_SPACE,
            glow: 0,
            flags: 0,
        }
    }

    /// Minimal `Renderer` that only implements the required P0 methods, to exercise the
    /// *provided* default of `render_stage` (contract §6): it must only draw `frame.base`.
    struct BareRenderer;

    impl Renderer for BareRenderer {
        fn resize(&mut self, _width: u32, _height: u32) {}

        fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError> {
            Ok(RenderStats {
                sprites_drawn: u32::try_from(frame.sprites.len()).unwrap_or(u32::MAX),
                draw_calls: 1,
                cpu_time: Duration::ZERO,
            })
        }

        fn backend_name(&self) -> &str {
            "Bare"
        }
    }

    #[test]
    fn default_render_stage_only_draws_base() {
        let mut renderer = BareRenderer;
        assert!(!renderer.supports_stage());

        let mut frame = StageFrame::new();
        frame.base.sprites.push(SpriteInstance::default());
        frame.bullets.push(accepted_bullet());
        frame.marker_sprites.push(SpriteInstance::default());
        frame.debug_sprites.push(SpriteInstance::default());
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });
        frame.point_lights.push(PointLight::default());
        frame.blob_shadows.push(BlobShadowInstance {
            position: [0.0, 0.0],
            radius: 1.0,
            softness: 0.5,
            strength: 0.5,
        });
        frame.camera_25d = Some(Camera25D::default());
        frame.key_light = Some(DirectionalLight::default());
        frame.shadow_config.mode = ShadowMode::KeyLight;

        let stats = renderer.render_stage(&frame).expect("render never fails");
        assert_eq!(stats.base.sprites_drawn, 1, "only base.sprites is drawn");
        assert_eq!(stats.base.draw_calls, 1);
        assert_eq!(stats.bullets_drawn, 0);
        assert_eq!(stats.bullets_rejected_palette_space, 0);
        assert_eq!(stats.bullets_rejected_invalid, 0);
        assert_eq!(
            stats.meshes_drawn, 0,
            "the provided default never extracts WP2.2 channels"
        );
        assert_eq!(stats.meshes_rejected_layer, 0);
        assert_eq!(stats.meshes_rejected_invalid, 0);
        assert_eq!(stats.meshes_rejected_unregistered, 0);
        assert_eq!(stats.materials_rejected_invalid, 0);
        assert_eq!(stats.point_lights_drawn, 0);
        assert_eq!(stats.point_lights_rejected_invalid, 0);
        assert_eq!(stats.bullet_point_lights_drawn, 0);
        assert!(!stats.key_light_rejected_invalid);
        assert!(!stats.ambient_rejected_invalid);
        assert!(!stats.bullet_light_cap_invalid);
        assert_eq!(
            stats.blob_shadows_drawn, 0,
            "the provided default never extracts WP2.6 channels either"
        );
        assert_eq!(stats.blob_shadows_rejected_invalid, 0);
        assert!(!stats.shadow_config_invalid);
        assert_eq!(stats.shadow_casters_drawn, 0, "no meshes were extracted");
        assert_eq!(stats.point_shadow_casters_drawn, 0);
    }

    #[test]
    fn null_renderer_supports_stage_and_counts_every_channel() {
        let mut renderer = NullRenderer::default();
        assert!(renderer.supports_stage());

        let mut frame = StageFrame::new();
        frame.base.sprites.push(SpriteInstance::default());
        frame.marker_sprites.push(SpriteInstance::default());
        frame.debug_sprites.push(SpriteInstance::default());
        frame.bullets.push(accepted_bullet());
        frame.camera_25d = Some(Camera25D::default());
        // One valid material referenced by one valid mesh, plus a second mesh pointing at an
        // out-of-range material index.
        frame.materials.push(PbrMaterial::default());
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(0),
            ..MeshInstance::default()
        });
        frame.meshes.push(MeshInstance {
            material: MaterialHandle(99),
            ..MeshInstance::default()
        });
        // One valid point light flagged as a bullet light, one invalid (negative range).
        frame.point_lights.push(PointLight {
            is_bullet_light: true,
            ..PointLight::default()
        });
        frame.point_lights.push(PointLight {
            range: -1.0,
            ..PointLight::default()
        });
        frame.key_light = Some(DirectionalLight::default());
        frame.shadow_config.mode = ShadowMode::KeyLight;
        frame.blob_shadows.push(BlobShadowInstance {
            position: [0.0, 0.0],
            radius: 1.0,
            softness: 0.5,
            strength: 0.5,
        });
        frame.blob_shadows.push(BlobShadowInstance {
            radius: -1.0,
            ..BlobShadowInstance::default()
        });

        let stats = renderer
            .render_stage(&frame)
            .expect("null renderer never fails");
        assert_eq!(
            stats.base.sprites_drawn, 3,
            "world + marker + debug sprites"
        );
        assert_eq!(stats.bullets_drawn, 1);
        assert_eq!(stats.bullets_rejected_palette_space, 0);
        assert_eq!(stats.bullets_rejected_invalid, 0);
        assert_eq!(stats.meshes_drawn, 1);
        assert_eq!(stats.meshes_rejected_layer, 0);
        assert_eq!(
            stats.meshes_rejected_invalid, 1,
            "out-of-range material index"
        );
        assert_eq!(
            stats.meshes_rejected_unregistered, 0,
            "NullRenderer has no mesh registry (contract §6): it never rejects for this reason"
        );
        assert_eq!(stats.materials_rejected_invalid, 0);
        assert_eq!(stats.point_lights_drawn, 1);
        assert_eq!(stats.point_lights_rejected_invalid, 1);
        assert_eq!(stats.bullet_point_lights_drawn, 1);
        assert!(!stats.key_light_rejected_invalid);
        assert!(!stats.ambient_rejected_invalid);
        assert!(!stats.bullet_light_cap_invalid);
        assert_eq!(stats.blob_shadows_drawn, 1);
        assert_eq!(stats.blob_shadows_rejected_invalid, 1);
        assert!(!stats.shadow_config_invalid);
        assert_eq!(
            stats.shadow_casters_drawn, 1,
            "one drawn mesh, a valid key light, and a mode wanting key-light shadows"
        );
        assert_eq!(stats.point_shadow_casters_drawn, 0);
        assert_eq!(
            renderer.last_sprite_count, 1,
            "only base.sprites, P0 semantics"
        );
    }
}
