//! Look-dev spike (plan 0002 WP2): renders one seeded dark-fantasy arena in three shading looks
//! (toon, stylized 3D, realistic) offscreen on the software adapter and writes comparison images,
//! composites and readability metrics.
//!
//! Every GPU process is kept short (one look x variant, or one timing round), so a guard command
//! can run between them; `render_all.sh` drives the sequence.
//!
//! ```text
//! # one look x variant: final and world-only frame, busy extras, run record, into out/
//! GRIMOIRE_GPU_ADAPTER=software cargo run --release -- --look stylized --variant busy --out out
//! # just the final frame
//! GRIMOIRE_GPU_ADAPTER=software cargo run --release -- --look toon --variant calm --out toon_calm.png
//! # one timing round: all three looks in rotated order
//! GRIMOIRE_GPU_ADAPTER=software cargo run --release -- --time-round 0 --variant busy --out out
//! # composites and metrics from the files in out/ (no GPU)
//! cargo run --release -- --compose --out out
//! ```

mod bullet_pass;
mod camera;
mod composite;
mod gpu_types;
mod image_io;
mod materials;
mod mesh;
mod metrics;
mod renderer;
mod scene;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use grimoire_gpu::{AdapterOverride, ENV_GPU_ADAPTER, OffscreenTarget};

use crate::camera::{Camera, Vec3};
use crate::image_io::Image;
use crate::materials as m;
use crate::metrics::{
    Calibration, FLOOR_TARGET, FLOOR_TOLERANCE, FloorSpace, FrameTiming, Report, RunRecord, TIMING_WARMUP_ROUNDS,
    VariantResult,
};
use crate::renderer::{ClassMask, Look, OUT_HEIGHT, OUT_WIDTH, Renderer, SceneGpu, Targets};
use crate::scene::{PLAYER_POS, Scene, Variant};

/// Sub-directory of the output directory for run records, class masks, per-look bullet renders
/// and timing samples.
const DATA_DIR: &str = "data";
const CROP_WIDTH: u32 = 640;
const CROP_HEIGHT: u32 = 360;
const SWAY_FRAMES: usize = 16;
const SWAY_YAW_DEG: f32 = 1.5;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

const USAGE: &str = "usage:
  look-dev --look <toon|stylized|realistic|all> --variant <calm|busy|all> --out <dir | file.png>
           [--floor-target 0.18] [--floor-space display|linear] [--sway]
  look-dev --time-round <k> --variant <calm|busy> --out <dir> [--warmup 1]
  look-dev --compose --out <dir>";

struct RenderArgs {
    looks: Vec<Look>,
    variants: Vec<Variant>,
    out: PathBuf,
    floor_target: f32,
    floor_space: FloorSpace,
    sway: bool,
}

enum Command {
    Render(RenderArgs),
    TimeRound {
        variant: Variant,
        round: usize,
        warmup: usize,
        out: PathBuf,
    },
    Compose {
        out: PathBuf,
    },
}

fn parse_args() -> Result<Command, String> {
    let mut looks = None;
    let mut variants = None;
    let mut out = None;
    let mut floor_target = FLOOR_TARGET;
    let mut floor_space = FloorSpace::Display;
    let mut sway = false;
    let mut round = None;
    let mut warmup = 1;
    let mut compose = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--look" => {
                let v = value()?;
                looks = Some(if v == "all" {
                    Look::ALL.to_vec()
                } else {
                    vec![Look::parse(&v).ok_or_else(|| format!("unknown look {v}\n{USAGE}"))?]
                });
            }
            "--variant" => {
                let v = value()?;
                variants = Some(if v == "all" {
                    Variant::ALL.to_vec()
                } else {
                    vec![Variant::parse(&v).ok_or_else(|| format!("unknown variant {v}\n{USAGE}"))?]
                });
            }
            "--out" => out = Some(PathBuf::from(value()?)),
            "--floor-target" => floor_target = value()?.parse().map_err(|e| format!("--floor-target: {e}"))?,
            "--floor-space" => {
                let v = value()?;
                floor_space = FloorSpace::parse(&v).ok_or_else(|| format!("unknown floor space {v}\n{USAGE}"))?;
            }
            "--sway" => sway = true,
            "--time-round" => round = Some(value()?.parse().map_err(|e| format!("--time-round: {e}"))?),
            "--warmup" => warmup = value()?.parse().map_err(|e| format!("--warmup: {e}"))?,
            "--compose" => compose = true,
            "--help" | "-h" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    let out = out.ok_or_else(|| format!("--out is required\n{USAGE}"))?;
    if compose {
        return Ok(Command::Compose { out });
    }
    if let Some(round) = round {
        let variant = match variants.as_deref() {
            Some([variant]) => *variant,
            _ => return Err(format!("--time-round needs exactly one --variant\n{USAGE}")),
        };
        return Ok(Command::TimeRound {
            variant,
            round,
            warmup,
            out,
        });
    }
    Ok(Command::Render(RenderArgs {
        looks: looks.ok_or_else(|| format!("--look is required\n{USAGE}"))?,
        variants: variants.ok_or_else(|| format!("--variant is required\n{USAGE}"))?,
        out,
        floor_target,
        floor_space,
        sway,
    }))
}

fn main() -> ExitCode {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
    let result = parse_args().and_then(|command| match command {
        Command::Render(args) => render(&args),
        Command::TimeRound {
            variant,
            round,
            warmup,
            out,
        } => time_round(variant, round, warmup, &out),
        Command::Compose { out } => compose(&out),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn gpu<T>(result: Result<T, grimoire_gpu::GpuError>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

fn io<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{}: {e}", path.display()))
}

fn aspect() -> f32 {
    OUT_WIDTH as f32 / OUT_HEIGHT as f32
}

/// The renderer on the CPU adapter; refuses to run on anything else.
fn cpu_renderer() -> Result<Renderer, String> {
    if AdapterOverride::from_env() != AdapterOverride::Software {
        return Err(format!(
            "set {ENV_GPU_ADAPTER}=software: this spike renders only on the CPU adapter (WARP / lavapipe)"
        ));
    }
    let renderer = gpu(Renderer::new())?;
    if !renderer.is_cpu_adapter() {
        return Err(format!("adapter is not a CPU adapter: {}", renderer.adapter_summary()));
    }
    eprintln!("adapter: {}", renderer.adapter_summary());
    Ok(renderer)
}

/// Light gain per look: the median floor luminance of the calm world-only frame hits `target`
/// (linear relative luminance). The gain scales only the direct light, so every iteration
/// renders the world pass again. The update is a secant step in log-log space, because the
/// tonemap toe makes the floor luminance grow faster than linearly with the gain.
fn calibrate(r: &Renderer, look: Look, calm: &SceneGpu, t: &Targets, target: f32) -> Result<Calibration, String> {
    const MAX_ITERATIONS: usize = 12;
    const CONVERGED: f32 = 0.01;
    let mut gain = 1.0f32;
    let mut previous: Option<(f32, f32)> = None;
    let mut mask = None;
    for iteration in 1..=MAX_ITERATIONS {
        gpu(r.world_pass(look, calm, t, gain))?;
        if look == Look::Toon {
            gpu(r.outline_pass(t))?;
        }
        gpu(r.post_pass(look, t))?;
        if mask.is_none() {
            // The class MRT does not depend on the gain.
            mask = Some(gpu(r.read_class_mask(t))?);
        }
        let world = gpu(r.read(&t.out))?;
        let median = metrics::floor_median(&world, mask.as_ref().expect("read above"))
            .ok_or("no unexcluded floor pixels")?;
        let ratio = median / target;
        eprintln!("  {} calibration {iteration}: gain {gain:.4}, floor median {median:.5}", look.key());
        if (ratio - 1.0).abs() <= CONVERGED || iteration == MAX_ITERATIONS {
            if (ratio - 1.0).abs() > FLOOR_TOLERANCE {
                return Err(format!("{}: light gain calibration missed the target (median {median:.4})", look.key()));
            }
            return Ok(Calibration {
                gain,
                floor_median: median,
                iterations: iteration,
            });
        }
        // d ln(median) / d ln(gain): 1 in the linear range, up to 2 in the tonemap toe.
        let slope = match previous {
            Some((g0, m0)) if (gain / g0 - 1.0).abs() > 1e-4 && (median / m0 - 1.0).abs() > 1e-4 => {
                ((median / m0).ln() / (gain / g0).ln()).clamp(0.5, 2.5)
            }
            _ => 1.5,
        };
        previous = Some((gain, median));
        gain *= (target / median.max(1e-6)).powf(1.0 / slope);
    }
    unreachable!("the loop returns on its last iteration")
}

struct Frame {
    world: Image,
    final_image: Image,
}

fn step_timing(r: &Renderer, look: Look, scene: &SceneGpu, t: &Targets, gain: f32) -> Result<FrameTiming, String> {
    let world = gpu(r.world_pass(look, scene, t, gain))?;
    let outline = if look == Look::Toon {
        gpu(r.outline_pass(t))?
    } else {
        Duration::ZERO
    };
    let post = gpu(r.post_pass(look, t))?;
    let bullets = gpu(r.bullet_pass(scene, &t.out, false))?;
    Ok(FrameTiming {
        world: world.as_secs_f64(),
        outline: outline.as_secs_f64(),
        post: post.as_secs_f64(),
        bullets: bullets.as_secs_f64(),
    })
}

fn render_frame(r: &Renderer, look: Look, scene: &SceneGpu, t: &Targets, gain: f32) -> Result<Frame, String> {
    gpu(r.world_pass(look, scene, t, gain))?;
    if look == Look::Toon {
        gpu(r.outline_pass(t))?;
    }
    gpu(r.post_pass(look, t))?;
    let world = gpu(r.read(&t.out))?;
    gpu(r.bullet_pass(scene, &t.out, false))?;
    let final_image = gpu(r.read(&t.out))?;
    Ok(Frame { world, final_image })
}

fn save(image: &Image, dir: &Path, name: &str) -> Result<(), String> {
    image.write_png(&dir.join(name))?;
    eprintln!("  wrote {name}");
    Ok(())
}

fn record_path(data: &Path, look: Look, variant: Variant) -> PathBuf {
    data.join(format!("{}_{}.txt", look.key(), variant.key()))
}

/// Top-left corner of the 640x360 production-AA crop: centred between the player and the floor
/// under the nearest torch whose pool centre is inside the frame.
fn crop_origin(scene: &Scene, camera: &Camera) -> (u32, u32) {
    let (w, h) = (OUT_WIDTH as f32, OUT_HEIGHT as f32);
    let player = camera
        .project([PLAYER_POS[0], PLAYER_POS[1], 0.9], w, h)
        .unwrap_or([w / 2.0, h / 2.0]);
    let inside = |p: &[f32; 2]| p[0] >= 0.0 && p[0] < w && p[1] >= 0.0 && p[1] < h;
    let distance2 = |t: &Vec3| (t[0] - PLAYER_POS[0]).powi(2) + (t[1] - PLAYER_POS[1]).powi(2);
    let pool = scene
        .torch_positions
        .iter()
        .filter_map(|t| {
            let p = camera.project([t[0], t[1], 0.0], w, h)?;
            inside(&p).then_some((distance2(t), p))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(player, |(_, p)| p);
    let cx = 0.5 * (player[0] + pool[0]);
    let cy = 0.5 * (player[1] + pool[1]);
    let x = (cx - CROP_WIDTH as f32 / 2.0).round().clamp(0.0, (OUT_WIDTH - CROP_WIDTH) as f32) as u32;
    let y = (cy - CROP_HEIGHT as f32 / 2.0).round().clamp(0.0, (OUT_HEIGHT - CROP_HEIGHT) as f32) as u32;
    (x, y)
}

/// Renders the requested looks x variants: per pair the final and world-only frame, the class
/// mask and a run record; for busy additionally the bullets-only frame and the 1x crop.
fn render(args: &RenderArgs) -> Result<(), String> {
    let renderer = cpu_renderer()?;
    let camera = Camera::new(aspect(), 0.0);
    let floor_target = args.floor_space.linear_target(args.floor_target);
    eprintln!(
        "exposure target: median floor luminance {} ({}) = {floor_target:.4} linear",
        args.floor_target,
        args.floor_space.key()
    );
    let calm = Scene::build(Variant::Calm);
    let calm_gpu = gpu(renderer.upload_scene(&calm, &camera))?;
    let busy = if args.variants.contains(&Variant::Busy) {
        let scene = Scene::build(Variant::Busy);
        let upload = gpu(renderer.upload_scene(&scene, &camera))?;
        Some((scene, upload))
    } else {
        None
    };
    let pick = |variant: Variant| match variant {
        Variant::Calm => (&calm, &calm_gpu),
        Variant::Busy => {
            let (scene, upload) = busy.as_ref().expect("the busy scene is built when busy is requested");
            (scene, upload)
        }
    };
    let targets = gpu(renderer.targets(2))?;

    if args.out.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        let (&[look], &[variant]) = (args.looks.as_slice(), args.variants.as_slice()) else {
            return Err("a .png output needs exactly one --look and one --variant".into());
        };
        let calibration = calibrate(&renderer, look, &calm_gpu, &targets, floor_target)?;
        let frame = render_frame(&renderer, look, pick(variant).1, &targets, calibration.gain)?;
        if let Some(parent) = args.out.parent().filter(|p| !p.as_os_str().is_empty()) {
            io(parent, std::fs::create_dir_all(parent))?;
        }
        frame.final_image.write_png(&args.out)?;
        eprintln!(
            "{} {}: light gain {:.4} (factor {}), wrote {}",
            look.key(),
            variant.key(),
            calibration.gain,
            m::LIGHT_INTENSITY_FACTOR,
            args.out.display()
        );
        return Ok(());
    }

    let dir = args.out.as_path();
    let data = dir.join(DATA_DIR);
    io(&data, std::fs::create_dir_all(&data))?;
    let busy_targets = match busy {
        Some(_) => Some((
            gpu(renderer.targets(1))?,
            gpu(OffscreenTarget::new(renderer.context(), OUT_WIDTH, OUT_HEIGHT))?,
        )),
        None => None,
    };
    for &look in &args.looks {
        // A busy-only run reuses the gain of this look's calm run when that run used the same
        // exposure target and light factor; otherwise it calibrates on the calm scene itself.
        let recorded = (!args.variants.contains(&Variant::Calm))
            .then(|| std::fs::read_to_string(record_path(&data, look, Variant::Calm)).ok())
            .flatten()
            .and_then(|text| RunRecord::parse(&text).ok())
            .filter(|r| {
                r.floor_space == args.floor_space
                    && r.floor_target.to_bits() == args.floor_target.to_bits()
                    && r.light_factor.to_bits() == m::LIGHT_INTENSITY_FACTOR.to_bits()
            });
        let calibration = match recorded {
            Some(record) => {
                eprintln!("{}: light gain taken from the calm run record", look.key());
                record.calibration
            }
            None => calibrate(&renderer, look, &calm_gpu, &targets, floor_target)?,
        };
        eprintln!(
            "{}: light gain {:.4} x factor {} (calm floor median {:.4} linear, {} iterations)",
            look.key(),
            calibration.gain,
            m::LIGHT_INTENSITY_FACTOR,
            calibration.floor_median,
            calibration.iterations
        );
        for &variant in &args.variants {
            let (scene, upload) = pick(variant);
            let stem = format!("{}_{}", look.key(), variant.key());
            let frame = render_frame(&renderer, look, upload, &targets, calibration.gain)?;
            let mask: ClassMask = gpu(renderer.read_class_mask(&targets))?;
            save(&frame.world, dir, &format!("{stem}_world.png"))?;
            save(&frame.final_image, dir, &format!("{stem}.png"))?;
            save(&mask.to_image(), &data, &format!("{stem}_mask.png"))?;
            let mut crop = None;
            if variant == Variant::Busy {
                let (t1, bullets_target) = busy_targets.as_ref().expect("busy targets exist when busy is requested");
                gpu(renderer.bullet_pass(upload, bullets_target, true))?;
                save(
                    &gpu(renderer.read(bullets_target))?,
                    &data,
                    &format!("bullets_only_busy_{}.png", look.key()),
                )?;
                let frame_1x = render_frame(&renderer, look, upload, t1, calibration.gain)?;
                let (x, y) = crop_origin(scene, &camera);
                save(
                    &frame_1x.final_image.crop(x, y, CROP_WIDTH, CROP_HEIGHT),
                    dir,
                    &format!("{}_busy_1x_crop.png", look.key()),
                )?;
                crop = Some((x, y));
                if args.sway {
                    let mut sway_scene = gpu(renderer.upload_scene(scene, &camera))?;
                    for k in 0..SWAY_FRAMES {
                        let yaw = SWAY_YAW_DEG * (std::f32::consts::TAU * k as f32 / SWAY_FRAMES as f32).sin();
                        renderer.set_camera(&mut sway_scene, &Camera::new(aspect(), yaw));
                        let frame = render_frame(&renderer, look, &sway_scene, t1, calibration.gain)?;
                        save(&frame.final_image, dir, &format!("{}_busy_sway_{k:02}.png", look.key()))?;
                    }
                }
            }
            // Written last: its presence marks a complete run.
            let record = RunRecord {
                adapter: renderer.adapter_summary(),
                floor_space: args.floor_space,
                floor_target: args.floor_target,
                light_factor: m::LIGHT_INTENSITY_FACTOR,
                calibration,
                crop_origin: crop,
            };
            let path = record_path(&data, look, variant);
            io(&path, std::fs::write(&path, record.to_text()))?;
        }
    }
    Ok(())
}

/// One timing round for `variant`: every look once untimed (`warmup` times), then all three
/// timed in an order rotated by `round`; samples are appended to `data/timing_<variant>.txt`.
fn time_round(variant: Variant, round: usize, warmup: usize, dir: &Path) -> Result<(), String> {
    let renderer = cpu_renderer()?;
    let camera = Camera::new(aspect(), 0.0);
    let scene = Scene::build(variant);
    let upload = gpu(renderer.upload_scene(&scene, &camera))?;
    let targets = gpu(renderer.targets(2))?;
    let data = dir.join(DATA_DIR);
    io(&data, std::fs::create_dir_all(&data))?;
    // The gain does not change the work done; the calibrated one is used when present.
    let gains: Vec<f32> = Look::ALL
        .iter()
        .map(|&look| {
            std::fs::read_to_string(record_path(&data, look, Variant::Calm))
                .ok()
                .and_then(|text| RunRecord::parse(&text).ok())
                .map_or(1.0, |record| record.calibration.gain)
        })
        .collect();
    for _ in 0..warmup {
        for (k, &look) in Look::ALL.iter().enumerate() {
            step_timing(&renderer, look, &upload, &targets, gains[k])?;
        }
    }
    let mut lines = String::new();
    for i in 0..Look::ALL.len() {
        let k = (i + round) % Look::ALL.len();
        let timing = step_timing(&renderer, Look::ALL[k], &upload, &targets, gains[k])?;
        lines.push_str(&metrics::timing_line(round, Look::ALL[k], &timing));
    }
    let path = data.join(format!("timing_{}.txt", variant.key()));
    let mut file = io(
        &path,
        std::fs::OpenOptions::new().create(true).append(true).open(&path),
    )?;
    io(&path, file.write_all(lines.as_bytes()))?;
    eprintln!("timing round {round} ({}) recorded", variant.key());
    Ok(())
}

/// Builds composites and metrics from the files of earlier render and timing runs. No GPU.
fn compose(dir: &Path) -> Result<(), String> {
    let data = dir.join(DATA_DIR);
    let camera = Camera::new(aspect(), 0.0);
    let calm = Scene::build(Variant::Calm);
    let busy = Scene::build(Variant::Busy);
    let scene_of = |variant: Variant| match variant {
        Variant::Calm => &calm,
        Variant::Busy => &busy,
    };
    let mut notes = Vec::new();
    // Free-text notes about this run, one per line (for example conditions during the timing rounds).
    if let Ok(text) = std::fs::read_to_string(data.join("notes.txt")) {
        notes.extend(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from));
    }
    let mut adapter = String::new();
    let mut floor: Option<(FloorSpace, f32, u32)> = None;
    let mut calibrations: Vec<(Look, Calibration)> = Vec::new();
    let mut results = Vec::new();
    let mut finals = BTreeMap::new();
    let mut bullet_hashes = Vec::new();
    let mut crop_origins = Vec::new();
    for look in Look::ALL {
        for variant in Variant::ALL {
            let stem = format!("{}_{}", look.key(), variant.key());
            let path = record_path(&data, look, variant);
            let Ok(text) = std::fs::read_to_string(&path) else {
                notes.push(format!("{stem}: nicht gerendert (kein Laufprotokoll)"));
                continue;
            };
            let record = RunRecord::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let target = (record.floor_space, record.floor_target, record.light_factor.to_bits());
            match floor {
                None => floor = Some(target),
                Some(first) if first != target => {
                    return Err(format!("{stem}: rendered with a different exposure target or light factor than the other runs"));
                }
                Some(_) => {}
            }
            adapter.clone_from(&record.adapter);
            match calibrations.iter().find(|(l, _)| *l == look) {
                None => calibrations.push((look, record.calibration)),
                Some((_, first)) if first.gain.to_bits() != record.calibration.gain.to_bits() => notes.push(format!(
                    "{stem}: Lichtverstärkung {:.6} weicht vom ersten Lauf des Looks ab ({:.6})",
                    record.calibration.gain, first.gain
                )),
                Some(_) => {}
            }
            let world = Image::read_png(&dir.join(format!("{stem}_world.png")))?;
            let mask = ClassMask::from_image(&Image::read_png(&data.join(format!("{stem}_mask.png")))?);
            let final_image = Image::read_png(&dir.join(format!("{stem}.png")))?;
            let scene = scene_of(variant);
            results.push(VariantResult {
                look,
                variant,
                bullets: metrics::bullet_contrast(&world, &scene.bullets, &camera),
                floor: metrics::floor_stats(&world, &mask),
                figures: metrics::figure_contrast(&world, &mask, &scene.figures),
                stats: bullet_pass::accept(&scene.bullets).1,
                lights: scene.lights.len(),
                light_breakdown: scene.light_breakdown(),
                bullet_count: scene.bullets.len(),
                friendly_bolts: scene.friendly_bolt_count,
                stress_counts: scene.stress_counts,
            });
            if variant == Variant::Busy {
                if let Some(origin) = record.crop_origin {
                    crop_origins.push((look, origin));
                }
                let only = Image::read_png(&data.join(format!("bullets_only_busy_{}.png", look.key())))?;
                if bullet_hashes.is_empty() {
                    save(&only, dir, "bullets_only_busy.png")?;
                }
                bullet_hashes.push((look, image_io::fnv1a64(&only.rgba)));
            }
            finals.insert((look, variant), final_image);
        }
    }

    if finals.len() == Look::ALL.len() * Variant::ALL.len() {
        let get = |look: Look, variant: Variant| &finals[&(look, variant)];
        let rows = Variant::ALL.map(|variant| Look::ALL.map(|look| get(look, variant)));
        save(&composite::side_by_side(&rows), dir, "composite_side_by_side.png")?;
        save(&composite::crops(rows[1]), dir, "composite_crops.png")?;
    } else {
        notes.push("Composites fehlen: nicht alle sechs Frames vorhanden".into());
    }

    let mut timing = BTreeMap::new();
    for variant in Variant::ALL {
        let path = data.join(format!("timing_{}.txt", variant.key()));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut samples: BTreeMap<Look, Vec<FrameTiming>> = BTreeMap::new();
        for (round, look, sample) in metrics::parse_timing(&text).map_err(|e| format!("{}: {e}", path.display()))? {
            if round >= TIMING_WARMUP_ROUNDS {
                samples.entry(look).or_default().push(sample);
            }
        }
        for (look, list) in samples {
            timing.insert((variant, look), metrics::summarize(&list));
        }
    }

    // Contract self-test: one injected FRIENDLY instance must be rejected by the pass.
    let mut injected = calm.bullets.clone();
    let mut friendly = injected[0];
    friendly.palette_space = scene::FRIENDLY;
    injected.push(friendly);

    let (floor_space, floor_target, light_factor_bits) =
        floor.unwrap_or((FloorSpace::Display, FLOOR_TARGET, m::LIGHT_INTENSITY_FACTOR.to_bits()));
    let light_factor = f32::from_bits(light_factor_bits);
    if light_factor.to_bits() != m::LIGHT_INTENSITY_FACTOR.to_bits() {
        notes.push(format!(
            "Die Läufe nutzen den Lichtfaktor {light_factor}, der Code steht auf {}: neu rendern.",
            m::LIGHT_INTENSITY_FACTOR
        ));
    }
    let report = Report {
        adapter,
        light_factor,
        floor_space,
        floor_target,
        calibrations,
        results,
        bullet_hashes,
        palette_selftest: selftest_rejection(&injected),
        timing,
        crop_origins,
        floor_triangles: calm.floor_triangles,
        notes,
    };
    metrics::write_json(&dir.join("metrics.json"), &report)?;
    metrics::write_markdown(&dir.join("metrics.md"), &report)?;
    eprintln!("  wrote metrics.md, metrics.json");
    Ok(())
}

/// Runs the pass filter on `bullets` without the debug abort, so the release binary can report it.
fn selftest_rejection(bullets: &[gpu_types::BulletInstance]) -> bullet_pass::BulletStats {
    if cfg!(debug_assertions) {
        // Debug builds abort on a foreign palette space by contract; count without the filter.
        let rejected = bullets
            .iter()
            .filter(|b| b.palette_space != bullet_pass::BULLET_PASS_PALETTE_SPACE)
            .count();
        return bullet_pass::BulletStats {
            rejected_palette_space: rejected as u32,
            ..Default::default()
        };
    }
    bullet_pass::accept(bullets).1
}
