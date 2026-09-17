//! The `lights_stage` scene (plan 0002 WP3.7), shared by the example window
//! (`examples/lights_stage/main.rs`) and the offscreen GIF showcase
//! (`tests/lights_stage_showcase.rs`, included there with `#[path]`), so the GIF shows exactly what
//! the example draws.
//!
//! The M2 showcase of WP3, one seamless loop over `t` in `0.0..1.0`:
//! - clustered forward+ at the High budget: a field of 192 flickering candle lights, three rings
//!   of 48 coloured lights orbiting the altar, 240 point lights in total (WP3.4);
//! - the bullet layer on top (WP3.5): five spiral arms of hostile bullets with glow streaming out
//!   of the altar, whose glow derives up to eight bullet-cloud lights through the renderer's only
//!   bullet-light path, filling the High budget of 256;
//! - the player marker circling on the ground plane under the tilted camera (WP3.6);
//! - pillars, an altar and a dim key light with its shadow map, so the point lights carry the
//!   picture.

// The example and the showcase test each use a different subset of this module.
#![allow(dead_code)]

use std::f32::consts::TAU;

use grimoire_render::procedural::{altar_block, floor_tile_grid, icosphere, octagonal_pillar};
use grimoire_render::{
    AmbientLight, BULLET_PASS_PALETTE_SPACE, BulletInstance, Camera25D, DirectionalLight,
    LightBudget, MaterialHandle, MeshHandle, MeshInstance, PbrMaterial, PointLight, RendererConfig,
    ShadowConfig, ShadowMode, SpriteInstance, StageFrame, StageRendererConfig, WgpuRenderer,
    bullet_palette, bullet_silhouette, shape,
};

/// Candle lights on the floor field.
pub const CANDLES: u32 = 192;
/// Coloured lights per orbiting ring.
pub const ORBIT_LIGHTS_PER_RING: u32 = 16;
/// Orbiting rings of coloured lights.
pub const ORBIT_RINGS: u32 = 3;
/// Spiral arms of the bullet pattern.
pub const BULLET_ARMS: u32 = 5;
/// Bullets per spiral arm.
pub const BULLETS_PER_ARM: u32 = 48;
/// Pillars around the arena.
const PILLARS: u32 = 8;
/// Distance of the pillars from the centre.
const PILLAR_RING: f32 = 15.0;

/// The renderer configuration the scene needs: the High light budget for 256 lights.
#[must_use]
pub fn renderer_config(base: RendererConfig) -> StageRendererConfig {
    let mut config = StageRendererConfig::default();
    config.base = base;
    config.light_budget = LightBudget::High;
    config
}

/// Mesh handles of the scene, registered once per renderer.
#[derive(Debug, Clone, Copy)]
pub struct Meshes {
    floor: MeshHandle,
    pillar: MeshHandle,
    altar: MeshHandle,
    orb: MeshHandle,
}

/// Registers the scene's procedural meshes with `renderer`.
///
/// # Panics
/// Only if a procedural mesh stops validating, which `grimoire_render`'s own tests catch.
pub fn register_meshes(renderer: &mut WgpuRenderer) -> Meshes {
    let mut register = |data| renderer.register_mesh(data).expect("valid procedural mesh");
    Meshes {
        floor: register(floor_tile_grid(24, 2.0)),
        pillar: register(octagonal_pillar(0.7, 7.0)),
        altar: register(altar_block(3.0, 3.0, 1.4)),
        orb: register(icosphere(2, 0.45)),
    }
}

fn translation(t: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

fn mesh(mesh: MeshHandle, material: u32, position: [f32; 3]) -> MeshInstance {
    let mut instance = MeshInstance::default();
    instance.mesh = mesh;
    instance.material = MaterialHandle(material);
    instance.transform = translation(position);
    instance
}

fn material(base_color: [f32; 3], metallic: f32, roughness: f32) -> PbrMaterial {
    let mut material = PbrMaterial::default();
    material.base_color_factor = [base_color[0], base_color[1], base_color[2], 1.0];
    material.metallic_factor = metallic;
    material.roughness_factor = roughness;
    material
}

fn point_light(position: [f32; 3], color: [f32; 3], intensity: f32, range: f32) -> PointLight {
    let mut light = PointLight::default();
    light.position = position;
    light.color = color;
    light.intensity = intensity;
    light.range = range;
    light
}

/// A cheap, deterministic pseudo-random value in `0.0..1.0` for `index` and `salt`.
fn hash01(index: u32, salt: u32) -> f32 {
    let mut x = index.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// Rebuilds `frame` for the loop position `t` in `0.0..1.0` (values outside wrap). Keeps the
/// frame's allocations: it clears and refills every channel.
pub fn build_frame(meshes: &Meshes, t: f32, frame: &mut StageFrame) {
    let t = t.rem_euclid(1.0);
    let phase = t * TAU;
    frame.clear();
    frame.base.clear_color = [0.012, 0.012, 0.02, 1.0];

    let mut camera = Camera25D::default();
    camera.target = [2.0 * phase.sin(), 1.5 + 1.5 * phase.cos()];
    camera.tilt_degrees = 62.0;
    camera.fov_y_degrees = 50.0;
    camera.distance = 32.0;
    frame.camera_25d = Some(camera);

    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.3, 0.45, -0.85];
    key_light.color = [0.45, 0.52, 0.75];
    key_light.intensity = 0.6;
    frame.key_light = Some(key_light);
    frame.ambient = AmbientLight::Hemisphere {
        sky_color: [0.06, 0.06, 0.1],
        ground_color: [0.02, 0.018, 0.02],
        intensity: 0.5,
    };
    let mut shadows = ShadowConfig::default();
    shadows.mode = ShadowMode::KeyLight;
    frame.shadow_config = shadows;

    frame.materials.push(material([0.42, 0.4, 0.38], 0.0, 0.55)); // 0: floor
    frame.materials.push(material([0.3, 0.29, 0.3], 0.0, 0.8)); // 1: pillars
    frame
        .materials
        .push(material([0.08, 0.06, 0.07], 0.2, 0.35)); // 2: altar
    frame.materials.push(material([0.85, 0.8, 0.75], 1.0, 0.25)); // 3: orbs

    frame.meshes.push(mesh(meshes.floor, 0, [0.0, 0.0, 0.0]));
    frame.meshes.push(mesh(meshes.altar, 2, [0.0, 0.0, 0.7]));
    for index in 0..PILLARS {
        let angle = index as f32 / PILLARS as f32 * TAU + TAU / 16.0;
        let position = [PILLAR_RING * angle.cos(), PILLAR_RING * angle.sin(), 3.5];
        frame.meshes.push(mesh(meshes.pillar, 1, position));
    }

    // Candle field: warm lights scattered over the floor, each flickering on its own phase.
    for index in 0..CANDLES {
        let radius = 4.0 + 18.0 * hash01(index, 1).sqrt();
        let angle = hash01(index, 2) * TAU;
        let flicker_phase = hash01(index, 3) * TAU;
        let flicker = 0.75 + 0.25 * (phase * 3.0 + flicker_phase).sin();
        let warmth = hash01(index, 4);
        frame.point_lights.push(point_light(
            [radius * angle.cos(), radius * angle.sin(), 0.5],
            [1.0, 0.45 + 0.25 * warmth, 0.12 + 0.1 * warmth],
            2.2 * flicker,
            2.4,
        ));
    }

    // Three rings of coloured lights orbiting the altar, alternating direction; every fourth light
    // carries a metallic orb that shows where the ring runs.
    let ring_colors = [[0.9, 0.2, 1.0], [0.2, 0.8, 1.0], [0.4, 1.0, 0.35]];
    for ring in 0..ORBIT_RINGS {
        let radius = 5.0 + 3.5 * ring as f32;
        let direction = if ring % 2 == 0 { 1.0 } else { -1.0 };
        for index in 0..ORBIT_LIGHTS_PER_RING {
            let angle = index as f32 / ORBIT_LIGHTS_PER_RING as f32 * TAU + direction * phase;
            let (x, y) = (radius * angle.cos(), radius * angle.sin());
            frame.point_lights.push(point_light(
                [x, y, 1.0],
                ring_colors[ring as usize],
                5.0,
                4.5,
            ));
            // Above its light, so the light glints off the orb instead of sitting inside it.
            if index % 4 == 0 {
                frame.meshes.push(mesh(meshes.orb, 3, [x, y, 2.1]));
            }
        }
    }

    // Five spiral arms of hostile bullets streaming out of the altar: every bullet moves outward by
    // one spacing per loop, so the loop is seamless (one bullet leaves at the rim as the next one
    // appears at the altar).
    let silhouettes = [
        bullet_silhouette::ORB,
        bullet_silhouette::RICE,
        bullet_silhouette::DIAMOND,
    ];
    for arm in 0..BULLET_ARMS {
        let arm_angle = arm as f32 / BULLET_ARMS as f32 * TAU;
        for index in 0..BULLETS_PER_ARM {
            let progress = (index as f32 + t) / BULLETS_PER_ARM as f32;
            let radius = 2.2 + 20.0 * progress;
            let angle = arm_angle + progress * 2.2;
            let heading = angle + std::f32::consts::FRAC_PI_2 * 0.35;
            frame.bullets.push(BulletInstance {
                position: [radius * angle.cos(), radius * angle.sin()],
                radius: 0.45,
                rotation: heading,
                silhouette: silhouettes[(arm % 3) as usize],
                palette: if arm % 2 == 0 {
                    bullet_palette::HEX_MAGENTA
                } else {
                    bullet_palette::POISON_LIME
                },
                palette_space: BULLET_PASS_PALETTE_SPACE,
                glow: 200,
                flags: 0,
            });
        }
    }

    // The player marker circling the altar on the ground plane.
    let marker_angle = -phase;
    frame.marker_sprites.push(SpriteInstance {
        position: [9.0 * marker_angle.cos(), 9.0 * marker_angle.sin()],
        half_size: [0.9, 0.9],
        rotation: 0.0,
        shape: shape::CIRCLE,
        color: [0.55, 0.85, 1.0, 1.0],
    });
}
