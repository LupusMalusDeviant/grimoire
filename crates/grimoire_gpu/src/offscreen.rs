//! Offscreen render targets with CPU read-back.

use std::sync::mpsc;

use crate::context::GpuContext;
use crate::error::GpuError;
use crate::readback::{padded_bytes_per_row, strip_row_padding};

/// Pixel format of [`OffscreenTarget`]: RGBA8 with sRGB encoding on write.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A texture that can be rendered into and read back to the CPU.
#[derive(Debug)]
pub struct OffscreenTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl OffscreenTarget {
    /// Creates a `width` x `height` target in [`OFFSCREEN_FORMAT`].
    ///
    /// # Errors
    /// [`GpuError::ZeroSize`] if a dimension is zero, [`GpuError::OutOfMemory`] or
    /// [`GpuError::Validation`] if the texture cannot be created.
    pub fn new(context: &GpuContext, width: u32, height: u32) -> Result<Self, GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::ZeroSize);
        }
        let texture = context.capture_errors(|device| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("grimoire offscreen target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OFFSCREEN_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            texture,
            view,
            width,
            height,
        })
    }

    /// The target texture.
    #[must_use]
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// A view for use as a colour attachment.
    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Format of [`OffscreenTarget::view`].
    #[must_use]
    pub const fn format(&self) -> wgpu::TextureFormat {
        OFFSCREEN_FORMAT
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Copies the texture to the CPU and returns tightly packed RGBA8 rows, top row first.
    ///
    /// Blocks until all previously submitted work and the copy have finished.
    ///
    /// # Errors
    /// [`GpuError::Readback`] if mapping fails, [`GpuError::OutOfMemory`] or
    /// [`GpuError::Validation`] if the staging buffer or the copy is rejected.
    pub fn read_rgba(&self, context: &GpuContext) -> Result<Vec<u8>, GpuError> {
        let padded_row = padded_bytes_per_row(self.width).ok_or_else(|| {
            GpuError::Readback(format!("row of {} pixels overflows u32", self.width))
        })?;
        let size = u64::from(padded_row) * u64::from(self.height);
        let staging = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grimoire offscreen read-back"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;

        let device = context.device();
        let submission = context.capture_errors(|device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grimoire read-back encoder"),
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: Some(self.height),
                    },
                },
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            );
            context.queue().submit([encoder.finish()])
        })?;

        let (sender, receiver) = mpsc::channel();
        staging.map_async(wgpu::MapMode::Read, .., move |result| {
            // The receiver outlives this callback: it is awaited below before returning.
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| GpuError::Readback(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| GpuError::Readback(error.to_string()))?
            .map_err(|error| GpuError::Readback(error.to_string()))?;

        let packed = {
            let mapped = staging
                .get_mapped_range(..)
                .map_err(|error| GpuError::Readback(error.to_string()))?;
            strip_row_padding(&mapped, self.width, self.height, padded_row)
        };
        staging.unmap();
        Ok(packed)
    }
}
