//! Plain-old-data types uploaded to the GPU. Layouts follow the WGSL uniform layout rules.
// bytemuck's derive macros expand to `unsafe impl` blocks; this is the only module that needs them.
#![allow(unsafe_code)]

use bytemuck::{Pod, Zeroable};

/// Mesh vertex, 32 bytes. `packed` = material | class << 8 | rim kind << 16.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub packed: u32,
    pub normal: [f32; 3],
    /// 0 = no figure, otherwise figure index + 1 (written into the class MRT for the metrics).
    pub figure: u32,
}

/// Point light, 32 bytes. `color` already contains the intensity (c * I).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct LightGpu {
    pub position: [f32; 3],
    pub radius: f32,
    pub color: [f32; 3],
    pub _pad: f32,
}

pub const MAX_LIGHTS: usize = 256;
pub const MAX_BLOBS: usize = 9;

/// Per-frame uniform (400 bytes), see `Frame` in world.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FrameUniform {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub light_count: u32,
    /// Unit vector towards the key light.
    pub key_dir: [f32; 3],
    pub key_intensity: f32,
    pub key_color: [f32; 3],
    pub ambient_intensity: f32,
    pub sky: [f32; 3],
    pub decal_emission: f32,
    pub ground: [f32; 3],
    pub blob_count: u32,
    pub cam_right: [f32; 3],
    pub near: f32,
    pub cam_up: [f32; 3],
    pub far: f32,
    pub decal_color: [f32; 3],
    /// Salt of the floor tile hash (drawn from the RNG stream, same as the CPU floor mesh).
    pub floor_salt: u32,
    /// x, y, radius, 0.
    pub blobs: [[f32; 4]; MAX_BLOBS],
    /// Global light-intensity factor times the look's calibrated light gain (light-derived terms only).
    pub light_gain: f32,
    pub _pad: [f32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct MaterialGpu {
    /// rgb albedo (linear), a = specular colour follows albedo (1) or white (0).
    pub albedo: [f32; 4],
    /// rgb emissive (HDR), a = multiplier.
    pub emissive: [f32; 4],
    /// rgb shadow tint (luminance 1) or flame core colour for emissive rows, a = gloss.
    pub tint: [f32; 4],
    /// ks, roughness, metalness, unused.
    pub params: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct RimGpu {
    pub color_power: [f32; 4],
    pub strength: [f32; 4],
}

/// Emissive billboard in the World layer (friendly bolts, busy light-source dots), 48 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct BoltInstance {
    pub center: [f32; 3],
    pub half_length: f32,
    /// Direction on the gameplay plane.
    pub dir: [f32; 2],
    pub half_width: f32,
    pub alpha: f32,
    pub color: [f32; 3],
    pub intensity: f32,
}

/// Contract mirror of `grimoire_render::BulletInstance` (crate contract §6), 24 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
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

/// GPU-side bullet instance produced by the bullet pass from accepted `BulletInstance`s, 32 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct BulletGpu {
    pub position: [f32; 2],
    pub radius: f32,
    pub rotation: f32,
    pub silhouette: u32,
    pub palette: u32,
    pub glow: f32,
    pub _pad: f32,
}

/// Bullet pass uniform (224 bytes), see bullets.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BulletUniform {
    pub view_proj: [[f32; 4]; 4],
    pub cam_right: [f32; 3],
    pub plane_z: f32,
    pub cam_up: [f32; 3],
    pub rim_px: f32,
    pub body: [[f32; 4]; 2],
    pub core: [[f32; 4]; 2],
    /// rgb rim colour, a = rim alpha.
    pub rim: [f32; 4],
    /// rgb marker colour, a = marker alpha.
    pub marker: [f32; 4],
    /// x, y, ring radius, ring width.
    pub marker_ring: [f32; 4],
    pub _pad: [f32; 4],
}

/// Post-processing parameters (32 bytes), see post.wgsl. Identical for every look.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct PostUniform {
    pub bloom_intensity: f32,
    pub threshold: f32,
    pub knee: f32,
    /// Supersampling factor of the world targets (2 or 1).
    pub ss_factor: u32,
    pub bloom_levels: f32,
    pub _pad: [f32; 3],
}

/// Outline parameters (16 bytes), see outline.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct OutlineUniform {
    pub near: f32,
    pub far: f32,
    pub tap_lo: i32,
    pub tap_hi: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn layouts_match_wgsl() {
        assert_eq!(size_of::<Vertex>(), 32);
        assert_eq!(size_of::<LightGpu>(), 32);
        assert_eq!(size_of::<FrameUniform>(), 416);
        assert_eq!(size_of::<MaterialGpu>(), 64);
        assert_eq!(size_of::<RimGpu>(), 32);
        assert_eq!(size_of::<BoltInstance>(), 48);
        assert_eq!(size_of::<BulletGpu>(), 32);
        assert_eq!(size_of::<BulletUniform>(), 208 + 16);
        assert_eq!(size_of::<PostUniform>(), 32);
        assert_eq!(size_of::<OutlineUniform>(), 16);
    }

    #[test]
    fn bullet_instance_matches_the_contract_layout() {
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
    }
}
