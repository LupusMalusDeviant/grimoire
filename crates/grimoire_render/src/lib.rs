//! # grimoire_render
//!
//! Renderer of the Grimoire engine. Consumers (the `grimoire` facade and games) describe *what*
//! to draw each frame as plain data ([`RenderFrame`]); *how* it is drawn stays inside this crate.
//!
//! Layer rule: `wgpu` types never appear in this crate's public API.
//!
//! Coordinate convention: world space is a 2D plane with X right and Y up, measured in arbitrary
//! world units. [`Camera2D`] maps world space to the screen.
//!
//! Colour convention: all colours ([`RenderFrame::clear_color`], [`SpriteInstance::color`]) are
//! linear RGBA. [`WgpuRenderer`] renders into sRGB targets, so the GPU encodes on write and
//! alpha blending happens in linear space; offscreen read-back returns sRGB-encoded bytes.

use std::time::Duration;

mod sprite_pass;
mod wgpu_renderer;

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
    /// The window surface was lost or became outdated and could not be recovered this frame.
    #[error("surface lost or outdated")]
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
    fn resize(&mut self, width: u32, height: u32);

    /// Draws `frame`. Never touches simulation state; must accept an empty sprite list.
    ///
    /// # Errors
    /// See [`RenderError`]; [`RenderError::SurfaceLost`] is recoverable by rendering again.
    fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError>;

    /// Backend name for diagnostics, e.g. `"Vulkan"`, `"Metal"`, `"Dx12"` or `"Null"`.
    fn backend_name(&self) -> &str;
}

/// Renderer that draws nothing — for headless runs and tests.
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
}
