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
}

const STONE_TINT: u32 = 0x3A3F66;

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

/// Toon band thresholds per point light on x = saturate(N.L) * atten (levels 0 / 0.45 / 1.0) and
/// the minimum anti-aliasing half-width of a band edge. world.wgsl holds the same values.
///
/// Bug fix: the specification's 0.02 / 0.20 (minimum width 0.004) assumed x reaches 1, but under
/// a pillar torch mounted at z 3.8 the floor response peaks at about 0.061, so the upper band was
/// unreachable. All three constants are scaled by the same factor 0.2, which keeps their ratios and
/// puts the upper threshold at two thirds of that peak.
pub const TOON_BAND_LOW: f32 = 0.004;
pub const TOON_BAND_HIGH: f32 = 0.04;
pub const TOON_EDGE_MIN: f32 = 0.0008;
/// Values of the specification before the bug fix (logged in metrics.md).
pub const TOON_BANDS_SPEC: (f32, f32, f32) = (0.02, 0.20, 0.004);

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
        };
    }
    table
}

/// Stylized rim light per rim kind: (power p, strength k, colour hex).
pub const RIMS: [(f32, f32, u32); 4] = [
    (1.0, 0.0, 0x000000),
    (3.0, 0.55, 0xA8E6FF),
    (2.5, 0.35, 0xFF9A6A),
    (2.5, 0.40, 0xFF9A6A),
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
    fn toon_band_constants_match_the_shader_and_are_reachable() {
        let shader = include_str!("shaders/world.wgsl");
        assert!(shader.contains(&format!("const TOON_BAND_LOW: f32 = {TOON_BAND_LOW};")));
        assert!(shader.contains(&format!("const TOON_BAND_HIGH: f32 = {TOON_BAND_HIGH};")));
        assert!(shader.contains(&format!("const TOON_EDGE_MIN: f32 = {TOON_EDGE_MIN};")));
        // Same scale factor for all three, so the designed ratios survive the fix.
        let k = TOON_BAND_HIGH / TOON_BANDS_SPEC.1;
        assert!((TOON_BAND_LOW / TOON_BANDS_SPEC.0 - k).abs() < 1e-6);
        assert!((TOON_EDGE_MIN / TOON_BANDS_SPEC.2 - k).abs() < 1e-6);
        // Floor response x = saturate(N.L) * atten under an intact pillar torch (light at z 3.8,
        // radius 9): both thresholds are crossed inside the pool, the upper one with margin.
        let response = |rho: f32| {
            let h = 3.8f32;
            let d = (rho * rho + h * h).sqrt();
            let q = (1.0 - (d / 9.0).powi(4)).clamp(0.0, 1.0);
            (h / d) * q * q / (1.0 + d * d)
        };
        let peak = response(0.0);
        assert!(peak > 1.4 * TOON_BAND_HIGH && peak < TOON_BANDS_SPEC.1, "peak {peak}");
        let crossing = |t: f32| (0..=900).map(|i| i as f32 * 0.01).find(|&rho| response(rho) < t).unwrap_or(9.0);
        let (inner, outer) = (crossing(TOON_BAND_HIGH), crossing(TOON_BAND_LOW));
        assert!(inner > 1.5 && outer > inner + 2.0 && outer < 7.5, "inner {inner}, outer {outer}");
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
