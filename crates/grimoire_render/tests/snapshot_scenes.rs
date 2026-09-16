//! Plan 0002 WP2.8: deterministic render test scenes, rendered offscreen on the software adapter
//! and compared against a checked-in reference image per platform.
//!
//! # The three scenes
//!
//! - `pbr_materials`: a row of spheres sweeping roughness (columns) and metalness (rows) under the
//!   key light plus one point light — proves the GGX/Metallic-Roughness pipeline (WP2.5) actually
//!   varies with both material parameters and reacts to both light types.
//! - `shadows`: a figure-like mesh and a pillar standing on a floor, lit by an angled key light;
//!   rendered twice, once with the key-light shadow map on ([`grimoire_render::ShadowMode::KeyLight`])
//!   and once with the cheap blob-shadow variant ([`grimoire_render::ShadowMode::Blob`]) — proves
//!   both WP2.6 shadow techniques actually darken the floor, not just that they run without error.
//! - `camera_tilt`: the same simple scene (a floor and one box) rendered at several
//!   [`grimoire_render::Camera25D::tilt_degrees`] values — proves the perspective projection: a
//!   shallower tilt reveals more of the box's side faces and pushes the horizon down the frame.
//!
//! # Tolerance metric
//!
//! [`support::compare`] reports two numbers, [`support::MEAN_ABS_DIFF_TOLERANCE`] and
//! [`support::MAX_ABS_DIFF_TOLERANCE`] (see their doc comments for the rationale): a mean absolute
//! per-channel difference across the whole image, and the single largest per-channel difference
//! anywhere in it. Both are plain, auditable numbers rather than a perceptual similarity score,
//! because this harness's job is narrow: catch a real rendering regression against a
//! previously-reviewed reference, not approximate human vision.
//!
//! # Warning mode
//!
//! [`FAIL_ON_MISMATCH`] is `false`: a mismatch against the reference is printed clearly (a
//! greppable `grimoire-snapshot-diff: ...` line, bypassing libtest's capture of passing-test output
//! like the WP2.1 adapter report in `tests/offscreen.rs`, plus a human-readable `::warning::`
//! annotation) but the test still **passes**. `.github/scripts/report-snapshot-diff.sh` folds every
//! such line from the CI log into that job's summary, so a mismatch is visible at a glance without
//! turning the run red.
//!
//! **How this becomes failing:** flip [`FAIL_ON_MISMATCH`] to `true` once WP3.6 (plan 0002)
//! empirically establishes, per platform, that its adapter's frame-to-frame variance sits reliably
//! under [`support::MEAN_ABS_DIFF_TOLERANCE`]/[`support::MAX_ABS_DIFF_TOLERANCE`] (PRD-0018
//! OF-18.2) — the plan's own risk register (R6) calls for exactly that staged rollout: warning mode
//! until M2, blocking only on adapters proven stable. Until then a flaky driver would otherwise
//! turn an unrelated PR red.
//!
//! # References per platform
//!
//! One reference PNG per scene/variant lives under `tests/snapshots/<platform>/`, `<platform>`
//! being [`support::platform_dir`]'s `"windows"`, `"linux"` or `"macos"` (matching the CI matrix's
//! three driver families — WARP, lavapipe, the paravirtual Metal device — per PRD-0018's adapter
//! table, OF-18.2). Only `tests/snapshots/windows/*.png` is checked in by this change: it is the
//! only platform this harness could actually run the software adapter on to produce one (no
//! hand-tuned pixels — each file is exactly what `WgpuRenderer` produced here). Linux and macOS
//! references are left for a follow-up once CI has produced a candidate to review (see the "no
//! reference yet" branch below and `.github/scripts/report-snapshot-diff.sh`).
//!
//! **macOS is a documented special case, not an oversight:** PRD-0018's adapter table shows
//! `macos-latest` handing out "Apple Paravirtual device" as a Metal adapter of device type
//! `IntegratedGpu` — not `Cpu`. `GRIMOIRE_GPU_ADAPTER=software` (`grimoire_gpu::AdapterOverride`)
//! forces `force_fallback_adapter` and then rejects whatever comes back unless its device type is
//! exactly `Cpu`, so on a macOS runner it currently resolves to [`RenderError::NoAdapter`] — the
//! same "no adapter" path a machine with no GPU at all takes, handled below by
//! [`try_offscreen_renderer`] the same way `tests/offscreen.rs`'s `skip_without_adapter` handles a
//! missing adapter — except this harness *always* treats it as a skip, never as a failure: unlike
//! `tests/offscreen.rs`'s correctness tests, `GRIMOIRE_REQUIRE_GPU_ADAPTER` would be the wrong tool
//! here (this harness's whole premise, "warning mode", is that a platform without a stable
//! reference should be visibly informational, never red). That is why macOS gets no snapshot
//! reference from this harness for now: there is currently no way to
//! obtain a literal CPU/software adapter there at all, so "what the software adapter produces" is
//! not yet a thing this platform has. Any future change here belongs with the adapter-table
//! decision the plan already reserves for WP3.6, not with this harness.
//!
//! # Image size
//!
//! 160x90 (16:9): [`support`]'s self-contained PNG writer deliberately does not compress (see its
//! module doc comment — no PNG dependency exists anywhere in this workspace, on purpose), so file
//! size scales directly with pixel count; at the plan's own example size, 320x180, every reference
//! would be roughly 225 KB (320 * 180 * 4 raw RGBA bytes plus one filter byte per row, barely
//! shrunk by the "stored" `DEFLATE` framing), which is a real cost once multiplied by seven
//! scene/variant images and, eventually, three platforms. Quartering the pixel count to 160x90
//! keeps every committed reference under 60 KB while every scene's distinguishing feature — the
//! roughness/metalness sweep's columns, the shadow's penumbra, the horizon line's height — still
//! spans several, sometimes several dozen, pixels: comfortably above both tolerance thresholds'
//! noise floor, which operates per pixel/channel regardless of image size.

use std::path::{Path, PathBuf};

use grimoire_render::procedural::{altar_block, capsule_actor, floor_tile_grid, icosphere};
use grimoire_render::{
    AmbientLight, BlobShadowInstance, Camera25D, DirectionalLight, MaterialHandle, MeshHandle,
    MeshInstance, PbrMaterial, PointLight, RenderError, Renderer, RendererConfig, ShadowConfig,
    ShadowMode, StageFrame, WgpuRenderer,
};

#[path = "support/mod.rs"]
mod support;
use support::Image;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;

/// See this module's doc comment ("Warning mode"): flip to `true` only once WP3.6 has established
/// that the current platform's adapter is stable enough (PRD-0018 OF-18.2).
const FAIL_ON_MISMATCH: bool = false;

/// Environment variable that, when set to `1`/`true`, writes the freshly rendered image as this
/// platform's new reference instead of comparing against the checked-in one. Used to seed or
/// deliberately update a reference after a reviewed rendering change; never set in CI.
const ENV_UPDATE_REFERENCES: &str = "GRIMOIRE_SNAPSHOT_UPDATE";

fn update_references_requested() -> bool {
    matches!(
        std::env::var(ENV_UPDATE_REFERENCES)
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true")
    )
}

/// Directory candidate images are written to when there is no reference to compare against yet, or
/// when a comparison mismatches — never committed (see the workspace root `.gitignore`'s blanket
/// `target/` rule), just a place a human or a follow-up CI step can pick the PNG up from.
fn candidate_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_SNAPSHOT_CANDIDATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("target").join("wp28-snapshots"))
}

fn reference_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(support::platform_dir())
        .join(format!("{name}.png"))
}

/// Renders `frame` on the software adapter and reports the outcome against `name`'s reference
/// image: a match, a mismatch (reported, not failed, unless [`FAIL_ON_MISMATCH`]),
/// or "no reference yet" (the candidate is written out and reported, not failed either — see this
/// module's doc comment on why macOS and Linux currently take this path). Returns normally in every
/// case; the caller does not need to branch on the outcome, only decide whether to keep asserting
/// (kept possible for [`FAIL_ON_MISMATCH`] callers) after this returns.
fn check_scene(name: &str, image: &Image) {
    let path = reference_path(name);
    if update_references_requested() {
        image
            .write_png(&path)
            .unwrap_or_else(|error| panic!("writing reference {}: {error}", path.display()));
        println!(
            "grimoire-snapshot-updated: name={name} path={}",
            path.display()
        );
        return;
    }

    if !path.exists() {
        let candidate = candidate_dir()
            .join(support::platform_dir())
            .join(format!("{name}.png"));
        image
            .write_png(&candidate)
            .unwrap_or_else(|error| panic!("writing candidate {}: {error}", candidate.display()));
        println!(
            "grimoire-snapshot-no-reference: name={name} platform={} candidate={}",
            support::platform_dir(),
            candidate.display()
        );
        let _ = std::io::Write::write_all(
            &mut std::io::stdout(),
            format!(
                "\n::warning title=WP2.8 snapshot has no reference yet::scene \"{name}\" on {} has \
                 no committed reference image; a candidate was written to {} for review (see \
                 tests/snapshot_scenes.rs's module doc comment)\n",
                support::platform_dir(),
                candidate.display()
            )
            .as_bytes(),
        );
        return;
    }

    let reference = Image::read_png(&path)
        .unwrap_or_else(|error| panic!("reading reference {}: {error}", path.display()));
    let Some(metric) = support::compare(&reference, image) else {
        panic!(
            "reference {} is {}x{}, rendered image is {}x{}: scene resolution changed, update the \
             reference deliberately ({ENV_UPDATE_REFERENCES}=1)",
            path.display(),
            reference.width,
            reference.height,
            image.width,
            image.height
        );
    };
    println!(
        "grimoire-snapshot-diff: name={name} platform={} mean_abs_diff={:.3} max_abs_diff={} \
         mean_tolerance={:.3} max_tolerance={} within_tolerance={}",
        support::platform_dir(),
        metric.mean_abs_diff,
        metric.max_abs_diff,
        support::MEAN_ABS_DIFF_TOLERANCE,
        support::MAX_ABS_DIFF_TOLERANCE,
        metric.within_tolerance()
    );
    if !metric.within_tolerance() {
        let platform_out = candidate_dir().join(support::platform_dir());
        let candidate = platform_out.join(format!("{name}.png"));
        let _ = image.write_png(&candidate);
        // Reference next to candidate, side by side: easier to spot *what* changed at a glance
        // than flipping between two separately named files.
        let comparison = platform_out.join(format!("{name}_reference_vs_candidate.png"));
        let _ = Image::beside(&[&reference, image]).write_png(&comparison);
        let _ = std::io::Write::write_all(
            &mut std::io::stdout(),
            format!(
                "\n::warning title=WP2.8 snapshot mismatch::scene \"{name}\" on {} exceeds tolerance \
                 (mean_abs_diff={:.3} > {:.3}, or max_abs_diff={} > {}); candidate written to {}, \
                 reference-vs-candidate strip at {}\n",
                support::platform_dir(),
                metric.mean_abs_diff,
                support::MEAN_ABS_DIFF_TOLERANCE,
                metric.max_abs_diff,
                support::MAX_ABS_DIFF_TOLERANCE,
                candidate.display(),
                comparison.display()
            )
            .as_bytes(),
        );
        // Not `assert!(!FAIL_ON_MISMATCH, ...)`: clippy's `assertions_on_constants` correctly
        // flags asserting a `const bool` directly, since with today's `false` it can never fire —
        // that is the point (see this module's doc comment, "Warning mode") until someone flips it.
        if FAIL_ON_MISMATCH {
            panic!(
                "scene \"{name}\" mismatches its reference beyond tolerance and FAIL_ON_MISMATCH \
                 is true: mean_abs_diff={:.3} (tolerance {:.3}), max_abs_diff={} (tolerance {})",
                metric.mean_abs_diff,
                support::MEAN_ABS_DIFF_TOLERANCE,
                metric.max_abs_diff,
                support::MAX_ABS_DIFF_TOLERANCE
            );
        }
    }
}

/// Creates an offscreen renderer on whatever adapter [`grimoire_gpu::AdapterOverride::from_env`]
/// resolves to (the caller is expected to run with `GRIMOIRE_GPU_ADAPTER=software`, per this
/// module's doc comment); `None` if none is available, exactly like every offscreen test in
/// `tests/offscreen.rs` and `tests/wp26_shadow_showcase.rs` — this is the path a macOS runner takes
/// today (see this module's doc comment).
fn try_offscreen_renderer() -> Option<WgpuRenderer> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    };
    match WgpuRenderer::new_offscreen(WIDTH, HEIGHT, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => {
            eprintln!(
                "no GPU adapter available under the current GRIMOIRE_GPU_ADAPTER setting; \
                 skipping this WP2.8 snapshot scene"
            );
            None
        }
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

// --- Shared scene-building helpers (mirrors `tests/wp26_shadow_showcase.rs`'s conventions) ------

fn translation(t: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

fn mesh_instance(
    mesh: MeshHandle,
    material: MaterialHandle,
    transform: [[f32; 4]; 4],
) -> MeshInstance {
    let mut instance = MeshInstance::default();
    instance.mesh = mesh;
    instance.material = material;
    instance.transform = transform;
    instance
}

fn material(base_color: [f32; 4], metallic: f32, roughness: f32) -> PbrMaterial {
    let mut material = PbrMaterial::default();
    material.base_color_factor = base_color;
    material.metallic_factor = metallic;
    material.roughness_factor = roughness;
    material
}

fn key_light(direction: [f32; 3], color: [f32; 3], intensity: f32) -> DirectionalLight {
    let mut light = DirectionalLight::default();
    light.direction = direction;
    light.color = color;
    light.intensity = intensity;
    light
}

fn point_light(position: [f32; 3], color: [f32; 3], range: f32, intensity: f32) -> PointLight {
    let mut light = PointLight::default();
    light.position = position;
    light.color = color;
    light.range = range;
    light.intensity = intensity;
    light
}

fn render_frame(renderer: &mut WgpuRenderer, frame: &StageFrame) -> Image {
    renderer.render_stage(frame).expect("render_stage");
    let rgba = renderer.read_offscreen_rgba().expect("read-back");
    Image::from_offscreen(WIDTH, HEIGHT, rgba)
}

// --- Scene 1: pbr_materials (WP2.5) --------------------------------------------------------------

const PBR_ROUGHNESS_STEPS: [f32; 5] = [0.05, 0.28, 0.5, 0.72, 0.95];
const PBR_METALLIC_ROWS: [f32; 2] = [0.0, 1.0];

/// A row of spheres sweeping roughness (columns) and metalness (rows), the same grey base colour
/// throughout so only the two PBR parameters (and the light types) drive any pixel difference.
fn pbr_materials_frame(renderer: &mut WgpuRenderer) -> StageFrame {
    let mesh = renderer
        .register_mesh(icosphere(2, 1.6))
        .expect("valid mesh");
    let floor = renderer
        .register_mesh(floor_tile_grid(6, 4.0))
        .expect("valid mesh");

    let mut camera = Camera25D::default();
    camera.target = [0.0, 2.0];
    camera.tilt_degrees = 55.0;
    camera.fov_y_degrees = 45.0;
    camera.distance = 16.0;

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.03, 0.03, 0.045, 1.0];
    frame.camera_25d = Some(camera);
    frame.key_light = Some(key_light([0.3, 0.5, -0.8], [1.0, 0.97, 0.9], 3.2));
    frame.ambient = AmbientLight::Hemisphere {
        sky_color: [0.12, 0.13, 0.18],
        ground_color: [0.05, 0.045, 0.05],
        intensity: 1.0,
    };
    frame
        .point_lights
        .push(point_light([-3.0, -2.0, 6.0], [1.0, 0.85, 0.65], 30.0, 6.0));

    frame
        .materials
        .push(material([0.18, 0.19, 0.22, 1.0], 0.0, 0.9)); // 0: floor
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 2.0, 0.0]),
    ));

    let columns = PBR_ROUGHNESS_STEPS.len() as f32;
    for (row_index, &metallic) in PBR_METALLIC_ROWS.iter().enumerate() {
        for (col_index, &roughness) in PBR_ROUGHNESS_STEPS.iter().enumerate() {
            let material_index = u32::try_from(frame.materials.len()).expect("far below u32::MAX");
            frame
                .materials
                .push(material([0.75, 0.75, 0.75, 1.0], metallic, roughness));
            let x = (col_index as f32 - (columns - 1.0) * 0.5) * 3.4;
            let y = 2.0 + row_index as f32 * 3.4;
            frame.meshes.push(mesh_instance(
                mesh,
                MaterialHandle(material_index),
                translation([x, y, 1.0]),
            ));
        }
    }

    frame
}

#[test]
#[ignore = "WP2.8 snapshot scene: run explicitly with GRIMOIRE_GPU_ADAPTER=software (see this file's module doc comment)"]
fn pbr_materials_scene() {
    let Some(mut renderer) = try_offscreen_renderer() else {
        return;
    };
    let frame = pbr_materials_frame(&mut renderer);
    let image = render_frame(&mut renderer, &frame);
    check_scene("pbr_materials", &image);
}

// --- Scene 2: shadows (WP2.6) ---------------------------------------------------------------------

/// A figure-like mesh and a pillar standing on a floor, lit by an angled key light. Shared by both
/// the key-light and blob variants below; only [`StageFrame::shadow_config`] and
/// [`StageFrame::blob_shadows`] differ per variant.
fn shadows_frame(renderer: &mut WgpuRenderer) -> StageFrame {
    let floor = renderer
        .register_mesh(floor_tile_grid(10, 2.0))
        .expect("valid mesh");
    let pillar = renderer
        .register_mesh(altar_block(1.2, 1.2, 5.0))
        .expect("valid mesh");
    let figure = renderer
        .register_mesh(capsule_actor(0.55, 1.6, 10, 2))
        .expect("valid mesh");

    let mut camera = Camera25D::default();
    camera.target = [0.0, 1.0];
    camera.tilt_degrees = 62.0;
    camera.fov_y_degrees = 48.0;
    camera.distance = 14.0;

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.02, 0.024, 0.036, 1.0];
    frame.camera_25d = Some(camera);
    // Shallow angle from the +X side: casts a long, clearly visible shadow across the floor,
    // the same reasoning `tests/wp26_shadow_showcase.rs`'s `build_arena_frame` documents.
    frame.key_light = Some(key_light([1.0, 0.2, -0.7], [1.0, 0.95, 0.85], 4.5));
    frame.ambient = AmbientLight::Flat {
        color: [0.10, 0.10, 0.13],
        intensity: 1.0,
    };
    frame
        .materials
        .push(material([0.55, 0.55, 0.58, 1.0], 0.0, 0.85)); // 0: floor and pillar
    frame
        .materials
        .push(material([0.65, 0.45, 0.35, 1.0], 0.0, 0.7)); // 1: figure

    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 1.0, 0.0]),
    ));
    // `altar_block(1.2, 1.2, 5.0)` is centred on the origin with half-height 2.5, so a Z
    // translation of 2.5 sits it on the floor (`Z = 0`), like `capsule_actor` below.
    frame.meshes.push(mesh_instance(
        pillar,
        MaterialHandle(0),
        translation([-2.5, 3.0, 2.5]),
    ));
    // `capsule_actor(0.55, 1.6, ..)`'s half-height along Z is `1.6 / 2 + 0.55 = 1.35`.
    frame.meshes.push(mesh_instance(
        figure,
        MaterialHandle(1),
        translation([1.5, 0.5, 1.35]),
    ));

    frame
}

#[test]
#[ignore = "WP2.8 snapshot scene: run explicitly with GRIMOIRE_GPU_ADAPTER=software (see this file's module doc comment)"]
fn shadows_scene() {
    let Some(mut renderer) = try_offscreen_renderer() else {
        return;
    };
    let mut frame = shadows_frame(&mut renderer);

    let mut key_light_config = ShadowConfig::default();
    key_light_config.mode = ShadowMode::KeyLight;
    frame.shadow_config = key_light_config;
    frame.blob_shadows.clear();
    let key_light_image = render_frame(&mut renderer, &frame);
    check_scene("shadows_keylight", &key_light_image);

    let mut blob_config = ShadowConfig::default();
    blob_config.mode = ShadowMode::Blob;
    frame.shadow_config = blob_config;
    frame.blob_shadows.push(BlobShadowInstance {
        position: [1.5, 0.5],
        radius: 1.0,
        softness: 0.45,
        strength: 0.85,
    });
    let blob_image = render_frame(&mut renderer, &frame);
    check_scene("shadows_blob", &blob_image);
}

// --- Scene 3: camera_tilt (WP2.2/WP2.4 projection) -----------------------------------------------

/// Tilt angles rendered for the `camera_tilt` scene: the shipped look's range boundaries
/// (`ADR-0014`, PRD-0003 FR-03) plus a shallower and a near-top-down extreme, so the reference set
/// actually spans "reveals the box's side faces" through "looks almost straight down".
const CAMERA_TILT_ANGLES_DEGREES: [f32; 4] = [35.0, 60.0, 75.0, 90.0];

fn camera_tilt_frame(renderer: &mut WgpuRenderer, tilt_degrees: f32) -> StageFrame {
    let floor = renderer
        .register_mesh(floor_tile_grid(10, 2.0))
        .expect("valid mesh");
    let box_mesh = renderer
        .register_mesh(altar_block(2.5, 2.5, 2.5))
        .expect("valid mesh");

    let mut camera = Camera25D::default();
    camera.target = [0.0, 0.0];
    camera.tilt_degrees = tilt_degrees;
    camera.fov_y_degrees = 50.0;
    camera.distance = 12.0;

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.05, 0.08, 0.14, 1.0]; // A distinct "sky" colour above the horizon.
    frame.camera_25d = Some(camera);
    frame.key_light = Some(key_light([0.25, 0.4, -0.85], [1.0, 1.0, 1.0], 3.0));
    frame.ambient = AmbientLight::Flat {
        color: [1.0, 1.0, 1.0],
        intensity: 0.35,
    };
    frame
        .materials
        .push(material([0.3, 0.5, 0.3, 1.0], 0.0, 0.9)); // 0: floor
    frame
        .materials
        .push(material([0.75, 0.25, 0.2, 1.0], 0.0, 0.6)); // 1: box
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    frame.meshes.push(mesh_instance(
        box_mesh,
        MaterialHandle(1),
        translation([0.0, 0.0, 1.25]),
    ));
    frame
}

#[test]
#[ignore = "WP2.8 snapshot scene: run explicitly with GRIMOIRE_GPU_ADAPTER=software (see this file's module doc comment)"]
fn camera_tilt_scene() {
    let Some(mut renderer) = try_offscreen_renderer() else {
        return;
    };
    for &tilt_degrees in &CAMERA_TILT_ANGLES_DEGREES {
        let frame = camera_tilt_frame(&mut renderer, tilt_degrees);
        let image = render_frame(&mut renderer, &frame);
        check_scene(&format!("camera_tilt_{}", tilt_degrees as i32), &image);
    }
}

// --- Edge-adjacency check (texture-quality package, strand B1) ------------------------------------
//
// B1 adds multisampling (`crate::Msaa::X4`, default) to the mesh pass, which smooths geometry
// silhouettes and is expected to move every one of this file's seven scene/variant references off
// their previous bit-exact match (see `mesh_pass.rs`'s module doc comment, "Mipmaps and
// multisampling"). The Festlegung this package follows requires proving *where* the resulting
// mismatch lands before deliberately regenerating any reference: at a colour edge in the old
// (non-multisampled) reference image, never in the middle of a flat region — the latter would be a
// bug, not anti-aliasing. This test renders every scene above without touching any reference file,
// and for every pixel whose largest per-channel difference against the still-checked-in reference
// reaches `HIGH_DIFF_THRESHOLD`, checks whether any of its 8 neighbours in the *reference* image
// differs from it in luminance by at least `EDGE_THRESHOLD` — a plain, auditable proxy for "this
// pixel sits on a colour or silhouette boundary" that needs no depth read-back (none of this
// crate's public API exposes one).

/// A pixel counts as "changed" once any RGBA channel differs by at least this much between the old
/// reference and a freshly rendered (MSAA-on) candidate — well below
/// [`support::MAX_ABS_DIFF_TOLERANCE`] (60), so this also catches partially-covered edge pixels
/// whose difference alone would not have failed the tolerance check.
const HIGH_DIFF_THRESHOLD: u8 = 20;

/// A neighbouring pixel counts as forming an edge with the centre pixel once their luminance
/// (ITU-R BT.601 luma weights, `0..=255`) differs by at least this much in the *reference* image.
const EDGE_THRESHOLD: i32 = 12;

fn luminance(image: &Image, x: u32, y: u32) -> i32 {
    let index = (y as usize * image.width as usize + x as usize) * 4;
    let r = i32::from(image.rgba[index]);
    let g = i32::from(image.rgba[index + 1]);
    let b = i32::from(image.rgba[index + 2]);
    (r * 299 + g * 587 + b * 114) / 1000
}

/// Whether `(x, y)` has at least one of its up-to-8 neighbours in `image` differing in luminance by
/// [`EDGE_THRESHOLD`] or more — see this section's header comment.
fn is_near_an_edge(image: &Image, x: u32, y: u32) -> bool {
    let center = luminance(image, x, y);
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (i64::from(x) + dx, i64::from(y) + dy);
            if nx < 0 || ny < 0 || nx >= i64::from(image.width) || ny >= i64::from(image.height) {
                continue;
            }
            let neighbor = luminance(image, nx as u32, ny as u32);
            if (center - neighbor).abs() >= EDGE_THRESHOLD {
                return true;
            }
        }
    }
    false
}

/// For every scene this file has a reference for, renders it fresh (MSAA on, per this package's
/// `StageRendererConfig::msaa` default) and reports how many pixels changed by
/// [`HIGH_DIFF_THRESHOLD`] or more against the still-checked-in reference, and what fraction of
/// those sit next to a colour edge in that reference ([`is_near_an_edge`]). Never asserts anything
/// itself — the Festlegung asks for this to be *measured and reported* before a human decides to
/// regenerate references, not gated automatically (the same "warn, do not fail" posture
/// `check_scene` already applies to the tolerance check above). Run explicitly:
/// `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test snapshot_scenes -- --ignored
/// --nocapture edge_adjacency`.
#[test]
#[ignore = "texture-quality B1 measurement, run explicitly (see this test's doc comment)"]
fn edge_adjacency_of_snapshot_mismatches() {
    let Some(mut renderer) = try_offscreen_renderer() else {
        return;
    };

    let mut scenes: Vec<(String, Image)> = Vec::new();

    let pbr_frame = pbr_materials_frame(&mut renderer);
    scenes.push((
        "pbr_materials".to_string(),
        render_frame(&mut renderer, &pbr_frame),
    ));

    let mut shadow_frame = shadows_frame(&mut renderer);
    let mut key_light_config = ShadowConfig::default();
    key_light_config.mode = ShadowMode::KeyLight;
    shadow_frame.shadow_config = key_light_config;
    shadow_frame.blob_shadows.clear();
    scenes.push((
        "shadows_keylight".to_string(),
        render_frame(&mut renderer, &shadow_frame),
    ));
    let mut blob_config = ShadowConfig::default();
    blob_config.mode = ShadowMode::Blob;
    shadow_frame.shadow_config = blob_config;
    shadow_frame.blob_shadows.push(BlobShadowInstance {
        position: [1.5, 0.5],
        radius: 1.0,
        softness: 0.45,
        strength: 0.85,
    });
    scenes.push((
        "shadows_blob".to_string(),
        render_frame(&mut renderer, &shadow_frame),
    ));

    for &tilt_degrees in &CAMERA_TILT_ANGLES_DEGREES {
        let frame = camera_tilt_frame(&mut renderer, tilt_degrees);
        let image = render_frame(&mut renderer, &frame);
        scenes.push((format!("camera_tilt_{}", tilt_degrees as i32), image));
    }

    for (name, candidate) in &scenes {
        let path = reference_path(name);
        if !path.exists() {
            println!("grimoire-edge-adjacency: name={name} skipped (no reference yet)");
            continue;
        }
        let reference = Image::read_png(&path)
            .unwrap_or_else(|error| panic!("reading reference {}: {error}", path.display()));
        if reference.width != candidate.width || reference.height != candidate.height {
            println!("grimoire-edge-adjacency: name={name} skipped (resolution changed)");
            continue;
        }
        let mut high_diff_total = 0u32;
        let mut high_diff_near_edge = 0u32;
        for y in 0..reference.height {
            for x in 0..reference.width {
                let index = (y as usize * reference.width as usize + x as usize) * 4;
                let max_channel_diff = (0..4)
                    .map(|c| reference.rgba[index + c].abs_diff(candidate.rgba[index + c]))
                    .max()
                    .unwrap_or(0);
                if max_channel_diff < HIGH_DIFF_THRESHOLD {
                    continue;
                }
                high_diff_total += 1;
                if is_near_an_edge(&reference, x, y) {
                    high_diff_near_edge += 1;
                }
            }
        }
        let percent_near_edge = if high_diff_total > 0 {
            100.0 * f64::from(high_diff_near_edge) / f64::from(high_diff_total)
        } else {
            100.0
        };
        println!(
            "grimoire-edge-adjacency: name={name} high_diff_pixels={high_diff_total} near_edge={high_diff_near_edge} percent_near_edge={percent_near_edge:.1}"
        );
    }
}
