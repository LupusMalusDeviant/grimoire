//! Readability metrics, exposure calibration statistic, run records, timing summaries and the
//! report files.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::bullet_pass::{BULLET_PASS_PALETTE_SPACE, BulletStats};
use crate::camera::{self, Camera};
use crate::gpu_types::BulletInstance;
use crate::image_io::Image;
use crate::materials::{self as m};
use crate::renderer::{self, ClassMask, Look};
use crate::scene::{self, Figure, FigureKind, LightGroup, Variant};

/// Target of the exposure calibration: median floor luminance of the calm world-only frame.
pub const FLOOR_TARGET: f32 = 0.18;
pub const FLOOR_TOLERANCE: f32 = 0.05;
/// Timing rounds per variant whose samples are discarded as warm-up.
pub const TIMING_WARMUP_ROUNDS: usize = 3;

/// How the floor target is read: as sRGB-encoded relative luminance (display value, default) or
/// as linear relative luminance Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorSpace {
    Display,
    Linear,
}

impl FloorSpace {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "display" => Some(Self::Display),
            "linear" => Some(Self::Linear),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::Linear => "linear",
        }
    }

    /// The target as linear relative luminance.
    pub fn linear_target(self, target: f32) -> f32 {
        match self {
            Self::Display => m::srgb_to_linear(target),
            Self::Linear => target,
        }
    }

    fn describe_de(self) -> &'static str {
        match self {
            Self::Display => "als sRGB-kodierte relative Luminanz (Anzeigewert)",
            Self::Linear => "als lineare relative Luminanz",
        }
    }
}

pub const WCAG_AA: f32 = 4.5;
pub const BULLET_RING_PX: (f32, f32) = (3.0, 6.0);
pub const FIGURE_RING_PX: (f32, f32) = (2.0, 6.0);
pub const CONTOUR_PX: f32 = 3.0;

pub fn wcag_contrast(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

pub fn hex_luminance(rgb: u32) -> f32 {
    m::luminance(m::hex(rgb))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Distribution {
    pub count: usize,
    pub min: f32,
    pub p5: f32,
    pub median: f32,
    pub share_aa: f32,
}

pub fn distribution(mut values: Vec<f32>) -> Distribution {
    if values.is_empty() {
        return Distribution::default();
    }
    values.sort_by(f32::total_cmp);
    let n = values.len();
    let pick = |q: f32| values[((q * (n - 1) as f32).round() as usize).min(n - 1)];
    Distribution {
        count: n,
        min: values[0],
        p5: pick(0.05),
        median: pick(0.5),
        share_aa: values.iter().filter(|v| **v >= WCAG_AA).count() as f32 / n as f32,
    }
}

fn median_f64(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        0.5 * (sorted[n / 2 - 1] + sorted[n / 2])
    }
}

/// Median luminance of floor pixels outside the decal and the blob shadows.
pub fn floor_median(world: &Image, mask: &ClassMask) -> Option<f32> {
    let mut values = Vec::new();
    for y in 0..mask.height {
        for x in 0..mask.width {
            let i = y as usize * mask.width as usize + x as usize;
            if mask.class[i] == 0 && !mask.excluded[i] {
                values.push(world.luminance(x, y));
            }
        }
    }
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some(values[values.len() / 2])
}

/// Result of the per-look calibration: the light gain on the light-derived terms (on top of the
/// global light-intensity factor) that puts the calm floor median on the target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    pub gain: f32,
    pub floor_median: f32,
    pub iterations: usize,
}

/// What one rendering process records next to its images (`data/<look>_<variant>.txt`).
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub adapter: String,
    pub floor_space: FloorSpace,
    pub floor_target: f32,
    /// Global light-intensity factor the run was rendered with.
    pub light_factor: f32,
    pub calibration: Calibration,
    /// Top-left corner of the 1x production-AA crop (busy only).
    pub crop_origin: Option<(u32, u32)>,
}

fn parse_bits(value: &str) -> Result<f32, String> {
    u32::from_str_radix(value, 16)
        .map(f32::from_bits)
        .map_err(|e| format!("{value}: {e}"))
}

impl RunRecord {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "# light factor {:.3}, light gain {:.4}, calm floor median {:.4} (linear), {} iterations",
            self.light_factor, self.calibration.gain, self.calibration.floor_median, self.calibration.iterations
        );
        let _ = writeln!(s, "adapter {}", self.adapter);
        let _ = writeln!(s, "floor_space {}", self.floor_space.key());
        let _ = writeln!(s, "floor_target_bits {:08x}", self.floor_target.to_bits());
        let _ = writeln!(s, "light_factor_bits {:08x}", self.light_factor.to_bits());
        let _ = writeln!(s, "gain_bits {:08x}", self.calibration.gain.to_bits());
        let _ = writeln!(s, "floor_median_bits {:08x}", self.calibration.floor_median.to_bits());
        let _ = writeln!(s, "iterations {}", self.calibration.iterations);
        if let Some((x, y)) = self.crop_origin {
            let _ = writeln!(s, "crop_origin {x} {y}");
        }
        s
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let (mut adapter, mut floor_space, mut floor_target, mut light_factor) = (None, None, None, None);
        let (mut gain, mut floor_median, mut iterations, mut crop_origin) = (None, None, None, None);
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once(' ').ok_or_else(|| format!("malformed record line: {line}"))?;
            match key {
                "adapter" => adapter = Some(value.to_string()),
                "floor_space" => floor_space = Some(FloorSpace::parse(value).ok_or_else(|| format!("unknown floor space {value}"))?),
                "floor_target_bits" => floor_target = Some(parse_bits(value)?),
                "light_factor_bits" => light_factor = Some(parse_bits(value)?),
                "gain_bits" => gain = Some(parse_bits(value)?),
                "floor_median_bits" => floor_median = Some(parse_bits(value)?),
                "iterations" => iterations = Some(value.parse().map_err(|e| format!("iterations {value}: {e}"))?),
                "crop_origin" => {
                    let (x, y) = value.split_once(' ').ok_or_else(|| format!("malformed crop origin: {value}"))?;
                    let coordinate = |v: &str| v.parse::<u32>().map_err(|e| format!("crop origin {v}: {e}"));
                    crop_origin = Some((coordinate(x)?, coordinate(y)?));
                }
                _ => {}
            }
        }
        let missing = |name: &str| format!("record without {name}");
        Ok(Self {
            adapter: adapter.ok_or_else(|| missing("adapter"))?,
            floor_space: floor_space.ok_or_else(|| missing("floor_space"))?,
            floor_target: floor_target.ok_or_else(|| missing("floor_target_bits"))?,
            light_factor: light_factor.ok_or_else(|| missing("light_factor_bits"))?,
            calibration: Calibration {
                gain: gain.ok_or_else(|| missing("gain_bits"))?,
                floor_median: floor_median.ok_or_else(|| missing("floor_median_bits"))?,
                iterations: iterations.ok_or_else(|| missing("iterations"))?,
            },
            crop_origin,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BulletContrast {
    pub body: Distribution,
    pub body_by_palette: [Distribution; 2],
    pub rim: Distribution,
    /// Bullets without any on-screen ring pixel.
    pub skipped: usize,
}

/// Per-bullet WCAG contrast of the body colour (and, for reference, the rim colour) against the
/// mean luminance of a ring 3 to 6 px outside the projected radius, in the world-only frame.
pub fn bullet_contrast(world: &Image, bullets: &[BulletInstance], camera: &Camera) -> BulletContrast {
    let (w, h) = (world.width as f32, world.height as f32);
    let body_luminance = [
        hex_luminance(m::HOSTILE_PALETTE[0].body_hex),
        hex_luminance(m::HOSTILE_PALETTE[1].body_hex),
    ];
    let rim_luminance = hex_luminance(m::BULLET_RIM_HEX);
    let mut body = Vec::new();
    let mut by_palette = [Vec::new(), Vec::new()];
    let mut rim = Vec::new();
    let mut skipped = 0;
    for b in bullets.iter().filter(|b| b.palette_space == BULLET_PASS_PALETTE_SPACE) {
        let centre = [b.position[0], b.position[1], renderer::BULLET_PLANE_Z];
        let edge = camera::add(centre, camera::scale(camera.right, b.radius));
        let (Some(c), Some(e)) = (camera.project(centre, w, h), camera.project(edge, w, h)) else {
            skipped += 1;
            continue;
        };
        let r_px = ((e[0] - c[0]).powi(2) + (e[1] - c[1]).powi(2)).sqrt();
        let (r0, r1) = (r_px + BULLET_RING_PX.0, r_px + BULLET_RING_PX.1);
        let x0 = (c[0] - r1).floor().max(0.0) as u32;
        let x1 = ((c[0] + r1).ceil().max(0.0) as u32).min(world.width);
        let y0 = (c[1] - r1).floor().max(0.0) as u32;
        let y1 = ((c[1] + r1).ceil().max(0.0) as u32).min(world.height);
        let (mut sum, mut n) = (0.0f32, 0usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = ((x as f32 + 0.5 - c[0]).powi(2) + (y as f32 + 0.5 - c[1]).powi(2)).sqrt();
                if d >= r0 && d <= r1 {
                    sum += world.luminance(x, y);
                    n += 1;
                }
            }
        }
        if n == 0 {
            skipped += 1;
            continue;
        }
        let background = sum / n as f32;
        let palette = (b.palette as usize).min(1);
        let contrast = wcag_contrast(body_luminance[palette], background);
        body.push(contrast);
        by_palette[palette].push(contrast);
        rim.push(wcag_contrast(rim_luminance, background));
    }
    let [p0, p1] = by_palette;
    BulletContrast {
        body: distribution(body),
        body_by_palette: [distribution(p0), distribution(p1)],
        rim: distribution(rim),
        skipped,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FigureContrast {
    pub kind: FigureKind,
    pub index: usize,
    pub pixels: usize,
    /// Mean luminance of the lit silhouette (without emissive eyes/orb) against the ring.
    pub whole: f32,
    /// Mean luminance of the inner contour band (0-3 px inside) against the ring.
    pub contour: f32,
}

/// Figure-versus-background contrast from the shared class mask: ring 2-6 px outside each
/// silhouette, excluding pixels of any figure.
pub fn figure_contrast(world: &Image, mask: &ClassMask, figures: &[Figure]) -> Vec<FigureContrast> {
    let (w, h) = (mask.width as i32, mask.height as i32);
    let at = |x: i32, y: i32| y as usize * mask.width as usize + x as usize;
    let mut out = Vec::new();
    for (index, figure) in figures.iter().enumerate() {
        let id = (index + 1) as u8;
        let in_silhouette = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && mask.figure[at(x, y)] == id;
        let mut bbox = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                if mask.figure[at(x, y)] == id {
                    bbox = (bbox.0.min(x), bbox.1.min(y), bbox.2.max(x), bbox.3.max(y));
                    pixels.push((x, y));
                }
            }
        }
        if pixels.is_empty() {
            continue;
        }
        let neighbours = |x: i32, y: i32| [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)];
        let inner_boundary: Vec<(i32, i32)> = pixels
            .iter()
            .copied()
            .filter(|&(x, y)| neighbours(x, y).iter().any(|&(nx, ny)| !in_silhouette(nx, ny)))
            .collect();
        let pad = FIGURE_RING_PX.1.ceil() as i32 + 1;
        let mut outer_boundary = Vec::new();
        let (mut ring_sum, mut ring_n) = (0.0f32, 0usize);
        for y in (bbox.1 - pad).max(0)..(bbox.3 + pad + 1).min(h) {
            for x in (bbox.0 - pad).max(0)..(bbox.2 + pad + 1).min(w) {
                if in_silhouette(x, y) {
                    continue;
                }
                if neighbours(x, y).iter().any(|&(nx, ny)| in_silhouette(nx, ny)) {
                    outer_boundary.push((x, y));
                }
                let i = at(x, y);
                let other_figure = mask.figure[i] != 0 || mask.class[i] == 2 || mask.class[i] == 3;
                if other_figure {
                    continue;
                }
                let d = nearest(&inner_boundary, x, y);
                if d >= FIGURE_RING_PX.0 && d <= FIGURE_RING_PX.1 {
                    ring_sum += world.luminance(x as u32, y as u32);
                    ring_n += 1;
                }
            }
        }
        let (mut whole_sum, mut whole_n, mut contour_sum, mut contour_n) = (0.0f32, 0usize, 0.0f32, 0usize);
        for &(x, y) in &pixels {
            let class = mask.class[at(x, y)];
            if class != 2 && class != 3 {
                continue;
            }
            let l = world.luminance(x as u32, y as u32);
            whole_sum += l;
            whole_n += 1;
            if nearest(&outer_boundary, x, y) <= CONTOUR_PX {
                contour_sum += l;
                contour_n += 1;
            }
        }
        if ring_n == 0 || whole_n == 0 {
            continue;
        }
        let ring = ring_sum / ring_n as f32;
        out.push(FigureContrast {
            kind: figure.kind,
            index,
            pixels: whole_n,
            whole: wcag_contrast(whole_sum / whole_n as f32, ring),
            contour: if contour_n > 0 {
                wcag_contrast(contour_sum / contour_n as f32, ring)
            } else {
                f32::NAN
            },
        });
    }
    out
}

fn nearest(points: &[(i32, i32)], x: i32, y: i32) -> f32 {
    points
        .iter()
        .map(|&(px, py)| (((px - x) * (px - x) + (py - y) * (py - y)) as f32).sqrt())
        .fold(f32::INFINITY, f32::min)
}

/// Wall times of one frame's steps, in seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameTiming {
    pub world: f64,
    pub outline: f64,
    pub post: f64,
    pub bullets: f64,
}

impl FrameTiming {
    pub fn total(&self) -> f64 {
        self.world + self.outline + self.post + self.bullets
    }
}

/// One line of `data/timing_<variant>.txt`.
pub fn timing_line(round: usize, look: Look, t: &FrameTiming) -> String {
    format!(
        "round {round} look {} world {:.6} outline {:.6} post {:.6} bullets {:.6}\n",
        look.key(),
        t.world,
        t.outline,
        t.post,
        t.bullets
    )
}

/// Parses the lines written by [`timing_line`].
pub fn parse_timing(text: &str) -> Result<Vec<(usize, Look, FrameTiming)>, String> {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let p: Vec<&str> = line.split_whitespace().collect();
        let labels = [(0, "round"), (2, "look"), (4, "world"), (6, "outline"), (8, "post"), (10, "bullets")];
        if p.len() != 12 || labels.iter().any(|&(i, label)| p[i] != label) {
            return Err(format!("malformed timing line: {line}"));
        }
        let seconds = |s: &str| s.parse::<f64>().map_err(|e| format!("{line}: {e}"));
        out.push((
            p[1].parse().map_err(|e| format!("{line}: {e}"))?,
            Look::parse(p[3]).ok_or_else(|| format!("unknown look in: {line}"))?,
            FrameTiming {
                world: seconds(p[5])?,
                outline: seconds(p[7])?,
                post: seconds(p[9])?,
                bullets: seconds(p[11])?,
            },
        ));
    }
    Ok(out)
}

/// Medians of each step and of the total, taken independently.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimingSummary {
    pub world: f64,
    pub outline: f64,
    pub post: f64,
    pub bullets: f64,
    pub total: f64,
    pub samples: usize,
}

pub fn summarize(samples: &[FrameTiming]) -> TimingSummary {
    let pick = |f: fn(&FrameTiming) -> f64| median_f64(&samples.iter().map(f).collect::<Vec<_>>());
    TimingSummary {
        world: pick(|t| t.world),
        outline: pick(|t| t.outline),
        post: pick(|t| t.post),
        bullets: pick(|t| t.bullets),
        total: pick(FrameTiming::total),
        samples: samples.len(),
    }
}

pub struct VariantResult {
    pub look: Look,
    pub variant: Variant,
    pub bullets: BulletContrast,
    pub figures: Vec<FigureContrast>,
    pub stats: BulletStats,
    pub lights: usize,
    pub light_breakdown: Vec<(LightGroup, usize)>,
    pub bullet_count: usize,
    pub friendly_bolts: usize,
    pub stress_counts: [usize; 4],
}

impl VariantResult {
    fn figure_summary(&self) -> (f32, f32, f32, f32) {
        let whole = distribution(self.figures.iter().map(|f| f.whole).collect());
        let contour = distribution(self.figures.iter().map(|f| f.contour).filter(|c| c.is_finite()).collect());
        (whole.median, whole.min, contour.median, contour.min)
    }
}

pub struct Report {
    pub adapter: String,
    /// Global light-intensity factor, identical for all looks.
    pub light_factor: f32,
    pub floor_space: FloorSpace,
    pub floor_target: f32,
    pub calibrations: Vec<(Look, Calibration)>,
    pub results: Vec<VariantResult>,
    pub bullet_hashes: Vec<(Look, u64)>,
    pub palette_selftest: BulletStats,
    pub timing: BTreeMap<(Variant, Look), TimingSummary>,
    pub crop_origins: Vec<(Look, (u32, u32))>,
    pub floor_triangles: usize,
    /// Missing runs and consistency warnings found while composing.
    pub notes: Vec<String>,
}

fn f(v: f32) -> String {
    if v.is_finite() { format!("{v:.2}") } else { "–".into() }
}

fn pct(v: f32) -> String {
    format!("{:.0} %", v * 100.0)
}

fn json_num(v: f64) -> String {
    if v.is_finite() { format!("{v:.4}") } else { "null".into() }
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn figure_name(kind: FigureKind) -> &'static str {
    match kind {
        FigureKind::Player => "player",
        FigureKind::Imp => "imp",
        FigureKind::Brute => "brute",
    }
}

fn dist_json(d: &Distribution) -> String {
    format!(
        "{{\"count\": {}, \"min\": {}, \"p5\": {}, \"median\": {}, \"share_ge_4_5\": {}}}",
        d.count,
        json_num(f64::from(d.min)),
        json_num(f64::from(d.p5)),
        json_num(f64::from(d.median)),
        json_num(f64::from(d.share_aa))
    )
}

fn timing_samples(report: &Report) -> usize {
    report.timing.values().map(|t| t.samples).min().unwrap_or(0)
}

pub fn write_json(path: &Path, report: &Report) -> Result<(), String> {
    let mut s = String::from("{\n");
    let _ = writeln!(s, "  \"adapter\": {},", json_str(&report.adapter));
    let _ = writeln!(
        s,
        "  \"exposure_target\": {{\"median_floor_luminance\": {}, \"space\": {}, \"linear\": {}, \"tolerance\": {}}},",
        json_num(f64::from(report.floor_target)),
        json_str(report.floor_space.key()),
        json_num(f64::from(report.floor_space.linear_target(report.floor_target))),
        json_num(f64::from(FLOOR_TOLERANCE))
    );
    let _ = writeln!(s, "  \"timing_label\": \"software adapter WARP, not a GPU budget; relative to toon of the same variant\",");
    let _ = writeln!(
        s,
        "  \"global_light_intensity_factor\": {},\n  \"post_exposure\": 1.0,",
        json_num(f64::from(report.light_factor))
    );
    s.push_str("  \"light_gain\": {");
    let items: Vec<String> = report
        .calibrations
        .iter()
        .map(|(look, c)| {
            format!(
                "\"{}\": {{\"gain\": {}, \"effective_light_scale\": {}, \"calm_floor_median_linear\": {}, \"iterations\": {}}}",
                look.key(),
                json_num(f64::from(c.gain)),
                json_num(f64::from(c.gain * report.light_factor)),
                json_num(f64::from(c.floor_median)),
                c.iterations
            )
        })
        .collect();
    s.push_str(&items.join(", "));
    s.push_str("},\n  \"results\": [\n");
    let items: Vec<String> = report
        .results
        .iter()
        .map(|r| {
            let figures: Vec<String> = r
                .figures
                .iter()
                .map(|fc| {
                    format!(
                        "{{\"figure\": {}, \"kind\": \"{}\", \"pixels\": {}, \"whole\": {}, \"contour\": {}}}",
                        fc.index + 1,
                        figure_name(fc.kind),
                        fc.pixels,
                        json_num(f64::from(fc.whole)),
                        json_num(f64::from(fc.contour))
                    )
                })
                .collect();
            let (wm, wmin, cm, cmin) = r.figure_summary();
            let rel = report.timing.get(&(r.variant, r.look)).copied();
            let base = report.timing.get(&(r.variant, Look::Toon)).copied();
            let timing = match (rel, base) {
                (Some(t), Some(b)) if b.total > 0.0 => format!(
                    "{{\"world_rel\": {}, \"outline_share_of_toon_frame\": {}, \"post_bullets_rel\": {}, \"total_rel\": {}, \"samples\": {}}}",
                    json_num(t.world / b.world),
                    json_num(t.outline / b.total),
                    json_num((t.post + t.bullets) / (b.post + b.bullets)),
                    json_num(t.total / b.total),
                    t.samples
                ),
                _ => "null".into(),
            };
            format!(
                "    {{\"look\": \"{}\", \"variant\": \"{}\", \"light_gain\": {}, \"lights\": {}, \"hostile_bullets\": {}, \"friendly_bolts\": {}, \"stress_placements\": [{}, {}, {}, {}],\n      \"bullet_body_contrast\": {},\n      \"bullet_body_contrast_h0_magenta\": {},\n      \"bullet_body_contrast_h1_lime\": {},\n      \"bullet_rim_contrast\": {},\n      \"bullets_without_ring\": {},\n      \"figure_contrast\": {{\"whole_median\": {}, \"whole_min\": {}, \"contour_median\": {}, \"contour_min\": {}, \"figures\": [{}]}},\n      \"bullets_drawn\": {}, \"bullets_rejected_palette_space\": {}, \"bullets_rejected_invalid\": {},\n      \"timing\": {}}}",
                r.look.key(),
                r.variant.key(),
                json_num(f64::from(gain_of(report, r.look))),
                r.lights,
                r.bullet_count,
                r.friendly_bolts,
                r.stress_counts[0],
                r.stress_counts[1],
                r.stress_counts[2],
                r.stress_counts[3],
                dist_json(&r.bullets.body),
                dist_json(&r.bullets.body_by_palette[0]),
                dist_json(&r.bullets.body_by_palette[1]),
                dist_json(&r.bullets.rim),
                r.bullets.skipped,
                json_num(f64::from(wm)),
                json_num(f64::from(wmin)),
                json_num(f64::from(cm)),
                json_num(f64::from(cmin)),
                figures.join(", "),
                r.stats.drawn,
                r.stats.rejected_palette_space,
                r.stats.rejected_invalid,
                timing
            )
        })
        .collect();
    s.push_str(&items.join(",\n"));
    s.push_str("\n  ],\n  \"bullets_only_busy_fnv1a64\": {");
    let hashes: Vec<String> = report
        .bullet_hashes
        .iter()
        .map(|(look, h)| format!("\"{}\": \"{h:016x}\"", look.key()))
        .collect();
    s.push_str(&hashes.join(", "));
    let notes: Vec<String> = report.notes.iter().map(|n| json_str(n)).collect();
    let _ = write!(
        s,
        "}},\n  \"bullets_only_identical\": {},\n  \"palette_space_selftest\": {{\"injected_friendly\": 1, \"rejected_palette_space\": {}}},\n  \"timing_warmup_rounds\": {}, \"timing_measured_rounds\": {},\n  \"notes\": [{}]\n}}\n",
        hashes_identical(report),
        report.palette_selftest.rejected_palette_space,
        TIMING_WARMUP_ROUNDS,
        timing_samples(report),
        notes.join(", ")
    );
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

fn gain_of(report: &Report, look: Look) -> f32 {
    report
        .calibrations
        .iter()
        .find(|(l, _)| *l == look)
        .map_or(f32::NAN, |(_, c)| c.gain)
}

fn hashes_identical(report: &Report) -> bool {
    report.bullet_hashes.len() == Look::ALL.len() && report.bullet_hashes.windows(2).all(|w| w[0].1 == w[1].1)
}

pub fn write_markdown(path: &Path, report: &Report) -> Result<(), String> {
    let mut s = String::new();
    s.push_str("# Look-Dev-Spike: Messwerte\n\n");
    s.push_str("Erzeugt von `spikes/look-dev/render_all.sh`. Eine Szene, ein Seed, eine Kamera, eine Materialtabelle; ");
    s.push_str("die Looks unterscheiden sich nur im Fragment-Einstieg (`fs_toon`, `fs_stylized`, `fs_realistic`) und im Outline-Pass von toon.\n\n");
    let _ = writeln!(s, "Adapter: {} (Software-Adapter).\n", report.adapter);
    if !report.notes.is_empty() {
        s.push_str("**Hinweise zu diesem Lauf:**\n\n");
        for note in &report.notes {
            let _ = writeln!(s, "- {note}");
        }
        s.push('\n');
    }

    s.push_str("## Legende\n\n");
    for look in Look::ALL {
        let _ = writeln!(s, "- `{}`: {}", look.key(), look.name_de());
    }
    s.push('\n');
    s.push_str("`composite_side_by_side.png` (1920x720): Spalten von links nach rechts **toon | stylized | realistic**, Zeilen **calm oben, busy unten**; ");
    s.push_str("Zellen 640x360 (Frames linear halbiert), 4-px-Stege #000000 über den Zellkanten.\n\n");
    let (cx, cy, cw, ch) = crate::composite::CROP_RECT;
    let _ = writeln!(
        s,
        "- `composite_crops.png` (1920x360): busy in nativer Auflösung (2x SSAA), Ausschnitt x {}–{}, y {}–{}; Spalten toon | stylized | realistic.",
        cx,
        cx + cw,
        cy,
        cy + ch
    );
    s.push_str("- `<look>_<variant>.png`: finaler Frame. `<look>_<variant>_world.png`: nach PostFxResolve, vor Bullets und Marker (Grundlage der Kontrastmessung).\n");
    s.push_str("- `<look>_busy_1x_crop.png`: 640x360, **ohne** SSAA gerendert (Produktions-AA), Ausschnitt zwischen Spieler und nächstem sichtbaren Fackelkegel:");
    for (look, (x, y)) in &report.crop_origins {
        let _ = write!(s, " {} ab ({x}, {y});", look.key());
    }
    s.push_str("\n- `bullets_only_busy.png`: Bullet-Pass und Marker auf transparentem Schwarz.\n\n");

    s.push_str("## Lichtkalibrierung\n\n");
    let _ = writeln!(
        s,
        "Kein Belichtungsskalar im Post-Stack (Belichtung 1,0 für alle). Stattdessen multipliziert der Shader jedes Looks die aus Lichtern abgeleiteten Terme \
         (Punktlichter, Mond, Hemisphären-Ambient; Diffus und Glanz) mit **globalem Lichtintensitätsfaktor {} × Lichtverstärkung des Looks**. \
         Der Faktor ist für alle Looks gleich und so gesetzt, dass realistic eine Verstärkung von etwa 1,0 bekommt. \
         Nicht skaliert werden Rimlight (stylized), Emissive-Meshes (Flammen, Augen, Orb), Ritualkreis-Emission, eigene Bolts und Bullets: ihre HDR-Multiplikatoren behalten die entworfene Bedeutung.\n",
        report.light_factor
    );
    let _ = writeln!(
        s,
        "Die Verstärkung je Look ist so gewählt, dass der Median der Boden-Luminanz (Klasse 0, ohne Ritualkreis und Blob-Schatten) im calm-World-only-Bild {:.2} ±{:.0} % trifft, {} (linear {:.4}); \
         Abbruch bei 1 % Abweichung. busy nutzt dieselbe Verstärkung unverändert.\n",
        report.floor_target,
        FLOOR_TOLERANCE * 100.0,
        report.floor_space.describe_de(),
        report.floor_space.linear_target(report.floor_target)
    );
    s.push_str("| Look | Lichtverstärkung | wirksame Lichtskala (Faktor × Verstärkung) | Boden-Median calm (linear) | Iterationen |\n|---|---:|---:|---:|---:|\n");
    for (look, c) in &report.calibrations {
        let _ = writeln!(
            s,
            "| {} | {:.3} | {:.3} | {:.4} | {} |",
            look.key(),
            c.gain,
            c.gain * report.light_factor,
            c.floor_median,
            c.iterations
        );
    }

    s.push_str("\n## Bullet-Kontrast\n\n");
    s.push_str("WCAG-Kontrast der Bullet-Körperfarbe gegen die mittlere Luminanz eines Rings 3–6 px außerhalb des projizierten Radius im World-only-Bild ");
    s.push_str("(kreisförmig um den großen Radius, auch bei Reis). Bullets ohne Ringpixel im Bild zählen nicht. Die Randfarbe #0A0510 steht zum Vergleich daneben. ");
    let _ = writeln!(
        s,
        "Obergrenzen: {} (L = {:.3}) erreicht 4,5:1 nur vor einem Hintergrund mit L < {:.3}, {} (L = {:.3}) bis L < {:.3}.\n",
        m::HOSTILE_PALETTE[0].name,
        hex_luminance(m::HOSTILE_PALETTE[0].body_hex),
        (hex_luminance(m::HOSTILE_PALETTE[0].body_hex) + 0.05) / WCAG_AA - 0.05,
        m::HOSTILE_PALETTE[1].name,
        hex_luminance(m::HOSTILE_PALETTE[1].body_hex),
        (hex_luminance(m::HOSTILE_PALETTE[1].body_hex) + 0.05) / WCAG_AA - 0.05
    );
    s.push_str("| Look | Variante | Bullets | min | 5. Perz. | Median | Anteil ≥ 4,5:1 | Median H0 Magenta | Median H1 Limette | Median Rand |\n");
    s.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in &report.results {
        let b = &r.bullets;
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.look.key(),
            r.variant.key(),
            b.body.count,
            f(b.body.min),
            f(b.body.p5),
            f(b.body.median),
            pct(b.body.share_aa),
            f(b.body_by_palette[0].median),
            f(b.body_by_palette[1].median),
            f(b.rim.median)
        );
    }

    s.push_str("\n## Figur-gegen-Hintergrund-Kontrast\n\n");
    s.push_str("Aus der gemeinsamen Klassenmaske: Ring 2–6 px außerhalb der Silhouette (ohne Pixel anderer Figuren) gegen ");
    s.push_str("(a) die mittlere Luminanz der beleuchteten Silhouette ohne Augen/Orb und (b) das innere Konturband 0–3 px. 9 Figuren.\n\n");
    s.push_str("| Look | Variante | Median gesamt | min gesamt | Median Kontur | min Kontur |\n|---|---|---:|---:|---:|---:|\n");
    for r in &report.results {
        let (wm, wmin, cm, cmin) = r.figure_summary();
        let _ = writeln!(s, "| {} | {} | {} | {} | {} | {} |", r.look.key(), r.variant.key(), f(wm), f(wmin), f(cm), f(cmin));
    }
    s.push_str("\nJe Figur (gesamt / Kontur), Reihenfolge: 1 Spieler, 2–7 Imps, 8–9 Brutes:\n\n");
    s.push_str("| Look | Variante | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in &report.results {
        let mut cells = vec!["–".to_string(); 9];
        for fc in &r.figures {
            cells[fc.index] = format!("{} / {}", f(fc.whole), f(fc.contour));
        }
        let _ = writeln!(s, "| {} | {} | {} |", r.look.key(), r.variant.key(), cells.join(" | "));
    }

    s.push_str("\n## Palettenraum und Bullet-Pass\n\n");
    s.push_str("| Look | Variante | Bullets gezeichnet | bullets_rejected_palette_space | bullets_rejected_invalid | Szenen-Lichter | Freundliche Bolts | Stress (Fackel/Kreis/Spieler/Säule) |\n");
    s.push_str("|---|---|---:|---:|---:|---:|---:|---|\n");
    for r in &report.results {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {}/{}/{}/{} |",
            r.look.key(),
            r.variant.key(),
            r.stats.drawn,
            r.stats.rejected_palette_space,
            r.stats.rejected_invalid,
            r.lights,
            r.friendly_bolts,
            r.stress_counts[0],
            r.stress_counts[1],
            r.stress_counts[2],
            r.stress_counts[3]
        );
    }
    s.push_str("\nPunktlichter je Gruppe:\n\n");
    for variant in Variant::ALL {
        if let Some(r) = report.results.iter().find(|r| r.variant == variant) {
            let groups: Vec<String> = r.light_breakdown.iter().map(|(g, n)| format!("{n} {}", g.name_de())).collect();
            let _ = writeln!(s, "- {}: {} = {}", variant.key(), r.lights, groups.join(" + "));
        }
    }
    let _ = writeln!(
        s,
        "\nSelbsttest: eine zusätzlich eingeschleuste FRIENDLY-Instanz wird verworfen (`bullets_rejected_palette_space` = {}).\n",
        report.palette_selftest.rejected_palette_space
    );
    s.push_str("Look-Unabhängigkeit: Bullet-Pass und Marker je Look in einem eigenen Prozess auf transparentes Schwarz gerendert (FNV-1a-64 der RGBA-Bytes):\n\n");
    for (look, hash) in &report.bullet_hashes {
        let _ = writeln!(s, "- {}: `{hash:016x}`", look.key());
    }
    let _ = writeln!(
        s,
        "\nByte-identisch: **{}**.\n",
        if hashes_identical(report) { "ja" } else { "nein" }
    );

    s.push_str("## Relative Kosten\n\n");
    s.push_str("**Software-Adapter (WARP), kein GPU-Budget.** CPU-Wandzeit um Queue-Submit plus `device.poll(Wait)` je Schritt, relativ zu toon derselben Variante. ");
    let _ = write!(
        s,
        "Jede Runde ist ein eigener kurzer Prozess: jeder Look wird zuerst einmal ungemessen gerendert, dann werden alle drei in je Runde rotierter Reihenfolge gemessen. \
         {TIMING_WARMUP_ROUNDS} Aufwärmrunden verworfen, Median aus {} Runden. ",
        timing_samples(report)
    );
    s.push_str("Die CPU-Rasterisierung verzerrt auch relative Kosten (SIMD-freundliche Mathematik gegen Verzweigungen und `pow`), ");
    s.push_str("und die Schleife über alle Lichter ohne Clustering staucht die Abstände. Der Toon-Lichtterm ist im Spike schwerer als entworfen, ");
    s.push_str("weil WGSL nach dem Radius-Early-out keine Ableitungen erlaubt und `fwidth(x)` je Licht analytisch nachgebildet wird.\n\n");
    if report.timing.is_empty() {
        s.push_str("(Keine Zeitmessung in diesem Lauf.)\n");
    } else {
        s.push_str("| Variante | Look | World-Pass | Outline-Pass (Anteil am toon-Frame) | Post + Bullets | Gesamt |\n|---|---|---:|---:|---:|---:|\n");
        for ((variant, look), t) in &report.timing {
            let Some(base) = report.timing.get(&(*variant, Look::Toon)) else {
                continue;
            };
            let _ = writeln!(
                s,
                "| {} | {} | {:.2} | {:.2} | {:.2} | {:.2} |",
                variant.key(),
                look.key(),
                t.world / base.world,
                t.outline / base.total,
                (t.post + t.bullets) / (base.post + base.bullets),
                t.total / base.total
            );
        }
    }

    s.push_str("\n## Eingefrorene Look-Parameter\n\n");
    s.push_str("Gemeinsam: Albedo je Material (Tabelle unten), Licht-Falloff `saturate(1-(d/r)^4)^2/(1+d^2)`, Mond #9AB0D8 I 0,35 entlang (-0,35, 0,5, -0,8), ");
    s.push_str("Hemisphären-Ambient Himmel #1C2438 / Boden #110D0B I 0,35, Bloom Schwelle 1,0 / Knie 0,5 / 4 Stufen / 0,08, Khronos PBR Neutral, 2x SSAA.\n\n");
    let _ = write!(
        s,
        "- **toon:** Bänder je Punktlicht bei x = {} und {} (Stufen 0 / 0,45 / 1,0; x = saturate(N·L)·atten), Beitrag `c·I·0,3·B(x)`, \
         Kantenbreite `max(fwidth(x), {})` (analytisch propagiert); Mond bei 0,05 und 0,5 mit `max(fwidth(x), 0,004)`; ",
        m::TOON_BAND_LOW,
        m::TOON_BAND_HIGH,
        m::TOON_EDGE_MIN
    );
    s.push_str("Outline: Roberts-Kreuz 3 SS-px (1x: 2 px), Tiefe 0,015 relativ, Normale 0,35 (Figur/Prop/Klassenwechsel) bzw. 0,6 (Boden–Boden), Klassenkante an Figuren, `hdr *= 1 - 0,92·edge`.\n");
    s.push_str("- **stylized:** Wrap 0,45, Terminator-Tönung `mix(S, 1, smoothstep(0, 0,6, d))`, normalisiertes Blinn-Phong; Rim `k·(1-N·V)^p·Farbe·(0,6+0,4·saturate(N.z+0,3))` ");
    s.push_str("nur an Figuren: Spieler p 3,0 k 0,55 #A8E6FF, Imps p 2,5 k 0,35 #FF9A6A, Brutes p 2,5 k 0,40 #FF9A6A.\n");
    s.push_str("- **realistic:** Cook-Torrance (GGX, höhenkorreliertes Smith, Schlick), Rauheit ≥ 0,25, Ambient `amb(N)·albedo·(1-m) + amb(R)·EnvBRDFApprox`.\n\n");
    s.push_str("| Material | Albedo | Schattenton (stylized) | Glanz g | ks | Rauheit | Metall |\n|---|---|---|---:|---:|---:|---:|\n");
    for mat in m::MATERIALS.iter().filter(|mat| mat.emissive_mult == 0.0) {
        let _ = writeln!(
            s,
            "| {} | #{:06X} | #{:06X} | {} | {} | {} | {} |",
            mat.name, mat.albedo_hex, mat.shadow_tint_hex, mat.gloss, mat.ks, mat.roughness, mat.metalness
        );
    }

    s.push_str("\n## Tuning-Protokoll und Korrekturen vor dem ersten gültigen Rendern\n\n");
    s.push_str("Keine Tuning-Runde nach dem Betrachten der Bilder. Vor dem ersten gültigen Rendern korrigiert:\n\n");
    let (old_low, old_high, old_edge) = m::TOON_BANDS_SPEC;
    let _ = writeln!(
        s,
        "- **Fehlerbehebung toon-Bänder:** alt {old_low} / {old_high} (Mindestkantenbreite {old_edge}), neu {} / {} (Mindestkantenbreite {}). \
         Die alte obere Schwelle war unter den Säulenfackeln unerreichbar: der Boden unter einer Fackel in 3,8 Höhe (Radius 9) erreicht höchstens x ≈ 0,061. \
         Alle drei Werte sind mit demselben Faktor {:.1} skaliert, damit ihre Verhältnisse bleiben; die obere Schwelle liegt bei zwei Dritteln dieses Maximums, \
         die Bandkanten unter einer Säulenfackel bei etwa 2,0 und 6,0 Einheiten Abstand. Weiter 3 Bänder, Beitragsfaktor 0,3 unverändert.",
        m::TOON_BAND_LOW,
        m::TOON_BAND_HIGH,
        m::TOON_EDGE_MIN,
        m::TOON_BAND_HIGH / old_high
    );
    s.push_str("- **Kalibrierung:** Belichtungsskalar im Post-Stack ersetzt durch eine Lichtverstärkung auf die Lichtterme (siehe Lichtkalibrierung). \
                Der Skalar hatte die gemeinsamen Emissives mitskaliert: Flammen waren in toon 1,65x heller, eigene Bolts liefen ins Weiße.\n");
    let _ = writeln!(
        s,
        "- **calm-Bullets:** 3 Spiralarme mit {} statt 20 Reiskörnern, damit calm die entworfenen {} Bullets hat (36 + 36 + 90 + 18). \
         Die 18 verstreuten Orbs sind die calm-Stressplatzierungen (6 Fackelkegel, 6 Ritualkreis, 3 am Spieler, 3 über Säulen); busy nimmt seine aus dem Driftfeld.",
        scene::CALM_SPIRAL_RICE,
        scene::CALM_BULLETS
    );

    s.push_str("\n## Auslegungen (für alle Looks gleich)\n\n");
    let [tx, ty] = scene::TOPPLED_PILLAR_CENTRE;
    let _ = writeln!(
        s,
        "- Umgestürzte Säule bei ({tx}, {ty}) statt (10, −6): an der Spezifikationsposition läuft sie durch den Sockel der intakten Säule bei 330°. Das Glutlicht wandert mit."
    );
    let ([wx, wy], [sx, sy]) = (scene::WALL_CENTRE, scene::WALL_CENTRE_SPEC);
    let _ = writeln!(
        s,
        "- Mauerbogen bei ({wx}, {wy}) statt ({sx}, {sy}): der tangential zum Arenakreis liegende Bogen (Radius 12,04, Länge {}) lief durch Säule und Sockel bei 150°. \
         Er ist entlang seines eigenen Bogens verschoben (Radius und tangentiale Ausrichtung bleiben), um das Minimum für mindestens 1 Einheit Abstand zum Sockel: 2,12 Einheiten, Abstand jetzt 1,01.",
        scene::WALL_LENGTH
    );
    s.push_str("- Gebrochene Säulen (90° und 210°) tragen ihre Fackel knapp unter der Bruchkante statt in 3,6 Höhe.\n");
    s.push_str("- Kalibrierziel: siehe Lichtkalibrierung; `--floor-space linear` rendert die lineare Lesart.\n");
    s.push_str("- busy: Randkerzen, Runensteine und schwebende Glut bekommen sichtbare Emissive-Punkte (World-Layer, in allen Looks gleich).\n");
    s.push_str("- Hintergrund: die Clear-Farbe ist durch den Tonemapper zurückgerechnet, damit #06070A ankommt.\n");

    s.push_str("\n## Befunde (berichtet, nicht getunt)\n\n");
    let target_linear = report.floor_space.linear_target(report.floor_target);
    let grey_in = (target_linear / 6.25).sqrt();
    let _ = writeln!(
        s,
        "- **Fuß von PBR Neutral:** für einen Minimum-Kanal x < 0,08 zieht der Tonemapper `x − 6,25x²` von allen Kanälen ab. Das Kalibrierziel liegt darin: \
         ein neutrales Grau braucht HDR {grey_in:.3}, um als {target_linear:.4} (linear) anzukommen, also {:.1}x dunkler. \
         Im Fuß wächst die Luminanz etwa quadratisch mit der Lichtverstärkung, und farbige dunkle Töne werden gesättigt, weil der kleinste Kanal relativ am stärksten sinkt. \
         Das gilt für alle Looks gleich, drückt aber Schattenzeichnung und Terminator-Tönung (stylized) zusammen.",
        grey_in / target_linear.max(1e-6)
    );
    let h0 = hex_luminance(m::HOSTILE_PALETTE[0].body_hex);
    let _ = writeln!(
        s,
        "- **Kontrastgrenze von H0 Hexenmagenta:** Körper-Luminanz L = {h0:.3}; 4,5:1 ist nur vor einem Hintergrund mit L < {:.3} möglich, also nur vor fast schwarzem Boden. \
         Die magentafarbenen Cluster-Lichter tönen den Boden unter dichten Bullet-Wolken zusätzlich zur Bullet-Farbe hin. \
         Die Lesbarkeit von H0 trägt der dunkle Rand #0A0510, nicht der Körper; das betrifft alle Looks, schadet kontrastarmen Looks aber mehr.",
        (h0 + 0.05) / WCAG_AA - 0.05
    );

    s.push_str("\n## Grenzen des Spikes (nicht dem Look anzulasten)\n\n");
    s.push_str("- Prozedurale Proxys ohne Texturen, Normal-Maps und Schatten: benachteiligt **realistic** am stärksten (GGX lebt von Materialdetail; ohne sie wirkt es wie Ton oder Plastik).\n");
    s.push_str("- Einfache konvexe Proxys schmeicheln dem **stylized**-Rimlight und geben **toon**-Outlines saubere Silhouetten; Blender-Modelle mit Konkaven und Stofffalten werden beides unruhiger machen.\n");
    s.push_str("- Kein Clustering: jedes Fragment läuft über alle Lichter mit gleichem Radius-Early-out; das staucht die Kostenabstände.\n");
    s.push_str("- Standbilder zeigen keine zeitliche Stabilität (Outline- und Specular-Flimmern); dafür die 1x-Ausschnitte.\n");
    let _ = writeln!(
        s,
        "- Engine-Grenzen gelten für alle Looks gleich: `Features::empty()` und WebGL2-Downlevel-Limits, Lichter in einem Uniform-Array (max. 256), keine Storage-Buffer. Boden {} Dreiecke.",
        report.floor_triangles
    );
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_contrast_matches_reference_values() {
        assert!((wcag_contrast(1.0, 0.0) - 21.0).abs() < 1e-4);
        assert!((wcag_contrast(0.2, 0.2) - 1.0).abs() < 1e-6);
        assert_eq!(wcag_contrast(0.0, 1.0), wcag_contrast(1.0, 0.0));
    }

    #[test]
    fn distribution_reports_order_statistics() {
        let d = distribution((1..=100).map(|v| v as f32 / 10.0).collect());
        assert_eq!(d.count, 100);
        assert!((d.min - 0.1).abs() < 1e-6);
        assert!((d.median - 5.0).abs() < 0.11);
        assert!((d.share_aa - 0.56).abs() < 1e-6);
    }

    #[test]
    fn timing_medians_are_per_step() {
        let samples = [
            FrameTiming { world: 1.0, outline: 0.0, post: 3.0, bullets: 0.0 },
            FrameTiming { world: 2.0, outline: 0.0, post: 1.0, bullets: 0.0 },
            FrameTiming { world: 3.0, outline: 0.0, post: 2.0, bullets: 0.0 },
        ];
        let s = summarize(&samples);
        assert_eq!(s.world, 2.0);
        assert_eq!(s.post, 2.0);
        assert_eq!(s.total, 4.0);
    }

    #[test]
    fn timing_lines_round_trip() {
        let t = FrameTiming { world: 1.25, outline: 0.125, post: 0.5, bullets: 0.0625 };
        let text = timing_line(4, Look::Stylized, &t) + &timing_line(5, Look::Toon, &t);
        let parsed = parse_timing(&text).expect("parses");
        assert_eq!(parsed, vec![(4, Look::Stylized, t), (5, Look::Toon, t)]);
        assert!(parse_timing("round x").is_err());
    }

    #[test]
    fn run_records_round_trip_bit_exact() {
        let record = RunRecord {
            adapter: "Microsoft Basic Render Driver (Cpu, Dx12)".into(),
            floor_space: FloorSpace::Display,
            floor_target: FLOOR_TARGET,
            light_factor: 8.25,
            calibration: Calibration { gain: 1.004_321, floor_median: 0.027_3, iterations: 7 },
            crop_origin: Some((320, 187)),
        };
        assert_eq!(RunRecord::parse(&record.to_text()).expect("parses"), record);
        let calm = RunRecord { crop_origin: None, ..record };
        assert_eq!(RunRecord::parse(&calm.to_text()).expect("parses"), calm);
    }

    #[test]
    fn display_target_is_the_srgb_encoded_value() {
        assert!((FloorSpace::Display.linear_target(0.18) - 0.027_27).abs() < 1e-4);
        assert_eq!(FloorSpace::Linear.linear_target(0.18), 0.18);
    }
}
