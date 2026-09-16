//! Plan 0002 WP2.6 (OF-3.2) showcase: offscreen comparison images and a relative-cost measurement
//! for the shadow techniques, on a small procedural arena (floor, pillars, an altar and a few
//! "figures", lit by a key light and a handful of torch point lights) — the kind of scene
//! `docs/adr/0014-realistischer-3d-look-statt-toon.md`'s look-dev spike and this crate's WP2.6 ADR
//! both describe. Both tests below are `#[ignore]`d spike-measurement tests, run explicitly (like
//! `tests/offscreen.rs`'s `measure_specular_aa_relative_cost_and_shimmer`): they are not part of
//! the default `cargo test` run and need `GRIMOIRE_GPU_ADAPTER=software`.
//!
//! No PNG-encoding dependency exists anywhere in this workspace (deliberately: this repo is
//! public, and no other engine code needs one), so [`png_writer`] is a small, self-contained
//! writer good enough for a lossless RGBA8 dump — a valid PNG using uncompressed ("stored")
//! `DEFLATE` blocks, which the PNG/zlib formats allow explicitly. It is not a general-purpose PNG
//! encoder (no filtering, no real compression) and lives only in this test file.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use grimoire_render::procedural::{altar_block, capsule_actor, floor_tile_grid, octagonal_pillar};
use grimoire_render::{
    AmbientLight, BlobShadowInstance, Camera25D, DirectionalLight, MaterialHandle, MeshHandle,
    MeshInstance, PbrMaterial, PointLight, RenderError, Renderer, RendererConfig, ShadowConfig,
    ShadowMode, StageFrame, WgpuRenderer,
};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

// --- Minimal PNG writer (see this file's header comment) ---------------------------------------

mod png_writer {
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Wraps `data` in a valid zlib stream made only of uncompressed ("stored", `BTYPE = 00`)
    /// `DEFLATE` blocks (max 65535 bytes each) — a real encoder would compress, but validity, not
    /// size, is this helper's only job.
    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 8);
        out.push(0x78);
        out.push(0x01);
        let mut i = 0;
        if data.is_empty() {
            out.push(1); // BFINAL = 1, BTYPE = 00, one empty final block.
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        }
        while i < data.len() {
            let remaining = data.len() - i;
            let block_len = remaining.min(65535);
            let is_final = i + block_len >= data.len();
            out.push(u8::from(is_final));
            let len = block_len as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(&data[i..i + block_len]);
            i += block_len;
        }
        out.extend_from_slice(&adler32(data).to_be_bytes());
        out
    }

    fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    /// Encodes tightly packed, top-row-first RGBA8 `rgba` (`width * height * 4` bytes) as a PNG.
    pub(super) fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        assert_eq!(
            rgba.len(),
            width as usize * height as usize * 4,
            "rgba length"
        );
        let row_bytes = width as usize * 4;
        let mut raw = Vec::with_capacity(rgba.len() + height as usize);
        for row in 0..height as usize {
            raw.push(0u8); // Filter type 0 (None) for every scanline.
            raw.extend_from_slice(&rgba[row * row_bytes..(row + 1) * row_bytes]);
        }
        let compressed = zlib_stored(&raw);

        let mut out = Vec::new();
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit depth, colour type 6 (RGBA).
        push_chunk(&mut out, b"IHDR", &ihdr);
        push_chunk(&mut out, b"IDAT", &compressed);
        push_chunk(&mut out, b"IEND", &[]);
        out
    }
}

/// A tightly packed, top-row-first RGBA8 image, for cropping and compositing before encoding.
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn from_offscreen(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), width as usize * height as usize * 4);
        Self {
            width,
            height,
            rgba,
        }
    }

    /// A `width x height` crop with its top-left corner at (`x`, `y`), clamped so it never reads
    /// outside `self` (a crop request near an edge is shrunk rather than panicking or wrapping).
    fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Image {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let width = width.min(self.width - x);
        let height = height.min(self.height - y);
        let mut out = vec![0u8; width as usize * height as usize * 4];
        for row in 0..height {
            let src_start = ((y + row) * self.width + x) as usize * 4;
            let dst_start = (row * width) as usize * 4;
            let len = width as usize * 4;
            out[dst_start..dst_start + len].copy_from_slice(&self.rgba[src_start..src_start + len]);
        }
        Image {
            width,
            height,
            rgba: out,
        }
    }

    /// Places `self`, `middle` and `right` side by side (same height) into one wide image, for an
    /// at-a-glance comparison of the shadow variants.
    fn beside(images: &[&Image]) -> Image {
        let height = images.iter().map(|image| image.height).max().unwrap_or(0);
        let width: u32 = images.iter().map(|image| image.width).sum();
        let mut out = vec![0u8; width as usize * height as usize * 4];
        let mut x_offset = 0u32;
        for image in images {
            for row in 0..image.height {
                let src_start = (row * image.width) as usize * 4;
                let dst_start = ((row * width) + x_offset) as usize * 4;
                let len = image.width as usize * 4;
                out[dst_start..dst_start + len]
                    .copy_from_slice(&image.rgba[src_start..src_start + len]);
            }
            x_offset += image.width;
        }
        Image {
            width,
            height,
            rgba: out,
        }
    }

    fn write_png(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = png_writer::encode(self.width, self.height, &self.rgba);
        let mut file = std::fs::File::create(path)?;
        file.write_all(&bytes)
    }
}

// --- Arena scene (floor, pillars, altar, figures, torches, key light) --------------------------

fn arena_camera() -> Camera25D {
    let mut camera = Camera25D::default();
    camera.target = [0.0, 2.0];
    camera.tilt_degrees = 65.0;
    camera.fov_y_degrees = 50.0;
    camera.distance = 34.0;
    camera
}

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

fn torch(position: [f32; 3]) -> PointLight {
    let mut light = PointLight::default();
    light.position = position;
    // Stilbibel's entsättigter Glut colour for cluster lights near the ground; used here for
    // every torch, not just bullet-cluster lights (`docs/art/stilbibel.md` "Licht und Stimmung").
    light.color = [0.83, 0.53, 0.33];
    light.intensity = 6.0;
    light.range = 14.0;
    light
}

/// World-space position of the figure nearest the camera: used both to place it and to find its
/// screen-space contact point for the crop (`ground_to_screen`), so the crop lands on the biggest,
/// clearest figure in the scene rather than a distant, small one.
const CONTACT_FIGURE_WORLD: [f32; 2] = [1.0, -6.5];

/// Registers the arena's meshes and returns a [`StageFrame`] with every instance, material and
/// light already filled in — everything except [`StageFrame::shadow_config`] and
/// [`StageFrame::blob_shadows`], which the caller sets per shadow variant.
fn build_arena_frame(renderer: &mut WgpuRenderer) -> StageFrame {
    let floor = renderer
        .register_mesh(floor_tile_grid(20, 2.0))
        .expect("valid mesh");
    let pillar = renderer
        .register_mesh(octagonal_pillar(0.6, 6.0))
        .expect("valid mesh");
    let altar = renderer
        .register_mesh(altar_block(3.0, 2.0, 1.2))
        .expect("valid mesh");
    let figure = renderer
        .register_mesh(capsule_actor(0.6, 1.8, 12, 3))
        .expect("valid mesh");

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.02, 0.024, 0.036, 1.0];
    frame.camera_25d = Some(arena_camera());

    let mut key_light = DirectionalLight::default();
    // A shallow, "moonlight" angle (per the stilbibel's key light): steep enough to read as
    // overhead light, shallow enough to cast long, clearly visible shadows across the arena floor.
    key_light.direction = [0.35, 0.5, -0.8];
    key_light.color = [0.60, 0.69, 0.85]; // Stilbibel "Mond" colour (`#9AB0D8`), linear-ish.
    // Boosted well past the stilbibel's calibrated in-game exposure (`docs/art/stilbibel.md`'s
    // Boden-Median target): this is a comparison demo for the shadow *techniques* themselves, and
    // needs to read clearly at a glance, not a colour-graded final frame.
    key_light.intensity = 5.5;
    frame.key_light = Some(key_light);
    frame.ambient = AmbientLight::Hemisphere {
        sky_color: [0.16, 0.18, 0.24],
        ground_color: [0.06, 0.055, 0.06],
        intensity: 1.0,
    };

    frame
        .materials
        .push(material([0.20, 0.21, 0.24, 1.0], 0.0, 0.85)); // 0: floor/pillars ("Bodenstein")
    frame
        .materials
        .push(material([0.11, 0.09, 0.095, 1.0], 0.0, 0.6)); // 1: altar ("Basalt")
    frame
        .materials
        .push(material([0.30, 0.27, 0.25, 1.0], 0.0, 0.7)); // 2: figures

    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    frame.meshes.push(mesh_instance(
        altar,
        MaterialHandle(1),
        translation([0.0, 0.0, 0.6]),
    ));

    const PILLAR_COUNT: u32 = 8;
    const PILLAR_RADIUS: f32 = 13.0;
    for i in 0..PILLAR_COUNT {
        let angle = i as f32 / PILLAR_COUNT as f32 * std::f32::consts::TAU;
        let (x, y) = (PILLAR_RADIUS * angle.cos(), PILLAR_RADIUS * angle.sin());
        frame.meshes.push(mesh_instance(
            pillar,
            MaterialHandle(0),
            translation([x, y, 3.0]),
        ));
        frame.point_lights.push(torch([x * 0.82, y * 0.82, 4.2]));
    }

    let figure_positions = [
        [-4.0, 6.0],
        [4.5, 4.0],
        [-2.5, -5.0],
        [3.0, 10.0],
        CONTACT_FIGURE_WORLD,
    ];
    for &[x, y] in &figure_positions {
        // `capsule_actor(0.6, 1.8, ..)`'s half-height along Z is `1.8 / 2 + 0.6 = 1.5`; this
        // places its bottom exactly on the floor (`Z = 0`), like the altar and pillars above.
        frame.meshes.push(mesh_instance(
            figure,
            MaterialHandle(2),
            translation([x, y, 1.5]),
        ));
    }

    frame
}

/// Blob shadow discs under every figure (plan 0002 WP2.6, `ShadowMode::Blob`): radius/softness
/// tuned to roughly cover a figure's footprint, matching the stylebook's "Low" preset description.
fn arena_blob_shadows() -> Vec<BlobShadowInstance> {
    [
        [-4.0, 6.0],
        [4.5, 4.0],
        [-2.5, -5.0],
        [3.0, 10.0],
        CONTACT_FIGURE_WORLD,
    ]
    .into_iter()
    .map(|position| BlobShadowInstance {
        position,
        radius: 1.1,
        softness: 0.5,
        strength: 0.85,
    })
    .collect()
}

fn offscreen_renderer(width: u32, height: u32) -> Option<WgpuRenderer> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    };
    match WgpuRenderer::new_offscreen(width, height, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => None,
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

fn out_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_SHADOW_SHOWCASE_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("target").join("wp26-shadows"))
}

/// Renders `frame` under `mode` and returns the resulting image plus [`StageStats`] (for the
/// caller to report caster/blob counts alongside each saved file).
fn render_variant(
    renderer: &mut WgpuRenderer,
    frame: &mut StageFrame,
    mode: ShadowMode,
) -> (Image, grimoire_render::StageStats) {
    let mut shadow_config = ShadowConfig::default();
    shadow_config.mode = mode;
    frame.shadow_config = shadow_config;
    frame.blob_shadows = if mode == ShadowMode::Blob {
        arena_blob_shadows()
    } else {
        Vec::new()
    };
    let stats = renderer.render_stage(frame).expect("render_stage");
    let rgba = renderer.read_offscreen_rgba().expect("read-back");
    (Image::from_offscreen(WIDTH, HEIGHT, rgba), stats)
}

/// Produces the WP2.6 PO-decision comparison images (plan 0002 WP2.6 step 4): `shadows_none.png`,
/// `shadows_blob.png`, `shadows_keylight.png`, a side-by-side `shadows_comparison.png`, and a
/// `shadows_contact_crop.png` around one figure's feet. Point-light shadow casters are deferred
/// (see `ShadowMode::KeyLightPlusPoints`'s doc comment and this crate's WP2.6 ADR), so no
/// `shadows_keylight_plus_points.png` is produced — it would currently be pixel-identical to the
/// key-light-only image, which would misrepresent what is actually implemented.
///
/// Run explicitly: `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test
/// wp26_shadow_showcase --locked -- --ignored --nocapture save_shadow_comparison_images`. Writes
/// under `GRIMOIRE_SHADOW_SHOWCASE_OUT_DIR` (default `target/wp26-shadows`); the caller is
/// responsible for choosing a directory outside the repository if the images should not be
/// committed (they never are — no `.gitignore` entry is needed because this crate's own
/// `target/` is already ignored by the workspace root `.gitignore`).
#[test]
#[ignore = "WP2.6 showcase: offscreen comparison images, run explicitly (see this test's doc comment)"]
fn save_shadow_comparison_images() {
    let Some(mut renderer) = offscreen_renderer(WIDTH, HEIGHT) else {
        eprintln!("no GPU adapter available; skipping the WP2.6 showcase");
        return;
    };
    let mut frame = build_arena_frame(&mut renderer);
    let dir = out_dir();

    let (none_image, none_stats) = render_variant(&mut renderer, &mut frame, ShadowMode::None);
    let (blob_image, blob_stats) = render_variant(&mut renderer, &mut frame, ShadowMode::Blob);
    let (key_light_image, key_light_stats) =
        render_variant(&mut renderer, &mut frame, ShadowMode::KeyLight);

    println!(
        "shadows_none: shadow_casters_drawn={} blob_shadows_drawn={}",
        none_stats.shadow_casters_drawn, none_stats.blob_shadows_drawn
    );
    println!(
        "shadows_blob: shadow_casters_drawn={} blob_shadows_drawn={}",
        blob_stats.shadow_casters_drawn, blob_stats.blob_shadows_drawn
    );
    println!(
        "shadows_keylight: shadow_casters_drawn={} blob_shadows_drawn={}",
        key_light_stats.shadow_casters_drawn, key_light_stats.blob_shadows_drawn
    );

    none_image
        .write_png(&dir.join("shadows_none.png"))
        .expect("write shadows_none.png");
    blob_image
        .write_png(&dir.join("shadows_blob.png"))
        .expect("write shadows_blob.png");
    key_light_image
        .write_png(&dir.join("shadows_keylight.png"))
        .expect("write shadows_keylight.png");

    let comparison = Image::beside(&[&none_image, &blob_image, &key_light_image]);
    comparison
        .write_png(&dir.join("shadows_comparison.png"))
        .expect("write shadows_comparison.png");

    // Crop around the contact figure's feet (world Z = 0, its base) in the key-light render, using
    // the same public ray-cast the mouse-aim/camera contract already relies on
    // (`Camera25D::ground_to_screen`), so the crop tracks the scene's own camera and figure
    // placement instead of a hand-guessed pixel offset.
    let camera = arena_camera();
    let contact_pixel = camera
        .ground_to_screen(CONTACT_FIGURE_WORLD, [WIDTH as f32, HEIGHT as f32])
        .expect("the contact figure is placed within the camera's view");
    let crop_size = 340u32;
    let crop_x = (contact_pixel[0] - crop_size as f32 / 2.0).max(0.0) as u32;
    let crop_y = (contact_pixel[1] - crop_size as f32 * 0.3).max(0.0) as u32;
    key_light_image
        .crop(crop_x, crop_y, crop_size, crop_size)
        .write_png(&dir.join("shadows_contact_crop.png"))
        .expect("write shadows_contact_crop.png");

    println!("WP2.6 showcase images written under {}", dir.display());
}

/// Median of `samples`, sorted first (samples must be non-empty). Chosen over a mean (the OF-3.5
/// spike measurement's own choice) because the task for this ADR specifically asks for a median,
/// and CI/software-adapter timings are exactly the kind of noisy measurement a median resists
/// better than a mean.
fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Relative CPU-side cost of each shadow variant plus the key-light shadow map's memory footprint
/// per resolution (plan 0002 WP2.6 step 5) — software-adapter timings only, **not a GPU budget**
/// (this module's and `measure_specular_aa_relative_cost_and_shimmer`'s shared caveat).
///
/// Run explicitly: `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test
/// wp26_shadow_showcase --locked -- --ignored --nocapture measure_shadow_relative_cost`.
#[test]
#[ignore = "WP2.6 spike measurement, run explicitly (see this test's doc comment)"]
fn measure_shadow_relative_cost() {
    let Some(mut renderer) = offscreen_renderer(WIDTH, HEIGHT) else {
        eprintln!("no GPU adapter available; skipping the WP2.6 cost measurement");
        return;
    };
    let mut frame = build_arena_frame(&mut renderer);

    let samples = 21; // Odd, so the median is a single real sample, not an average of two.
    for mode in [ShadowMode::None, ShadowMode::Blob, ShadowMode::KeyLight] {
        let mut shadow_config = ShadowConfig::default();
        shadow_config.mode = mode;
        frame.shadow_config = shadow_config;
        frame.blob_shadows = if mode == ShadowMode::Blob {
            arena_blob_shadows()
        } else {
            Vec::new()
        };
        for _ in 0..5 {
            renderer.render_stage(&frame).expect("warm-up render");
        }
        renderer.read_offscreen_rgba().expect("flush");
        let mut times = Vec::with_capacity(samples);
        for _ in 0..samples {
            let stats = renderer.render_stage(&frame).expect("render");
            times.push(stats.base.cpu_time);
        }
        renderer.read_offscreen_rgba().expect("flush");
        println!(
            "backend {} (software adapter, NOT a GPU budget): mode={mode:?}, {samples} frames, median cpu_time {:?}",
            renderer.backend_name(),
            median(times)
        );
    }

    println!("shadow map memory per resolution (Depth32Float, 4 bytes/texel):");
    for map_size in [512u32, 1024, 2048] {
        let bytes = u64::from(map_size) * u64::from(map_size) * 4;
        println!(
            "  {map_size}x{map_size}: {bytes} bytes ({:.2} MiB)",
            bytes as f64 / (1024.0 * 1024.0)
        );
    }
}
