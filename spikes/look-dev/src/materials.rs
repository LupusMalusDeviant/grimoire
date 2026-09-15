//! The single material and palette table shared by all looks.
//!
//! All colours are written as sRGB hex and converted to linear exactly once here. Base albedo is
//! identical in every look; the look-specific parameters (shadow tint and gloss for `stylized`,
//! roughness and metalness for `realistic`) sit next to it and are listed in `metrics.md`.

use crate::gpu_types::{MaterialGpu, RimGpu};

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear RGB from an sRGB hex value `0xRRGGBB`.
pub fn hex(rgb: u32) -> [f32; 3] {
    let channel = |shift: u32| srgb_to_linear(((rgb >> shift) & 0xFF) as f32 / 255.0);
    [channel(16), channel(8), channel(0)]
}

pub fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Scales a colour to luminance 1 (shadow tints of the stylized look).
pub fn unit_luminance(c: [f32; 3]) -> [f32; 3] {
    let l = luminance(c).max(1e-6);
    [c[0] / l, c[1] / l, c[2] / l]
}

pub fn mul(c: [f32; 3], s: f32) -> [f32; 3] {
    [c[0] * s, c[1] * s, c[2] * s]
}

// ---- Material ids (vertex attribute, index into the uniform table) ----
pub const FLOOR: u32 = 0;
pub const PILLAR: u32 = 2;
pub const PLINTH: u32 = 3;
pub const RUIN_WALL: u32 = 4;
pub const MOSS: u32 = 5;
pub const RUBBLE: u32 = 6;
pub const ALTAR: u32 = 7;
pub const BRONZE: u32 = 8;
pub const WAX: u32 = 9;
pub const CLOAK: u32 = 10;
pub const HOOD_INSIDE: u32 = 11;
pub const MASK: u32 = 12;
pub const WOOD: u32 = 13;
pub const IMP_SKIN: u32 = 14;
pub const HORN: u32 = 15;
pub const BRUTE_FLESH: u32 = 16;
pub const PLATES: u32 = 17;
pub const TORCH_FLAME: u32 = 18;
pub const CANDLE_FLAME: u32 = 19;
pub const EYE: u32 = 20;
pub const ORB: u32 = 21;
pub const MATERIAL_COUNT: usize = 22;
pub const MATERIAL_SLOTS: usize = 32;

// ---- Render classes (normal+class MRT alpha) ----
pub const CLASS_FLOOR: u32 = 0;
pub const CLASS_PROP: u32 = 1;
pub const CLASS_PLAYER: u32 = 2;
pub const CLASS_ENEMY: u32 = 3;
pub const CLASS_EMISSIVE: u32 = 4;

// ---- Rim kinds (stylized look only) ----
pub const RIM_NONE: u32 = 0;
pub const RIM_PLAYER: u32 = 1;
pub const RIM_IMP: u32 = 2;
pub const RIM_BRUTE: u32 = 3;

/// One row of the material table.
#[derive(Debug, Clone, Copy)]
pub struct Material {
    pub name: &'static str,
    pub albedo_hex: u32,
    /// Emissive colour and HDR multiplier (0 for lit materials).
    pub emissive_hex: u32,
    pub emissive_mult: f32,
    /// Core colour of emissive flames (hex), 0 = none.
    pub core_hex: u32,
    // stylized
    pub shadow_tint_hex: u32,
    pub gloss: f32,
    pub ks: f32,
    /// Specular colour = albedo (bronze) instead of white.
    pub spec_albedo: bool,
    // realistic
    pub roughness: f32,
    pub metalness: f32,
    /// Metal base colour F0 (hex) for metallic rows, 0 = none (non-metals use the albedo). Used as
    /// F0 in realistic and as the specular colour of `spec_albedo` rows in stylized and toon.
    pub metal_hex: u32,
}

const STONE_TINT: u32 = 0x3A3F66;
/// Base reflectance of bronze (sRGB). The albedo #6B4A2A was authored as a diffuse colour; used as F0
/// it rendered the metal almost black in the realistic look (corrected in the polish round).
pub const BRONZE_F0_HEX: u32 = 0xF2C28A;

const fn lit(
    name: &'static str,
    albedo_hex: u32,
    shadow_tint_hex: u32,
    gloss: f32,
    ks: f32,
    roughness: f32,
    metalness: f32,
) -> Material {
    Material {
        name,
        albedo_hex,
        emissive_hex: 0,
        emissive_mult: 0.0,
        core_hex: 0,
        shadow_tint_hex,
        gloss,
        ks,
        spec_albedo: false,
        roughness,
        metalness,
        metal_hex: 0,
    }
}

const fn emissive(name: &'static str, emissive_hex: u32, emissive_mult: f32, core_hex: u32) -> Material {
    Material {
        name,
        albedo_hex: 0x000000,
        emissive_hex,
        emissive_mult,
        core_hex,
        shadow_tint_hex: STONE_TINT,
        gloss: 1.0,
        ks: 0.0,
        spec_albedo: false,
        roughness: 1.0,
        metalness: 0.0,
        metal_hex: 0,
    }
}

pub const MATERIALS: [Material; MATERIAL_COUNT] = [
    lit("floor stone", 0x33363D, STONE_TINT, 12.0, 0.06, 0.85, 0.0),
    lit("grout", 0x1A1C21, STONE_TINT, 12.0, 0.06, 0.95, 0.0),
    lit("pillar stone", 0x3E4047, STONE_TINT, 12.0, 0.06, 0.75, 0.0),
    lit("plinth", 0x2C2E34, STONE_TINT, 12.0, 0.06, 0.85, 0.0),
    lit("ruin wall", 0x383A3F, STONE_TINT, 12.0, 0.06, 0.85, 0.0),
    lit("moss", 0x2E3A2A, STONE_TINT, 12.0, 0.06, 0.85, 0.0),
    lit("rubble", 0x45464A, STONE_TINT, 12.0, 0.06, 0.85, 0.0),
    lit("altar basalt", 0x2A2528, 0x2A1830, 20.0, 0.10, 0.6, 0.0),
    Material {
        spec_albedo: true,
        metal_hex: BRONZE_F0_HEX,
        ..lit("brazier bronze", 0x6B4A2A, 0x5A3A20, 32.0, 0.5, 0.35, 1.0)
    },
    lit("candle wax", 0xD8CDB4, STONE_TINT, 12.0, 0.06, 0.85, 0.0),
    lit("cloak", 0x24505C, 0x1E2A4A, 6.0, 0.03, 0.9, 0.0),
    lit("hood inside", 0x0A0C0E, 0x1E2A4A, 6.0, 0.03, 0.9, 0.0),
    lit("bone mask", 0xCFC3A8, 0x1E2A4A, 16.0, 0.08, 0.6, 0.0),
    lit("staff wood", 0x4A3524, 0x1E2A4A, 10.0, 0.05, 0.7, 0.0),
    lit("imp ash skin", 0x7A7068, 0x4A3550, 20.0, 0.12, 0.55, 0.0),
    lit("imp horns", 0xC9BBA0, 0x4A3550, 20.0, 0.12, 0.55, 0.0),
    lit("brute rust flesh", 0x5E3530, 0x3A1F3A, 18.0, 0.10, 0.6, 0.0),
    lit("brute shoulder plates", 0x3B3A3E, 0x3A1F3A, 40.0, 0.30, 0.4, 0.0),
    emissive("torch flame", 0xFFB25A, 6.0, 0xFFE2B0),
    emissive("candle flame", 0xFFC07A, 4.0, 0xFFE2B0),
    emissive("enemy eyes", 0xFF6A2A, 3.0, 0),
    emissive("staff orb", 0x7FE3FF, 4.0, 0),
];

/// Minimum roughness of the realistic look (no specular anti-aliasing beyond SSAA).
pub const MIN_ROUGHNESS: f32 = 0.25;

/// Global light-intensity factor, identical for every look. It multiplies every term derived
/// from lights (point lights, moon, hemisphere ambient; diffuse and specular) and was chosen so
/// that the calibrated light gain of the realistic look is about 1.0. Emissive HDR multipliers,
/// the stylized rim, decal emission, friendly bolts and bullets are not scaled by it.
pub const LIGHT_INTENSITY_FACTOR: f32 = 8.0;

/// Toon ramp. The toon look accumulates the same Lambert irradiance as the other looks (moon plus
/// point lights) and quantises it once, after the light loop, in absolute luminance at light gain 1.
/// The reference luminance is the floor directly under an intact pillar torch (moon included); the
/// two thresholds sit at these fractions of it. Band levels: 0 (shadow band, ambient only), and the
/// area-weighted mean of the smooth response inside the mid and the top band of that torch pool.
pub const TOON_THRESHOLD_FRACTIONS: (f32, f32) = (0.25, 0.60);
/// Minimum anti-aliasing half-width of a ramp edge, as a fraction of the upper threshold. world.wgsl
/// holds the same value.
pub const TOON_EDGE_MIN: f32 = 0.01;
/// Toon hard highlight: step of the Blinn-Phong lobe at this value, level = ks (g+8)/8 times the mean
/// of the lobe above the step (about 0.72 for any gloss). world.wgsl holds the same values.
pub const TOON_HIGHLIGHT_EDGE: f32 = 0.5;
pub const TOON_HIGHLIGHT_LEVEL: f32 = 0.72;
/// Per-light band thresholds before the polish round (logged in metrics.md).
pub const TOON_BANDS_BEFORE_POLISH: (f32, f32) = (0.004, 0.04);

/// Pillar torch light used as the ramp reference (scene.rs places torches with these values).
pub const TORCH_INTENSITY: f32 = 6.0;
pub const TORCH_RADIUS: f32 = 9.0;
/// Height of an intact pillar torch light above the floor.
pub const REFERENCE_TORCH_HEIGHT: f32 = 3.8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToonRamp {
    /// Luminance of the floor directly under the reference torch (moon plus torch, gain 1).
    pub reference: f32,
    pub threshold_low: f32,
    pub threshold_high: f32,
    pub level_mid: f32,
    pub level_top: f32,
}

impl ToonRamp {
    pub fn to_gpu(self) -> [f32; 4] {
        [self.threshold_low, self.threshold_high, self.level_mid, self.level_top]
    }
}

/// Moon irradiance luminance on the flat floor at light gain 1.
pub fn moon_floor_luminance() -> f32 {
    let d = MOON_DIRECTION;
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let n_dot_l = (-d[2] / len).max(0.0);
    luminance(hex(MOON_HEX)) * MOON_INTENSITY * LIGHT_INTENSITY_FACTOR * n_dot_l
}

/// The toon ramp derived from the reference torch pool (no image-based tuning).
pub fn toon_ramp() -> ToonRamp {
    let moon = moon_floor_luminance();
    let torch = luminance(hex(TORCH_LIGHT_HEX)) * TORCH_INTENSITY * LIGHT_INTENSITY_FACTOR;
    let h = REFERENCE_TORCH_HEIGHT;
    let response = |rho: f32| {
        let d = (rho * rho + h * h).sqrt();
        let x = d / TORCH_RADIUS;
        let q = (1.0 - x * x * x * x).clamp(0.0, 1.0);
        moon + torch * (h / d) * q * q / (1.0 + d * d)
    };
    let reference = response(0.0);
    let threshold_low = TOON_THRESHOLD_FRACTIONS.0 * reference;
    let threshold_high = TOON_THRESHOLD_FRACTIONS.1 * reference;
    let (mut mid, mut top) = ((0.0f64, 0.0f64), (0.0f64, 0.0f64));
    let steps = 9000;
    for i in 0..steps {
        let rho = (i as f32 + 0.5) * TORCH_RADIUS / steps as f32;
        let y = response(rho);
        let weight = f64::from(rho);
        if y >= threshold_high {
            top = (top.0 + f64::from(y) * weight, top.1 + weight);
        } else if y >= threshold_low {
            mid = (mid.0 + f64::from(y) * weight, mid.1 + weight);
        }
    }
    ToonRamp {
        reference,
        threshold_low,
        threshold_high,
        level_mid: (mid.0 / mid.1.max(1e-9)) as f32,
        level_top: (top.0 / top.1.max(1e-9)) as f32,
    }
}

pub fn material_table() -> [MaterialGpu; MATERIAL_SLOTS] {
    let mut table = [MaterialGpu::default(); MATERIAL_SLOTS];
    for (slot, material) in table.iter_mut().zip(MATERIALS.iter()) {
        let albedo = hex(material.albedo_hex);
        let emissive = mul(hex(material.emissive_hex), material.emissive_mult);
        let tint = if material.emissive_mult > 0.0 {
            // Emissive rows reuse the tint slot for the flame core colour (HDR).
            if material.core_hex != 0 {
                mul(hex(material.core_hex), material.emissive_mult)
            } else {
                emissive
            }
        } else {
            unit_luminance(hex(material.shadow_tint_hex))
        };
        *slot = MaterialGpu {
            albedo: [albedo[0], albedo[1], albedo[2], f32::from(u8::from(material.spec_albedo))],
            emissive: [emissive[0], emissive[1], emissive[2], material.emissive_mult],
            tint: [tint[0], tint[1], tint[2], material.gloss],
            params: [
                material.ks,
                material.roughness.max(MIN_ROUGHNESS),
                material.metalness,
                0.0,
            ],
            metal: {
                let f0 = if material.metal_hex != 0 { hex(material.metal_hex) } else { albedo };
                [f0[0], f0[1], f0[2], 0.0]
            },
        };
    }
    table
}

/// Neutral cold rim colour for every figure. Before the polish round the rim was hue-matched
/// (player #A8E6FF on a cyan cloak, imps and brutes #FF9A6A on orange bodies under warm torches) and
/// barely separated anything; strengths and powers are unchanged.
pub const RIM_HEX: u32 = 0xD8ECFF;
pub const RIM_HEXES_BEFORE_POLISH: (u32, u32) = (0xA8E6FF, 0xFF9A6A);

/// Stylized rim light per rim kind: (power p, strength k, colour hex).
pub const RIMS: [(f32, f32, u32); 4] = [
    (1.0, 0.0, 0x000000),
    (3.0, 0.55, RIM_HEX),
    (2.5, 0.35, RIM_HEX),
    (2.5, 0.40, RIM_HEX),
];

pub fn rim_table() -> [RimGpu; 4] {
    let mut table = [RimGpu::default(); 4];
    for (slot, (power, k, color)) in table.iter_mut().zip(RIMS) {
        let c = hex(color);
        *slot = RimGpu {
            color_power: [c[0], c[1], c[2], power],
            strength: [k, 0.0, 0.0, 0.0],
        };
    }
    table
}

// ---- Scene palette (lights, emissives, background) ----
pub const VOID_HEX: u32 = 0x06070A;
pub const MOON_HEX: u32 = 0x9AB0D8;
pub const MOON_INTENSITY: f32 = 0.35;
pub const MOON_DIRECTION: [f32; 3] = [-0.35, 0.5, -0.8];
pub const SKY_HEX: u32 = 0x1C2438;
pub const GROUND_HEX: u32 = 0x110D0B;
pub const AMBIENT_INTENSITY: f32 = 0.35;
pub const TORCH_LIGHT_HEX: u32 = 0xFF9A4A;
pub const CANDLE_LIGHT_HEX: u32 = 0xFFC07A;
pub const EDGE_CANDLE_HEX: u32 = 0xFFB066;
pub const RITUAL_LIGHT_HEX: u32 = 0x8A5CFF;
pub const ORB_LIGHT_HEX: u32 = 0x7FE3FF;
pub const BRUTE_EYE_LIGHT_HEX: u32 = 0xFF6A2A;
pub const EMBER_FLOAT_HEX: u32 = 0xFF7A3A;
pub const EMBER_CRACK_HEX: u32 = 0xFF5A1F;
/// Hue of the bullet-cluster lights (desaturated ember, outside both hostile bullet hues). Each
/// cluster light keeps the luminance its bullet colour had, so only the hue changes. Before the polish
/// round the clusters used the bullet body colours, which PRD-0003 forbids for the environment.
pub const CLUSTER_LIGHT_HEX: u32 = 0xB07850;
pub const DECAL_HEX: u32 = 0x7A4CFF;
pub const DECAL_EMISSION_CALM: f32 = 0.35;
pub const DECAL_EMISSION_BUSY: f32 = 0.6;

// ---- Bullet palettes (palette_space 1 = HOSTILE), drawn in LDR ----
pub struct BulletPaletteEntry {
    pub name: &'static str,
    pub body_hex: u32,
    pub core_hex: u32,
}

pub const HOSTILE_PALETTE: [BulletPaletteEntry; 2] = [
    BulletPaletteEntry {
        name: "H0 Hexenmagenta",
        body_hex: 0xFF2FB4,
        core_hex: 0xFFE3F4,
    },
    BulletPaletteEntry {
        name: "H1 Giftlimette",
        body_hex: 0xB6FF2E,
        core_hex: 0xF6FFE0,
    },
];
pub const BULLET_RIM_HEX: u32 = 0x0A0510;
pub const BULLET_RIM_ALPHA: f32 = 0.9;
pub const BULLET_RIM_PX: f32 = 1.5;
pub const BULLET_GLOW: u8 = 200;

// ---- Friendly palette (palette_space 2): sprite/mesh channel in the World layer ----
pub const FRIENDLY_BODY_HEX: u32 = 0x8FE8FF;
pub const FRIENDLY_MULT: f32 = 3.0;
pub const FRIENDLY_ALPHA: f32 = 0.75;
pub const MARKER_HEX: u32 = 0xBFF3FF;
pub const MARKER_ALPHA: f32 = 0.8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip() {
        for i in 0..=255u32 {
            let c = i as f32 / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(c)) - c).abs() < 1e-4);
        }
    }

    #[test]
    fn shadow_tints_have_unit_luminance() {
        for m in &MATERIALS[..18] {
            let t = unit_luminance(hex(m.shadow_tint_hex));
            assert!((luminance(t) - 1.0).abs() < 1e-4, "{}", m.name);
        }
    }

    #[test]
    fn toon_constants_match_the_shader() {
        let shader = include_str!("shaders/world.wgsl");
        assert!(shader.contains(&format!("const TOON_EDGE_MIN: f32 = {TOON_EDGE_MIN};")));
        assert!(shader.contains(&format!("const TOON_HIGHLIGHT_EDGE: f32 = {TOON_HIGHLIGHT_EDGE};")));
        assert!(shader.contains(&format!("const TOON_HIGHLIGHT_LEVEL: f32 = {TOON_HIGHLIGHT_LEVEL};")));
    }

    #[test]
    fn toon_ramp_is_ordered_and_keeps_the_torch_pool_energy() {
        let ramp = toon_ramp();
        let moon = moon_floor_luminance();
        assert!(ramp.threshold_low < moon && moon < ramp.threshold_high, "{ramp:?}, moon {moon}");
        assert!(ramp.threshold_low < ramp.level_mid && ramp.level_mid < ramp.threshold_high, "{ramp:?}");
        assert!(ramp.threshold_high < ramp.level_top && ramp.level_top < ramp.reference, "{ramp:?}");
    }

    #[test]
    fn bronze_uses_a_bright_metal_base_colour() {
        let table = material_table();
        let bronze = table[BRONZE as usize];
        assert!(luminance([bronze.metal[0], bronze.metal[1], bronze.metal[2]]) > 0.5);
        let stone = table[FLOOR as usize];
        assert_eq!(stone.metal[..3], stone.albedo[..3]);
    }

    #[test]
    fn ids_match_table_order() {
        assert_eq!(MATERIALS[FLOOR as usize].name, "floor stone");
        // world.wgsl mixes the grout row in by index.
        assert_eq!(MATERIALS[1].name, "grout");
        assert!(include_str!("shaders/world.wgsl").contains("const GROUT: u32 = 1u;"));
        assert_eq!(MATERIALS[PLATES as usize].name, "brute shoulder plates");
        assert_eq!(MATERIALS[ORB as usize].name, "staff orb");
    }
}
