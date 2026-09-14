//! Offscreen rendering tests.
//!
//! Without any GPU adapter (not even a software one) they skip and pass, and print a GitHub
//! Actions warning that stays visible in the log. With `GRIMOIRE_REQUIRE_GPU_ADAPTER=1` a missing
//! adapter fails them instead; CI sets it wherever WARP or lavapipe guarantee an adapter.

use std::io::Write;
use std::sync::Once;
use std::time::Duration;

use grimoire_render::{
    Camera2D, RenderError, RenderFrame, Renderer, RendererConfig, SpriteInstance, WgpuRenderer,
    shape,
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

/// Fails the calling test if an adapter is required, otherwise announces the skip once.
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
        Ok(renderer) => Some(renderer),
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
