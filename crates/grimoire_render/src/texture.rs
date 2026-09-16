//! CPU-side texture data and the texture registry (plan 0002 WP2.5; contract §6:
//! [`crate::TextureHandle`] is a frozen contract type, but — like [`crate::MeshHandle`]'s registry
//! (WP2.3) — the registry that turns raw pixels into a handle is this work package's job, not the
//! contract's).
//!
//! Mirrors the shape of `mesh.rs`: [`TextureData`] is validated CPU-side pixel data,
//! [`TextureRegistry`] assigns deterministic, sequential [`TextureHandle`]s to valid textures.
//! Uploading a registered texture's pixels to a GPU texture is `mesh_pass`'s job
//! ([`crate::WgpuRenderer::register_texture`]), exactly like [`crate::mesh::MeshRegistry`] and
//! `mesh_pass`'s own `upload_mesh`.
//!
//! This module is entirely GPU-free (no `wgpu` type appears here, per engine ADR-0002).
//!
//! **Scope (WP2.5):** only uncompressed RGBA8 pixels, no mipmaps. Texture format, compression and
//! how the Blender pipeline produces these bytes are OF-3.4, deferred to P2 alongside that
//! pipeline; this registry only needs *some* way to get pixels onto the GPU so the mesh pass's PBR
//! shader can sample [`crate::PbrMaterial`]'s optional texture slots when a game supplies them.

use crate::TextureHandle;

/// Colour space of a [`TextureData`]'s pixels (contract §6: a [`crate::PbrMaterial::base_color_texture`]
/// is sRGB-encoded and linearised on sampling; [`crate::PbrMaterial::normal_texture`] and
/// [`crate::PbrMaterial::occlusion_roughness_metallic_texture`] are linear).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureColorSpace {
    /// sRGB-encoded pixels; the GPU linearises them on sampling.
    Srgb,
    /// Linear pixels, sampled exactly as stored.
    Linear,
}

/// CPU-side RGBA8 texture, validated and registered through
/// [`crate::WgpuRenderer::register_texture`].
///
/// `pixels` is row-major, top row first, 4 bytes (R, G, B, A) per pixel, `width * height * 4`
/// bytes total — the same layout [`crate::WgpuRenderer::read_offscreen_rgba`] reads back, so a
/// tool or test can round-trip through it without a repacking step.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureData {
    /// Width in pixels; must be non-zero.
    pub width: u32,
    /// Height in pixels; must be non-zero.
    pub height: u32,
    /// Row-major RGBA8 pixels, top row first; length must be exactly `width * height * 4`.
    pub pixels: Vec<u8>,
    /// Colour space of `pixels` (contract §6).
    pub color_space: TextureColorSpace,
}

/// Failure registering a [`TextureData`] (plan 0002 WP2.5). Registration never panics: every
/// structural problem is reported through this type instead, matching [`crate::MeshError`]'s
/// style (contract §2 rule 9).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextureError {
    /// `width` or `height` is zero.
    #[error("texture has zero width or height")]
    ZeroSize,
    /// `width * height * 4` overflows `usize`/`u32` before it can be checked against
    /// `pixels.len()`.
    #[error("texture is {width}x{height}: byte-length arithmetic overflows")]
    DimensionsOverflow {
        /// The offending width.
        width: u32,
        /// The offending height.
        height: u32,
    },
    /// `pixels.len()` does not match `width * height * 4`.
    #[error(
        "texture is {width}x{height} ({expected} bytes at 4 bytes/pixel), but pixels.len() is {actual}"
    )]
    WrongByteLength {
        /// The texture's width.
        width: u32,
        /// The texture's height.
        height: u32,
        /// Expected byte length (`width * height * 4`).
        expected: usize,
        /// Actual `pixels.len()`.
        actual: usize,
    },
    /// Uploading the (structurally valid) texture's pixels to the GPU failed, for example because
    /// the device ran out of memory. Never returned by [`TextureData::validate`] itself, only by
    /// [`crate::WgpuRenderer::register_texture`].
    #[error("uploading texture pixels to the GPU failed: {0}")]
    Gpu(String),
}

impl TextureData {
    /// Validates structure without ever panicking (contract §2a style): zero dimensions, an
    /// overflowing byte-length computation and a mismatched `pixels.len()` are all reported as a
    /// [`TextureError`] instead.
    ///
    /// # Errors
    /// See [`TextureError`] (every variant except [`TextureError::Gpu`], which only a GPU upload
    /// can produce).
    pub fn validate(&self) -> Result<(), TextureError> {
        if self.width == 0 || self.height == 0 {
            return Err(TextureError::ZeroSize);
        }
        let expected = self
            .width
            .checked_mul(self.height)
            .and_then(|pixels| pixels.checked_mul(4))
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or(TextureError::DimensionsOverflow {
                width: self.width,
                height: self.height,
            })?;
        if self.pixels.len() != expected {
            return Err(TextureError::WrongByteLength {
                width: self.width,
                height: self.height,
                expected,
                actual: self.pixels.len(),
            });
        }
        Ok(())
    }
}

/// Deterministic, GPU-independent registry of validated [`TextureData`] (plan 0002 WP2.5),
/// exactly like [`crate::mesh::MeshRegistry`]: handles are assigned in registration order starting
/// at `0`.
///
/// [`crate::WgpuRenderer::register_texture`] wraps one of these to validate before uploading its
/// GPU texture. [`crate::NullRenderer`] does not get one of its own (it never samples textures).
#[derive(Debug, Clone, Default)]
pub(crate) struct TextureRegistry {
    textures: Vec<TextureData>,
}

impl TextureRegistry {
    /// Validates `texture` and, if valid, stores it under the next sequential [`TextureHandle`].
    ///
    /// # Errors
    /// See [`TextureData::validate`]; the registry is left unchanged on error.
    pub(crate) fn register(&mut self, texture: TextureData) -> Result<TextureHandle, TextureError> {
        texture.validate()?;
        let index =
            u32::try_from(self.textures.len()).map_err(|_| TextureError::DimensionsOverflow {
                width: texture.width,
                height: texture.height,
            })?;
        self.textures.push(texture);
        Ok(TextureHandle(index))
    }

    /// The registered texture for `handle`, if any.
    pub(crate) fn get(&self, handle: TextureHandle) -> Option<&TextureData> {
        self.textures.get(handle.0 as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_texture() -> TextureData {
        TextureData {
            width: 1,
            height: 1,
            pixels: vec![255, 255, 255, 255],
            color_space: TextureColorSpace::Srgb,
        }
    }

    #[test]
    fn valid_texture_passes_validation() {
        assert_eq!(tiny_texture().validate(), Ok(()));
    }

    #[test]
    fn zero_width_is_rejected_without_panic() {
        let texture = TextureData {
            width: 0,
            ..tiny_texture()
        };
        assert_eq!(texture.validate(), Err(TextureError::ZeroSize));
    }

    #[test]
    fn zero_height_is_rejected_without_panic() {
        let texture = TextureData {
            height: 0,
            ..tiny_texture()
        };
        assert_eq!(texture.validate(), Err(TextureError::ZeroSize));
    }

    #[test]
    fn wrong_byte_length_is_rejected_without_panic() {
        let texture = TextureData {
            width: 2,
            height: 2,
            pixels: vec![0; 8], // needs 16 bytes, not 8
            color_space: TextureColorSpace::Linear,
        };
        assert_eq!(
            texture.validate(),
            Err(TextureError::WrongByteLength {
                width: 2,
                height: 2,
                expected: 16,
                actual: 8,
            })
        );
    }

    #[test]
    fn dimensions_overflow_is_rejected_without_panic() {
        let texture = TextureData {
            width: u32::MAX,
            height: u32::MAX,
            pixels: Vec::new(),
            color_space: TextureColorSpace::Linear,
        };
        assert_eq!(
            texture.validate(),
            Err(TextureError::DimensionsOverflow {
                width: u32::MAX,
                height: u32::MAX,
            })
        );
    }

    #[test]
    fn registry_assigns_sequential_handles_deterministically() {
        let mut registry = TextureRegistry::default();
        let a = registry.register(tiny_texture()).expect("valid texture");
        let b = registry.register(tiny_texture()).expect("valid texture");
        assert_eq!(a, TextureHandle(0));
        assert_eq!(b, TextureHandle(1));
        assert_eq!(registry.get(a), Some(&tiny_texture()));
        assert_eq!(registry.get(b), Some(&tiny_texture()));
    }

    #[test]
    fn registry_rejects_invalid_texture_without_storing_it_or_advancing_handles() {
        let mut registry = TextureRegistry::default();
        let invalid = TextureData {
            width: 0,
            ..tiny_texture()
        };
        assert_eq!(registry.register(invalid), Err(TextureError::ZeroSize));
        let handle = registry.register(tiny_texture()).expect("valid texture");
        assert_eq!(handle, TextureHandle(0));
    }

    #[test]
    fn registry_get_of_unknown_handle_is_none_not_a_panic() {
        let registry = TextureRegistry::default();
        assert_eq!(registry.get(TextureHandle(0)), None);
        assert_eq!(registry.get(TextureHandle(u32::MAX)), None);
    }
}
