//! Plan 0002 WP2.8: deterministic render test scenes, rendered offscreen on the software adapter
//! and compared against a checked-in reference image per platform.
//!
//! # The scenes
//!
//! Plan 0002 WP3.6 adds `lights_256` (256 coloured point lights over a floor through clustered
//! forward+ at the High budget; asserts that no light is dropped and that the lit patches show
//! distinct hues) and `bullets_on_top` (hostile bullets with glow and the player marker right on a
//! floor spot a point light burns white; asserts that body colour and rim stay readable there,
//! PRD-0003 rule 1). Both assert their structural property before the reference comparison, so
//! they fail on a real regression even where the reference comparison only warns. [`scene_variance`] measures the run-to-run
//! and cross-adapter variance of every scene for OF-18.2.
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
//! # Blocking per platform
//!
//! Every comparison prints a greppable `grimoire-snapshot-diff: ...` line (bypassing libtest's
//! capture of passing-test output like the WP2.1 adapter report in `tests/offscreen.rs`), and
//! `.github/scripts/report-snapshot-diff.sh` folds those lines from the CI log into the job
//! summary. What a mismatch beyond tolerance, or a scene without a reference, does depends on the
//! platform ([`support::BLOCKING_PLATFORMS`]):
//!
//! - **Windows (WARP) and Linux (lavapipe): the test fails** (plan 0002 WP3.6, from M2 on). WP3.6
//!   measured both adapters for OF-18.2: every scene reproduces bit-exactly within a run, and the
//!   distance to the references stays far inside the tolerance (see [`scene_variance`]). A missing
//!   reference fails too, so a new scene cannot slip past the gate; its candidate lands in the CI
//!   artifact `snapshot-candidates-<runner>` for review and adoption.
//! - **Everywhere else, today macOS: warning mode until the P3 gate.** A mismatch is reported with a
//!   `::warning::` annotation and the test passes. The macOS runner has no software adapter, so the
//!   scenes are skipped there anyway (see "References per platform").
//!
//! [`blocking_comparison_fails_on_a_real_regression`] proves the rule without a GPU in every
//! `cargo test`: two references that differ like a real regression (blob instead of key-light
//! shadows, a camera tilted 15 degrees off) trip the tolerance and fail on both blocking platforms,
//! while the WARP and lavapipe references of the same scene stay within it.
//!
//! # References per platform
//!
//! One reference PNG per scene/variant lives under `tests/snapshots/<platform>/`, `<platform>`
//! being [`support::platform_dir`]'s `"windows"`, `"linux"` or `"macos"` (matching the CI matrix's
//! three driver families — WARP, lavapipe, the paravirtual Metal device — per PRD-0018's adapter
//! table, OF-18.2). No reference is hand-tuned: each file is exactly what `WgpuRenderer` produced
//! on that platform's software adapter. The Windows references were rendered on WARP directly; the
//! Linux references (plan 0002 WP3.6) are the lavapipe candidates of a CI run, taken unchanged from
//! its `snapshot-candidates-ubuntu-latest` artifact (see the "no reference yet" branch below and
//! `.github/scripts/report-snapshot-diff.sh`). macOS has none, see below.
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
//! here (a platform without a stable reference stays visibly informational, never red, see
//! "Blocking per platform"). That is why macOS gets no snapshot
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

use grimoire_render::procedural::{
    altar_block, capsule_actor, floor_tile_grid, icosphere, octagonal_pillar,
};
use grimoire_render::{
    AmbientLight, BULLET_PASS_PALETTE_SPACE, BlobShadowInstance, BulletInstance, Camera25D,
    DirectionalLight, LightBudget, MaterialHandle, MeshHandle, MeshInstance, PbrMaterial,
    PointLight, RenderError, Renderer, RendererConfig, ShadowConfig, ShadowMode, SpriteInstance,
    StageFrame, StageRendererConfig, WgpuRenderer, bullet_palette, bullet_silhouette, shape,
};

#[path = "support/mod.rs"]
mod support;
use support::Image;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;

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

/// Why the scene `name` fails its test on `platform`, or `None` if it passes there. `metric` is
/// the comparison against the reference, `None` if the platform has no reference for the scene.
/// A match always passes; a mismatch beyond tolerance or a missing reference fails only on a
/// blocking platform (this module's doc comment, "Blocking per platform").
fn failure(name: &str, platform: &str, metric: Option<&support::DiffMetric>) -> Option<String> {
    if !support::blocks_on_mismatch(platform) {
        return None;
    }
    match metric {
        Some(metric) if metric.within_tolerance() => None,
        Some(metric) => Some(format!(
            "scene \"{name}\" on {platform} mismatches its reference beyond tolerance: \
             mean_abs_diff={:.3} (tolerance {:.3}), max_abs_diff={} (tolerance {}); rendering on \
             {platform} blocks, see tests/snapshot_scenes.rs, \"Blocking per platform\"",
            metric.mean_abs_diff,
            support::MEAN_ABS_DIFF_TOLERANCE,
            metric.max_abs_diff,
            support::MAX_ABS_DIFF_TOLERANCE
        )),
        None => Some(format!(
            "scene \"{name}\" has no reference on {platform}, where rendering blocks: review the \
             candidate (CI artifact snapshot-candidates-<runner>) and commit it as \
             tests/snapshots/{platform}/{name}.png"
        )),
    }
}

/// The annotation level for a scene that did not match on this platform: `error` where it fails
/// the test, `warning` where it is only reported.
fn annotation_level(platform: &str) -> &'static str {
    if support::blocks_on_mismatch(platform) {
        "error"
    } else {
        "warning"
    }
}

/// Compares the rendered `image` against `name`'s reference image for this platform and reports
/// the outcome: a match, a mismatch beyond tolerance, or "no reference yet" (the candidate is
/// written out for review). The two last outcomes fail the test on a blocking platform and are
/// only reported elsewhere (this module's doc comment, "Blocking per platform").
fn check_scene(name: &str, image: &Image) {
    let platform = support::platform_dir();
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
        let candidate = candidate_dir().join(platform).join(format!("{name}.png"));
        image
            .write_png(&candidate)
            .unwrap_or_else(|error| panic!("writing candidate {}: {error}", candidate.display()));
        println!(
            "grimoire-snapshot-no-reference: name={name} platform={platform} candidate={}",
            candidate.display()
        );
        let _ = std::io::Write::write_all(
            &mut std::io::stdout(),
            format!(
                "\n::{} title=Render test scene has no reference::scene \"{name}\" on {platform} \
                 has no committed reference image; a candidate was written to {} for review (see \
                 tests/snapshot_scenes.rs's module doc comment)\n",
                annotation_level(platform),
                candidate.display()
            )
            .as_bytes(),
        );
        if let Some(message) = failure(name, platform, None) {
            panic!("{message}");
        }
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
        "grimoire-snapshot-diff: name={name} platform={platform} mean_abs_diff={:.3} max_abs_diff={} \
         mean_tolerance={:.3} max_tolerance={} within_tolerance={}",
        metric.mean_abs_diff,
        metric.max_abs_diff,
        support::MEAN_ABS_DIFF_TOLERANCE,
        support::MAX_ABS_DIFF_TOLERANCE,
        metric.within_tolerance()
    );
    if !metric.within_tolerance() {
        let platform_out = candidate_dir().join(platform);
        let candidate = platform_out.join(format!("{name}.png"));
        let _ = image.write_png(&candidate);
        // Reference next to candidate, side by side: easier to spot *what* changed at a glance
        // than flipping between two separately named files.
        let comparison = platform_out.join(format!("{name}_reference_vs_candidate.png"));
        let _ = Image::beside(&[&reference, image]).write_png(&comparison);
        let _ = std::io::Write::write_all(
            &mut std::io::stdout(),
            format!(
                "\n::{} title=Render test scene mismatch::scene \"{name}\" on {platform} exceeds \
                 tolerance (mean_abs_diff={:.3} > {:.3}, or max_abs_diff={} > {}); candidate \
                 written to {}, reference-vs-candidate strip at {}\n",
                annotation_level(platform),
                metric.mean_abs_diff,
                support::MEAN_ABS_DIFF_TOLERANCE,
                metric.max_abs_diff,
                support::MAX_ABS_DIFF_TOLERANCE,
                candidate.display(),
                comparison.display()
            )
            .as_bytes(),
        );
    }
    if let Some(message) = failure(name, platform, Some(&metric)) {
        panic!("{message}");
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

// --- Scene 4: lights_256 (WP3.6, clustered forward+ at the High budget) ---------------------------

/// Point lights of the `lights_256` scene: the High light budget exactly (PRD-0003 FR-11).
const LIGHTS_256_COUNT: u32 = 256;

/// Creates an offscreen renderer with the High light budget (256 lights), or `None` without an
/// adapter, like [`try_offscreen_renderer`].
fn try_offscreen_renderer_high_budget() -> Option<WgpuRenderer> {
    let mut config = StageRendererConfig::default();
    config.base = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    };
    config.light_budget = LightBudget::High;
    match WgpuRenderer::new_offscreen_staged(WIDTH, HEIGHT, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter available; skipping this WP3.6 snapshot scene");
            None
        }
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

/// A dark floor under a 16x16 grid of 256 small coloured point lights, one per floor patch, plus a
/// few spheres they light from all sides: every light must reach its own patch through the
/// clustered forward+ path at the High budget, none may be dropped.
fn lights_256_frame(renderer: &mut WgpuRenderer) -> StageFrame {
    let floor = renderer
        .register_mesh(floor_tile_grid(12, 2.0))
        .expect("valid mesh");
    let sphere = renderer
        .register_mesh(icosphere(2, 0.9))
        .expect("valid mesh");

    let mut camera = Camera25D::default();
    camera.target = [0.0, 1.5];
    camera.tilt_degrees = 58.0;
    camera.fov_y_degrees = 50.0;
    camera.distance = 17.0;

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.01, 0.01, 0.015, 1.0];
    frame.camera_25d = Some(camera);
    frame.ambient = AmbientLight::Flat {
        color: [1.0, 1.0, 1.0],
        intensity: 0.04,
    };
    frame
        .materials
        .push(material([0.6, 0.6, 0.6, 1.0], 0.0, 0.7)); // 0: floor
    frame
        .materials
        .push(material([0.8, 0.8, 0.8, 1.0], 0.0, 0.4)); // 1: spheres
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 1.5, 0.0]),
    ));
    for (x, y) in [
        (-5.0, 4.0),
        (0.0, 1.5),
        (5.0, 4.0),
        (-3.0, -2.0),
        (3.0, -2.0),
    ] {
        frame.meshes.push(mesh_instance(
            sphere,
            MaterialHandle(1),
            translation([x, y, 0.9]),
        ));
    }
    // Hues cycle through the grid so neighbouring patches differ visibly.
    let palette: [[f32; 3]; 6] = [
        [1.0, 0.2, 0.1],
        [1.0, 0.7, 0.1],
        [0.3, 1.0, 0.2],
        [0.1, 0.8, 1.0],
        [0.3, 0.3, 1.0],
        [1.0, 0.2, 0.9],
    ];
    for index in 0..LIGHTS_256_COUNT {
        let (column, row) = (index % 16, index / 16);
        let x = (column as f32 - 7.5) * 1.5;
        let y = (row as f32 - 7.5) * 1.5 + 1.5;
        let color = palette[((column + row) % 6) as usize];
        frame
            .point_lights
            .push(point_light([x, y, 0.8], color, 1.9, 2.5));
    }
    frame
}

#[test]
#[ignore = "WP3.6 snapshot scene: run explicitly with GRIMOIRE_GPU_ADAPTER=software (see this file's module doc comment)"]
fn lights_256_scene() {
    let Some(mut renderer) = try_offscreen_renderer_high_budget() else {
        return;
    };
    let frame = lights_256_frame(&mut renderer);
    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.point_lights_drawn, LIGHTS_256_COUNT);
    assert_eq!(
        stats.point_lights_over_budget, 0,
        "the High budget holds all 256 lights"
    );
    let image = Image::from_offscreen(
        WIDTH,
        HEIGHT,
        renderer.read_offscreen_rgba().expect("read-back"),
    );
    // Structural check, independent of any reference: the lights produce distinctly coloured lit
    // patches, not one flat tone.
    let lit_hues = distinct_lit_hues(&image);
    assert!(
        lit_hues >= 5,
        "only {lit_hues} of the six light hues show up on the floor"
    );
    check_scene("lights_256", &image);
}

/// Number of the six hue sextants in which at least 20 pixels are clearly lit and saturated.
fn distinct_lit_hues(image: &Image) -> usize {
    let mut counts = [0u32; 6];
    for pixel in image.rgba.as_chunks::<4>().0 {
        let [r, g, b, _] = pixel.map(i32::from);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max < 90 || max - min < 40 {
            continue;
        }
        let hue = if max == r {
            (60 * (g - b) / (max - min)).rem_euclid(360)
        } else if max == g {
            60 * (b - r) / (max - min) + 120
        } else {
            60 * (r - g) / (max - min) + 240
        };
        counts[(hue.rem_euclid(360) / 60) as usize] += 1;
    }
    counts.iter().filter(|&&count| count >= 20).count()
}

// --- Scene 5: bullets_on_top (WP3.6, PRD-0003 rule 1) ---------------------------------------------

/// Ground position of the centre bullet, right on the brightest spot of the floor.
const BULLETS_ON_TOP_CENTRE: [f32; 2] = [0.0, 0.0];
/// Radius of the scene's bullets in world units.
const BULLETS_ON_TOP_RADIUS: f32 = 1.0;

fn bullets_on_top_camera() -> Camera25D {
    let mut camera = Camera25D::default();
    camera.target = [0.0, 0.5];
    camera.tilt_degrees = 62.0;
    camera.fov_y_degrees = 48.0;
    camera.distance = 12.0;
    camera
}

/// A floor lit almost to white by a very bright point light just above it, a pillar, and on top of
/// the brightest pixels a ring of hostile bullets with glow plus the player marker: the bullets must
/// stay readable over the light (PRD-0003 rule 1), never washed out or covered by it.
fn bullets_on_top_frame(renderer: &mut WgpuRenderer, with_bullets: bool) -> StageFrame {
    let floor = renderer
        .register_mesh(floor_tile_grid(10, 2.0))
        .expect("valid mesh");
    let pillar = renderer
        .register_mesh(octagonal_pillar(0.5, 3.0))
        .expect("valid mesh");

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.02, 0.02, 0.03, 1.0];
    frame.camera_25d = Some(bullets_on_top_camera());
    frame.key_light = Some(key_light([0.3, 0.4, -0.85], [0.7, 0.75, 0.9], 0.8));
    frame.ambient = AmbientLight::Flat {
        color: [1.0, 1.0, 1.0],
        intensity: 0.15,
    };
    frame
        .materials
        .push(material([0.7, 0.68, 0.62, 1.0], 0.0, 0.6)); // 0: floor, pillar
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.5, 0.0]),
    ));
    frame.meshes.push(mesh_instance(
        pillar,
        MaterialHandle(0),
        translation([-4.0, 3.0, 1.5]),
    ));
    // The bright light: close above the centre bullet, saturating the floor around it.
    frame.point_lights.push(point_light(
        [BULLETS_ON_TOP_CENTRE[0], BULLETS_ON_TOP_CENTRE[1], 1.2],
        [1.0, 0.95, 0.85],
        7.0,
        60.0,
    ));

    if with_bullets {
        frame.bullets.push(scene_bullet(
            BULLETS_ON_TOP_CENTRE,
            0.0,
            bullet_silhouette::ORB,
            bullet_palette::HEX_MAGENTA,
            220,
        ));
        for index in 0..8u8 {
            let angle = f32::from(index) / 8.0 * std::f32::consts::TAU;
            let silhouette = if index % 2 == 0 {
                bullet_silhouette::RICE
            } else {
                bullet_silhouette::DIAMOND
            };
            frame.bullets.push(scene_bullet(
                [angle.cos() * 2.6, angle.sin() * 2.6],
                angle + std::f32::consts::FRAC_PI_2,
                silhouette,
                bullet_palette::POISON_LIME,
                140,
            ));
        }
        frame.marker_sprites.push(SpriteInstance {
            position: [0.0, -3.6],
            half_size: [0.6, 0.6],
            rotation: 0.0,
            shape: shape::CIRCLE,
            color: [0.55, 0.85, 1.0, 1.0],
        });
    }
    frame
}

fn scene_bullet(
    position: [f32; 2],
    rotation: f32,
    silhouette: u16,
    palette: u16,
    glow: u8,
) -> BulletInstance {
    BulletInstance {
        position,
        radius: BULLETS_ON_TOP_RADIUS,
        rotation,
        silhouette,
        palette,
        palette_space: BULLET_PASS_PALETTE_SPACE,
        glow,
        flags: 0,
    }
}

/// Luminance range `(min, max)` and the count of magenta-dominant pixels inside the disc of
/// `radius_px` around `centre` (pixel coordinates).
fn disc_statistics(image: &Image, centre: [f32; 2], radius_px: f32) -> (i32, i32, u32) {
    let (mut min, mut max, mut magenta) = (i32::MAX, i32::MIN, 0u32);
    for y in 0..image.height {
        for x in 0..image.width {
            let dx = x as f32 + 0.5 - centre[0];
            let dy = y as f32 + 0.5 - centre[1];
            if dx * dx + dy * dy > radius_px * radius_px {
                continue;
            }
            let lum = luminance(image, x, y);
            min = min.min(lum);
            max = max.max(lum);
            let index = ((y * image.width + x) * 4) as usize;
            let [r, g, b] = [
                i32::from(image.rgba[index]),
                i32::from(image.rgba[index + 1]),
                i32::from(image.rgba[index + 2]),
            ];
            if r > g + 60 && b > g + 30 {
                magenta += 1;
            }
        }
    }
    (min, max, magenta)
}

#[test]
#[ignore = "WP3.6 snapshot scene: run explicitly with GRIMOIRE_GPU_ADAPTER=software (see this file's module doc comment)"]
fn bullets_on_top_scene() {
    let Some(mut renderer) = try_offscreen_renderer() else {
        return;
    };
    let without = bullets_on_top_frame(&mut renderer, false);
    let floor_only = render_frame(&mut renderer, &without);
    let frame = bullets_on_top_frame(&mut renderer, true);
    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.bullets_drawn, 9);
    let image = Image::from_offscreen(
        WIDTH,
        HEIGHT,
        renderer.read_offscreen_rgba().expect("read-back"),
    );

    // Structural check, independent of any reference (PRD-0003 rule 1): around the centre bullet
    // the floor alone is white, and with the bullet its magenta body and darker rim show right on
    // those bright pixels.
    let centre = bullets_on_top_camera()
        .ground_to_screen(BULLETS_ON_TOP_CENTRE, [WIDTH as f32, HEIGHT as f32])
        .expect("the centre bullet is in view");
    let (floor_min, _, floor_magenta) = disc_statistics(&floor_only, centre, 3.0);
    let (bullet_min, bullet_max, bullet_magenta) = disc_statistics(&image, centre, 6.0);
    check_scene("bullets_on_top", &image);
    println!(
        "grimoire-bullets-on-top: floor_min_luminance={floor_min} bullet_luminance={bullet_min}..{bullet_max} magenta_pixels={bullet_magenta}"
    );
    assert!(
        floor_min > 170,
        "the light makes the floor under the bullet bright (darkest pixel {floor_min})"
    );
    assert_eq!(floor_magenta, 0);
    assert!(
        bullet_magenta >= 4,
        "the magenta body stays visible over the light ({bullet_magenta} pixels)"
    );
    assert!(
        bullet_min + 60 < floor_min && bullet_max > 200,
        "the dark rim stands out from the white floor and the core stays bright (floor {floor_min}, bullet luminance {bullet_min}..{bullet_max})"
    );
}

// --- OF-18.2: variance per adapter (WP3.6) --------------------------------------------------------

/// Every reference scene of this file, by reference name.
const VARIANCE_SCENES: [&str; 9] = [
    "pbr_materials",
    "shadows_keylight",
    "shadows_blob",
    "camera_tilt_35",
    "camera_tilt_60",
    "camera_tilt_75",
    "camera_tilt_90",
    "lights_256",
    "bullets_on_top",
];

/// A new renderer of the right configuration with the frame of the scene `name` built on it, or
/// `None` without an adapter.
fn scene_by_name(name: &str) -> Option<(WgpuRenderer, StageFrame)> {
    let mut renderer = if name == "lights_256" {
        try_offscreen_renderer_high_budget()?
    } else {
        try_offscreen_renderer()?
    };
    let frame = match name {
        "pbr_materials" => pbr_materials_frame(&mut renderer),
        "shadows_keylight" | "shadows_blob" => {
            let mut frame = shadows_frame(&mut renderer);
            let mut config = ShadowConfig::default();
            if name == "shadows_keylight" {
                config.mode = ShadowMode::KeyLight;
            } else {
                config.mode = ShadowMode::Blob;
                frame.blob_shadows.push(BlobShadowInstance {
                    position: [1.5, 0.5],
                    radius: 1.0,
                    softness: 0.45,
                    strength: 0.85,
                });
            }
            frame.shadow_config = config;
            frame
        }
        "lights_256" => lights_256_frame(&mut renderer),
        "bullets_on_top" => bullets_on_top_frame(&mut renderer, true),
        tilt => {
            let degrees: f32 = tilt
                .trim_start_matches("camera_tilt_")
                .parse()
                .expect("a camera_tilt_<degrees> scene");
            camera_tilt_frame(&mut renderer, degrees)
        }
    };
    Some((renderer, frame))
}

/// OF-18.2 measurement (plan 0002 WP3.6): renders every scene three times on this runner's
/// adapter — twice with one renderer (frame to frame) and once with a second renderer (device to
/// device) — and reports the largest difference between those runs, plus the difference to the
/// Windows (WARP) reference when this runner is not Windows. Prints one greppable
/// `grimoire-snapshot-variance:` line per scene and never fails on a difference. Runs on whatever
/// adapter `GRIMOIRE_GPU_ADAPTER` selects, so the macOS CI job measures its Metal device with the
/// variable unset.
#[test]
#[ignore = "OF-18.2 variance measurement: run explicitly (see this test's doc comment)"]
fn scene_variance() {
    let platform = support::platform_dir();
    for name in VARIANCE_SCENES {
        let Some((mut renderer, frame)) = scene_by_name(name) else {
            println!(
                "grimoire-snapshot-variance: name={name} platform={platform} skipped=no-adapter"
            );
            continue;
        };
        let adapter = renderer.adapter_report_line();
        let first = render_frame(&mut renderer, &frame);
        let frame_to_frame = render_frame(&mut renderer, &frame);
        drop(renderer);
        let (mut second_renderer, second_frame) =
            scene_by_name(name).expect("the adapter was there a moment ago");
        let device_to_device = render_frame(&mut second_renderer, &second_frame);

        let mut run_mean = 0.0f64;
        let mut run_max = 0u8;
        for other in [&frame_to_frame, &device_to_device] {
            let metric = support::compare(&first, other).expect("same size");
            run_mean = run_mean.max(metric.mean_abs_diff);
            run_max = run_max.max(metric.max_abs_diff);
        }
        let versus_windows = if platform == "windows" {
            String::from("vs_windows=self")
        } else {
            match Image::read_png(&reference_path_for(name, "windows")) {
                Ok(reference) => {
                    let metric = support::compare(&reference, &first).expect("same size");
                    format!(
                        "vs_windows_mean_abs_diff={:.3} vs_windows_max_abs_diff={} vs_windows_within_tolerance={}",
                        metric.mean_abs_diff,
                        metric.max_abs_diff,
                        metric.within_tolerance()
                    )
                }
                Err(_) => String::from("vs_windows=no-reference"),
            }
        };
        let adapter = adapter.trim_start_matches("grimoire-gpu-adapter: ");
        println!(
            "grimoire-snapshot-variance: name={name} platform={platform} runs=3 run_mean_abs_diff={run_mean:.3} run_max_abs_diff={run_max} {versus_windows} adapter=[{adapter}]"
        );
    }
}

/// Path of `name`'s reference image for `platform`.
fn reference_path_for(name: &str, platform: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(platform)
        .join(format!("{name}.png"))
}

// --- Self-test of the blocking comparison (plan 0002 M2), no GPU ---------------------------------

fn read_reference(name: &str, platform: &str) -> Image {
    let path = reference_path_for(name, platform);
    Image::read_png(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Proves the blocking rule on the committed references, without a GPU, in every `cargo test`:
/// a rendering that differs from the reference like a real regression fails on Windows and Linux
/// and only warns on macOS, a missing reference fails on Windows and Linux, and the WARP and
/// lavapipe renderings of every scene stay within tolerance of each other, so the rule does not
/// turn a correct rendering on the other adapter red.
#[test]
fn blocking_comparison_fails_on_a_real_regression() {
    // Two regressions the references themselves contain: the key-light shadow map silently
    // replaced by a blob shadow, and the camera tilted 90 instead of 75 degrees.
    let regressions = [
        ("shadows_keylight", "shadows_blob"),
        ("camera_tilt_75", "camera_tilt_90"),
    ];
    for platform in support::BLOCKING_PLATFORMS {
        for (expected, rendered) in regressions {
            let metric = support::compare(
                &read_reference(expected, platform),
                &read_reference(rendered, platform),
            )
            .expect("same size");
            assert!(
                !metric.within_tolerance(),
                "{rendered} passes as {expected} on {platform}: {metric:?}"
            );
            assert!(failure(expected, platform, Some(&metric)).is_some());
            assert!(
                failure(expected, "macos", Some(&metric)).is_none(),
                "macOS only warns"
            );
        }
        assert!(failure("pbr_materials", platform, None).is_some());
    }
    assert!(failure("pbr_materials", "macos", None).is_none());

    for name in VARIANCE_SCENES {
        let metric = support::compare(
            &read_reference(name, "windows"),
            &read_reference(name, "linux"),
        )
        .expect("same size");
        assert!(
            metric.within_tolerance(),
            "{name}: WARP and lavapipe references differ beyond tolerance: {metric:?}"
        );
        for platform in support::BLOCKING_PLATFORMS {
            assert!(failure(name, platform, Some(&metric)).is_none());
        }
    }
}
