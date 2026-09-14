//! Window surfaces: configuration, resize and frame acquisition.

use std::fmt;
use std::sync::Arc;

use grimoire_platform::PlatformWindow;

use crate::context::GpuContext;
use crate::error::GpuError;

/// Picks the surface format, in order of preference: a native sRGB format, a format with an sRGB
/// variant (rendered through an sRGB view, see [`WindowSurface::view_format`]), otherwise the
/// first format. Returns `None` for an empty list.
#[must_use]
pub fn select_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .or_else(|| {
            formats
                .iter()
                .copied()
                .find(|format| format.add_srgb_suffix().is_srgb())
        })
        .or_else(|| formats.first().copied())
}

/// Picks the present mode: `Fifo` with vsync; without vsync `Mailbox`, then `Immediate` if
/// supported, otherwise `Fifo` (which every surface supports).
#[must_use]
pub fn select_present_mode(supported: &[wgpu::PresentMode], vsync: bool) -> wgpu::PresentMode {
    if vsync {
        return wgpu::PresentMode::Fifo;
    }
    [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
        .into_iter()
        .find(|mode| supported.contains(mode))
        .unwrap_or(wgpu::PresentMode::Fifo)
}

/// An acquired window frame. Render into [`SurfaceFrame::view`], submit, then call
/// [`SurfaceFrame::present`]. Dropping it without presenting discards the frame.
pub struct SurfaceFrame {
    texture: wgpu::SurfaceTexture,
    view: wgpu::TextureView,
}

impl fmt::Debug for SurfaceFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SurfaceFrame").finish_non_exhaustive()
    }
}

impl SurfaceFrame {
    /// Colour attachment view in [`WindowSurface::view_format`].
    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Schedules the frame for presentation. Call after submitting the work that renders it.
    pub fn present(self, queue: &wgpu::Queue) {
        queue.present(self.texture);
    }
}

/// A `wgpu` surface bound to a [`PlatformWindow`].
pub struct WindowSurface {
    surface: wgpu::Surface<'static>,
    // Keeps the window alive for as long as the surface uses its handles.
    window: Arc<dyn PlatformWindow>,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
    configured: bool,
    needs_reconfigure: bool,
}

impl fmt::Debug for WindowSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowSurface")
            .field("config", &self.config)
            .field("view_format", &self.view_format)
            .field("configured", &self.configured)
            .finish_non_exhaustive()
    }
}

impl WindowSurface {
    pub(crate) fn new(
        surface: wgpu::Surface<'static>,
        window: Arc<dyn PlatformWindow>,
        context: &GpuContext,
        vsync: bool,
    ) -> Result<Self, GpuError> {
        let capabilities = surface.get_capabilities(context.adapter());
        let format = select_surface_format(&capabilities.formats).ok_or_else(|| {
            GpuError::NoAdapter(String::from("adapter cannot present to this surface"))
        })?;
        let present_mode = select_present_mode(&capabilities.present_modes, vsync);
        // Without a native sRGB format the sRGB encoding comes from an sRGB view of the target.
        let view_format = format.add_srgb_suffix();
        if !view_format.is_srgb() {
            log::warn!(
                "surface offers no sRGB-capable format; {format:?} receives linear colours unencoded"
            );
        }
        let view_formats = if view_format == format {
            Vec::new()
        } else {
            vec![view_format]
        };
        let size = window.inner_size();
        log::info!(
            "surface format {format:?} (view {view_format:?}), present mode {present_mode:?}"
        );
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width,
            height: size.height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats,
        };
        let mut this = Self {
            surface,
            window,
            config,
            view_format,
            configured: false,
            needs_reconfigure: false,
        };
        this.resize(context, size.width, size.height)?;
        Ok(this)
    }

    /// Format of the views returned by [`WindowSurface::acquire`]; sRGB-encoded unless the
    /// surface offers no sRGB-capable format at all (a warning is logged in that case).
    #[must_use]
    pub const fn view_format(&self) -> wgpu::TextureFormat {
        self.view_format
    }

    /// Current configured size in physical pixels (may be stale while the window is minimised).
    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Present mode chosen at creation.
    #[must_use]
    pub const fn present_mode(&self) -> wgpu::PresentMode {
        self.config.present_mode
    }

    /// Whether the surface has been configured with a non-zero size and can deliver frames.
    #[must_use]
    pub const fn is_configured(&self) -> bool {
        self.configured
    }

    /// The window this surface presents to.
    #[must_use]
    pub fn window(&self) -> &Arc<dyn PlatformWindow> {
        &self.window
    }

    /// Reconfigures the surface for a new size. Zero sizes are ignored and return `Ok(false)`;
    /// the previous configuration stays in place until a valid size arrives.
    ///
    /// # Errors
    /// [`GpuError::Validation`] or [`GpuError::OutOfMemory`] if `wgpu` rejects the configuration
    /// (for example a size above the device's texture limit); the surface is then unconfigured.
    pub fn resize(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
    ) -> Result<bool, GpuError> {
        if width == 0 || height == 0 {
            return Ok(false);
        }
        self.config.width = width;
        self.config.height = height;
        let configured =
            context.capture_errors(|device| self.surface.configure(device, &self.config));
        self.configured = configured.is_ok();
        self.needs_reconfigure = false;
        configured.map(|()| true)
    }

    /// Acquires the next frame.
    ///
    /// # Errors
    /// - [`GpuError::ZeroSize`] if the surface has no valid configuration.
    /// - [`GpuError::SurfaceLost`] if the surface was outdated (reconfigured) or lost (recreated
    ///   and reconfigured); acquire again next frame.
    /// - [`GpuError::SurfaceUnavailable`] on timeout or occlusion; skip this frame.
    /// - [`GpuError::CreateSurface`] if a lost surface cannot be recreated.
    /// - [`GpuError::Validation`] or [`GpuError::OutOfMemory`] if `wgpu` reported such an error.
    ///
    /// # Panics
    /// On macOS (Metal), recreating a lost surface panics off the main thread; call this from the
    /// thread that runs the platform event loop.
    pub fn acquire(&mut self, context: &GpuContext) -> Result<SurfaceFrame, GpuError> {
        if !self.configured {
            return Err(GpuError::ZeroSize);
        }
        if self.needs_reconfigure {
            self.reconfigure(context)?;
        }
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                // Reconfiguring needs the texture released, so it happens on the next acquire.
                self.needs_reconfigure = true;
                texture
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Err(GpuError::SurfaceUnavailable);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                log::warn!("surface outdated; reconfiguring");
                self.reconfigure(context)?;
                return Err(GpuError::SurfaceLost);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                // A lost surface cannot be revived by `configure`; it has to be recreated.
                log::warn!("surface lost; recreating");
                self.configured = false;
                let surface = context
                    .instance()
                    .create_surface(Arc::clone(&self.window))?;
                // Drop the old surface (and its swap chain) before configuring the new one.
                self.surface = surface;
                self.reconfigure(context)?;
                return Err(GpuError::SurfaceLost);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(GpuError::Validation(String::from(
                    "surface texture acquisition failed validation",
                )));
            }
        };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("grimoire surface view"),
            format: Some(self.view_format),
            ..Default::default()
        });
        Ok(SurfaceFrame { texture, view })
    }

    fn reconfigure(&mut self, context: &GpuContext) -> Result<(), GpuError> {
        let size = self.window.inner_size();
        let (width, height) = if size.is_empty() {
            (self.config.width, self.config.height)
        } else {
            (size.width, size.height)
        };
        self.resize(context, width, height).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{PresentMode, TextureFormat};

    #[test]
    fn prefers_srgb_format() {
        let formats = [TextureFormat::Bgra8Unorm, TextureFormat::Bgra8UnormSrgb];
        assert_eq!(
            select_surface_format(&formats),
            Some(TextureFormat::Bgra8UnormSrgb)
        );
    }

    #[test]
    fn prefers_format_with_srgb_variant_over_linear_only_format() {
        let formats = [TextureFormat::Rgb10a2Unorm, TextureFormat::Bgra8Unorm];
        assert_eq!(
            select_surface_format(&formats),
            Some(TextureFormat::Bgra8Unorm)
        );
    }

    #[test]
    fn falls_back_to_first_format() {
        let formats = [TextureFormat::Rgba16Float, TextureFormat::Rgb10a2Unorm];
        assert_eq!(
            select_surface_format(&formats),
            Some(TextureFormat::Rgba16Float)
        );
        assert_eq!(select_surface_format(&[]), None);
    }

    #[test]
    fn vsync_always_uses_fifo() {
        let modes = [
            PresentMode::Mailbox,
            PresentMode::Immediate,
            PresentMode::Fifo,
        ];
        assert_eq!(select_present_mode(&modes, true), PresentMode::Fifo);
    }

    #[test]
    fn no_vsync_prefers_mailbox_then_immediate_then_fifo() {
        let all = [
            PresentMode::Fifo,
            PresentMode::Immediate,
            PresentMode::Mailbox,
        ];
        assert_eq!(select_present_mode(&all, false), PresentMode::Mailbox);
        let immediate = [PresentMode::Fifo, PresentMode::Immediate];
        assert_eq!(
            select_present_mode(&immediate, false),
            PresentMode::Immediate
        );
        let fifo = [PresentMode::Fifo];
        assert_eq!(select_present_mode(&fifo, false), PresentMode::Fifo);
    }
}
