//! `wgpu` implementation of [`Renderer`].

use std::sync::Arc;
use std::time::Instant;

use grimoire_gpu::{ContextOptions, GpuContext, GpuError, OffscreenTarget, WindowSurface, wgpu};
use grimoire_platform::PlatformWindow;

use crate::sprite_pass::SpritePass;
use crate::{RenderError, RenderFrame, RenderStats, Renderer, RendererConfig};

enum Target {
    Window(WindowSurface),
    Offscreen(OffscreenTarget),
}

/// GPU renderer based on `wgpu`.
///
/// Draws all sprites of a frame with one instanced draw call. Colours in [`RenderFrame`] are
/// linear; the render target is written through an sRGB view, so the GPU applies the sRGB
/// encoding and alpha blending happens in linear space.
pub struct WgpuRenderer {
    context: GpuContext,
    target: Target,
    sprites: SpritePass,
    width: u32,
    height: u32,
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

fn context_options(config: &RendererConfig) -> ContextOptions {
    ContextOptions {
        high_performance: true,
        allow_software_fallback: config.allow_software_fallback,
        vsync: config.vsync,
    }
}

impl WgpuRenderer {
    /// Creates a renderer presenting to `window`.
    ///
    /// Rendering is skipped until the window has a non-zero size.
    ///
    /// # Errors
    /// [`RenderError::NoAdapter`] if no adapter can present to the window.
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
        })
    }

    /// Creates a renderer drawing into an offscreen RGBA8 texture of the given size.
    ///
    /// # Errors
    /// [`RenderError::NoAdapter`] if no adapter is available.
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
        })
    }

    /// Reads back the last rendered offscreen image as tightly packed RGBA8 rows, top row first.
    ///
    /// The bytes are sRGB-encoded.
    ///
    /// # Errors
    /// [`RenderError::NotOffscreen`] for window renderers.
    pub fn read_offscreen_rgba(&mut self) -> Result<Vec<u8>, RenderError> {
        match &self.target {
            Target::Offscreen(target) => target.read_rgba(&self.context).map_err(map_gpu_error),
            Target::Window(_) => Err(RenderError::NotOffscreen),
        }
    }
}

impl Renderer for WgpuRenderer {
    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.width = 0;
            self.height = 0;
            return;
        }
        match &mut self.target {
            Target::Window(surface) => {
                surface.resize(&self.context, width, height);
            }
            Target::Offscreen(target) => {
                if (target.width(), target.height()) != (width, height) {
                    match OffscreenTarget::new(&self.context, width, height) {
                        Ok(resized) => *target = resized,
                        Err(error) => {
                            log::error!("offscreen resize to {width}x{height} failed: {error}");
                            self.width = 0;
                            self.height = 0;
                            return;
                        }
                    }
                }
            }
        }
        self.width = width;
        self.height = height;
    }

    fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError> {
        let start = Instant::now();
        if self.width == 0 || self.height == 0 {
            return Ok(RenderStats {
                cpu_time: start.elapsed(),
                ..RenderStats::default()
            });
        }

        let (surface_frame, view) = match &mut self.target {
            Target::Window(surface) => match surface.acquire(&self.context) {
                Ok(acquired) => {
                    let view = acquired.view().clone();
                    (Some(acquired), view)
                }
                Err(GpuError::ZeroSize) => {
                    return Ok(RenderStats {
                        cpu_time: start.elapsed(),
                        ..RenderStats::default()
                    });
                }
                Err(error) => return Err(map_gpu_error(error)),
            },
            Target::Offscreen(target) => (None, target.view().clone()),
        };

        let aspect = self.width as f32 / self.height as f32;
        let view_projection = frame.camera.view_projection(aspect);
        let count = self
            .sprites
            .prepare(&self.context, &view_projection, &frame.sprites)
            .map_err(map_gpu_error)?;

        let [r, g, b, a] = frame.clear_color;
        let mut encoder =
            self.context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
            self.sprites.draw(&mut pass, count)
        };
        self.context.queue().submit([encoder.finish()]);
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
}
