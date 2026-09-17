//! Registering GPU assets from plugins (plan 0002 P1, PO decision 2026-09-17 "Fassaden-Haken für
//! Assets", contract §9.2).
//!
//! The main loop owns its renderer, so a plugin cannot call `WgpuRenderer::register_mesh` itself.
//! Instead, once per run and right after every plugin's [`crate::GamePlugin::build`], the loop
//! hands each plugin a [`RenderAssets`] in [`crate::GamePlugin::register_assets`]: the real
//! renderer in [`crate::AppBuilder::run`] and [`crate::AppBuilder::run_offscreen`], a
//! [`HeadlessRenderAssets`] (validation and handles, no GPU) in
//! [`crate::AppBuilder::run_headless_frames`]. The plugin keeps the returned handles in its own
//! fields and uses them in [`crate::GamePlugin::extract_stage`]; handles never reach the
//! simulation, so no state hash depends on them.
//!
//! Registration stays renderer-specific (contract §6, clarification of 2026-09-16): this trait
//! lives in the facade, and `grimoire_render::Renderer` is unchanged.

use std::error::Error;

use grimoire_render::{
    MeshData, MeshError, MeshHandle, NullRenderer, Renderer, TextureData, TextureError,
    TextureHandle, WgpuRenderer,
};

/// Error a plugin returns from [`crate::GamePlugin::register_assets`]; any error type converts
/// into it with `?` or `.into()`.
pub type PluginError = Box<dyn Error + Send + Sync + 'static>;

/// Where a plugin registers meshes and textures (contract §9.2). Object-safe.
///
/// Handles are assigned in call order per kind, starting at 0, by every implementation in this
/// crate, so a plugin sees the same handles on the desktop, offscreen and headless.
pub trait RenderAssets {
    /// Validates `mesh`, uploads it and returns its handle for
    /// [`grimoire_render::MeshInstance::mesh`].
    ///
    /// # Errors
    /// [`MeshError`] if `mesh` is invalid ([`MeshData::validate`]) or cannot be uploaded.
    fn register_mesh(&mut self, mesh: MeshData) -> Result<MeshHandle, MeshError>;

    /// Validates `texture`, uploads it and returns its handle for the texture slots of
    /// [`grimoire_render::PbrMaterial`].
    ///
    /// # Errors
    /// [`TextureError`] if `texture` is invalid ([`TextureData::validate`]) or cannot be
    /// uploaded.
    fn register_texture(&mut self, texture: TextureData) -> Result<TextureHandle, TextureError>;
}

impl RenderAssets for WgpuRenderer {
    fn register_mesh(&mut self, mesh: MeshData) -> Result<MeshHandle, MeshError> {
        WgpuRenderer::register_mesh(self, mesh)
    }

    fn register_texture(&mut self, texture: TextureData) -> Result<TextureHandle, TextureError> {
        WgpuRenderer::register_texture(self, texture)
    }
}

/// [`RenderAssets`] without a GPU: validates like `WgpuRenderer` and hands out the same handles,
/// but keeps and uploads nothing. The headless frame loop uses it, and tests can call a plugin's
/// [`crate::GamePlugin::register_assets`] with it directly.
///
/// A mesh drawn later through `NullRenderer` counts as drawn regardless: `NullRenderer` has no
/// registry (contract §6), so these handles are never checked against one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadlessRenderAssets {
    meshes: u32,
    textures: u32,
}

impl HeadlessRenderAssets {
    /// No asset registered yet. Identical to [`HeadlessRenderAssets::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of meshes registered successfully.
    #[must_use]
    pub fn meshes_registered(&self) -> u32 {
        self.meshes
    }

    /// Number of textures registered successfully.
    #[must_use]
    pub fn textures_registered(&self) -> u32 {
        self.textures
    }
}

impl RenderAssets for HeadlessRenderAssets {
    fn register_mesh(&mut self, mesh: MeshData) -> Result<MeshHandle, MeshError> {
        mesh.validate()?;
        let handle = MeshHandle(self.meshes);
        self.meshes = self
            .meshes
            .checked_add(1)
            .ok_or(MeshError::TooManyElements)?;
        Ok(handle)
    }

    fn register_texture(&mut self, texture: TextureData) -> Result<TextureHandle, TextureError> {
        texture.validate()?;
        let handle = TextureHandle(self.textures);
        // The renderer's texture registry reports an exhausted handle space the same way.
        self.textures = self
            .textures
            .checked_add(1)
            .ok_or(TextureError::DimensionsOverflow {
                width: texture.width,
                height: texture.height,
            })?;
        Ok(handle)
    }
}

/// How the main loop reaches a [`RenderAssets`] for its renderer type (crate-internal: the loop is
/// generic over `Renderer`, and registration is not part of that trait).
pub(crate) trait LoopRenderer: Renderer {
    /// The registration target of this renderer; `headless` stands in for renderers without GPU
    /// resources of their own.
    fn render_assets<'a>(
        &'a mut self,
        headless: &'a mut HeadlessRenderAssets,
    ) -> &'a mut dyn RenderAssets;
}

impl LoopRenderer for WgpuRenderer {
    fn render_assets<'a>(
        &'a mut self,
        _headless: &'a mut HeadlessRenderAssets,
    ) -> &'a mut dyn RenderAssets {
        self
    }
}

impl LoopRenderer for NullRenderer {
    fn render_assets<'a>(
        &'a mut self,
        headless: &'a mut HeadlessRenderAssets,
    ) -> &'a mut dyn RenderAssets {
        headless
    }
}

#[cfg(test)]
mod tests {
    use grimoire_render::{MeshVertex, TextureColorSpace};

    use super::*;

    fn triangle() -> MeshData {
        MeshData {
            vertices: vec![MeshVertex::default(); 3],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn headless_assets_validate_and_number_like_the_renderer() {
        let mut assets = HeadlessRenderAssets::new();
        assert_eq!(assets.register_mesh(triangle()), Ok(MeshHandle(0)));
        assert_eq!(
            assets.register_mesh(MeshData::default()),
            Err(MeshError::EmptyVertices),
            "an invalid mesh takes no handle"
        );
        assert_eq!(assets.register_mesh(triangle()), Ok(MeshHandle(1)));
        let texture = TextureData {
            width: 1,
            height: 1,
            pixels: vec![255; 4],
            color_space: TextureColorSpace::Srgb,
        };
        assert_eq!(assets.register_texture(texture), Ok(TextureHandle(0)));
        let broken = TextureData {
            width: 2,
            height: 2,
            pixels: vec![0; 3],
            color_space: TextureColorSpace::Linear,
        };
        assert!(assets.register_texture(broken).is_err());
        assert_eq!(
            (assets.meshes_registered(), assets.textures_registered()),
            (2, 1)
        );
    }

    #[test]
    fn render_assets_is_object_safe() {
        let mut headless = HeadlessRenderAssets::new();
        let assets: &mut dyn RenderAssets = &mut headless;
        assert!(assets.register_mesh(triangle()).is_ok());
    }
}
