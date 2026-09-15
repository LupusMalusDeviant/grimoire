//! The seeded procedural scene: floor, props, figures, lights, bullets and bolts.
//!
//! One RNG stream (seed `SEED`) is consumed in a fixed order: floor, props, enemies, lights,
//! bullets. The first three stages are identical for both variants, so calm and busy share the
//! same geometry bit for bit.

use std::f32::consts::{PI, TAU};

use crate::camera::{Vec3, normalize};
use crate::gpu_types::{BoltInstance, BulletInstance, LightGpu};
use crate::materials::{self as m, hex, mul};
use crate::mesh::{Mesh, Tag, Xform};

pub const SEED: u64 = 0x5EED_F1E9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Variant {
    Calm,
    Busy,
}

impl Variant {
    pub const ALL: [Variant; 2] = [Variant::Calm, Variant::Busy];

    pub fn key(self) -> &'static str {
        match self {
            Variant::Calm => "calm",
            Variant::Busy => "busy",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "calm" => Some(Variant::Calm),
            "busy" => Some(Variant::Busy),
            _ => None,
        }
    }
}

/// xorshift64* generator.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

// ---------------------------------------------------------------------------------------------
// Floor height function. world.wgsl contains the same function (floor_height); both must agree.
// ---------------------------------------------------------------------------------------------

pub fn hash_u32(x: u32) -> u32 {
    let state = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

pub fn hash2(ix: i32, iy: i32, salt: u32) -> u32 {
    hash_u32((ix as u32).wrapping_mul(1_597_334_677) ^ (iy as u32).wrapping_mul(3_812_015_801) ^ salt)
}

fn unit_from(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub const TILE_OFFSET: f32 = 0.03;
pub const TILE_TILT_DEG: f32 = 1.5;
pub const GROOVE_DEPTH: f32 = 0.06;
pub const GROOVE_INNER: f32 = 0.015;
pub const GROOVE_OUTER: f32 = 0.055;
pub const NOISE_AMPLITUDE: f32 = 0.08;
pub const NOISE_SCALE: f32 = 5.0;
pub const ARENA_RADIUS: f32 = 15.0;
pub const LEDGE_DROP: f32 = 0.25;
pub const LEDGE_BEVEL: f32 = 0.25;

fn value_noise(x: f32, y: f32, salt: u32) -> f32 {
    let (cx, cy) = (x.floor(), y.floor());
    let (fx, fy) = (x - cx, y - cy);
    let (ix, iy) = (cx as i32, cy as i32);
    let corner = |dx: i32, dy: i32| unit_from(hash2(ix + dx, iy + dy, salt ^ 0x00A5_A5A5)) * 2.0 - 1.0;
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let a = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * ux;
    let b = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * ux;
    a + (b - a) * uy
}

/// Floor height at `(x, y)` and the grout weight (1 in the groove, 0 on the tile).
pub fn floor_height(x: f32, y: f32, salt: u32) -> (f32, f32) {
    let (cx, cy) = (x.floor(), y.floor());
    let (fx, fy) = (x - cx, y - cy);
    let h = hash2(cx as i32, cy as i32, salt);
    let offset = (unit_from(h) * 2.0 - 1.0) * TILE_OFFSET;
    let tilt_x = ((unit_from(hash_u32(h ^ 0x68E3_1DA4)) * 2.0 - 1.0) * TILE_TILT_DEG).to_radians().tan();
    let tilt_y = ((unit_from(hash_u32(h ^ 0xB529_7A4D)) * 2.0 - 1.0) * TILE_TILT_DEG).to_radians().tan();
    let tile = offset + tilt_x * (fx - 0.5) + tilt_y * (fy - 0.5);
    let edge = fx.min(1.0 - fx).min(fy.min(1.0 - fy));
    let s = smoothstep(GROOVE_INNER, GROOVE_OUTER, edge);
    let groove = -GROOVE_DEPTH * (1.0 - s);
    let noise = value_noise(x / NOISE_SCALE, y / NOISE_SCALE, salt) * NOISE_AMPLITUDE;
    let r = (x * x + y * y).sqrt();
    let ledge = -LEDGE_DROP * smoothstep(ARENA_RADIUS, ARENA_RADIUS + LEDGE_BEVEL, r);
    (noise + tile * s + groove + ledge, 1.0 - s)
}

// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureKind {
    Player,
    Imp,
    Brute,
}

#[derive(Debug, Clone, Copy)]
pub struct Figure {
    pub kind: FigureKind,
    pub position: [f32; 2],
    /// Footprint radius; the blob shadow radius is 1.2x this.
    pub footprint: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightGroup {
    Torch,
    Ritual,
    AltarCandle,
    StaffOrb,
    BruteEye,
    CalmCluster,
    EmberCrack,
    BusyCluster,
    EdgeCandle,
    RuneStone,
    FloatingEmber,
}

impl LightGroup {
    pub fn name_de(self) -> &'static str {
        match self {
            LightGroup::Torch => "Fackeln und Kohlebecken",
            LightGroup::Ritual => "Ritual",
            LightGroup::AltarCandle => "Altarkerzen",
            LightGroup::StaffOrb => "Stab-Orb",
            LightGroup::BruteEye => "Brute-Augen",
            LightGroup::CalmCluster => "Bullet-Cluster calm",
            LightGroup::EmberCrack => "Glutspalt",
            LightGroup::BusyCluster => "Bullet-Cluster busy",
            LightGroup::EdgeCandle => "Randkerzen",
            LightGroup::RuneStone => "Runensteine",
            LightGroup::FloatingEmber => "schwebende Glut",
        }
    }
}

pub struct Scene {
    /// Lit geometry; props and figures first, floor last (less overdraw, identical for all looks).
    pub opaque: Mesh,
    pub emissive: Mesh,
    pub lights: Vec<LightGpu>,
    pub light_groups: Vec<LightGroup>,
    pub bullets: Vec<BulletInstance>,
    /// Friendly bolts and busy light-source dots (World layer, emissive, identical in all looks).
    pub bolts: Vec<BoltInstance>,
    pub friendly_bolt_count: usize,
    pub figures: Vec<Figure>,
    pub decal_emission: f32,
    pub floor_salt: u32,
    pub floor_triangles: usize,
    pub torch_positions: Vec<Vec3>,
    pub stress_counts: [usize; 4],
}

pub const PLAYER_POS: [f32; 2] = [0.0, -3.0];
/// Centre of the toppled pillar. The design places it at (10, -6), only 0.9 units from the intact
/// pillar at 330 degrees (10.83, -6.25), so the fallen shaft would run through that pillar's plinth.
/// It is moved towards the arena centre until it clears the plinth; length, radius and yaw are kept.
pub const TOPPLED_PILLAR_CENTRE: [f32; 2] = [8.5, -4.0];
pub const TOPPLED_PILLAR_YAW_DEG: f32 = 20.0;
/// Centre of the broken wall arc. The design places it at (-9, 8); the arc (tangential to the
/// arena circle, radius 12.04) then runs into the 150-degree pillar and its plinth. It is moved
/// along its own arc, keeping the radius and the tangential orientation, by the minimum that
/// leaves at least 1 unit between the wall and the plinth: 2.12 units, to (-7.45, 9.46).
pub const WALL_CENTRE: [f32; 2] = [-7.45, 9.46];
pub const WALL_CENTRE_SPEC: [f32; 2] = [-9.0, 8.0];
pub const WALL_LENGTH: f32 = 5.0;
pub const WALL_HALF_THICKNESS: f32 = 0.3;
/// Plinth half extent of the pillars (plinth 2.0 x 2.0 x 0.5).
pub const PLINTH_HALF: f32 = 1.0;
pub const IMP_POSITIONS: [[f32; 2]; 6] = [[-3.0, 5.0], [3.0, 4.5], [-8.0, 1.0], [8.0, 0.0], [-4.0, 10.0], [5.0, 11.0]];
pub const BRUTE_POSITIONS: [[f32; 2]; 2] = [[-6.0, 7.0], [7.0, 5.0]];

pub const SIL_ORB: u16 = 0;
pub const SIL_RICE: u16 = 1;
pub const SIL_DIAMOND: u16 = 2;
pub const SILHOUETTE_RADIUS: [f32; 3] = [0.22, 0.32, 0.26];
pub const HOSTILE: u8 = 1;
pub const FRIENDLY: u8 = 2;

fn light(position: Vec3, hex_color: u32, intensity: f32, radius: f32) -> LightGpu {
    LightGpu {
        position,
        radius,
        color: mul(hex(hex_color), intensity),
        _pad: 0.0,
    }
}

fn yaw_towards(from: [f32; 2], to: [f32; 2]) -> f32 {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    (-dx).atan2(dy)
}

impl Scene {
    pub fn build(variant: Variant) -> Self {
        let mut rng = Rng::new(SEED);

        // ---- 1. floor ----
        let floor_salt = rng.next_u32();
        let floor = build_floor(floor_salt);
        let floor_triangles = floor.triangle_count();

        // ---- 2. props ----
        let mut props = Mesh::default();
        let mut emissive = Mesh::default();
        let mut torch_positions = Vec::new();
        build_pillars(&mut rng, &mut props, &mut emissive, &mut torch_positions);
        build_ruins(&mut rng, &mut props, &mut emissive, &mut torch_positions);

        // ---- 3. figures (player + enemies) ----
        let mut figures_mesh = Mesh::default();
        let mut figures = Vec::new();
        build_player(&mut figures_mesh, &mut emissive, &mut figures);
        let _enemy_stream_marker = rng.next_u32();
        build_enemies(&mut figures_mesh, &mut emissive, &mut figures);

        let mut opaque = Mesh::default();
        opaque.append(&figures_mesh);
        opaque.append(&props);
        opaque.append(&floor);

        // ---- 4. lights ----
        let staff_orb = [PLAYER_POS[0] + 0.45, PLAYER_POS[1] + 0.1, 1.84];
        let mut lights = Vec::new();
        let mut groups = Vec::new();
        let mut push = |l: LightGpu, g: LightGroup, lights: &mut Vec<LightGpu>| -> () {
            lights.push(l);
            groups.push(g);
        };
        for t in &torch_positions {
            push(light([t[0], t[1], t[2] + 0.2], m::TORCH_LIGHT_HEX, 6.0, 9.0), LightGroup::Torch, &mut lights);
        }
        for k in 0..4 {
            let a = (45.0 + 90.0 * k as f32).to_radians();
            push(light([5.0 * a.cos(), 5.0 * a.sin(), 0.6], m::RITUAL_LIGHT_HEX, 2.5, 6.0), LightGroup::Ritual, &mut lights);
        }
        for sx in [-0.8f32, 0.8] {
            push(light([sx, 6.0, 1.33], m::CANDLE_LIGHT_HEX, 1.2, 3.0), LightGroup::AltarCandle, &mut lights);
        }
        push(light(staff_orb, m::ORB_LIGHT_HEX, 2.0, 5.0), LightGroup::StaffOrb, &mut lights);
        for b in BRUTE_POSITIONS {
            let yaw = yaw_towards(b, PLAYER_POS);
            let xf = Xform::yaw(yaw, [b[0], b[1], 0.0]);
            push(light(xf.point([0.0, 1.2, 1.7]), m::BRUTE_EYE_LIGHT_HEX, 1.5, 4.0), LightGroup::BruteEye, &mut lights);
        }
        // Calm cluster lights are placed after the bullets are known (no RNG use); reserve slot.
        let calm_cluster_slot = lights.len();
        let [tx, ty] = TOPPLED_PILLAR_CENTRE;
        push(light([tx, ty, 0.15], m::EMBER_CRACK_HEX, 1.2, 4.0), LightGroup::EmberCrack, &mut lights);

        let mut dots = Vec::new();
        if variant == Variant::Busy {
            for k in 0..64 {
                let a = TAU * (k as f32 + rng.range(-0.3, 0.3)) / 64.0;
                let p = [16.0 * a.cos(), 16.0 * a.sin(), 0.1];
                push(light(p, m::EDGE_CANDLE_HEX, 0.8, 3.0), LightGroup::EdgeCandle, &mut lights);
                dots.push(dot([p[0], p[1], p[2] + 0.05], 0.07, m::EDGE_CANDLE_HEX, 4.0));
            }
            for k in 0..48 {
                let a = TAU * (k as f32 + 0.5) / 48.0;
                let p = [6.2 * a.cos(), 6.2 * a.sin(), 0.3];
                push(light(p, m::RITUAL_LIGHT_HEX, 0.6, 2.5), LightGroup::RuneStone, &mut lights);
                dots.push(dot([p[0], p[1], 0.12], 0.06, m::RITUAL_LIGHT_HEX, 3.0));
            }
            for k in 0..32 {
                let pillar = (30.0 + 60.0 * (k % 6) as f32).to_radians();
                let base = [12.5 * pillar.cos(), 12.5 * pillar.sin()];
                let a = rng.range(0.0, TAU);
                let r = rng.range(0.9, 2.2);
                let p = [base[0] + r * a.cos(), base[1] + r * a.sin(), rng.range(1.0, 3.0)];
                push(light(p, m::EMBER_FLOAT_HEX, 0.7, 3.0), LightGroup::FloatingEmber, &mut lights);
                dots.push(dot(p, 0.05, m::EMBER_FLOAT_HEX, 4.0));
            }
        }

        // ---- 5. bullets and friendly bolts ----
        let brightest_pool = brightest_torch_pool(&torch_positions);
        let (calm_bullets, calm_stress) = calm_bullets(&mut Rng::new(CALM_BULLET_SEED), brightest_pool);
        let (bullets, stress_counts) = match variant {
            Variant::Calm => (calm_bullets.clone(), calm_stress),
            Variant::Busy => busy_bullets(&mut rng, brightest_pool),
        };

        // Calm cluster lights (both variants): 6 clusters of the calm pattern.
        let calm_clusters = kmeans_clusters(&calm_bullets, 6);
        let mut cluster_lights = Vec::new();
        for (centre, palette) in &calm_clusters {
            let body = m::HOSTILE_PALETTE[*palette as usize].body_hex;
            cluster_lights.push(light([centre[0], centre[1], 0.5], body, 1.5, 5.0));
        }
        for (offset, l) in cluster_lights.into_iter().enumerate() {
            lights.insert(calm_cluster_slot + offset, l);
            groups.insert(calm_cluster_slot + offset, LightGroup::CalmCluster);
        }
        if variant == Variant::Busy {
            let busy_clusters = kmeans_clusters(&bullets, 64);
            let mut busy_lights = Vec::new();
            for (centre, palette) in &busy_clusters {
                let body = m::HOSTILE_PALETTE[*palette as usize].body_hex;
                busy_lights.push(light([centre[0], centre[1], 0.5], body, 1.0, 4.0));
            }
            // Keep the documented order: calm 24, then 64 cluster lights, then the rest.
            for (offset, l) in busy_lights.into_iter().enumerate() {
                lights.insert(24 + offset, l);
                groups.insert(24 + offset, LightGroup::BusyCluster);
            }
        }

        let bolt_count = match variant {
            Variant::Calm => 12,
            Variant::Busy => 40,
        };
        let mut bolts = friendly_bolts(&mut rng, bolt_count, &figures);
        let friendly_bolt_count = bolts.len();
        bolts.extend(dots);

        Self {
            opaque,
            emissive,
            lights,
            light_groups: groups,
            bullets,
            bolts,
            friendly_bolt_count,
            figures,
            decal_emission: match variant {
                Variant::Calm => m::DECAL_EMISSION_CALM,
                Variant::Busy => m::DECAL_EMISSION_BUSY,
            },
            floor_salt,
            floor_triangles,
            torch_positions,
            stress_counts,
        }
    }
}

impl Scene {
    /// Number of point lights per group, in the order the groups first appear.
    pub fn light_breakdown(&self) -> Vec<(LightGroup, usize)> {
        let mut out: Vec<(LightGroup, usize)> = Vec::new();
        for &group in &self.light_groups {
            match out.iter_mut().find(|(g, _)| *g == group) {
                Some((_, n)) => *n += 1,
                None => out.push((group, 1)),
            }
        }
        out
    }
}

/// Seed of the calm bullet sub-stream. The calm layout must be identical in both variants (busy
/// keeps the calm cluster lights), but the main stream differs after the busy-only lights, so the
/// calm layout is drawn from its own sub-stream in both variants.
pub const CALM_BULLET_SEED: u64 = SEED ^ 0xCA1B_B011;
/// Designed calm bullet count: ring 36 + 3 spiral arms x 12 rice + 6 fans x 15 diamonds + 18 stress orbs.
pub const CALM_BULLETS: usize = 180;
pub const CALM_SPIRAL_RICE: usize = 12;

fn dot(p: Vec3, radius: f32, hex_color: u32, intensity: f32) -> BoltInstance {
    BoltInstance {
        center: p,
        half_length: radius,
        dir: [1.0, 0.0],
        half_width: radius,
        alpha: 1.0,
        color: hex(hex_color),
        intensity,
    }
}

fn build_floor(salt: u32) -> Mesh {
    const QUADS_PER_UNIT: usize = 4;
    const HALF: f32 = 20.0;
    let n = (2.0 * HALF) as usize * QUADS_PER_UNIT;
    let step = 1.0 / QUADS_PER_UNIT as f32;
    let tag = Tag::new(m::FLOOR, m::CLASS_FLOOR);
    let mut mesh = Mesh::default();
    for j in 0..=n {
        for i in 0..=n {
            let x = -HALF + i as f32 * step;
            let y = -HALF + j as f32 * step;
            let (h, _) = floor_height(x, y, salt);
            // Geometric normal from central differences over one grid step (smooth, no grout detail).
            let hx = (floor_height(x + step, y, salt).0 - floor_height(x - step, y, salt).0) / (2.0 * step);
            let hy = (floor_height(x, y + step, salt).0 - floor_height(x, y - step, salt).0) / (2.0 * step);
            mesh.vertices.push(crate::gpu_types::Vertex {
                position: [x, y, h],
                packed: tag.material | (tag.class << 8),
                normal: normalize([-hx, -hy, 1.0]),
                figure: 0,
            });
        }
    }
    let row = n as u32 + 1;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * row + i;
            let b = a + 1;
            let c = a + row + 1;
            let d = a + row;
            mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    mesh
}

fn build_pillars(rng: &mut Rng, props: &mut Mesh, emissive: &mut Mesh, torches: &mut Vec<Vec3>) {
    let prop = Tag::new(m::PILLAR, m::CLASS_PROP);
    let flame = Tag::new(m::TORCH_FLAME, m::CLASS_EMISSIVE);
    for k in 0..6 {
        let angle_deg = 30.0 + 60.0 * k as f32;
        let a = angle_deg.to_radians();
        let pos = [12.5 * a.cos(), 12.5 * a.sin()];
        let broken_top = match k {
            1 => Some(2.2),
            3 => Some(1.4),
            _ => None,
        };
        let xf = Xform::yaw(a, [pos[0], pos[1], 0.0]);
        // Plinth (half extents 1.0 x 1.0, height 0.5), moss hashed on the top.
        let plinth_top = if rng.unit() < 0.45 { m::MOSS } else { m::PLINTH };
        props.box_flat(
            &xf.compose(&Xform::translate([0.0, 0.0, 0.25])),
            [PLINTH_HALF, PLINTH_HALF, 0.25],
            prop.with_material(m::PLINTH),
            prop.with_material(plinth_top),
        );
        let tops: Vec<f32> = match broken_top {
            None => vec![5.0; 8],
            Some(top) => (0..8).map(|_| top + rng.range(-0.35, 0.3)).collect(),
        };
        props.prism_flat(&xf, 8, 0.7, 0.5, &tops, prop, prop);
        let top = broken_top.unwrap_or(5.0);
        if broken_top.is_none() {
            props.box_flat(
                &xf.compose(&Xform::translate([0.0, 0.0, 5.175])),
                [0.9, 0.9, 0.175],
                prop,
                prop,
            );
        }
        // Sconce on the inner face.
        let flame_z = if broken_top.is_some() { (top - 0.45).min(3.6) } else { 3.6 };
        let inward = [-a.cos(), -a.sin()];
        let bracket = [pos[0] + inward[0] * 0.8, pos[1] + inward[1] * 0.8, flame_z - 0.08];
        props.box_flat(
            &Xform::yaw(a, bracket),
            [0.14, 0.1, 0.06],
            prop.with_material(m::PLINTH),
            prop.with_material(m::PLINTH),
        );
        let flame_base = [pos[0] + inward[0] * 0.9, pos[1] + inward[1] * 0.9, flame_z];
        emissive.flame(flame_base, 0.11, 0.35, flame);
        torches.push(flame_base);
        // Rubble beside the broken pillars.
        if broken_top.is_some() {
            for _ in 0..4 {
                let ra = rng.range(0.0, TAU);
                let rr = rng.range(1.3, 2.4);
                let size = rng.range(0.3, 0.8);
                let yaw = rng.range(0.0, TAU);
                let half = [size * 0.5, size * rng.range(0.3, 0.5), size * rng.range(0.2, 0.35)];
                let centre = [pos[0] + rr * ra.cos(), pos[1] + rr * ra.sin(), half[2]];
                let tag = Tag::new(m::RUBBLE, m::CLASS_PROP);
                props.box_flat(&Xform::yaw(yaw, centre), half, tag, tag);
            }
        }
    }
}

fn build_ruins(rng: &mut Rng, props: &mut Mesh, emissive: &mut Mesh, torches: &mut Vec<Vec3>) {
    let wall = Tag::new(m::RUIN_WALL, m::CLASS_PROP);
    // Broken wall arc around the arena centre, centred at WALL_CENTRE: 5.0 long, 0.6 thick, 2.2 high.
    let [wx, wy] = WALL_CENTRE;
    let centre_angle = wy.atan2(wx);
    let radius = (wx * wx + wy * wy).sqrt();
    let gap = 0.5;
    let segment_len = (WALL_LENGTH - 2.0 * gap) / 3.0;
    let start = -0.5 * WALL_LENGTH;
    for s in 0..3 {
        let s0 = start + s as f32 * (segment_len + gap);
        let columns = 4;
        let heights: Vec<f32> = (0..=columns).map(|_| 2.2 - rng.range(0.0, 0.9)).collect();
        let point = |along: f32, offset: f32, z: f32| {
            let a = centre_angle + along / radius;
            let r = radius + offset;
            [r * a.cos(), r * a.sin(), z]
        };
        for c in 0..columns {
            let u0 = s0 + segment_len * c as f32 / columns as f32;
            let u1 = s0 + segment_len * (c + 1) as f32 / columns as f32;
            let (h0, h1) = (heights[c], heights[c + 1]);
            let mid_a = centre_angle + 0.5 * (u0 + u1) / radius;
            let outward = [mid_a.cos(), mid_a.sin(), 0.0];
            let inward = [-mid_a.cos(), -mid_a.sin(), 0.0];
            props.flat_quad_facing(point(u0, WALL_HALF_THICKNESS, 0.0), point(u1, WALL_HALF_THICKNESS, 0.0), point(u1, WALL_HALF_THICKNESS, h1), point(u0, WALL_HALF_THICKNESS, h0), outward, wall);
            props.flat_quad_facing(point(u0, -WALL_HALF_THICKNESS, 0.0), point(u1, -WALL_HALF_THICKNESS, 0.0), point(u1, -WALL_HALF_THICKNESS, h1), point(u0, -WALL_HALF_THICKNESS, h0), inward, wall);
            let top = if rng.unit() < 0.35 { wall.with_material(m::MOSS) } else { wall };
            props.flat_quad_facing(point(u0, -WALL_HALF_THICKNESS, h0), point(u1, -WALL_HALF_THICKNESS, h1), point(u1, WALL_HALF_THICKNESS, h1), point(u0, WALL_HALF_THICKNESS, h0), [0.0, 0.0, 1.0], top);
        }
        let tangent_at = |u: f32| {
            let a = centre_angle + u / radius;
            [-a.sin(), a.cos(), 0.0]
        };
        let u_end = s0 + segment_len;
        let t0 = tangent_at(s0);
        let t1 = tangent_at(u_end);
        props.flat_quad_facing(point(s0, -WALL_HALF_THICKNESS, 0.0), point(s0, WALL_HALF_THICKNESS, 0.0), point(s0, WALL_HALF_THICKNESS, heights[0]), point(s0, -WALL_HALF_THICKNESS, heights[0]), [-t0[0], -t0[1], 0.0], wall);
        props.flat_quad_facing(point(u_end, -WALL_HALF_THICKNESS, 0.0), point(u_end, WALL_HALF_THICKNESS, 0.0), point(u_end, WALL_HALF_THICKNESS, heights[columns]), point(u_end, -WALL_HALF_THICKNESS, heights[columns]), t1, wall);
    }

    // Toppled pillar, length 4, r 0.6, yaw 20 degrees, resting on a facet (see TOPPLED_PILLAR_CENTRE).
    let yaw = TOPPLED_PILLAR_YAW_DEG.to_radians();
    let axis = [yaw.cos(), yaw.sin(), 0.0];
    let rest = 0.6 * (PI / 8.0).cos();
    let [cx, cy] = TOPPLED_PILLAR_CENTRE;
    let origin = [cx - axis[0] * 2.0, cy - axis[1] * 2.0, rest];
    let pillar = Tag::new(m::PILLAR, m::CLASS_PROP);
    props.prism_flat(&Xform::along(axis, origin), 8, 0.6, 0.0, &[4.0; 8], pillar, pillar);

    // Basalt altar with two candles.
    let altar = Tag::new(m::ALTAR, m::CLASS_PROP);
    props.box_flat(&Xform::translate([0.0, 6.0, 0.5]), [1.2, 0.6, 0.5], altar, altar);
    for sx in [-0.8f32, 0.8] {
        props.cylinder(&Xform::translate([sx, 6.0, 1.0]), 0.06, 0.25, 10, Tag::new(m::WAX, m::CLASS_PROP));
        emissive.flame([sx, 6.0, 1.25], 0.035, 0.12, Tag::new(m::CANDLE_FLAME, m::CLASS_EMISSIVE));
    }

    // Two bronze braziers on tripods.
    let bronze = Tag::new(m::BRONZE, m::CLASS_PROP);
    for sx in [-2.2f32, 2.2] {
        let centre = [sx, 6.0];
        for leg in 0..3 {
            let a = (90.0 + 120.0 * leg as f32).to_radians();
            let foot = [centre[0] + 0.32 * a.cos(), centre[1] + 0.32 * a.sin(), 0.0];
            let head = [centre[0] + 0.14 * a.cos(), centre[1] + 0.14 * a.sin(), 0.72];
            let axis = [head[0] - foot[0], head[1] - foot[1], head[2] - foot[2]];
            let len = crate::camera::length(axis);
            props.cylinder(&Xform::along(axis, foot), 0.035, len, 8, bronze);
        }
        props.lathe(
            &Xform::translate([centre[0], centre[1], 0.0]),
            &[(0.0, 0.66), (0.16, 0.68), (0.28, 0.76), (0.35, 0.88), (0.33, 0.92)],
            20,
            bronze,
        );
        let flame_base = [centre[0], centre[1], 0.9];
        emissive.flame(flame_base, 0.11, 0.35, Tag::new(m::TORCH_FLAME, m::CLASS_EMISSIVE));
        torches.push(flame_base);
    }
}

fn figure_tag(material: u32, class: u32, rim: u32, figure: u32) -> Tag {
    Tag {
        material,
        class,
        rim,
        figure,
    }
}

fn build_player(mesh: &mut Mesh, emissive: &mut Mesh, figures: &mut Vec<Figure>) {
    let figure = figures.len() as u32 + 1;
    let tag = |material| figure_tag(material, m::CLASS_PLAYER, m::RIM_PLAYER, figure);
    let xf = Xform::yaw(0.0, [PLAYER_POS[0], PLAYER_POS[1], 0.0]);
    mesh.capsule(&xf, 0.35, 1.3, tag(m::CLOAK));
    mesh.cone(&xf, 0.55, 0.9, 20, tag(m::CLOAK));
    mesh.ellipsoid(&xf.compose(&Xform::translate([0.0, 0.0, 1.55])), [0.28; 3], 10, 16, tag(m::CLOAK));
    mesh.cone(&xf.compose(&Xform::along([0.0, -1.0, 0.6], [0.0, -0.12, 1.62])), 0.15, 0.32, 12, tag(m::CLOAK));
    let face = xf.compose(&Xform::along([0.0, 1.0, 0.0], [0.0, 0.265, 1.53]));
    mesh.disc(&face, 0.17, 0.0, [0.0, 0.0, 1.0], 16, tag(m::HOOD_INSIDE));
    mesh.disc(&face, 0.1, 0.012, [0.0, 0.0, 1.0], 12, tag(m::MASK));
    mesh.cylinder(&xf.compose(&Xform::translate([0.45, 0.1, 0.05])), 0.04, 1.7, 8, tag(m::WOOD));
    emissive.sphere(
        xf.point([0.45, 0.1, 1.84]),
        0.09,
        figure_tag(m::ORB, m::CLASS_EMISSIVE, m::RIM_NONE, figure),
    );
    figures.push(Figure {
        kind: FigureKind::Player,
        position: PLAYER_POS,
        footprint: 0.55,
    });
}

fn build_enemies(mesh: &mut Mesh, emissive: &mut Mesh, figures: &mut Vec<Figure>) {
    for pos in IMP_POSITIONS {
        let figure = figures.len() as u32 + 1;
        let tag = |material| figure_tag(material, m::CLASS_ENEMY, m::RIM_IMP, figure);
        let xf = Xform::yaw(yaw_towards(pos, PLAYER_POS), [pos[0], pos[1], 0.0]);
        mesh.capsule(&xf, 0.3, 0.9, tag(m::IMP_SKIN));
        for sx in [-1.0f32, 1.0] {
            let horn = Xform::along(xf.vector([0.45 * sx, 0.1, 1.0]), xf.point([0.13 * sx, 0.04, 0.76]));
            mesh.cone(&horn, 0.06, 0.25, 10, tag(m::HORN));
            emissive.sphere(
                xf.point([0.1 * sx, 0.28, 0.68]),
                0.04,
                figure_tag(m::EYE, m::CLASS_EMISSIVE, m::RIM_NONE, figure),
            );
        }
        figures.push(Figure {
            kind: FigureKind::Imp,
            position: pos,
            footprint: 0.3,
        });
    }
    for pos in BRUTE_POSITIONS {
        let figure = figures.len() as u32 + 1;
        let tag = |material| figure_tag(material, m::CLASS_ENEMY, m::RIM_BRUTE, figure);
        let base = Xform::yaw(yaw_towards(pos, PLAYER_POS), [pos[0], pos[1], 0.0]);
        let hunch = -12.0f32.to_radians();
        let tilt = Xform {
            x: [1.0, 0.0, 0.0],
            y: [0.0, hunch.cos(), hunch.sin()],
            z: [0.0, -hunch.sin(), hunch.cos()],
            t: [0.0, 0.0, 0.0],
        };
        let xf = base.compose(&tilt);
        mesh.ellipsoid(&xf.compose(&Xform::translate([0.0, 0.0, 1.2])), [1.1, 0.9, 1.2], 14, 24, tag(m::BRUTE_FLESH));
        for sx in [-1.0f32, 1.0] {
            mesh.ellipsoid(&xf.compose(&Xform::translate([0.95 * sx, 0.05, 1.85])), [0.45; 3], 10, 16, tag(m::PLATES));
            emissive.sphere(
                xf.point([0.25 * sx, 0.8, 1.75]),
                0.07,
                figure_tag(m::EYE, m::CLASS_EMISSIVE, m::RIM_NONE, figure),
            );
        }
        figures.push(Figure {
            kind: FigureKind::Brute,
            position: pos,
            footprint: 1.1,
        });
    }
}

/// Torch whose flame gives the highest horizontal irradiance on the floor directly below it.
fn brightest_torch_pool(torches: &[Vec3]) -> [f32; 2] {
    let atten = |d: f32, r: f32| {
        let q = (1.0 - (d / r).powi(4)).clamp(0.0, 1.0);
        q * q / (1.0 + d * d)
    };
    let mut best = ([0.0, 0.0], f32::MIN);
    for t in torches {
        let ground = [t[0], t[1]];
        let e: f32 = torches
            .iter()
            .map(|l| {
                let d = [l[0] - ground[0], l[1] - ground[1], l[2] + 0.2];
                let dist = crate::camera::length(d);
                atten(dist, 9.0) * (d[2] / dist).max(0.0) * 6.0
            })
            .sum();
        if e > best.1 {
            best = (ground, e);
        }
    }
    best.0
}

fn bullet(position: [f32; 2], silhouette: u16, rotation: f32, palette: u16) -> BulletInstance {
    BulletInstance {
        position,
        radius: SILHOUETTE_RADIUS[silhouette as usize],
        rotation,
        silhouette,
        palette,
        palette_space: HOSTILE,
        glow: m::BULLET_GLOW,
        flags: 0,
    }
}

fn pillar_overlap_position(rng: &mut Rng, index: usize) -> [f32; 2] {
    // Intact pillars (30, 150, 330 degrees) and the broken one at 90 degrees; bullets placed just
    // behind the base so they overlap the projected pillar silhouette on screen.
    let angles = [30.0f32, 150.0, 330.0, 90.0];
    let a = angles[index % angles.len()].to_radians();
    [12.5 * a.cos() + rng.range(-0.5, 0.5), 12.5 * a.sin() + rng.range(0.4, 2.4)]
}

/// Stress placements. `orbs_only` keeps the calm variant's "scattered orbs"; busy mixes silhouettes.
fn stress_bullets(rng: &mut Rng, pool: [f32; 2], counts: [usize; 4], orbs_only: bool, out: &mut Vec<BulletInstance>) {
    let random_type = |rng: &mut Rng, p: [f32; 2], out: &mut Vec<BulletInstance>| {
        let sil = if orbs_only { SIL_ORB } else { (rng.next_u32() % 3) as u16 };
        let pal = (rng.next_u32() % 2) as u16;
        out.push(bullet(p, sil, rng.range(0.0, TAU), pal));
    };
    for _ in 0..counts[0] {
        let a = rng.range(0.0, TAU);
        let r = 1.6 * rng.unit().sqrt();
        random_type(rng, [pool[0] + r * a.cos(), pool[1] + r * a.sin()], out);
    }
    for _ in 0..counts[1] {
        let a = rng.range(0.0, TAU);
        let r = rng.range(3.6, 6.0);
        random_type(rng, [r * a.cos(), r * a.sin()], out);
    }
    for _ in 0..counts[2] {
        let a = rng.range(0.0, TAU);
        let r = rng.range(0.7, 2.0);
        random_type(rng, [PLAYER_POS[0] + r * a.cos(), PLAYER_POS[1] + r * a.sin()], out);
    }
    for i in 0..counts[3] {
        let p = pillar_overlap_position(rng, i);
        random_type(rng, p, out);
    }
}

fn calm_bullets(rng: &mut Rng, pool: [f32; 2]) -> (Vec<BulletInstance>, [usize; 4]) {
    let mut out = Vec::with_capacity(180);
    // Ring of 36 orbs around brute A at radius 3.5.
    let phase = rng.range(0.0, TAU);
    let a_pos = BRUTE_POSITIONS[0];
    for i in 0..36 {
        let a = phase + TAU * i as f32 / 36.0;
        out.push(bullet([a_pos[0] + 3.5 * a.cos(), a_pos[1] + 3.5 * a.sin()], SIL_ORB, a, 0));
    }
    // 3 spiral arms of 12 rice around brute B (the design's 20 would give 204 instead of 180).
    let phase = rng.range(0.0, TAU);
    let b_pos = BRUTE_POSITIONS[1];
    for arm in 0..3 {
        for j in 0..CALM_SPIRAL_RICE {
            let r = 1.0 + j as f32 * 0.3;
            let a = phase + TAU * arm as f32 / 3.0 + (14.0 * j as f32).to_radians();
            out.push(bullet([b_pos[0] + r * a.cos(), b_pos[1] + r * a.sin()], SIL_RICE, a + 60f32.to_radians(), 1));
        }
    }
    // 6 imp fans of 5 diamonds x 3 rows.
    for (i, imp) in IMP_POSITIONS.iter().enumerate() {
        let aim = (PLAYER_POS[1] - imp[1]).atan2(PLAYER_POS[0] - imp[0]);
        for row in 0..3 {
            let dist = 1.2 + row as f32 * 0.9;
            for c in 0..5 {
                let a = aim + ((c as f32 - 2.0) * 12.0).to_radians();
                out.push(bullet([imp[0] + dist * a.cos(), imp[1] + dist * a.sin()], SIL_DIAMOND, a, (i % 2) as u16));
            }
        }
    }
    // 18 scattered orbs = the calm stress placements (scaled from 20/20/10/10).
    let counts = [6, 6, 3, 3];
    stress_bullets(rng, pool, counts, true, &mut out);
    debug_assert_eq!(out.len(), CALM_BULLETS);
    (out, counts)
}

fn busy_bullets(rng: &mut Rng, pool: [f32; 2]) -> (Vec<BulletInstance>, [usize; 4]) {
    let mut out = Vec::with_capacity(2000);
    // 8 rings of 48 orbs around both brutes at radii 2..9.
    for (b, centre) in BRUTE_POSITIONS.iter().enumerate() {
        for q in 0..4 {
            let r = 2.0 + q as f32 * 7.0 / 3.0;
            let phase = rng.range(0.0, TAU);
            for i in 0..48 {
                let a = phase + TAU * i as f32 / 48.0;
                out.push(bullet([centre[0] + r * a.cos(), centre[1] + r * a.sin()], SIL_ORB, a, ((b + q) % 2) as u16));
            }
        }
    }
    // 6 spiral arms of 120 rice around brute B.
    let phase = rng.range(0.0, TAU);
    let b_pos = BRUTE_POSITIONS[1];
    for arm in 0..6 {
        for j in 0..120 {
            let r = 1.0 + j as f32 * 0.1;
            let a = phase + TAU * arm as f32 / 6.0 + (6.0 * j as f32).to_radians();
            out.push(bullet([b_pos[0] + r * a.cos(), b_pos[1] + r * a.sin()], SIL_RICE, a + 60f32.to_radians(), (arm % 2) as u16));
        }
    }
    // 6 imp fans of 10 x 8 diamonds.
    for (i, imp) in IMP_POSITIONS.iter().enumerate() {
        let aim = (PLAYER_POS[1] - imp[1]).atan2(PLAYER_POS[0] - imp[0]);
        for row in 0..8 {
            let dist = 1.0 + row as f32 * 0.6;
            for c in 0..10 {
                let a = aim + ((c as f32 - 4.5) * 8.0).to_radians();
                out.push(bullet([imp[0] + dist * a.cos(), imp[1] + dist * a.sin()], SIL_DIAMOND, a, (i % 2) as u16));
            }
        }
    }
    // Drift field of 416 mixed bullets, including the stress placements.
    let counts = [20, 20, 10, 10];
    stress_bullets(rng, pool, counts, false, &mut out);
    let stress: usize = counts.iter().sum();
    for _ in 0..(416 - stress) {
        let a = rng.range(0.0, TAU);
        let r = 15.5 * rng.unit().sqrt();
        let sil = (rng.next_u32() % 3) as u16;
        let pal = (rng.next_u32() % 2) as u16;
        out.push(bullet([r * a.cos(), r * a.sin()], sil, rng.range(0.0, TAU), pal));
    }
    debug_assert_eq!(out.len(), 2000);
    (out, counts)
}

/// Deterministic k-means on bullet positions. Returns (centroid, majority palette) per cluster.
fn kmeans_clusters(bullets: &[BulletInstance], k: usize) -> Vec<([f32; 2], u16)> {
    let n = bullets.len();
    let mut centres: Vec<[f32; 2]> = (0..k).map(|i| bullets[i * n / k].position).collect();
    let mut assignment = vec![0usize; n];
    for _ in 0..16 {
        for (b, slot) in bullets.iter().zip(assignment.iter_mut()) {
            let mut best = (0, f32::MAX);
            for (c, centre) in centres.iter().enumerate() {
                let dx = b.position[0] - centre[0];
                let dy = b.position[1] - centre[1];
                let d = dx * dx + dy * dy;
                if d < best.1 {
                    best = (c, d);
                }
            }
            *slot = best.0;
        }
        let mut sums = vec![[0.0f32; 3]; k];
        for (b, &c) in bullets.iter().zip(assignment.iter()) {
            sums[c][0] += b.position[0];
            sums[c][1] += b.position[1];
            sums[c][2] += 1.0;
        }
        for (centre, sum) in centres.iter_mut().zip(sums) {
            if sum[2] > 0.0 {
                *centre = [sum[0] / sum[2], sum[1] / sum[2]];
            }
        }
    }
    (0..k)
        .map(|c| {
            let mut votes = [0usize; 2];
            for (b, &a) in bullets.iter().zip(assignment.iter()) {
                if a == c {
                    votes[b.palette as usize % 2] += 1;
                }
            }
            (centres[c], if votes[1] > votes[0] { 1 } else { 0 })
        })
        .collect()
}

fn friendly_bolts(rng: &mut Rng, count: usize, figures: &[Figure]) -> Vec<BoltInstance> {
    let staff = [PLAYER_POS[0] + 0.45, PLAYER_POS[1] + 0.1];
    let enemies: Vec<[f32; 2]> = figures
        .iter()
        .filter(|f| f.kind != FigureKind::Player)
        .map(|f| f.position)
        .collect();
    (0..count)
        .map(|i| {
            let target = enemies[i % enemies.len()];
            let t = rng.range(0.12, 0.85);
            let d = [target[0] - staff[0], target[1] - staff[1]];
            let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
            let dir = [d[0] / len, d[1] / len];
            let jitter = rng.range(-0.25, 0.25);
            BoltInstance {
                center: [
                    staff[0] + d[0] * t - dir[1] * jitter,
                    staff[1] + d[1] * t + dir[0] * jitter,
                    1.2,
                ],
                half_length: 0.225,
                dir,
                half_width: 0.04,
                alpha: m::FRIENDLY_ALPHA,
                color: hex(m::FRIENDLY_BODY_HEX),
                intensity: m::FRIENDLY_MULT,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_share_geometry_and_counts_match_the_spec() {
        let calm = Scene::build(Variant::Calm);
        let busy = Scene::build(Variant::Busy);
        assert_eq!(calm.opaque.vertices, busy.opaque.vertices);
        assert_eq!(calm.opaque.indices, busy.opaque.indices);
        assert_eq!(calm.emissive.vertices, busy.emissive.vertices);
        assert_eq!(calm.lights.len(), 24);
        assert_eq!(busy.lights.len(), 232);
        assert_eq!(calm.lights[..24], busy.lights[..24]);
        let calm_groups: Vec<usize> = calm.light_breakdown().iter().map(|(_, n)| *n).collect();
        assert_eq!(calm_groups, [8, 4, 2, 1, 2, 6, 1]);
        assert_eq!(busy.light_breakdown().len(), 11);
        assert_eq!(calm.bullets.len(), CALM_BULLETS);
        assert_eq!(calm.stress_counts.iter().sum::<usize>(), 18);
        assert_eq!(busy.bullets.len(), 2000);
        assert_eq!(calm.friendly_bolt_count, 12);
        assert_eq!(busy.friendly_bolt_count, 40);
        assert_eq!(calm.figures.len(), 9);
        assert_eq!(calm.floor_triangles, 51_200);
        assert!(calm.bullets.iter().chain(&busy.bullets).all(|b| b.palette_space == HOSTILE && b.flags == 0));
    }

    #[test]
    fn toppled_pillar_clears_the_standing_pillars() {
        let yaw = TOPPLED_PILLAR_YAW_DEG.to_radians();
        let dir = [yaw.cos(), yaw.sin()];
        let [cx, cy] = TOPPLED_PILLAR_CENTRE;
        for k in 0..6 {
            let a = (30.0 + 60.0 * k as f32).to_radians();
            let p = [12.5 * a.cos() - cx, 12.5 * a.sin() - cy];
            let along = (p[0] * dir[0] + p[1] * dir[1]).clamp(-2.0, 2.0);
            let gap = ((p[0] - along * dir[0]).powi(2) + (p[1] - along * dir[1]).powi(2)).sqrt();
            // Plinth half diagonal plus the shaft radius.
            assert!(gap > 2.0f32.sqrt() + 0.6 + 0.2, "pillar {k}: gap {gap}");
        }
    }

    /// Smallest distance between the wall footprint (thick arc) and any pillar plinth (square,
    /// local x radial); negative when they overlap.
    fn wall_plinth_clearance(centre: [f32; 2]) -> f32 {
        let radius = (centre[0] * centre[0] + centre[1] * centre[1]).sqrt();
        let centre_angle = centre[1].atan2(centre[0]);
        let mut best = f32::MAX;
        for k in 0..6 {
            let a = (30.0 + 60.0 * k as f32).to_radians();
            let pillar = [12.5 * a.cos(), 12.5 * a.sin()];
            for i in 0..=500 {
                let along = WALL_LENGTH * (i as f32 / 500.0 - 0.5);
                for j in 0..=4 {
                    let r = radius + WALL_HALF_THICKNESS * (j as f32 / 2.0 - 1.0);
                    let t = centre_angle + along / radius;
                    let d = [r * t.cos() - pillar[0], r * t.sin() - pillar[1]];
                    let local = [d[0] * a.cos() + d[1] * a.sin(), -d[0] * a.sin() + d[1] * a.cos()];
                    let q = [local[0].abs() - PLINTH_HALF, local[1].abs() - PLINTH_HALF];
                    let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt();
                    best = best.min(outside + q[0].max(q[1]).min(0.0));
                }
            }
        }
        best
    }

    #[test]
    fn wall_clears_the_plinths_by_one_unit() {
        assert!(wall_plinth_clearance(WALL_CENTRE_SPEC) < 0.0, "the design position overlaps a plinth");
        let clearance = wall_plinth_clearance(WALL_CENTRE);
        assert!(clearance >= 1.0 && clearance < 1.05, "clearance {clearance}");
        // Same arc radius as the design position: the move is along the arc only.
        let r = |c: [f32; 2]| (c[0] * c[0] + c[1] * c[1]).sqrt();
        assert!((r(WALL_CENTRE) - r(WALL_CENTRE_SPEC)).abs() < 0.01);
    }

    #[test]
    fn floor_groove_is_lower_than_the_tile() {
        let salt = 7;
        let (tile, g_tile) = floor_height(3.5, 2.5, salt);
        let (groove, g_groove) = floor_height(3.0, 2.5, salt);
        assert!(groove < tile - 0.02);
        assert!(g_tile < 0.01 && g_groove > 0.99);
        let (outside, _) = floor_height(17.5, 0.5, salt);
        assert!(outside < -0.1);
    }

    #[test]
    fn hash_spreads_neighbouring_cells() {
        // world.wgsl implements the same function; this only guards against degenerate edits.
        let a = hash2(0, 0, 1);
        let b = hash2(1, 0, 1);
        let c = hash2(0, 1, 1);
        assert!(a != b && b != c && a != c);
        assert_eq!(hash2(-3, 7, 99), hash2(-3, 7, 99));
    }
}
