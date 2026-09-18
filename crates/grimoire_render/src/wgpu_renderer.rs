//! `wgpu` implementation of [`Renderer`].

use std::sync::Arc;
use std::time::Instant;

use grimoire_gpu::{
    ContextOptions, GpuContext, GpuError, OffscreenTarget, SurfaceFrame, WindowSurface, wgpu,
};
use grimoire_platform::PlatformWindow;

use crate::bullet_lights::derive_bullet_lights;
use crate::bullet_pass::{BulletPass, BulletView, bullet_view};
use crate::gpu_timer::GpuTimer;
use crate::mesh_pass::MeshPass;
use crate::pass_graph::{self, PassLog};
use crate::sprite_pass::{SpriteCamera, SpritePass};
use crate::stage;
use crate::texture::{TextureData, TextureError};
use crate::{
    BulletInstance, Camera2D, LightBudget, MeshData, MeshError, MeshHandle, PointLight,
    RenderError, RenderFrame, RenderLayer, RenderStats, Renderer, RendererConfig, SpriteInstance,
    StageFrame, StageRendererConfig, StageStats, TextureHandle,
};

enum Target {
    Window(WindowSurface),
    Offscreen(OffscreenTarget),
}

/// GPU renderer based on `wgpu`.
///
/// Draws all sprites of a frame with one instanced draw call. Colours in [`RenderFrame`] are
/// linear; the render target is written through an sRGB view, so the GPU applies the sRGB
/// encoding and alpha blending happens in linear space.
///
/// [`Renderer::render_stage`] additionally draws [`crate::StageFrame::meshes`] on
/// [`crate::RenderLayer::World`], depth-tested against a dedicated depth buffer, *before* the
/// sprite pass (plan 0002 WP2.3; see the `mesh_pass` module) — the sprite pass itself, and its
/// draw order relative to the other channels (contract §6), are unchanged.
///
/// GPU validation and out-of-memory errors raised while a frame is prepared and submitted are
/// returned from [`Renderer::render`]; a lost GPU device makes every later `render` call fail
/// with [`RenderError::Backend`].
///
/// A failed [`Renderer::resize`] (for example beyond the device's texture size limit, or out of
/// memory) is not clamped or retried: every later `render` call returns that error, as
/// [`RenderError::OutOfMemory`] or [`RenderError::Backend`], until another `resize` replaces it.
pub struct WgpuRenderer {
    context: GpuContext,
    target: Target,
    sprites: SpritePass,
    mesh_pass: MeshPass,
    /// Layer 6 (plan 0002 WP3.5, engine ADR-0014).
    bullet_pass: BulletPass,
    /// The frame's own point lights followed by its derived bullet-cloud lights (WP3.5), reused
    /// across frames so shading them allocates nothing once the capacity has grown.
    lights: Vec<PointLight>,
    /// Slots the stage pass graph executed during the most recent `render_stage` call.
    last_pass_order: PassLog,
    width: u32,
    height: u32,
    /// Error of the most recent `resize`; never `SurfaceLost`, which callers treat as a skip.
    resize_error: Option<RenderError>,
    /// Configured clustered forward+ light budget (plan 0002 WP3.4), fixed since construction —
    /// threaded into [`crate::StageStats::point_lights_over_budget`] on every `render_stage` call,
    /// including the zero-size-skip paths (see `render_stage_impl`).
    light_budget: LightBudget,
    /// GPU pass timing through timestamp queries (plan 0002 WP6.3); disabled when the device has
    /// none. Feeds [`StageStats::gpu_time`].
    gpu_timer: GpuTimer,
}

impl std::fmt::Debug for WgpuRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgpuRenderer")
            .field("backend", &self.context.backend_name())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("offscreen", &matches!(self.target, Target::Offscreen(_)))
            .finish_non_exhaustive()
    }
}

fn map_gpu_error(error: GpuError) -> RenderError {
    match error {
        GpuError::NoAdapter(_) => RenderError::NoAdapter,
        GpuError::SurfaceLost | GpuError::SurfaceUnavailable => RenderError::SurfaceLost,
        GpuError::OutOfMemory => RenderError::OutOfMemory,
        other => RenderError::Backend(other.to_string()),
    }
}

fn map_resize_error(error: GpuError, width: u32, height: u32) -> RenderError {
    match error {
        GpuError::OutOfMemory => RenderError::OutOfMemory,
        other => RenderError::Backend(format!("resize to {width}x{height} failed: {other}")),
    }
}

/// `RenderError` is not `Clone` in its public contract; the kept resize error is returned again.
fn repeat_error(error: &RenderError) -> RenderError {
    match error {
        RenderError::NoAdapter => RenderError::NoAdapter,
        RenderError::SurfaceLost => RenderError::SurfaceLost,
        RenderError::OutOfMemory => RenderError::OutOfMemory,
        RenderError::NotOffscreen => RenderError::NotOffscreen,
        RenderError::Backend(message) => RenderError::Backend(message.clone()),
    }
}

fn context_options(config: &RendererConfig) -> ContextOptions {
    ContextOptions {
        high_performance: true,
        allow_software_fallback: config.allow_software_fallback,
        vsync: config.vsync,
    }
}

fn skipped_frame(start: Instant) -> RenderStats {
    RenderStats {
        cpu_time: start.elapsed(),
        ..RenderStats::default()
    }
}

impl WgpuRenderer {
    /// Creates a renderer presenting to `window`.
    ///
    /// Rendering is skipped until the window has a non-zero size.
    ///
    /// # Errors
    /// - [`RenderError::NoAdapter`] if no adapter can present to the window.
    /// - [`RenderError::OutOfMemory`] if GPU resources cannot be allocated.
    /// - [`RenderError::Backend`] if the surface or the device cannot be created, or `wgpu`
    ///   rejects the surface configuration, shader or pipeline.
    ///
    /// # Panics
    /// On macOS (Metal) `wgpu` panics if this is not called on the main thread; create the
    /// renderer from the platform event loop thread (for example in `AppHandler::init`).
    pub fn new_for_window(
        window: Arc<dyn PlatformWindow>,
        config: RendererConfig,
    ) -> Result<Self, RenderError> {
        Self::new_for_window_staged(
            window,
            StageRendererConfig {
                base: config,
                ..StageRendererConfig::default()
            },
        )
    }

    /// Creates a renderer presenting to `window`, additionally choosing the clustered forward+
    /// light budget (plan 0002 WP3.4, [`crate::LightBudget`]) — see [`StageRendererConfig`]'s doc
    /// comment for why this is a separate constructor rather than a new [`RendererConfig`] field.
    /// [`WgpuRenderer::new_for_window`] is equivalent to
    /// `new_for_window_staged(window, StageRendererConfig { base: config, ..Default::default() })`.
    ///
    /// # Errors
    /// Same as [`WgpuRenderer::new_for_window`].
    ///
    /// # Panics
    /// Same as [`WgpuRenderer::new_for_window`].
    pub fn new_for_window_staged(
        window: Arc<dyn PlatformWindow>,
        config: StageRendererConfig,
    ) -> Result<Self, RenderError> {
        let (context, surface) = GpuContext::new_for_window(window, context_options(&config.base))
            .map_err(map_gpu_error)?;
        let sprites = SpritePass::new(
            &context,
            surface.view_format(),
            config.base.initial_sprite_capacity,
        )
        .map_err(map_gpu_error)?;
        let (width, height) = if surface.is_configured() {
            surface.size()
        } else {
            (0, 0)
        };
        let mesh_pass = MeshPass::new(
            &context,
            surface.view_format(),
            width,
            height,
            config.light_budget,
            config.msaa,
        )
        .map_err(map_gpu_error)?;
        let bullet_pass =
            BulletPass::new(&context, surface.view_format()).map_err(map_gpu_error)?;
        let gpu_timer = GpuTimer::new(&context);
        Ok(Self {
            context,
            target: Target::Window(surface),
            sprites,
            mesh_pass,
            bullet_pass,
            lights: Vec::new(),
            last_pass_order: PassLog::default(),
            width,
            height,
            resize_error: None,
            light_budget: config.light_budget,
            gpu_timer,
        })
    }

    /// Creates a renderer drawing into an offscreen RGBA8 texture of the given size.
    ///
    /// # Errors
    /// - [`RenderError::NoAdapter`] if no adapter is available.
    /// - [`RenderError::OutOfMemory`] if GPU resources cannot be allocated.
    /// - [`RenderError::Backend`] if a dimension is zero or exceeds the device's texture limit,
    ///   the device cannot be created, or `wgpu` rejects the shader or pipeline.
    pub fn new_offscreen(
        width: u32,
        height: u32,
        config: RendererConfig,
    ) -> Result<Self, RenderError> {
        Self::new_offscreen_staged(
            width,
            height,
            StageRendererConfig {
                base: config,
                ..StageRendererConfig::default()
            },
        )
    }

    /// Creates an offscreen renderer, additionally choosing the clustered forward+ light budget
    /// (plan 0002 WP3.4, [`crate::LightBudget`]) — see [`StageRendererConfig`]'s doc comment for
    /// why this is a separate constructor rather than a new [`RendererConfig`] field.
    /// [`WgpuRenderer::new_offscreen`] is equivalent to
    /// `new_offscreen_staged(width, height, StageRendererConfig { base: config, ..Default::default() })`.
    ///
    /// # Errors
    /// Same as [`WgpuRenderer::new_offscreen`].
    pub fn new_offscreen_staged(
        width: u32,
        height: u32,
        config: StageRendererConfig,
    ) -> Result<Self, RenderError> {
        let context =
            GpuContext::new_offscreen(context_options(&config.base)).map_err(map_gpu_error)?;
        let target = OffscreenTarget::new(&context, width, height).map_err(map_gpu_error)?;
        let sprites = SpritePass::new(
            &context,
            target.format(),
            config.base.initial_sprite_capacity,
        )
        .map_err(map_gpu_error)?;
        let mesh_pass = MeshPass::new(
            &context,
            target.format(),
            width,
            height,
            config.light_budget,
            config.msaa,
        )
        .map_err(map_gpu_error)?;
        let bullet_pass = BulletPass::new(&context, target.format()).map_err(map_gpu_error)?;
        let gpu_timer = GpuTimer::new(&context);
        Ok(Self {
            context,
            target: Target::Offscreen(target),
            sprites,
            mesh_pass,
            bullet_pass,
            lights: Vec::new(),
            last_pass_order: PassLog::default(),
            width,
            height,
            resize_error: None,
            light_budget: config.light_budget,
            gpu_timer,
        })
    }

    /// Reads back the last rendered offscreen image as tightly packed RGBA8 rows, top row first.
    ///
    /// The bytes are sRGB-encoded.
    ///
    /// # Errors
    /// - [`RenderError::NotOffscreen`] for window renderers.
    /// - [`RenderError::OutOfMemory`] if the staging buffer cannot be allocated.
    /// - [`RenderError::Backend`] if the copy or mapping the staging buffer fails.
    pub fn read_offscreen_rgba(&mut self) -> Result<Vec<u8>, RenderError> {
        match &self.target {
            Target::Offscreen(target) => target.read_rgba(&self.context).map_err(map_gpu_error),
            Target::Window(_) => Err(RenderError::NotOffscreen),
        }
    }

    /// The stage pass graph slots the most recent [`Renderer::render_stage`] call executed, in
    /// execution order (plan 0002 WP3.5): [`RenderLayer::ORDER`] after a rendered frame, empty
    /// before the first frame and after a frame skipped for a zero-size target.
    ///
    /// A diagnostic hook for the structural layer-order test (contract §6, "Ein Strukturtest
    /// vergleicht die vom Pass-Graph protokollierte Pass-Reihenfolge mit `ORDER`"), in the same
    /// spirit as [`WgpuRenderer::render_stage_with_specular_aa`]: it reads the log the pass graph
    /// writes while it submits the GPU passes, so it reports the order they actually ran in.
    #[must_use]
    pub fn last_stage_pass_order(&self) -> &[RenderLayer] {
        self.last_pass_order.layers()
    }

    /// Whether this renderer measures GPU pass time (its device has timestamp queries).
    #[cfg(test)]
    pub(crate) fn gpu_timer_enabled(&self) -> bool {
        self.gpu_timer.is_enabled()
    }

    /// Single greppable line identifying the selected adapter for CI logs; see
    /// [`grimoire_gpu::GpuContext::adapter_report_line`] (WP2.1).
    #[must_use]
    pub fn adapter_report_line(&self) -> String {
        self.context.adapter_report_line()
    }

    /// Registers `mesh`'s CPU geometry, uploads it to the GPU as static vertex/index buffers, and
    /// returns a [`MeshHandle`] [`crate::MeshInstance::mesh`] can reference afterwards (contract
    /// §6: "the registry itself... is WP2.3's job", plan 0002 WP2.3). Registration is
    /// deterministic: handles are assigned in call order, starting at `0` for the first mesh
    /// registered with this renderer.
    ///
    /// Static: `mesh`'s buffers are uploaded once here and reused by every
    /// [`crate::MeshInstance`] referencing this handle across every later frame, never re-uploaded
    /// or mutated by [`Renderer::render_stage`].
    ///
    /// # Errors
    /// [`MeshError`] if `mesh` is structurally invalid ([`crate::MeshData::validate`]) or its GPU
    /// buffers could not be created (for example out of memory). Never panics.
    pub fn register_mesh(&mut self, mesh: MeshData) -> Result<MeshHandle, MeshError> {
        self.mesh_pass.register(&self.context, mesh)
    }

    /// Registers `texture`'s CPU pixels, uploads them to the GPU, and returns a
    /// [`TextureHandle`] a [`crate::PbrMaterial`]'s texture slots can reference afterwards (plan
    /// 0002 WP2.5, mirroring [`WgpuRenderer::register_mesh`]). Registration is deterministic:
    /// handles are assigned in call order, starting at `0` for the first texture registered with
    /// this renderer.
    ///
    /// A material referencing a handle this renderer never registered (or one from a different
    /// renderer) is not an error: [`Renderer::render_stage`] falls back to that texture slot's
    /// plain factor instead, exactly as if the slot were `None` (`mesh.wgsl`'s header comment).
    ///
    /// # Errors
    /// [`TextureError`] if `texture` is structurally invalid ([`TextureData::validate`]) or its
    /// GPU texture could not be created (for example out of memory). Never panics.
    pub fn register_texture(
        &mut self,
        texture: TextureData,
    ) -> Result<TextureHandle, TextureError> {
        self.mesh_pass.register_texture(&self.context, texture)
    }

    /// Measurement hook (texture-quality package, strand B1): registers `texture` exactly like
    /// [`WgpuRenderer::register_texture`], except it uploads only mip level 0 — no CPU-side mip
    /// chain generation at all — so `tests/offscreen.rs` can measure
    /// [`WgpuRenderer::register_texture`]'s relative cost against a no-mipmap baseline on the same
    /// public API a game would use. Mirrors [`WgpuRenderer::render_stage_with_specular_aa`]'s
    /// reason for existing (OF-3.5), applied to mip generation instead.
    ///
    /// **Not part of the render contract** and not meant for production use: a game always calls
    /// [`WgpuRenderer::register_texture`]. May disappear once this measurement is no longer
    /// needed.
    ///
    /// # Errors
    /// Same as [`WgpuRenderer::register_texture`].
    pub fn register_texture_single_level_for_measurement(
        &mut self,
        texture: TextureData,
    ) -> Result<TextureHandle, TextureError> {
        self.mesh_pass
            .register_texture_single_level(&self.context, texture)
    }
}

impl WgpuRenderer {
    /// Acquires the render target for this frame: `Some(surface_frame)` (present it after
    /// submitting) plus its view for a window renderer, or `None` plus the offscreen view.
    /// Recovers `self.width`/`self.height` from the surface after acquiring, since recovery from
    /// `Outdated`/`Lost`/`Suboptimal` may have changed it. Shared by [`Renderer::render`] and
    /// [`Renderer::render_stage`] so both agree on acquisition and recovery.
    ///
    /// # Errors
    /// [`GpuError::ZeroSize`] if a window surface is not configured for a valid size (callers
    /// treat this as a skip, not a failure); any other [`GpuError`] the surface reports.
    fn acquire_target(&mut self) -> Result<(Option<SurfaceFrame>, wgpu::TextureView), GpuError> {
        match &mut self.target {
            Target::Window(surface) => {
                let acquired = surface.acquire(&self.context)?;
                (self.width, self.height) = surface.size();
                let view = acquired.view().clone();
                Ok((Some(acquired), view))
            }
            Target::Offscreen(target) => Ok((None, target.view().clone())),
        }
    }

    /// Draws `sprites` in one instanced pass into `view` through `camera`, with `load` controlling
    /// whether the pass clears (P0's [`Renderer::render`], the only caller before WP2.3) or loads
    /// existing pixels (the stage's sprite channels drawn on top of earlier layers:
    /// `render_stage`'s world sprites from WP2.3, marker and debug sprites from WP3.5). Returns
    /// `(sprites_drawn, draw_calls)`.
    fn draw_sprites(
        &mut self,
        view: &wgpu::TextureView,
        camera: &Camera2D,
        sprites: &[SpriteInstance],
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Result<(u32, u32), GpuError> {
        let aspect = self.width as f32 / self.height as f32;
        let view_projection = camera.view_projection(aspect);
        // Clip space spans 2 units over the target height.
        let pixels_per_unit = view_projection[1][1] * self.height as f32 * 0.5;
        let sprite_camera = SpriteCamera::Flat {
            view_projection,
            pixels_per_unit,
        };
        self.draw_sprites_with(view, &sprite_camera, sprites, load)
    }

    /// Draws `sprites` like [`WgpuRenderer::draw_sprites`], but through any [`SpriteCamera`]: the
    /// flat 2D path or camera-facing billboards on the ground plane (the player marker under the
    /// 2.5D camera, plan 0002 WP3.6).
    fn draw_sprites_with(
        &mut self,
        view: &wgpu::TextureView,
        sprite_camera: &SpriteCamera,
        sprites: &[SpriteInstance],
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Result<(u32, u32), GpuError> {
        let context = &self.context;
        let sprite_pass = &mut self.sprites;
        // `None` outside a measured `render_stage` frame, including the P0 `render` path.
        let timestamps = self.gpu_timer.next_pass();
        context
            .capture_errors(|device| {
                let count = sprite_pass.prepare(context, sprite_camera, sprites)?;
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("grimoire frame encoder"),
                });
                let draw_calls = {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("grimoire sprite pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            // Linear values; the sRGB view encodes them like any shaded colour.
                            ops: wgpu::Operations {
                                load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: timestamps.as_ref().map(|stamps| stamps.render()),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    sprite_pass.draw(&mut pass, count)
                };
                context.queue().submit([encoder.finish()]);
                Ok::<_, GpuError>((count, draw_calls))
            })
            .and_then(|result| result)
    }

    /// Draws the accepted instances of `bullets` in one instanced pass on top of `view` (layer 6,
    /// plan 0002 WP3.5). No render pass is recorded when nothing is accepted, so a frame without
    /// bullets submits exactly the passes it submitted before WP3.5. Returns the draw calls issued.
    fn draw_bullets(
        &mut self,
        view: &wgpu::TextureView,
        bullet_view: &BulletView,
        bullets: &[BulletInstance],
    ) -> Result<u32, GpuError> {
        if bullets.is_empty() {
            return Ok(0);
        }
        let context = &self.context;
        let bullet_pass = &mut self.bullet_pass;
        let gpu_timer = &mut self.gpu_timer;
        let viewport = (self.width, self.height);
        context
            .capture_errors(|device| {
                let count = bullet_pass.prepare(context, bullet_view, viewport, bullets)?;
                if count == 0 {
                    return Ok::<_, GpuError>(0);
                }
                // Taken only once the pass is certain to be recorded: every handed-out pair is
                // resolved at the end of the frame.
                let timestamps = gpu_timer.next_pass();
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("grimoire bullet encoder"),
                });
                let draw_calls = {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("grimoire bullet pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: timestamps.as_ref().map(|stamps| stamps.render()),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    bullet_pass.draw(&mut pass, count)
                };
                context.queue().submit([encoder.finish()]);
                Ok(draw_calls)
            })
            .and_then(|result| result)
    }
}

impl Renderer for WgpuRenderer {
    fn resize(&mut self, width: u32, height: u32) {
        self.resize_error = None;
        if width == 0 || height == 0 {
            self.width = 0;
            self.height = 0;
            return;
        }
        let resized = match &mut self.target {
            Target::Window(surface) => surface.resize(&self.context, width, height).map(|_| ()),
            Target::Offscreen(target) if (target.width(), target.height()) != (width, height) => {
                OffscreenTarget::new(&self.context, width, height).map(|resized| *target = resized)
            }
            Target::Offscreen(_) => Ok(()),
        };
        // The mesh pass's depth buffer must track the colour target's size too (WP2.3).
        let resized = resized.and_then(|()| self.mesh_pass.resize(&self.context, width, height));
        if let Err(error) = resized {
            log::error!("resize to {width}x{height} failed: {error}");
            self.width = 0;
            self.height = 0;
            self.resize_error = Some(map_resize_error(error, width, height));
            return;
        }
        self.width = width;
        self.height = height;
    }

    fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError> {
        let start = Instant::now();
        if self.context.is_device_lost() {
            return Err(RenderError::Backend(String::from("GPU device lost")));
        }
        if let Some(error) = &self.resize_error {
            return Err(repeat_error(error));
        }
        if self.width == 0 || self.height == 0 {
            return Ok(skipped_frame(start));
        }

        let (surface_frame, view) = match self.acquire_target() {
            Ok(result) => result,
            Err(GpuError::ZeroSize) => return Ok(skipped_frame(start)),
            Err(error) => return Err(map_gpu_error(error)),
        };

        let [r, g, b, a] = frame.clear_color;
        let clear_color = wgpu::Color {
            r: f64::from(r),
            g: f64::from(g),
            b: f64::from(b),
            a: f64::from(a),
        };
        let (count, draw_calls) = self
            .draw_sprites(
                &view,
                &frame.camera,
                &frame.sprites,
                wgpu::LoadOp::Clear(clear_color),
            )
            .map_err(map_gpu_error)?;
        if let Some(acquired) = surface_frame {
            acquired.present(self.context.queue());
        }

        Ok(RenderStats {
            sprites_drawn: count,
            draw_calls,
            cpu_time: start.elapsed(),
        })
    }

    fn backend_name(&self) -> &str {
        self.context.backend_name()
    }

    fn supports_stage(&self) -> bool {
        true
    }

    /// Applies the full stage semantics of contract §6 and draws every layer through the fixed
    /// pass graph (plan 0002 WP3.5, `pass_graph` module): World → VFX (empty) → post-FX resolve
    /// (empty) → telegraphy (layer 4, reserved, empty) → bullets (layer 6) → player marker
    /// (layer 7) → debug/UI. On [`crate::RenderLayer::World`] the mesh pass (this module's
    /// `mesh_pass::MeshPass`, depth-tested, real PBR shading from WP2.5) runs first, clearing
    /// colour and depth, then the unchanged sprite pass draws `frame.base.sprites` on top with
    /// `LoadOp::Load`. Bullets are validated (palette space, finiteness, `radius > 0`, table
    /// indices), counted into [`StageStats`] — including the debug-only `debug_assert!` on a
    /// foreign palette space — and, from WP3.5, the accepted ones are drawn by the bullet pass as
    /// billboards with silhouette, palette and glow; their glow also yields a few bullet-cloud
    /// lights through [`crate::point_light_from_bullet`] (the `bullet_lights` module).
    /// `marker_sprites` and `debug_sprites` are drawn by the sprite pipeline through
    /// [`crate::Camera2D`], like the world sprites. Point lights, the key light and ambient are
    /// validated and counted (as before WP2.3) and, from WP2.5, actually shaded — every valid
    /// point light through the same GGX term as the key light. From WP3.4 (engine ADR-0015
    /// "compute clustering"), shading is clustered forward+: lights are clamped to this renderer's
    /// configured [`crate::LightBudget`] (the `Low 32`/`High 256` count budget, contract §6), a
    /// bullet light's contribution is capped per [`crate::BulletLightCap`] (PRD-0003 rule 5 /
    /// FR-15), and a GPU compute pass assigns lights to froxels instead of every fragment scanning
    /// every light. `StageStats::base.draw_calls` counts every pass (contract §6: "draw_calls: alle
    /// Pässe") — the mesh pass's draw calls plus those of every sprite channel and the bullet
    /// pass. A structurally valid mesh instance whose `mesh` handle was never registered with this
    /// renderer (`WgpuRenderer::register_mesh`) is not counted in [`StageStats::meshes_drawn`] but
    /// in [`StageStats::meshes_rejected_unregistered`] instead (contract §6, PO decision V-20,
    /// 2026-09-16).
    ///
    /// # Errors
    /// Same as [`Renderer::render`], applied across every pass.
    fn render_stage(&mut self, frame: &StageFrame) -> Result<StageStats, RenderError> {
        self.render_stage_impl(frame, true)
    }
}

impl WgpuRenderer {
    /// Shared implementation of [`Renderer::render_stage`], parameterised over the OF-3.5
    /// geometric specular anti-aliasing toggle. See
    /// [`WgpuRenderer::render_stage_with_specular_aa`] for why this parameter exists.
    fn render_stage_impl(
        &mut self,
        frame: &StageFrame,
        specular_aa: bool,
    ) -> Result<StageStats, RenderError> {
        let start = Instant::now();
        if self.context.is_device_lost() {
            return Err(RenderError::Backend(String::from("GPU device lost")));
        }
        if let Some(error) = &self.resize_error {
            return Err(repeat_error(error));
        }
        // Bullet-cloud lights (plan 0002 WP3.5): derived from the frame alone, so a skipped frame
        // counts them exactly like a drawn one (and like `NullRenderer`).
        let bullet_lights = derive_bullet_lights(frame);
        if self.width == 0 || self.height == 0 {
            self.last_pass_order = PassLog::default();
            return Ok(stage::stage_stats_from_base(
                skipped_frame(start),
                frame,
                Some(&|handle: MeshHandle| self.mesh_pass.is_registered(handle)),
                Some(self.light_budget.light_count()),
                None,
                bullet_lights.as_slice(),
            ));
        }

        let (surface_frame, view) = match self.acquire_target() {
            Ok(result) => result,
            Err(GpuError::ZeroSize) => {
                self.last_pass_order = PassLog::default();
                return Ok(stage::stage_stats_from_base(
                    skipped_frame(start),
                    frame,
                    Some(&|handle: MeshHandle| self.mesh_pass.is_registered(handle)),
                    Some(self.light_budget.light_count()),
                    None,
                    bullet_lights.as_slice(),
                ));
            }
            Err(error) => return Err(map_gpu_error(error)),
        };

        let [r, g, b, a] = frame.base.clear_color;
        let clear_color = wgpu::Color {
            r: f64::from(r),
            g: f64::from(g),
            b: f64::from(b),
            a: f64::from(a),
        };
        let aspect = self.width as f32 / self.height as f32;
        // The frame's own lights first, bullet-cloud lights after them: the light budget clamp
        // drops bullet lights first (`bullet_lights` module doc comment). Without bullets this is
        // exactly `frame.point_lights`, so every pre-WP3.5 scene shades bit-identically.
        self.lights.clear();
        self.lights.extend_from_slice(&frame.point_lights);
        self.lights.extend_from_slice(bullet_lights.as_slice());

        // The fixed pass graph (contract §6, plan 0002 WP3.5): `pass_graph::run` alone decides the
        // order; every slot below only says what it draws. Each drawing slot submits its own
        // command buffer, so the recorded order is the order the GPU work was queued in.
        let mut cluster_stats = None;
        let mut sprite_count = 0;
        let mut draw_calls = 0;
        // Plan 0002 WP6.3: picks up an earlier frame's completed GPU time without waiting, then
        // times this frame's passes if no readback is pending.
        self.gpu_timer.begin_frame(&self.context);
        let pass_order = pass_graph::run(|layer| -> Result<(), GpuError> {
            match layer {
                RenderLayer::World => {
                    // Layers 1-3: meshes (clearing colour and depth), then the world sprites.
                    let mesh_pass_stats = self.mesh_pass.render(
                        &self.context,
                        &view,
                        clear_color,
                        aspect,
                        frame.camera_25d.as_ref(),
                        frame.key_light.as_ref(),
                        &frame.ambient,
                        &self.lights,
                        &frame.bullet_light_cap,
                        &frame.meshes,
                        &frame.materials,
                        specular_aa,
                        &frame.shadow_config,
                        &frame.blob_shadows,
                        &frame.joint_matrices,
                        frame.rim_light,
                        &mut self.gpu_timer,
                    )?;
                    cluster_stats = Some(mesh_pass_stats.cluster);
                    draw_calls += mesh_pass_stats.draw_calls;
                    let (count, calls) = self.draw_sprites(
                        &view,
                        &frame.base.camera,
                        &frame.base.sprites,
                        wgpu::LoadOp::Load,
                    )?;
                    sprite_count = count;
                    draw_calls += calls;
                }
                // Empty in P1: particles (VFX), the post-processing resolve and telegraphy
                // (layer 4, reserved) have no channel yet. They still occupy their slots, so
                // whatever fills them later is ordered before the bullets by construction.
                RenderLayer::Vfx | RenderLayer::PostFxResolve | RenderLayer::Telegraphy => {}
                RenderLayer::Bullets => {
                    if let Some(bullet_view) = bullet_view(frame, aspect) {
                        draw_calls += self.draw_bullets(&view, &bullet_view, &frame.bullets)?;
                    }
                }
                RenderLayer::PlayerMarker => {
                    if !frame.marker_sprites.is_empty() {
                        // Contract §6 (plan 0002 WP3.6): under the 2.5D camera the marker stands on
                        // its ground position as a camera-facing billboard, projected exactly like
                        // the bullets; without it, the flat 2D camera as before. A camera the
                        // bullet pass cannot use draws no marker either.
                        let calls = if frame.camera_25d.is_some() {
                            match bullet_view(frame, aspect) {
                                Some(ground) => {
                                    let sprite_camera = SpriteCamera::Billboard {
                                        view_projection: ground.view_proj,
                                        axis_x: ground.axis_x,
                                        axis_y: ground.axis_y,
                                        viewport: (self.width, self.height),
                                    };
                                    self.draw_sprites_with(
                                        &view,
                                        &sprite_camera,
                                        &frame.marker_sprites,
                                        wgpu::LoadOp::Load,
                                    )?
                                    .1
                                }
                                None => 0,
                            }
                        } else {
                            self.draw_sprites(
                                &view,
                                &frame.base.camera,
                                &frame.marker_sprites,
                                wgpu::LoadOp::Load,
                            )?
                            .1
                        };
                        draw_calls += calls;
                    }
                }
                RenderLayer::DebugUi => {
                    if !frame.debug_sprites.is_empty() {
                        let (_, calls) = self.draw_sprites(
                            &view,
                            &frame.base.camera,
                            &frame.debug_sprites,
                            wgpu::LoadOp::Load,
                        )?;
                        draw_calls += calls;
                    }
                }
            }
            Ok(())
        })
        .map_err(map_gpu_error)?;
        self.last_pass_order = pass_order;
        self.gpu_timer.end_frame(&self.context);

        if let Some(acquired) = surface_frame {
            acquired.present(self.context.queue());
        }

        let base_stats = RenderStats {
            sprites_drawn: sprite_count,
            draw_calls,
            cpu_time: start.elapsed(),
        };
        let mut stats = stage::stage_stats_from_base(
            base_stats,
            frame,
            Some(&|handle: MeshHandle| self.mesh_pass.is_registered(handle)),
            Some(self.light_budget.light_count()),
            cluster_stats,
            bullet_lights.as_slice(),
        );
        stats.gpu_time = self.gpu_timer.last();
        Ok(stats)
    }

    /// Measurement hook for plan 0002 OF-3.5 (WP2.5): renders exactly like
    /// [`Renderer::render_stage`], except the caller chooses whether the mesh pass's geometric
    /// specular anti-aliasing is active, instead of it always being on.
    ///
    /// **Not part of the render contract (§6)** and not meant for production rendering — a game
    /// always calls [`Renderer::render_stage`], which is equivalent to
    /// `render_stage_with_specular_aa(frame, true)`. This exists only so `tests/offscreen.rs` can
    /// compare the technique against a no-AA baseline on the same public API a game would use,
    /// without a `RendererConfig` field (contract §6: `RendererConfig` is not `#[non_exhaustive]`,
    /// so a new field would be incompatible) or an environment variable (which — unlike
    /// `GRIMOIRE_GPU_ADAPTER` — would need to be mutated per-test in a suite that runs tests
    /// in parallel). May change or disappear once OF-3.5 is decided and, if TAA is chosen instead,
    /// no longer applies.
    ///
    /// # Errors
    /// Same as [`Renderer::render_stage`].
    pub fn render_stage_with_specular_aa(
        &mut self,
        frame: &StageFrame,
        specular_aa: bool,
    ) -> Result<StageStats, RenderError> {
        self.render_stage_impl(frame, specular_aa)
    }
}
