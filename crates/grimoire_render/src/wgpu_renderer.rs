//! `wgpu` implementation of [`Renderer`].

use std::sync::Arc;
use std::time::Instant;

use grimoire_gpu::{
    ContextOptions, GpuContext, GpuError, OffscreenTarget, SurfaceFrame, WindowSurface, wgpu,
};
use grimoire_platform::PlatformWindow;

use crate::mesh_pass::MeshPass;
use crate::sprite_pass::SpritePass;
use crate::stage;
use crate::{
    MeshData, MeshError, MeshHandle, RenderError, RenderFrame, RenderStats, Renderer,
    RendererConfig, StageFrame, StageStats,
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
    width: u32,
    height: u32,
    /// Error of the most recent `resize`; never `SurfaceLost`, which callers treat as a skip.
    resize_error: Option<RenderError>,
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
        let (context, surface) =
            GpuContext::new_for_window(window, context_options(&config)).map_err(map_gpu_error)?;
        let sprites = SpritePass::new(
            &context,
            surface.view_format(),
            config.initial_sprite_capacity,
        )
        .map_err(map_gpu_error)?;
        let (width, height) = if surface.is_configured() {
            surface.size()
        } else {
            (0, 0)
        };
        let mesh_pass =
            MeshPass::new(&context, surface.view_format(), width, height).map_err(map_gpu_error)?;
        Ok(Self {
            context,
            target: Target::Window(surface),
            sprites,
            mesh_pass,
            width,
            height,
            resize_error: None,
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
        let context = GpuContext::new_offscreen(context_options(&config)).map_err(map_gpu_error)?;
        let target = OffscreenTarget::new(&context, width, height).map_err(map_gpu_error)?;
        let sprites = SpritePass::new(&context, target.format(), config.initial_sprite_capacity)
            .map_err(map_gpu_error)?;
        let mesh_pass =
            MeshPass::new(&context, target.format(), width, height).map_err(map_gpu_error)?;
        Ok(Self {
            context,
            target: Target::Offscreen(target),
            sprites,
            mesh_pass,
            width,
            height,
            resize_error: None,
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

    /// Draws `frame.sprites` in one instanced pass into `view`, with `load` controlling whether
    /// the pass clears (P0's [`Renderer::render`], the only caller before WP2.3) or loads existing
    /// pixels (the stage's sprite layer drawn on top of the mesh pass, WP2.3's `render_stage`).
    /// Returns `(sprites_drawn, draw_calls)`.
    fn draw_sprites(
        &mut self,
        view: &wgpu::TextureView,
        frame: &RenderFrame,
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Result<(u32, u32), GpuError> {
        let aspect = self.width as f32 / self.height as f32;
        let view_projection = frame.camera.view_projection(aspect);
        // Clip space spans 2 units over the target height.
        let pixels_per_unit = view_projection[1][1] * self.height as f32 * 0.5;

        let context = &self.context;
        let sprites = &mut self.sprites;
        context
            .capture_errors(|device| {
                let count =
                    sprites.prepare(context, &view_projection, pixels_per_unit, &frame.sprites)?;
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
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    sprites.draw(&mut pass, count)
                };
                context.queue().submit([encoder.finish()]);
                Ok::<_, GpuError>((count, draw_calls))
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
            .draw_sprites(&view, frame, wgpu::LoadOp::Clear(clear_color))
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

    /// Applies the full stage semantics of contract §6 and, from WP2.3, actually draws pixels:
    /// the mesh pass (this module's `mesh_pass::MeshPass`, depth-tested, provisional shading) runs
    /// first, clearing colour and depth, then the unchanged sprite pass draws `frame.base.sprites`
    /// on top with `LoadOp::Load` — both share [`crate::RenderLayer::World`] per contract §6, mesh
    /// pass first. Bullets are validated (palette space, finiteness, `radius > 0`) and counted
    /// into [`StageStats`] — including the debug-only `debug_assert!` on a foreign palette space —
    /// but still never rasterised: the GPU bullet pass is WP3.5's job. `marker_sprites` and
    /// `debug_sprites` are likewise only counted, not drawn (no pipeline for them yet). Point
    /// lights, the key light and ambient are validated and counted (as before WP2.3) and now also
    /// *consumed* by the mesh pass's provisional shading (key light + ambient only; point lights
    /// stay WP3.4's clustered forward+ job). `StageStats::base.draw_calls` counts every pass
    /// (contract §6: "draw_calls: alle Pässe") — the mesh pass's draw calls plus the sprite pass's.
    /// A structurally valid mesh instance whose `mesh` handle was never registered with this
    /// renderer (`WgpuRenderer::register_mesh`) is not counted in [`StageStats::meshes_drawn`]
    /// but in [`StageStats::meshes_rejected_unregistered`] instead (contract §6, PO decision V-20,
    /// 2026-09-16).
    ///
    /// # Errors
    /// Same as [`Renderer::render`], applied across both passes.
    fn render_stage(&mut self, frame: &StageFrame) -> Result<StageStats, RenderError> {
        let start = Instant::now();
        if self.context.is_device_lost() {
            return Err(RenderError::Backend(String::from("GPU device lost")));
        }
        if let Some(error) = &self.resize_error {
            return Err(repeat_error(error));
        }
        if self.width == 0 || self.height == 0 {
            return Ok(stage::stage_stats_from_base(
                skipped_frame(start),
                frame,
                Some(&|handle: MeshHandle| self.mesh_pass.is_registered(handle)),
            ));
        }

        let (surface_frame, view) = match self.acquire_target() {
            Ok(result) => result,
            Err(GpuError::ZeroSize) => {
                return Ok(stage::stage_stats_from_base(
                    skipped_frame(start),
                    frame,
                    Some(&|handle: MeshHandle| self.mesh_pass.is_registered(handle)),
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

        let mesh_draw_calls = self
            .mesh_pass
            .render(
                &self.context,
                &view,
                clear_color,
                aspect,
                frame.camera_25d.as_ref(),
                frame.key_light.as_ref(),
                &frame.ambient,
                &frame.meshes,
                &frame.materials,
            )
            .map_err(map_gpu_error)?;

        let (sprite_count, sprite_draw_calls) = self
            .draw_sprites(&view, &frame.base, wgpu::LoadOp::Load)
            .map_err(map_gpu_error)?;

        if let Some(acquired) = surface_frame {
            acquired.present(self.context.queue());
        }

        let base_stats = RenderStats {
            sprites_drawn: sprite_count,
            draw_calls: sprite_draw_calls + mesh_draw_calls,
            cpu_time: start.elapsed(),
        };
        Ok(stage::stage_stats_from_base(
            base_stats,
            frame,
            Some(&|handle: MeshHandle| self.mesh_pass.is_registered(handle)),
        ))
    }
}
