//! Offscreen rendering tests.
//!
//! Without any GPU adapter (not even a software one) they skip and pass, and print a GitHub
//! Actions warning that stays visible in the log. With `GRIMOIRE_REQUIRE_GPU_ADAPTER=1` a missing
//! adapter fails them instead; CI sets it wherever WARP or lavapipe guarantee an adapter.
//!
//! CI-render-honesty (WP2.1, groundwork for OF-18.2): whichever adapter a run actually obtained is
//! printed once per test binary as a greppable `grimoire-gpu-adapter: ...` line (name, backend,
//! device type, driver info), and every skip for lack of an adapter bumps a running
//! `grimoire-gpu-tests-skipped: <n>` total. Both lines bypass libtest's capture of passing-test
//! output (see the comment on the direct `stdout()` write below) so CI can grep them out of the
//! job log and fold them into the job summary (`.github/scripts/report-gpu-adapter.sh`).

use std::io::Write;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use grimoire_render::procedural::{altar_block, floor_tile_grid, icosphere};
use grimoire_render::{
    AmbientLight, BULLET_PASS_PALETTE_SPACE, BlobShadowInstance, BulletInstance, Camera2D,
    Camera25D, DirectionalLight, MaterialHandle, MeshData, MeshHandle, MeshInstance, MeshVertex,
    Msaa, PbrMaterial, PointLight, RenderError, RenderFrame, RenderLayer, Renderer, RendererConfig,
    ShadowMode, SpriteInstance, StageFrame, StageRendererConfig, TextureColorSpace, TextureData,
    TextureHandle, WgpuRenderer, bullet_palette, bullet_silhouette, shape,
};

const SIZE: u32 = 64;

/// Environment variable that turns a missing GPU adapter from a skip into a test failure.
const ENV_REQUIRE_GPU_ADAPTER: &str = "GRIMOIRE_REQUIRE_GPU_ADAPTER";

fn adapter_required(value: Option<&str>) -> bool {
    matches!(
        value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true")
    )
}

/// Total offscreen/GPU tests in this process that skipped for lack of an adapter (WP2.1). CI reads
/// the highest `grimoire-gpu-tests-skipped: <n>` line out of the log, so the running total (rather
/// than a single deduplicated announcement) survives no matter which test happens to run last.
static SKIPPED_GPU_TESTS: AtomicUsize = AtomicUsize::new(0);

/// Fails the calling test if an adapter is required, otherwise announces the skip and counts it.
fn skip_without_adapter(required: bool) {
    assert!(
        !required,
        "no GPU adapter found although {ENV_REQUIRE_GPU_ADAPTER}=1: the render tests would pass          without rendering"
    );
    static ANNOUNCED: Once = Once::new();
    ANNOUNCED.call_once(|| {
        // libtest captures print!/eprint! of passing tests, but not direct writes to the stream.
        let _ = writeln!(
            std::io::stdout(),
            "
::warning title=GPU tests skipped::no GPU adapter found; grimoire_render offscreen              tests passed without rendering"
        );
    });
    let total = SKIPPED_GPU_TESTS.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = writeln!(std::io::stdout(), "grimoire-gpu-tests-skipped: {total}");
}

/// Prints [`WgpuRenderer::adapter_report_line`] once per test binary (WP2.1), the first time a
/// test actually obtains an adapter.
fn report_adapter_once(renderer: &WgpuRenderer) {
    static REPORTED: Once = Once::new();
    REPORTED.call_once(|| {
        // Same rationale as the skip warning above: a direct stdout write reaches the CI log even
        // though these tests pass.
        let _ = writeln!(std::io::stdout(), "\n{}", renderer.adapter_report_line());
    });
}

fn offscreen_renderer(
    width: u32,
    height: u32,
    initial_sprite_capacity: u32,
) -> Option<WgpuRenderer> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity,
        allow_software_fallback: true,
    };
    match WgpuRenderer::new_offscreen(width, height, config) {
        Ok(renderer) => {
            report_adapter_once(&renderer);
            Some(renderer)
        }
        Err(RenderError::NoAdapter) => {
            let required = std::env::var(ENV_REQUIRE_GPU_ADAPTER).ok();
            skip_without_adapter(adapter_required(required.as_deref()));
            None
        }
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

fn pixel(image: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    [
        image[index],
        image[index + 1],
        image[index + 2],
        image[index + 3],
    ]
}

fn assert_near(actual: [u8; 4], expected: [u8; 4], tolerance: u8, what: &str) {
    for (a, e) in actual.iter().zip(expected) {
        assert!(
            a.abs_diff(e) <= tolerance,
            "{what}: expected ~{expected:?}, got {actual:?}"
        );
    }
}

fn circle(position: [f32; 2], radius: f32, color: [f32; 4]) -> SpriteInstance {
    SpriteInstance {
        position,
        half_size: [radius, radius],
        rotation: 0.0,
        shape: shape::CIRCLE,
        color,
    }
}

/// Frame whose camera maps the 64x64 target to world [-50, 50]^2.
fn black_frame() -> RenderFrame {
    RenderFrame {
        clear_color: [0.0, 0.0, 0.0, 1.0],
        camera: Camera2D {
            center: [0.0, 0.0],
            world_height: 100.0,
        },
        sprites: Vec::new(),
    }
}

#[test]
fn adapter_requirement_is_read_from_the_variable() {
    for value in ["1", "true", "TRUE", " 1 "] {
        assert!(adapter_required(Some(value)), "{value:?}");
    }
    for value in [None, Some(""), Some("0"), Some("false"), Some("yes")] {
        assert!(!adapter_required(value), "{value:?}");
    }
}

#[test]
#[should_panic(expected = "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1")]
fn missing_adapter_fails_when_required() {
    skip_without_adapter(true);
}

#[test]
fn red_circle_in_the_centre_on_black() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame
        .sprites
        .push(circle([0.0, 0.0], 20.0, [1.0, 0.0, 0.0, 1.0]));

    let stats = renderer.render(&frame).expect("render");
    assert_eq!(stats.sprites_drawn, 1);
    assert_eq!(stats.draw_calls, 1);

    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_eq!(image.len(), (SIZE * SIZE * 4) as usize);
    let centre = pixel(&image, SIZE, SIZE / 2, SIZE / 2);
    println!("backend {}: centre {centre:?}", renderer.backend_name());
    assert_near(centre, [255, 0, 0, 255], 3, "centre pixel");
    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)] {
        assert_near(pixel(&image, SIZE, x, y), [0, 0, 0, 255], 3, "corner pixel");
    }
    // Pixel centre (43.5, 21.5) is world (18.0, 16.4): inside the sprite's bounding square
    // (half size 20) but 24.3 units from the centre, so outside the circle.
    assert_near(
        pixel(&image, SIZE, 43, 21),
        [0, 0, 0, 255],
        3,
        "bounding-square corner outside the circle",
    );
    // The signed-distance edge is anti-aliased: some pixel on the centre row is partially red.
    let rim: Vec<u8> = (SIZE / 2..SIZE)
        .map(|x| pixel(&image, SIZE, x, SIZE / 2)[0])
        .collect();
    println!("centre row from the middle to the right edge, red channel: {rim:?}");
    assert!(
        rim.iter().any(|&red| (10..=245).contains(&red)),
        "circle edge is not anti-aliased: {rim:?}"
    );
}

#[test]
fn world_y_points_up() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame
        .sprites
        .push(circle([0.0, 25.0], 6.0, [1.0, 0.0, 0.0, 1.0]));
    renderer.render(&frame).expect("render");
    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_near(
        pixel(&image, SIZE, 32, 16),
        [255, 0, 0, 255],
        3,
        "upper half",
    );
    assert_near(pixel(&image, SIZE, 32, 48), [0, 0, 0, 255], 3, "lower half");
}

#[test]
fn quad_is_rotated() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame.sprites.push(SpriteInstance {
        position: [0.0, 0.0],
        half_size: [10.0, 10.0],
        rotation: std::f32::consts::FRAC_PI_4,
        shape: shape::QUAD,
        color: [0.0, 1.0, 0.0, 1.0],
    });
    renderer.render(&frame).expect("render");
    let image = renderer.read_offscreen_rgba().expect("read-back");
    // Pixel centre (32.5, 24.5) is 11.7 world units above the centre: outside the unrotated
    // square (half size 10) but inside the diamond (reach 14.1).
    assert_near(
        pixel(&image, SIZE, 32, 24),
        [0, 255, 0, 255],
        3,
        "diamond tip",
    );
    assert_near(
        pixel(&image, SIZE, 32, 32),
        [0, 255, 0, 255],
        3,
        "quad centre",
    );
}

#[test]
fn rotation_is_counter_clockwise_and_half_size_is_per_axis() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame.sprites.push(SpriteInstance {
        position: [0.0, 0.0],
        half_size: [20.0, 3.0],
        rotation: std::f32::consts::FRAC_PI_6,
        shape: shape::QUAD,
        color: [0.0, 1.0, 0.0, 1.0],
    });
    renderer.render(&frame).expect("render");
    let image = renderer.read_offscreen_rgba().expect("read-back");
    // Pixel centre (40.5, 27.5) is world (13.3, 7.0): 15.0 units along the long axis rotated
    // counter-clockwise by 30 degrees, 0.6 units off it.
    assert_near(
        pixel(&image, SIZE, 40, 27),
        [0, 255, 0, 255],
        3,
        "upper right, on the rotated long axis",
    );
    // Its mirror image world (-13.3, 7.0) would be covered by a clockwise rotation or by
    // swapped half-size components.
    assert_near(
        pixel(&image, SIZE, 23, 27),
        [0, 0, 0, 255],
        3,
        "upper left, off the rotated long axis",
    );
    assert_near(
        pixel(&image, SIZE, 32, 12),
        [0, 0, 0, 255],
        3,
        "above the centre, beyond the short half size",
    );
}

#[test]
fn alpha_blends_in_linear_space() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame.sprites.push(SpriteInstance {
        position: [0.0, 0.0],
        half_size: [50.0, 50.0],
        rotation: 0.0,
        shape: shape::QUAD,
        color: [1.0, 1.0, 1.0, 0.5],
    });
    renderer.render(&frame).expect("render");
    let image = renderer.read_offscreen_rgba().expect("read-back");
    let value = pixel(&image, SIZE, 10, 10);
    println!("50 % white over black: {value:?}");
    // Linear 0.5 encodes to sRGB 0.735 (~188), not 128.
    assert_near(value, [188, 188, 188, 255], 3, "blended pixel");
}

#[test]
fn ten_thousand_sprites_in_one_draw_call() {
    let Some(mut renderer) = offscreen_renderer(256, 256, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame.sprites = swarm(10_000);
    // The last instance lies far beyond the initial capacity of 16 and is drawn on top, so it
    // is only visible if the grown buffer is uploaded and bound completely.
    frame.sprites[9_999] = SpriteInstance {
        position: [0.0, 0.0],
        half_size: [10.0, 10.0],
        rotation: 0.0,
        shape: shape::QUAD,
        color: [1.0, 0.0, 1.0, 1.0],
    };
    let stats = renderer.render(&frame).expect("render");
    assert_eq!(stats.sprites_drawn, 10_000);
    assert_eq!(stats.draw_calls, 1);
    assert!(stats.cpu_time > Duration::ZERO);
    // A second frame reuses the grown buffer.
    let stats = renderer.render(&frame).expect("second render");
    assert_eq!(stats.sprites_drawn, 10_000);
    assert_eq!(stats.draw_calls, 1);
    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_eq!(image.len(), 256 * 256 * 4);
    assert_near(
        pixel(&image, 256, 128, 128),
        [255, 0, 255, 255],
        3,
        "last instance on top",
    );
}

#[test]
fn empty_frame_clears_only() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame.clear_color = [0.0, 0.0, 1.0, 1.0];
    let stats = renderer.render(&frame).expect("render");
    assert_eq!(stats.sprites_drawn, 0);
    assert_eq!(stats.draw_calls, 0);
    let image = renderer.read_offscreen_rgba().expect("read-back");
    for (x, y) in [(0, 0), (SIZE / 2, SIZE / 2), (SIZE - 1, SIZE - 1)] {
        assert_near(
            pixel(&image, SIZE, x, y),
            [0, 0, 255, 255],
            0,
            "clear colour",
        );
    }
}

#[test]
fn failed_resize_is_returned_from_every_render_until_a_resize_succeeds() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame
        .sprites
        .push(circle([0.0, 0.0], 20.0, [1.0, 0.0, 0.0, 1.0]));

    // Beyond the 2D texture limit of every wgpu device.
    let too_wide = 1 << 20;
    renderer.resize(too_wide, SIZE);
    for attempt in 0..2 {
        match renderer.render(&frame) {
            Err(RenderError::Backend(message)) => assert!(
                message.contains(&format!("resize to {too_wide}x{SIZE} failed")),
                "attempt {attempt}: {message}"
            ),
            other => panic!("attempt {attempt}: expected the resize error, got {other:?}"),
        }
    }

    renderer.resize(SIZE, SIZE);
    let stats = renderer
        .render(&frame)
        .expect("render after a valid resize");
    assert_eq!((stats.sprites_drawn, stats.draw_calls), (1, 1));
    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_eq!(image.len(), (SIZE * SIZE * 4) as usize);
    assert_near(
        pixel(&image, SIZE, SIZE / 2, SIZE / 2),
        [255, 0, 0, 255],
        3,
        "centre after recovery",
    );
}

#[test]
fn zero_size_skips_rendering_until_resized() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = black_frame();
    frame
        .sprites
        .push(circle([0.0, 0.0], 20.0, [1.0, 0.0, 0.0, 1.0]));

    renderer.resize(0, 0);
    let stats = renderer
        .render(&frame)
        .expect("zero-size render is skipped");
    assert_eq!((stats.sprites_drawn, stats.draw_calls), (0, 0));

    renderer.resize(32, 16);
    let stats = renderer.render(&frame).expect("render after resize");
    assert_eq!((stats.sprites_drawn, stats.draw_calls), (1, 1));
    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_eq!(image.len(), 32 * 16 * 4);
    assert_near(
        pixel(&image, 32, 16, 8),
        [255, 0, 0, 255],
        3,
        "centre after resize",
    );
}

// --- Mesh pass (WP2.3): depth ordering, and the sprite pass drawn on top of it --------------

/// Column-major translation-only transform (no rotation or scale), same matrix convention as
/// [`grimoire_render::Camera2D::view_projection`] and [`MeshInstance::transform`].
fn translation(t: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

/// A camera looking straight down (`tilt_degrees: 90.0`) from `(0, 0, 10)`, so screen pixels map
/// to world X/Y in a simple, symmetric way: the centre pixel always sees world `(0, 0, ..)`, and
/// `ndc = world_xy / (depth * tan(fov_y / 2))` for any other point at world depth
/// `depth = 10.0 - world_z` (derived from the same camera basis `stage3d::view_projection` uses).
///
/// Built by mutating [`Camera25D::default()`]'s fields rather than a struct literal: it is
/// `#[non_exhaustive]`, so an external crate (like this integration test) cannot construct one
/// with a literal at all, even with `..Default::default()` (contract §2 rule 13).
fn top_down_camera() -> Camera25D {
    let mut camera = Camera25D::default();
    camera.target = [0.0, 0.0];
    camera.tilt_degrees = 90.0;
    camera.fov_y_degrees = 50.0;
    camera.distance = 10.0;
    camera
}

/// A material with only `base_color_factor` set; used with a bright key light plus ambient so the
/// provisional shading saturates to (close to) the pure colour, keeping pixel assertions simple.
fn flat_material(color: [f32; 4]) -> PbrMaterial {
    let mut material = PbrMaterial::default();
    material.base_color_factor = color;
    material
}

/// One [`MeshInstance`] on [`grimoire_render::RenderLayer::World`] (its `Default`) with the given
/// mesh, material and transform.
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

/// A very bright, straight-down key light plus a modest flat white ambient term, chosen so every
/// up-facing surface in the tests below (a horizontal top face, straight-down camera) saturates
/// its lit channels to (at or above, before clamping) full brightness.
///
/// `flat_material` leaves `metallic_factor` at [`PbrMaterial::default`]'s `1.0`: a fully metallic
/// surface has no diffuse term at all (`mesh.wgsl`'s `kd = (1 - F) * (1 - metallic)`), and its
/// Fresnel reflectance `F0` *is* `base_color_factor`, so an unlit channel (`base_color_factor`
/// component `0.0`) gets exactly `F0 = 0.0` and therefore exactly zero contribution from every
/// light — cleaner for these pixel assertions than PBR's usual dielectric default (`F0 = 0.04`
/// grey for every channel, which would tint "pure" colours slightly grey). The key light's
/// intensity is high because the physically normalised GGX specular this pass now uses is far
/// dimmer per unit of light intensity than WP2.3's provisional Lambert term was (no `/PI`
/// normalisation there, and no dependence on `roughness`/`F` at all).
fn full_bright_lighting(frame: &mut StageFrame) {
    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.0, 0.0, -1.0];
    key_light.color = [1.0, 1.0, 1.0];
    key_light.intensity = 60.0;
    frame.key_light = Some(key_light);
    frame.ambient = AmbientLight::Flat {
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
    };
}

#[test]
fn nearer_mesh_occludes_a_farther_one_through_the_depth_buffer() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    // A small box (near, top face at world Z = 3) and a huge floor underneath it (far, at world
    // Z = -5); same XY region at the centre, so the centre pixel's ray hits both.
    let near_mesh = renderer
        .register_mesh(altar_block(2.0, 2.0, 2.0))
        .expect("valid mesh");
    let far_mesh = renderer
        .register_mesh(floor_tile_grid(4, 20.0))
        .expect("valid mesh");

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.camera_25d = Some(top_down_camera());
    full_bright_lighting(&mut frame);
    frame.materials.push(flat_material([1.0, 0.0, 0.0, 1.0])); // index 0: red, the near box
    frame.materials.push(flat_material([0.0, 1.0, 0.0, 1.0])); // index 1: green, the far floor
    // Submission order deliberately puts the *nearer* mesh first and the *farther* one second: a
    // painter's-algorithm bug (draw order deciding the pixel instead of the depth buffer) would
    // show the farther, later-drawn floor on top at the centre; a correct depth test keeps the
    // nearer box visible regardless of draw order.
    frame.meshes.push(mesh_instance(
        near_mesh,
        MaterialHandle(0),
        translation([0.0, 0.0, 2.0]),
    ));
    frame.meshes.push(mesh_instance(
        far_mesh,
        MaterialHandle(1),
        translation([0.0, 0.0, -5.0]),
    ));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.meshes_drawn, 2, "both meshes were registered above");
    assert_eq!(stats.meshes_rejected_layer, 0);
    assert_eq!(stats.meshes_rejected_invalid, 0);
    assert_eq!(
        stats.meshes_rejected_unregistered, 0,
        "a registered mesh must still count as drawn (contract §6, PO decision V-20)"
    );
    assert_eq!(
        stats.base.draw_calls, 2,
        "one draw call per distinct mesh handle, no sprites this frame"
    );

    let image = renderer.read_offscreen_rgba().expect("read-back");
    let center = pixel(&image, SIZE, SIZE / 2, SIZE / 2);
    assert_near(
        center,
        [255, 0, 0, 255],
        20,
        "centre pixel: the nearer box must occlude the farther floor",
    );

    // World (5, 0, -5) is outside the box's XY footprint ([-1, 1]^2 around the origin) but still
    // on the floor; project it to a pixel with the same maths `top_down_camera`'s doc comment
    // describes (depth = 10 - (-5) = 15).
    let tan_half_fov: f32 = 25.0_f32.to_radians().tan();
    let ndc_x = 5.0 / (15.0 * tan_half_fov);
    let off_center_x = (((ndc_x + 1.0) * 0.5 * SIZE as f32) as u32).min(SIZE - 1);
    let off_center = pixel(&image, SIZE, off_center_x, SIZE / 2);
    assert_near(
        off_center,
        [0, 255, 0, 255],
        20,
        "outside the box's footprint: the floor must be visible, unoccluded",
    );
}

#[test]
fn render_stage_still_draws_sprites_on_top_of_the_mesh_pass() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mesh = renderer
        .register_mesh(altar_block(2.0, 2.0, 2.0))
        .expect("valid mesh");

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.base.camera = Camera2D {
        center: [0.0, 0.0],
        world_height: 100.0,
    };
    frame.camera_25d = Some(top_down_camera());
    full_bright_lighting(&mut frame);
    frame.materials.push(flat_material([1.0, 0.0, 0.0, 1.0]));
    frame.meshes.push(mesh_instance(
        mesh,
        MaterialHandle(0),
        translation([0.0, 0.0, 2.0]),
    ));
    // Far corner of the sprite camera's view, well outside the mesh's small screen footprint at
    // the centre: proves the (otherwise unchanged) sprite pass still draws, on top, after the new
    // mesh pass — not merely that `render_stage` no longer errors.
    frame
        .base
        .sprites
        .push(circle([-40.0, 40.0], 8.0, [0.0, 0.0, 1.0, 1.0]));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.base.sprites_drawn, 1);
    assert_eq!(stats.meshes_drawn, 1);
    assert_eq!(
        stats.base.draw_calls, 2,
        "one mesh draw call plus one sprite draw call"
    );

    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_near(
        pixel(&image, SIZE, 6, 6),
        [0, 0, 255, 255],
        40,
        "sprite corner, drawn after (on top of) the mesh pass",
    );
    assert_near(
        pixel(&image, SIZE, SIZE / 2, SIZE / 2),
        [255, 0, 0, 255],
        20,
        "mesh centre, unaffected by the sprite pass drawn on top of it",
    );
}

// --- PBR shading (WP2.5): point lights, metal vs. dielectric, specular anti-aliasing, textures --

/// Perceptually-irrelevant but monotonic brightness measure, good enough to rank pixels or compare
/// two renders of the same scene; not a real luminance transform.
fn luminance(pixel: [u8; 4]) -> u32 {
    2 * u32::from(pixel[0]) + 4 * u32::from(pixel[1]) + u32::from(pixel[2])
}

/// A white point light, not a bullet light (contract §6 `PointLight`). Built from
/// [`PointLight::default`] rather than a struct literal: it is `#[non_exhaustive]` (contract §2
/// rule 13), so an external crate like this integration test cannot construct one with a literal
/// at all, even with `..Default::default()`.
fn point_light(position: [f32; 3], range: f32, intensity: f32) -> PointLight {
    let mut light = PointLight::default();
    light.position = position;
    light.color = [1.0, 1.0, 1.0];
    light.intensity = intensity;
    light.range = range;
    light
}

/// No key light, no ambient: only whatever [`PointLight`]s a test adds can light anything, so an
/// unlit pixel is exactly the clear colour.
fn dark_scene(camera: Camera25D) -> StageFrame {
    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.camera_25d = Some(camera);
    frame.ambient = AmbientLight::Flat {
        color: [0.0, 0.0, 0.0],
        intensity: 0.0,
    };
    frame
}

#[test]
fn point_light_range_is_a_hard_cutoff() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let floor = renderer
        .register_mesh(floor_tile_grid(4, 20.0))
        .expect("valid mesh");
    let mut material = PbrMaterial::default();
    material.base_color_factor = [1.0, 1.0, 1.0, 1.0];
    material.metallic_factor = 0.0;
    material.roughness_factor = 0.8;

    let mut frame = dark_scene(top_down_camera());
    frame.materials.push(material);
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    // The centre pixel's world point is (0, 0, 0) (`top_down_camera`'s doc comment); a light
    // straight above it at height `h` is exactly `h` world units from that point.
    frame
        .point_lights
        .push(point_light([0.0, 0.0, 8.0], 5.0, 100.0)); // 8 > range 5: out of range

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(
        stats.point_lights_drawn, 1,
        "structurally valid (contract §6 PointLight::is_valid), even though it lights nothing here"
    );
    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_near(
        pixel(&image, SIZE, SIZE / 2, SIZE / 2),
        [0, 0, 0, 255],
        3,
        "beyond range: zero contribution, not just a small one",
    );

    // The same light, moved within range: now it must light the floor.
    frame.point_lights[0].position = [0.0, 0.0, 3.0]; // 3 < range 5
    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.point_lights_drawn, 1);
    let image = renderer.read_offscreen_rgba().expect("read-back");
    let lit = pixel(&image, SIZE, SIZE / 2, SIZE / 2);
    assert!(
        lit[0] > 30,
        "within range: the floor must be visibly lit, got {lit:?}"
    );
}

#[test]
fn point_light_specular_highlight_tracks_the_lights_horizontal_offset() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mesh = renderer
        .register_mesh(icosphere(3, 2.0))
        .expect("valid mesh");
    let mut material = PbrMaterial::default();
    material.base_color_factor = [0.9, 0.9, 0.9, 1.0];
    material.metallic_factor = 1.0;
    material.roughness_factor = 0.3;

    let mut frame = dark_scene(top_down_camera());
    frame.materials.push(material);
    frame.meshes.push(mesh_instance(
        mesh,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    // Offset well to the +X side and above: the specular lobe peaks where the surface normal
    // bisects the view and light directions, which (for a camera looking straight down) tilts the
    // highlight toward the light's horizontal offset.
    frame
        .point_lights
        .push(point_light([8.0, 0.0, 8.0], 40.0, 40.0));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.point_lights_drawn, 1);
    let image = renderer.read_offscreen_rgba().expect("read-back");
    let mut brightest = (0u32, 0u32, 0u32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let l = luminance(pixel(&image, SIZE, x, y));
            if l > brightest.2 {
                brightest = (x, y, l);
            }
        }
    }
    assert!(
        brightest.0 > SIZE / 2,
        "expected the highlight on the +X (light) side of the sphere, brightest pixel at x={} (centre {})",
        brightest.0,
        SIZE / 2
    );
}

/// Renders a single sphere with the given material parameters, lit by one `light`, on an
/// otherwise unlit background, and returns the brightest pixel's [`luminance`] — `None` if no GPU
/// adapter is available (the caller must then skip, like every other offscreen test).
fn render_sphere_peak_luminance(metallic: f32, roughness: f32, light: PointLight) -> Option<u32> {
    let mut renderer = offscreen_renderer(SIZE, SIZE, 16)?;
    let mesh = renderer
        .register_mesh(icosphere(3, 2.0))
        .expect("valid mesh");
    let mut material = PbrMaterial::default();
    material.base_color_factor = [0.8, 0.8, 0.8, 1.0];
    material.metallic_factor = metallic;
    material.roughness_factor = roughness;

    let mut frame = dark_scene(top_down_camera());
    frame.materials.push(material);
    frame.meshes.push(mesh_instance(
        mesh,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    frame.point_lights.push(light);

    renderer.render_stage(&frame).expect("render_stage");
    let image = renderer.read_offscreen_rgba().expect("read-back");
    Some(
        (0..image.len() / 4)
            .map(|i| {
                luminance([
                    image[i * 4],
                    image[i * 4 + 1],
                    image[i * 4 + 2],
                    image[i * 4 + 3],
                ])
            })
            .max()
            .unwrap_or(0),
    )
}

#[test]
fn metal_sphere_has_a_brighter_specular_peak_than_a_dielectric_sphere_at_matched_settings() {
    // Aligned camera-above/light-above geometry (like `point_light_range_is_a_hard_cutoff`'s
    // centre pixel), so both spheres' highlights land near the same, easily comparable point: a
    // metal's Fresnel reflectance F0 *is* its albedo (0.8 here), a dielectric's is a fixed 0.04 —
    // a 20x difference in the (shared) GGX specular term, which a dielectric's diffuse term (which
    // a metal has none of) cannot make up at this roughness.
    let light = point_light([0.0, 0.0, 8.0], 40.0, 0.3);
    let Some(metal_peak) = render_sphere_peak_luminance(1.0, 0.4, light) else {
        return;
    };
    let Some(dielectric_peak) = render_sphere_peak_luminance(0.0, 0.4, light) else {
        return;
    };
    assert!(
        metal_peak > dielectric_peak,
        "metal peak luminance {metal_peak} must exceed the dielectric's {dielectric_peak} at matched albedo/roughness/light"
    );
}

#[test]
fn specular_anti_aliasing_changes_the_highlight_but_leaves_the_background_alone() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mesh = renderer
        .register_mesh(icosphere(3, 2.0))
        .expect("valid mesh");
    let mut material = PbrMaterial::default();
    material.base_color_factor = [0.8, 0.8, 0.8, 1.0];
    material.metallic_factor = 1.0;
    // Sharp highlight: geometric specular AA (OF-3.5) widens the roughness the most here, so any
    // effect is easiest to see.
    material.roughness_factor = 0.05;

    let mut frame = dark_scene(top_down_camera());
    frame.materials.push(material);
    frame.meshes.push(mesh_instance(
        mesh,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    frame
        .point_lights
        .push(point_light([0.0, 0.0, 8.0], 40.0, 0.05));

    renderer
        .render_stage_with_specular_aa(&frame, true)
        .expect("render with AA");
    let with_aa = renderer.read_offscreen_rgba().expect("read-back");
    renderer
        .render_stage_with_specular_aa(&frame, false)
        .expect("render without AA");
    let without_aa = renderer.read_offscreen_rgba().expect("read-back");

    assert_eq!(with_aa.len(), without_aa.len());
    let (with_aa_pixels, _) = with_aa.as_chunks::<4>();
    let (without_aa_pixels, _) = without_aa.as_chunks::<4>();
    let differing = with_aa_pixels
        .iter()
        .zip(without_aa_pixels)
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 2))
        .count();
    assert!(
        differing > 0,
        "expected specular AA to change at least some pixels near the highlight"
    );
    // Layers 4 (telegraphy) and 6 (bullets, PRD-0003 rule 1) have no channel in this pass at all
    // (contract §6: the mesh pass only ever draws `RenderLayer::World`), so they are untouched by
    // construction; this checks the analogous claim the mesh pass itself can make: pixels far
    // outside any specular content (here, the clear-coloured background corners) are unaffected.
    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)] {
        assert_eq!(
            pixel(&with_aa, SIZE, x, y),
            pixel(&without_aa, SIZE, x, y),
            "background corners must be unaffected by the specular-AA toggle"
        );
    }
}

// --- Shadows (plan 0002 WP2.6, OF-3.2): key-light shadow map and blob shadows ------------------

#[test]
fn key_light_shadow_map_darkens_the_floor_behind_an_occluder() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let floor = renderer
        .register_mesh(floor_tile_grid(4, 20.0))
        .expect("valid mesh");
    let occluder = renderer
        .register_mesh(altar_block(2.0, 2.0, 2.0))
        .expect("valid mesh");

    let mut floor_material = PbrMaterial::default();
    floor_material.base_color_factor = [1.0, 1.0, 1.0, 1.0];
    floor_material.metallic_factor = 0.0;
    floor_material.roughness_factor = 0.9;

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.camera_25d = Some(top_down_camera());
    frame.ambient = AmbientLight::Flat {
        color: [0.0, 0.0, 0.0],
        intensity: 0.0,
    };
    let mut key_light = DirectionalLight::default();
    // Travels down and to the +X side, at 45 degrees: the occluder's shadow falls on the floor's
    // +X side, away from the light, where the top-down camera can still see it (unlike a straight-
    // down light, whose shadow would hide directly under the occluder, out of view).
    key_light.direction = [1.0, 0.0, -1.0];
    key_light.color = [1.0, 1.0, 1.0];
    key_light.intensity = 4.0;
    frame.key_light = Some(key_light);
    frame.materials.push(floor_material);
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    // Sits on the floor (bottom at Z = 0, like the floor itself): `altar_block(2, 2, 2)` is
    // centred on the origin with half-height 1, so a Z translation of 1 puts its base at Z = 0.
    frame.meshes.push(mesh_instance(
        occluder,
        MaterialHandle(0),
        translation([0.0, 0.0, 1.0]),
    ));

    frame.shadow_config.mode = ShadowMode::None;
    let stats_none = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(
        stats_none.shadow_casters_drawn, 0,
        "ShadowMode::None never wants a key-light shadow map"
    );
    let without_shadow = renderer.read_offscreen_rgba().expect("read-back");

    frame.shadow_config.mode = ShadowMode::KeyLight;
    let stats_key_light = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(
        stats_key_light.shadow_casters_drawn, 2,
        "both the floor and the occluder are valid, registered casters"
    );
    let with_shadow = renderer.read_offscreen_rgba().expect("read-back");

    // Layers 4 (telegraphy) and 6 (bullets) have no channel in this pass at all (contract §6: the
    // mesh pass only ever draws `RenderLayer::World`), so PRD-0003 rule 1 is unaffected by
    // construction, the same reasoning `specular_anti_aliasing_...` already relies on above.
    let mut darkened = 0usize;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let before = luminance(pixel(&without_shadow, SIZE, x, y));
            let after = luminance(pixel(&with_shadow, SIZE, x, y));
            if after + 10 < before {
                darkened += 1;
            }
        }
    }
    assert!(
        darkened > 0,
        "expected the key-light shadow map to measurably darken at least one floor pixel"
    );
}

#[test]
fn shadow_mode_none_never_samples_the_shadow_map_even_with_casters_present() {
    // A structural counterpart to the pixel-based test above: with the exact same occluder scene,
    // `ShadowMode::None` must report zero shadow casters regardless of how many meshes could have
    // cast a shadow, and the renderer must not fail or panic while a shadow-casting scene exists
    // but shadows are switched off (the shadow map render is skipped, not merely unused).
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let floor = renderer
        .register_mesh(floor_tile_grid(4, 20.0))
        .expect("valid mesh");
    let mut frame = StageFrame::new();
    frame.camera_25d = Some(top_down_camera());
    frame.key_light = Some(DirectionalLight::default());
    frame.materials.push(flat_material([1.0, 1.0, 1.0, 1.0]));
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    frame.shadow_config.mode = ShadowMode::None;

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.meshes_drawn, 1);
    assert_eq!(stats.shadow_casters_drawn, 0);
}

#[test]
fn blob_shadow_darkens_the_ground_under_the_disc() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let floor = renderer
        .register_mesh(floor_tile_grid(4, 20.0))
        .expect("valid mesh");

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.camera_25d = Some(top_down_camera());
    full_bright_lighting(&mut frame);
    frame.materials.push(flat_material([1.0, 1.0, 1.0, 1.0]));
    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    frame.blob_shadows.push(BlobShadowInstance {
        position: [0.0, 0.0],
        radius: 2.0,
        softness: 0.3,
        strength: 0.9,
    });

    frame.shadow_config.mode = ShadowMode::None;
    let stats_none = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(
        stats_none.blob_shadows_drawn, 1,
        "counted as a valid instance regardless of whether the mode actually draws it, like \
         `StageStats::meshes_drawn` counting a mesh a renderer without a registry cannot draw"
    );
    let without_blob = renderer.read_offscreen_rgba().expect("read-back");

    frame.shadow_config.mode = ShadowMode::Blob;
    let stats_blob = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats_blob.blob_shadows_drawn, 1);
    let with_blob = renderer.read_offscreen_rgba().expect("read-back");

    let before = pixel(&without_blob, SIZE, SIZE / 2, SIZE / 2);
    let after = pixel(&with_blob, SIZE, SIZE / 2, SIZE / 2);
    assert!(
        luminance(after) + 20 < luminance(before),
        "expected the blob shadow to darken the centre pixel: before {before:?}, after {after:?}"
    );
    // Regression against `blob_shadow.wgsl` accidentally writing outside the disc: a far corner,
    // well beyond `radius`, must stay unaffected.
    assert_eq!(
        pixel(&without_blob, SIZE, 2, 2),
        pixel(&with_blob, SIZE, 2, 2),
        "a corner far outside the disc's radius must be untouched"
    );
}

#[test]
fn base_color_texture_tints_the_material_factor() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mesh = renderer
        .register_mesh(altar_block(2.0, 2.0, 2.0))
        .expect("valid mesh");
    let texture = renderer
        .register_texture(TextureData {
            width: 1,
            height: 1,
            pixels: vec![0, 128, 0, 255],
            color_space: TextureColorSpace::Srgb,
        })
        .expect("valid texture");

    let mut material = PbrMaterial::default();
    material.base_color_factor = [1.0, 1.0, 1.0, 1.0];
    // Metallic, like `flat_material`: isolates the texture's own colour as F0 (see
    // `full_bright_lighting`'s doc comment) instead of mixing in a grey dielectric specular tint.
    material.metallic_factor = 1.0;
    material.roughness_factor = 1.0;
    material.base_color_texture = Some(texture);

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.camera_25d = Some(top_down_camera());
    full_bright_lighting(&mut frame);
    frame.materials.push(material);
    frame.meshes.push(mesh_instance(
        mesh,
        MaterialHandle(0),
        translation([0.0, 0.0, 2.0]),
    ));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.meshes_drawn, 1);
    let image = renderer.read_offscreen_rgba().expect("read-back");
    let center = pixel(&image, SIZE, SIZE / 2, SIZE / 2);
    assert!(
        center[1] > center[0] && center[1] > center[2],
        "the texture's green must dominate: {center:?}"
    );
    assert!(
        center[0] < 40 && center[2] < 40,
        "red/blue must stay near zero (F0 is zero on those channels): {center:?}"
    );
}

#[test]
fn unregistered_texture_handle_falls_back_to_the_material_factor() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mesh = renderer
        .register_mesh(altar_block(2.0, 2.0, 2.0))
        .expect("valid mesh");

    let mut material = flat_material([1.0, 0.0, 0.0, 1.0]);
    // Never registered with this renderer (its texture registry is empty): contract-wise
    // indistinguishable from `None` (see `mesh.wgsl`'s header comment), not a validation error.
    material.base_color_texture = Some(TextureHandle(0));

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.camera_25d = Some(top_down_camera());
    full_bright_lighting(&mut frame);
    frame.materials.push(material);
    frame.meshes.push(mesh_instance(
        mesh,
        MaterialHandle(0),
        translation([0.0, 0.0, 2.0]),
    ));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(
        stats.meshes_drawn, 1,
        "an unregistered *texture* handle is never a rejection reason"
    );
    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_near(
        pixel(&image, SIZE, SIZE / 2, SIZE / 2),
        [255, 0, 0, 255],
        20,
        "falls back to the plain red factor, exactly like flat_material's other tests",
    );
}

/// A flat quad facing world `+Z` (normal `[0, 0, 1]`), UV-mapped `(0,0)`-`(1,1)`, tangent
/// `[1, 0, 0, handedness]` on every vertex — the smallest mesh that gives `mesh.wgsl`'s real
/// tangent-space TBN branch (`abs(tangent.w) >= 0.5`) something to build a bitangent from.
fn tangent_quad(handedness: f32) -> MeshData {
    let corners: [([f32; 3], [f32; 2]); 4] = [
        ([-0.5, -0.5, 0.0], [0.0, 0.0]),
        ([0.5, -0.5, 0.0], [1.0, 0.0]),
        ([0.5, 0.5, 0.0], [1.0, 1.0]),
        ([-0.5, 0.5, 0.0], [0.0, 1.0]),
    ];
    let vertices = corners
        .into_iter()
        .map(|(position, uv)| MeshVertex {
            tangent: [1.0, 0.0, 0.0, handedness],
            ..MeshVertex::new(position, [0.0, 0.0, 1.0], uv)
        })
        .collect();
    MeshData {
        vertices,
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

/// Texture-quality package strand B2's mandatory proof ("ein Test ... prüft, dass die Bitangente
/// wirklich umklappt" — a test proving the bitangent really flips): [`tangent_quad`]'s tangent is
/// `[1, 0, 0, w]`, so `bitangent = cross(normal, tangent.xyz) * w = [0, w, 0]` (`mesh.wgsl`) —
/// exactly world `+Y` for `w = 1` and `-Y` for `w = -1`, with `normal = [0, 0, 1]` unaffected
/// either way. A 1x1 normal map encoding the fixed tangent-space normal `(0, 0.6, 0.8)` (already
/// unit length, packed the way a sampled normal map is decoded, `* 2 - 1`) therefore shades to
/// world normal `(0, 0.6, 0.8)` under `w = 1` but `(0, -0.6, 0.8)` under `w = -1`. A key light
/// travelling `-Y` (so its "towards" vector is exactly `+Y`) then lands on opposite sides of
/// `mesh.wgsl`'s hard `dot(n, l) <= 0` cutoff for the two cases: `dot((0,0.6,0.8),(0,1,0)) = 0.6`
/// (lit) versus `dot((0,-0.6,0.8),(0,1,0)) = -0.6` (fully unlit, clipped to zero) — not a subtle
/// shading difference, a lit-or-not one, so this is a direct proof of the handedness flip rather
/// than a coincidental brightness change that some other bug could also produce.
#[test]
fn tangent_handedness_flips_the_bitangent() {
    let Some(mut renderer) = offscreen_renderer(64, 64, 4) else {
        return;
    };
    let normal_map = TextureData {
        width: 1,
        height: 1,
        // Tangent-space normal (0, 0.6, 0.8): x = 0 -> 128, y = 0.6 -> 204, z = 0.8 -> 230
        // (`(n * 0.5 + 0.5) * 255`, rounded).
        pixels: vec![128, 204, 230, 255],
        color_space: TextureColorSpace::Linear,
    };
    let normal_handle = renderer
        .register_texture(normal_map)
        .expect("valid texture");
    let mesh_lit = renderer
        .register_mesh(tangent_quad(1.0))
        .expect("valid mesh");
    let mesh_unlit = renderer
        .register_mesh(tangent_quad(-1.0))
        .expect("valid mesh");

    let mut material = flat_material([0.8, 0.8, 0.8, 1.0]);
    material.metallic_factor = 0.0;
    material.roughness_factor = 0.8;
    material.normal_texture = Some(normal_handle);

    let render_with = |renderer: &mut WgpuRenderer, mesh: MeshHandle| -> [u8; 4] {
        let mut frame = StageFrame::new();
        frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
        frame.camera_25d = Some(top_down_camera());
        let mut key_light = DirectionalLight::default();
        // Travels -Y, so the "towards light" vector `mesh.wgsl` uses is exactly world +Y.
        key_light.direction = [0.0, -1.0, 0.0];
        key_light.color = [1.0, 1.0, 1.0];
        key_light.intensity = 4.0;
        frame.key_light = Some(key_light);
        frame.ambient = AmbientLight::Flat {
            color: [0.03, 0.03, 0.03],
            intensity: 1.0,
        };
        frame.materials.push(material);
        frame.meshes.push(mesh_instance(
            mesh,
            MaterialHandle(0),
            translation([0.0; 3]),
        ));
        renderer.render_stage(&frame).expect("render_stage");
        let rgba = renderer.read_offscreen_rgba().expect("read-back");
        pixel(&rgba, 64, 32, 32)
    };
    let lit = render_with(&mut renderer, mesh_lit);
    let unlit = render_with(&mut renderer, mesh_unlit);
    assert!(
        luminance(lit) > luminance(unlit) + 40,
        "handedness +1 should be lit by the key light and handedness -1 should not: \
         lit={lit:?} (luminance {}) unlit={unlit:?} (luminance {})",
        luminance(lit),
        luminance(unlit)
    );
}

// --- Bullet pass and pass graph (plan 0002 WP3.5) ------------------------------------------------

/// A hostile bullet of the given silhouette and palette; no glow, heading `+X`.
fn bullet(position: [f32; 2], radius: f32, silhouette: u16, palette: u16) -> BulletInstance {
    BulletInstance {
        position,
        radius,
        rotation: 0.0,
        silhouette,
        palette,
        palette_space: BULLET_PASS_PALETTE_SPACE,
        glow: 0,
        flags: 0,
    }
}

/// Stage frame with a flat 2D camera mapping the 64x64 target to world `[-32, 32]^2`, one world
/// unit per pixel, over a black clear colour.
fn flat_bullet_stage() -> StageFrame {
    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.base.camera = Camera2D {
        center: [0.0, 0.0],
        world_height: SIZE as f32,
    };
    frame
}

/// Pixel whose centre is the given world point under [`flat_bullet_stage`]'s camera (Y up).
fn flat_pixel(image: &[u8], world: [f32; 2]) -> [u8; 4] {
    let x = (world[0] + SIZE as f32 / 2.0).floor() as u32;
    let y = (SIZE as f32 / 2.0 - world[1]).floor() as u32;
    pixel(image, SIZE, x.min(SIZE - 1), y.min(SIZE - 1))
}

#[test]
fn bullet_pass_draws_core_body_and_rim_on_top_of_the_world_layer() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = flat_bullet_stage();
    // A dark blue world sprite covering the whole target: layer 1-3 content under the bullet.
    frame.base.sprites.push(SpriteInstance {
        position: [0.0, 0.0],
        half_size: [40.0, 40.0],
        rotation: 0.0,
        shape: shape::QUAD,
        color: [0.0, 0.0, 0.25, 1.0],
    });
    frame.bullets.push(bullet(
        [0.5, 0.5],
        20.0,
        bullet_silhouette::ORB,
        bullet_palette::HEX_MAGENTA,
    ));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.bullets_drawn, 1);
    assert_eq!(stats.bullets_rejected_invalid, 0);
    assert_eq!(
        stats.base.draw_calls, 3,
        "world sprites plus the bullet pass's halo and body draws"
    );
    assert_eq!(
        renderer.last_stage_pass_order(),
        RenderLayer::ORDER.as_slice(),
        "the pass graph executed every slot in the contract's order"
    );

    let image = renderer.read_offscreen_rgba().expect("read-back");
    // Core: stylebook `#FFE3F4`, white-hot centre.
    assert_near(
        flat_pixel(&image, [0.5, 0.5]),
        [255, 227, 244, 255],
        12,
        "bullet core",
    );
    // Body near the edge: stylebook H0 `#FF2FB4`, magenta.
    let body = flat_pixel(&image, [0.5, 17.5]);
    assert!(
        body[0] > 220 && body[1] < 130 && body[2] > 150,
        "bullet body should be magenta, got {body:?}"
    );
    // Rim just outside the radius: dark `#0A0510` at 0.9 opacity over the blue world sprite. The
    // rim is only 1.5 px wide and anti-aliased, so how much of it one pixel centre catches differs
    // between adapters (WARP, lavapipe); take the darkest pixel on the ray from the body edge
    // outwards instead of one fixed pixel.
    let rim = (0..5)
        .map(|step| flat_pixel(&image, [0.5, 19.5 + step as f32]))
        .min_by_key(|pixel| luminance(*pixel))
        .expect("five probes");
    // A 0.9-opaque rim leaves a tenth of the blue sprite (sRGB 137) showing: about 48 at full
    // coverage, higher where a pixel centre catches only part of the rim.
    assert!(
        rim[0] < 30 && rim[1] < 30 && rim[2] < 137 - 50,
        "dark rim outside the body, got {rim:?}"
    );
    // Far from the bullet: the world layer, untouched.
    assert_near(
        flat_pixel(&image, [-29.5, -29.5]),
        [0, 0, 137, 255],
        3,
        "world sprite far away from the bullet",
    );
}

#[test]
fn player_marker_and_debug_sprites_are_drawn_on_top_of_bullets_in_that_order() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = flat_bullet_stage();
    frame.bullets.push(bullet(
        [0.5, 0.5],
        20.0,
        bullet_silhouette::ORB,
        bullet_palette::POISON_LIME,
    ));
    frame
        .marker_sprites
        .push(circle([0.5, 0.5], 8.0, [0.0, 1.0, 0.0, 1.0]));
    frame.debug_sprites.push(SpriteInstance {
        position: [0.5, 0.5],
        half_size: [2.0, 2.0],
        rotation: 0.0,
        shape: shape::QUAD,
        color: [1.0, 0.0, 0.0, 1.0],
    });

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.base.sprites_drawn, 2, "marker plus debug sprite");
    assert_eq!(
        stats.base.draw_calls, 4,
        "bullets (halos, bodies), marker and debug; the empty world layer issues none"
    );

    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert_near(
        flat_pixel(&image, [0.5, 0.5]),
        [255, 0, 0, 255],
        3,
        "debug/UI on top of everything",
    );
    assert_near(
        flat_pixel(&image, [0.5, 5.5]),
        [0, 255, 0, 255],
        3,
        "layer 7 marker on top of layer 6 bullet",
    );
    let body = flat_pixel(&image, [0.5, 16.5]);
    assert!(
        body[1] > 220 && body[2] < 140,
        "the lime bullet body stays visible outside the marker, got {body:?}"
    );
}

/// Renders one bullet of `silhouette` (radius 20, heading `rotation`) and reports whether the
/// pixels at each silhouette-local point (units of the radius, `+X` = the unrotated heading) are
/// covered by the bullet body or core rather than the black background.
fn silhouette_coverage(
    renderer: &mut WgpuRenderer,
    silhouette: u16,
    rotation: f32,
    probes: &[[f32; 2]],
) -> Vec<bool> {
    let mut frame = flat_bullet_stage();
    let mut instance = bullet([0.5, 0.5], 20.0, silhouette, bullet_palette::HEX_MAGENTA);
    instance.rotation = rotation;
    frame.bullets.push(instance);
    renderer.render_stage(&frame).expect("render_stage");
    let image = renderer.read_offscreen_rgba().expect("read-back");
    probes
        .iter()
        .map(|local| {
            let p = flat_pixel(&image, [0.5 + local[0] * 20.0, 0.5 + local[1] * 20.0]);
            // Body and core are bright in red; background and rim are not.
            p[0] > 150
        })
        .collect()
}

#[test]
fn silhouettes_differ_in_outline_not_only_in_colour() {
    // PRD-0003 rule 3 on real pixels: the same colour, three different outlines.
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    // Probes in silhouette-local units: across the heading, off-axis, along the heading.
    let probes = [[0.0, 0.75], [0.7, 0.3], [0.85, 0.0]];
    let orb = silhouette_coverage(&mut renderer, bullet_silhouette::ORB, 0.0, &probes);
    let rice = silhouette_coverage(&mut renderer, bullet_silhouette::RICE, 0.0, &probes);
    let diamond = silhouette_coverage(&mut renderer, bullet_silhouette::DIAMOND, 0.0, &probes);
    assert_eq!(orb, [true, true, true], "orb: a full disc");
    assert_eq!(
        rice,
        [false, true, true],
        "rice: long along the heading, thin across"
    );
    assert_eq!(
        diamond,
        [false, false, true],
        "diamond: tips on the axes only"
    );

    // The heading turns the silhouette: a rice grain flying along +Y is long vertically.
    let turned = silhouette_coverage(
        &mut renderer,
        bullet_silhouette::RICE,
        std::f32::consts::FRAC_PI_2,
        &[[0.0, 0.85], [0.75, 0.0]],
    );
    assert_eq!(turned, [true, false], "rice turned by 90 degrees");
}

#[test]
fn glow_draws_a_halo_outside_the_rim() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut render_halo = |glow: u8| {
        let mut frame = flat_bullet_stage();
        let mut instance = bullet(
            [0.5, 0.5],
            12.0,
            bullet_silhouette::ORB,
            bullet_palette::HEX_MAGENTA,
        );
        instance.glow = glow;
        frame.bullets.push(instance);
        let stats = renderer.render_stage(&frame).expect("render_stage");
        let image = renderer.read_offscreen_rgba().expect("read-back");
        // 1.35 radii from the centre: beyond the body and the 1.5 px rim, inside the glow reach.
        (stats, flat_pixel(&image, [0.5 + 16.0, 0.5]))
    };
    let (dark_stats, dark) = render_halo(0);
    let (lit_stats, lit) = render_halo(255);
    assert_near(
        dark,
        [0, 0, 0, 255],
        2,
        "no glow: background outside the rim",
    );
    assert!(
        luminance(lit) > luminance(dark) + 60 && lit[0] > lit[1],
        "full glow: a magenta halo outside the rim, got {lit:?}"
    );
    assert_eq!(dark_stats.bullet_point_lights_drawn, 0, "no glow, no light");
    assert_eq!(
        lit_stats.bullet_point_lights_drawn, 1,
        "a glowing bullet yields one bullet-cloud light"
    );
}

#[test]
fn rejected_bullets_are_skipped_and_the_rest_still_drawn() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut frame = flat_bullet_stage();
    frame.bullets.push(bullet(
        [-15.5, 0.5],
        8.0,
        bullet_silhouette::ORB,
        bullet_palette::HEX_MAGENTA,
    ));
    // Structurally invalid (no debug assertion): out-of-table silhouette, then a zero radius.
    frame.bullets.push(bullet(
        [0.5, 0.5],
        8.0,
        bullet_silhouette::COUNT,
        bullet_palette::HEX_MAGENTA,
    ));
    frame.bullets.push(bullet(
        [0.5, 20.5],
        0.0,
        bullet_silhouette::ORB,
        bullet_palette::HEX_MAGENTA,
    ));
    frame.bullets.push(bullet(
        [16.5, 0.5],
        8.0,
        bullet_silhouette::DIAMOND,
        bullet_palette::POISON_LIME,
    ));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.bullets_drawn, 2);
    assert_eq!(stats.bullets_rejected_invalid, 2);
    let image = renderer.read_offscreen_rgba().expect("read-back");
    assert!(
        flat_pixel(&image, [-15.5, 0.5])[0] > 200,
        "left bullet drawn"
    );
    assert!(
        flat_pixel(&image, [16.5, 0.5])[1] > 200,
        "right bullet drawn"
    );
    assert_near(
        flat_pixel(&image, [0.5, 0.5]),
        [0, 0, 0, 255],
        2,
        "the out-of-table instance draws nothing",
    );
}

#[test]
fn bullets_under_the_tilted_camera_sit_on_their_ground_position() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let mut camera = Camera25D::default();
    camera.target = [1.0, 2.0];
    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
    frame.camera_25d = Some(camera);
    let ground = [3.0, 4.0];
    frame.bullets.push(bullet(
        ground,
        2.5,
        bullet_silhouette::ORB,
        bullet_palette::POISON_LIME,
    ));

    let stats = renderer.render_stage(&frame).expect("render_stage");
    assert_eq!(stats.bullets_drawn, 1);
    let image = renderer.read_offscreen_rgba().expect("read-back");
    let [sx, sy] = camera
        .ground_to_screen(ground, [SIZE as f32, SIZE as f32])
        .expect("the bullet is in view");
    let centre = pixel(&image, SIZE, sx as u32, sy as u32);
    assert_near(
        centre,
        [246, 255, 224, 255],
        40,
        "the bullet core lands on the projected ground position",
    );
    // Mirror the bullet's screen position through the target centre: plain background there.
    let mirrored = pixel(&image, SIZE, SIZE - 1 - sx as u32, SIZE - 1 - sy as u32);
    assert_near(mirrored, [0, 0, 0, 255], 2, "nothing drawn elsewhere");
}

#[test]
fn glowing_bullets_light_the_floor_through_the_bullet_light_path() {
    let Some(mut renderer) = offscreen_renderer(SIZE, SIZE, 16) else {
        return;
    };
    let floor = renderer
        .register_mesh(floor_tile_grid(4, 40.0))
        .expect("valid mesh");
    let mut render_floor = |glow: u8| {
        let mut frame = StageFrame::new();
        frame.base.clear_color = [0.0, 0.0, 0.0, 1.0];
        frame.camera_25d = Some(top_down_camera());
        frame.ambient = AmbientLight::Flat {
            color: [1.0, 1.0, 1.0],
            intensity: 0.0,
        };
        let mut material = PbrMaterial::default();
        material.base_color_factor = [0.8, 0.8, 0.8, 1.0];
        material.metallic_factor = 0.0;
        material.roughness_factor = 0.9;
        frame.materials.push(material);
        frame.meshes.push(mesh_instance(
            floor,
            MaterialHandle(0),
            translation([0.0; 3]),
        ));
        let mut instance = bullet(
            [0.0, 0.0],
            0.3,
            bullet_silhouette::ORB,
            bullet_palette::HEX_MAGENTA,
        );
        instance.glow = glow;
        frame.bullets.push(instance);
        let stats = renderer.render_stage(&frame).expect("render_stage");
        let image = renderer.read_offscreen_rgba().expect("read-back");
        // World (1.5, 0) on the floor: beyond the small bullet's own halo, inside the light range.
        let tan_half_fov: f32 = 25.0_f32.to_radians().tan();
        let x = (((1.5 / (10.0 * tan_half_fov) + 1.0) * 0.5 * SIZE as f32) as u32).min(SIZE - 1);
        (stats, pixel(&image, SIZE, x, SIZE / 2))
    };
    let (dark_stats, dark) = render_floor(0);
    let (lit_stats, lit) = render_floor(255);
    assert_eq!(dark_stats.bullet_point_lights_drawn, 0);
    assert_eq!(lit_stats.bullet_point_lights_drawn, 1);
    assert_eq!(lit_stats.point_lights_drawn, 1);
    assert!(
        luminance(lit) > luminance(dark),
        "the derived, capped bullet light must reach the floor: dark {dark:?}, lit {lit:?}"
    );
}

/// Deterministic pseudo-random mesh field for the OF-3.5 stress scene: `count` small spheres
/// scattered across the same world area [`swarm`] covers, each with a distinct, plausible PBR
/// material so the specular term has real work to do across the scene.
fn mesh_swarm(renderer: &mut WgpuRenderer, count: usize) -> (Vec<MeshInstance>, Vec<PbrMaterial>) {
    let mesh = renderer
        .register_mesh(icosphere(1, 0.5))
        .expect("valid mesh");
    let mut state: u32 = 0x2545_F491;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    let mut meshes = Vec::with_capacity(count);
    let mut materials = Vec::with_capacity(count);
    for i in 0..count {
        let x = next() * 100.0 - 50.0;
        let y = next() * 100.0 - 50.0;
        let mut material = PbrMaterial::default();
        material.base_color_factor = [next(), next(), next(), 1.0];
        material.metallic_factor = next();
        material.roughness_factor = 0.05 + next() * 0.9;
        materials.push(material);
        meshes.push(mesh_instance(
            mesh,
            MaterialHandle(u32::try_from(i).unwrap_or(u32::MAX)),
            translation([x, y, 0.5]),
        ));
    }
    (meshes, materials)
}

/// OF-3.5 spike measurement (plan 0002 WP2.5): relative CPU-side frame cost and a shimmer proxy
/// (per-pixel luminance RMS difference between two frames rendered 0.5 degrees of camera tilt
/// apart, standing in for the frame-to-frame specular flicker TAA would otherwise smooth over) for
/// geometric specular anti-aliasing versus no anti-aliasing at all, on a stress scene of 500 PBR
/// meshes plus 10k sprites. Software-adapter timings only (labelled below), never a GPU budget; run
/// explicitly with `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test offscreen --
/// --ignored --nocapture measure_specular_aa`.
#[test]
#[ignore = "OF-3.5 spike measurement, run explicitly (see this test's doc comment)"]
fn measure_specular_aa_relative_cost_and_shimmer() {
    let Some(mut renderer) = offscreen_renderer(1280, 720, 16_384) else {
        return;
    };
    let (meshes, materials) = mesh_swarm(&mut renderer, 500);

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.02, 0.02, 0.03, 1.0];
    frame.base.camera = Camera2D {
        center: [0.0, 0.0],
        world_height: 100.0,
    };
    frame.base.sprites = swarm(10_000);
    frame.camera_25d = Some(top_down_camera());
    full_bright_lighting(&mut frame);
    frame.materials = materials;
    frame.meshes = meshes;
    for i in 0..8 {
        frame.point_lights.push(point_light(
            [(i as f32 - 4.0) * 12.0, (i as f32 * 7.0).sin() * 20.0, 6.0],
            25.0,
            2.0,
        ));
    }

    let frames = 30;
    for &specular_aa in &[true, false] {
        for _ in 0..5 {
            renderer
                .render_stage_with_specular_aa(&frame, specular_aa)
                .expect("warm-up render");
        }
        renderer.read_offscreen_rgba().expect("flush");
        let mut total = Duration::ZERO;
        for _ in 0..frames {
            let stats = renderer
                .render_stage_with_specular_aa(&frame, specular_aa)
                .expect("render");
            total += stats.base.cpu_time;
        }
        renderer.read_offscreen_rgba().expect("flush");
        println!(
            "backend {} (software adapter, NOT a GPU budget): specular_aa={specular_aa}, 500 meshes + 10k sprites, {frames} frames: cpu_time avg {:?}",
            renderer.backend_name(),
            total / frames
        );
    }

    let mut rotated_frame = frame.clone();
    if let Some(camera) = rotated_frame.camera_25d.as_mut() {
        camera.tilt_degrees += 0.5;
    }
    for &specular_aa in &[true, false] {
        renderer
            .render_stage_with_specular_aa(&frame, specular_aa)
            .expect("render a");
        let image_a = renderer.read_offscreen_rgba().expect("read-back a");
        renderer
            .render_stage_with_specular_aa(&rotated_frame, specular_aa)
            .expect("render b");
        let image_b = renderer.read_offscreen_rgba().expect("read-back b");
        let mut sum_sq_diff = 0f64;
        let mut count = 0f64;
        let (image_a_pixels, _) = image_a.as_chunks::<4>();
        let (image_b_pixels, _) = image_b.as_chunks::<4>();
        for (&a, &b) in image_a_pixels.iter().zip(image_b_pixels) {
            let la = f64::from(luminance(a));
            let lb = f64::from(luminance(b));
            sum_sq_diff += (la - lb) * (la - lb);
            count += 1.0;
        }
        let shimmer = (sum_sq_diff / count).sqrt();
        println!(
            "shimmer (RMS luminance difference between two 0.5-degree-rotated frames), specular_aa={specular_aa}: {shimmer:.3}"
        );
    }
}

/// Deterministic pseudo-random sprite field covering the view.
fn swarm(count: usize) -> Vec<SpriteInstance> {
    let mut state: u32 = 0x9E37_79B9;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    (0..count)
        .map(|i| SpriteInstance {
            position: [next() * 100.0 - 50.0, next() * 100.0 - 50.0],
            half_size: [0.5 + next(), 0.5 + next()],
            rotation: next() * std::f32::consts::TAU,
            shape: if i % 2 == 0 {
                shape::CIRCLE
            } else {
                shape::QUAD
            },
            color: [next(), next(), next(), 0.8],
        })
        .collect()
}

/// CPU frame-time measurement; run with
/// `cargo test --release -p grimoire_render --test offscreen -- --ignored --nocapture`.
#[test]
#[ignore = "performance measurement, run explicitly in release mode"]
fn measure_ten_thousand_sprite_cpu_time() {
    let Some(mut renderer) = offscreen_renderer(1280, 720, 16_384) else {
        return;
    };
    let mut frame = black_frame();
    frame.camera.world_height = 100.0;
    frame.sprites = swarm(10_000);
    for _ in 0..20 {
        renderer.render(&frame).expect("warm-up render");
    }
    renderer.read_offscreen_rgba().expect("flush");

    let frames = 500;
    let mut total = Duration::ZERO;
    let mut min = Duration::MAX;
    let mut max = Duration::ZERO;
    for _ in 0..frames {
        let stats = renderer.render(&frame).expect("render");
        total += stats.cpu_time;
        min = min.min(stats.cpu_time);
        max = max.max(stats.cpu_time);
    }
    let wall = std::time::Instant::now();
    renderer.read_offscreen_rgba().expect("flush");
    println!(
        "backend {}: 10k sprites @1280x720, {frames} frames: cpu_time avg {:?}, min {min:?}, max {max:?}; final flush {:?}",
        renderer.backend_name(),
        total / frames,
        wall.elapsed()
    );
}

// --- Texture-quality package, strand B1: mipmap and multisampling cost measurements -------------

fn offscreen_renderer_staged(
    width: u32,
    height: u32,
    initial_sprite_capacity: u32,
    msaa: Msaa,
) -> Option<WgpuRenderer> {
    // `StageRendererConfig` is `#[non_exhaustive]`: even `..Default::default()` struct-update
    // syntax is rejected outside its defining crate, so this builds it field by field instead
    // (the same pattern `snapshot_scenes.rs` already uses for other non-exhaustive/default types).
    let mut config = StageRendererConfig::default();
    config.base = RendererConfig {
        vsync: false,
        initial_sprite_capacity,
        allow_software_fallback: true,
    };
    config.msaa = msaa;
    match WgpuRenderer::new_offscreen_staged(width, height, config) {
        Ok(renderer) => {
            report_adapter_once(&renderer);
            Some(renderer)
        }
        Err(RenderError::NoAdapter) => {
            let required = std::env::var(ENV_REQUIRE_GPU_ADAPTER).ok();
            skip_without_adapter(adapter_required(required.as_deref()));
            None
        }
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

fn median(durations: &mut [Duration]) -> Duration {
    durations.sort();
    durations[durations.len() / 2]
}

/// Texture-quality package strand B1 measurement: relative CPU-side frame cost of the mesh pass's
/// 4x multisampling ([`Msaa::X4`], the default) against no multisampling ([`Msaa::Off`]), on the
/// same 500-mesh-plus-10k-sprite stress scene [`measure_specular_aa_relative_cost_and_shimmer`]
/// uses. Software-adapter timings only, never a GPU budget (see that test's doc comment for why);
/// run explicitly with `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test
/// offscreen -- --ignored --nocapture measure_msaa`.
#[test]
#[ignore = "texture-quality B1 measurement, run explicitly (see this test's doc comment)"]
fn measure_msaa_relative_cost() {
    for msaa in [Msaa::Off, Msaa::X4] {
        let Some(mut renderer) = offscreen_renderer_staged(1280, 720, 16_384, msaa) else {
            return;
        };
        let (meshes, materials) = mesh_swarm(&mut renderer, 500);

        let mut frame = StageFrame::new();
        frame.base.clear_color = [0.02, 0.02, 0.03, 1.0];
        frame.base.camera = Camera2D {
            center: [0.0, 0.0],
            world_height: 100.0,
        };
        frame.base.sprites = swarm(10_000);
        frame.camera_25d = Some(top_down_camera());
        full_bright_lighting(&mut frame);
        frame.materials = materials;
        frame.meshes = meshes;
        for i in 0..8 {
            frame.point_lights.push(point_light(
                [(i as f32 - 4.0) * 12.0, (i as f32 * 7.0).sin() * 20.0, 6.0],
                25.0,
                2.0,
            ));
        }

        for _ in 0..5 {
            renderer.render_stage(&frame).expect("warm-up render");
        }
        renderer.read_offscreen_rgba().expect("flush");
        // `RenderStats::cpu_time` only covers command *encoding*, not actual completion (`submit`
        // does not block) — MSAA's extra work happens on the WARP driver thread and would not show
        // up there at all. Wall-clock time around the whole batch, forced to completion by a final
        // `read_offscreen_rgba` (a blocking map-and-read), is what actually reflects it.
        let frames = 30;
        let mut cpu_time_total = Duration::ZERO;
        let wall_start = std::time::Instant::now();
        for _ in 0..frames {
            let stats = renderer.render_stage(&frame).expect("render");
            cpu_time_total += stats.base.cpu_time;
        }
        renderer.read_offscreen_rgba().expect("flush");
        let wall_total = wall_start.elapsed();
        println!(
            "backend {} (software adapter, NOT a GPU budget): msaa={msaa:?}, 500 meshes + 10k sprites, {frames} frames: cpu_time avg {:?}, wall-clock avg (incl. driver completion) {:?}",
            renderer.backend_name(),
            cpu_time_total / frames,
            wall_total / frames
        );
    }
}

/// Texture-quality package strand B1 measurement: relative cost (CPU box-filter generation plus
/// GPU upload of every level) of [`WgpuRenderer::register_texture`]'s full mip chain against
/// uploading only mip level 0
/// ([`WgpuRenderer::register_texture_single_level_for_measurement`], a measurement-only hook — see
/// its doc comment), for a few representative base-colour texture sizes. Median of 5 per-call
/// timings, software-adapter timings only. Run explicitly with `GRIMOIRE_GPU_ADAPTER=software
/// cargo test -p grimoire_render --test offscreen -- --ignored --nocapture measure_mipmap`.
#[test]
#[ignore = "texture-quality B1 measurement, run explicitly (see this test's doc comment)"]
fn measure_mipmap_generation_relative_cost() {
    let Some(mut renderer) = offscreen_renderer(64, 64, 16) else {
        return;
    };
    for &size in &[256u32, 512, 1024, 2048] {
        let pixels = vec![128u8; size as usize * size as usize * 4];
        let sample = || TextureData {
            width: size,
            height: size,
            pixels: pixels.clone(),
            color_space: TextureColorSpace::Srgb,
        };

        let mut with_mips = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            renderer.register_texture(sample()).expect("valid texture");
            with_mips.push(start.elapsed());
        }
        let mut without_mips = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            renderer
                .register_texture_single_level_for_measurement(sample())
                .expect("valid texture");
            without_mips.push(start.elapsed());
        }
        println!(
            "backend {} (software adapter, NOT a GPU budget): {size}x{size} base-colour texture, 5 calls: with mips median {:?}, without mips median {:?}",
            renderer.backend_name(),
            median(&mut with_mips),
            median(&mut without_mips),
        );
    }
}
