//! `wgpu` implementation of [`Renderer`].

use std::sync::Arc;
use std::time::Instant;

use grimoire_gpu::{ContextOptions, GpuContext, GpuError, OffscreenTarget, WindowSurface, wgpu};
use grimoire_platform::PlatformWindow;

use crate::sprite_pass::SpritePass;
use crate::stage;
use crate::{
    RenderError, RenderFrame, RenderStats, Renderer, RendererConfig, StageFrame, StageStats,
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
        Ok(Self {
            context,
            target: Target::Window(surface),
            sprites,
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
        Ok(Self {
            context,
            target: Target::Offscreen(target),
            sprites,
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

        let (surface_frame, view) = match &mut self.target {
            Target::Window(surface) => match surface.acquire(&self.context) {
                Ok(acquired) => {
                    // Recovery from Outdated/Lost/Suboptimal may have changed the surface size.
                    (self.width, self.height) = surface.size();
                    let view = acquired.view().clone();
                    (Some(acquired), view)
                }
                Err(GpuError::ZeroSize) => return Ok(skipped_frame(start)),
                Err(error) => return Err(map_gpu_error(error)),
            },
            Target::Offscreen(target) => (None, target.view().clone()),
        };

        let aspect = self.width as f32 / self.height as f32;
        let view_projection = frame.camera.view_projection(aspect);
        // Clip space spans 2 units over the target height.
        let pixels_per_unit = view_projection[1][1] * self.height as f32 * 0.5;
        let [r, g, b, a] = frame.clear_color;

        let context = &self.context;
        let sprites = &mut self.sprites;
        let (count, draw_calls) = context
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
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                // Linear values; the sRGB view encodes them like any shaded colour.
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: f64::from(r),
                                    g: f64::from(g),
                                    b: f64::from(b),
                                    a: f64::from(a),
                                }),
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
            .map_err(map_gpu_error)?;
        if let Some(acquired) = surface_frame {
            acquired.present(context.queue());
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

    /// Applies the full stage semantics of contract §6 for extraction and counting, but — unlike
    /// the name might suggest — does **not yet draw bullet, marker or debug pixels**: it only
    /// forwards `frame.base` to the existing sprite pipeline via [`Renderer::render`], exactly as
    /// [`crate::NullRenderer`]'s `render_stage` does. Bullets are validated (palette space, finiteness,
    /// `radius > 0`) and counted into [`StageStats`] — including the debug-only
    /// `debug_assert!` on a foreign palette space — but never rasterised: a real GPU bullet pass
    /// is WP3.5's job, built on the OF-3.3 ADR. `marker_sprites` and `debug_sprites` are likewise
    /// only counted into `base.sprites_drawn`, not drawn, because no pipeline for them exists
    /// yet. This method therefore never touches the GPU device beyond what `render` already does.
    ///
    /// # Errors
    /// Same as [`Renderer::render`], applied to `frame.base`.
    fn render_stage(&mut self, frame: &StageFrame) -> Result<StageStats, RenderError> {
        let base = self.render(&frame.base)?;
        Ok(stage::stage_stats_from_base(base, frame))
    }
}
