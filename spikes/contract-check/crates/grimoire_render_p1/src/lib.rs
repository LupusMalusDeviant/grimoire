//! P1 additions to `grimoire_render` (contract §6, §2a). `Renderer` is mirrored: the P0 trait
//! verbatim plus the two provided P1 methods, so P0-style impls can be compiled against it.

use grimoire_render::{NullRenderer, RenderError, RenderFrame, RenderStats, SpriteInstance};

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct StageFrame {
    pub base: RenderFrame,
    pub bullets: Vec<BulletInstance>,
    pub marker_sprites: Vec<SpriteInstance>,
    pub debug_sprites: Vec<SpriteInstance>,
}

impl StageFrame {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.base.clear();
        self.bullets.clear();
        self.marker_sprites.clear();
        self.debug_sprites.clear();
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct StageStats {
    pub base: RenderStats,
    pub bullets_drawn: u32,
    pub bullets_rejected_palette_space: u32,
    pub bullets_rejected_invalid: u32,
}

pub trait Renderer {
    fn resize(&mut self, width: u32, height: u32);
    fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError>;
    fn backend_name(&self) -> &str;

    fn supports_stage(&self) -> bool {
        false
    }

    fn render_stage(&mut self, frame: &StageFrame) -> Result<StageStats, RenderError> {
        let base = self.render(&frame.base)?;
        Ok(StageStats {
            base,
            ..StageStats::default()
        })
    }
}

impl Renderer for NullRenderer {
    fn resize(&mut self, width: u32, height: u32) {
        grimoire_render::Renderer::resize(self, width, height);
    }

    fn render(&mut self, frame: &RenderFrame) -> Result<RenderStats, RenderError> {
        grimoire_render::Renderer::render(self, frame)
    }

    fn backend_name(&self) -> &str {
        "Null"
    }

    fn supports_stage(&self) -> bool {
        true
    }

    fn render_stage(&mut self, frame: &StageFrame) -> Result<StageStats, RenderError> {
        let _ = frame;
        unimplemented!()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BulletInstance {
    pub position: [f32; 2],
    pub radius: f32,
    pub rotation: f32,
    pub silhouette: u16,
    pub palette: u16,
    pub palette_space: u8,
    pub glow: u8,
    pub flags: u16,
}

pub mod palette_space {
    pub const UNASSIGNED: u8 = 0;
    pub const HOSTILE: u8 = 1;
    pub const FRIENDLY: u8 = 2;
}

pub const BULLET_PASS_PALETTE_SPACE: u8 = palette_space::HOSTILE;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RenderLayer {
    World,
    Vfx,
    PostFxResolve,
    Telegraphy,
    Bullets,
    PlayerMarker,
    DebugUi,
}

impl RenderLayer {
    pub const ORDER: [RenderLayer; 7] = [
        RenderLayer::World,
        RenderLayer::Vfx,
        RenderLayer::PostFxResolve,
        RenderLayer::Telegraphy,
        RenderLayer::Bullets,
        RenderLayer::PlayerMarker,
        RenderLayer::DebugUi,
    ];
}

/// PLACEHOLDER behind `wp2-2-camera`: the contract leaves `Camera25D` to WP2.2 (§9.2).
#[cfg(feature = "wp2-2-camera")]
pub mod placeholders {
    #[derive(Clone, Copy, Debug, PartialEq, Default)]
    pub struct Camera25D;

    impl Camera25D {
        pub fn screen_to_ground(&self, pixel: [f32; 2], viewport: [f32; 2]) -> Option<[f32; 2]> {
            let _ = (pixel, viewport);
            unimplemented!()
        }
    }
}

#[cfg(feature = "conformance")]
pub mod conformance {
    use super::Renderer;

    pub fn renderer(renderer: &mut dyn Renderer) {
        let _ = renderer;
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn bullet_instance_layout() {
        assert_eq!(size_of::<BulletInstance>(), 24);
        let offsets = [
            offset_of!(BulletInstance, position),
            offset_of!(BulletInstance, radius),
            offset_of!(BulletInstance, rotation),
            offset_of!(BulletInstance, silhouette),
            offset_of!(BulletInstance, palette),
            offset_of!(BulletInstance, palette_space),
            offset_of!(BulletInstance, glow),
            offset_of!(BulletInstance, flags),
        ];
        assert_eq!(offsets, [0, 8, 12, 16, 18, 20, 21, 22]);
        let _object_safe: Option<&mut dyn Renderer> = None;
        assert!(RenderLayer::ORDER.windows(2).all(|w| w[0] < w[1]));
    }
}
