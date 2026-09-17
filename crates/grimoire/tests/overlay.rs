//! The stats overlay in the main loop (plan 0002 WP6.4, contract §9.2, §9.3, §9.7): the overlay key
//! toggles it in every frame loop, it goes into `StageFrame::debug_sprites`, and it never touches
//! simulation state.
//!
//! [`the_overlay_key_toggles_the_overlay_in_the_offscreen_image`] checks the rendered image through
//! `AppBuilder::run_offscreen`; skipped without an adapter, failing instead with
//! `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`.

mod common;

use std::cell::RefCell;
use std::io::Write as _;
use std::rc::Rc;
use std::time::Duration;

use common::Scenario;
use grimoire::platform::{PlatformEvent, RawInputEvent};
use grimoire::prelude::*;
use grimoire::render::{RenderError, StageRendererConfig};
use grimoire::{GrimoireError, OffscreenRun};

/// One 60 Hz tick per frame (see `tests/headless.rs`).
const ONE_TICK: Duration = Duration::from_nanos(16_666_667);

fn press(code: KeyCode) -> PlatformEvent {
    PlatformEvent::Input(RawInputEvent::Key {
        code,
        pressed: true,
        repeat: false,
    })
}

/// Sprite count of every frame (`FrameStats::render.sprites_drawn` counts the debug channel).
#[derive(Default)]
struct SpriteCounts(Rc<RefCell<Vec<u32>>>);

impl GamePlugin for SpriteCounts {
    fn name(&self) -> &str {
        "sprite_counts"
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        self.0.borrow_mut().push(stats.render.sprites_drawn);
    }
}

fn sprite_counts(builder: AppBuilder, script: &[(u64, KeyCode)]) -> Vec<u32> {
    let counts = SpriteCounts::default();
    let seen = Rc::clone(&counts.0);
    builder
        .plugin(counts)
        .run_headless_frames_with_events(8, ONE_TICK, &mut |frame, events| {
            for &(at, code) in script {
                if at == frame {
                    events.push(press(code));
                }
            }
        })
        .expect("runs");
    seen.take()
}

#[test]
fn f3_toggles_the_overlay_by_default() {
    let counts = sprite_counts(
        App::new(WindowConfig::default()),
        &[(2, KeyCode::F3), (5, KeyCode::F3)],
    );
    assert_eq!(&counts[..2], [0, 0], "hidden until the first press");
    assert!(
        counts[2..5].iter().all(|&count| count > 20),
        "visible from the frame of the press: {counts:?}"
    );
    assert_eq!(
        &counts[5..],
        [0, 0, 0],
        "hidden again after the second press"
    );
}

#[test]
fn the_overlay_key_can_be_changed_or_disabled() {
    let custom = sprite_counts(
        App::new(WindowConfig::default()).overlay_key(Some(KeyCode::F1)),
        &[(1, KeyCode::F3), (3, KeyCode::F1)],
    );
    assert_eq!(&custom[..3], [0, 0, 0], "F3 no longer toggles");
    assert!(custom[3] > 0, "F1 does: {custom:?}");

    let disabled = sprite_counts(
        App::new(WindowConfig::default()).overlay_key(None),
        &[(1, KeyCode::F3)],
    );
    assert!(disabled.iter().all(|&count| count == 0), "{disabled:?}");
}

#[test]
fn without_the_profiler_the_overlay_shows_only_its_header() {
    let profiled = sprite_counts(App::new(WindowConfig::default()), &[(0, KeyCode::F3)]);
    let header_only = sprite_counts(
        App::new(WindowConfig::default()).profiler(false),
        &[(0, KeyCode::F3)],
    );
    assert!(header_only[7] > 0, "the header is drawn: {header_only:?}");
    assert!(
        header_only[7] < profiled[7],
        "no scope rows without a profile: {header_only:?} vs {profiled:?}"
    );
}

#[test]
fn the_overlay_never_changes_a_state_hash() {
    let script = &mut |frame: u64, events: &mut Vec<PlatformEvent>| {
        if frame % 7 == 3 {
            events.push(press(KeyCode::F3));
        }
    };
    let with_overlay = App::new(WindowConfig::default())
        .seed(21)
        .hash_every(10)
        .plugin(Scenario { movers: 32 })
        .run_headless_frames_with_events(90, ONE_TICK, script)
        .expect("runs");
    let without = App::new(WindowConfig::default())
        .seed(21)
        .hash_every(10)
        .overlay_key(None)
        .plugin(Scenario { movers: 32 })
        .run_headless_frames_with_events(90, ONE_TICK, script)
        .expect("runs");
    assert_eq!(with_overlay, without);
}

// --- Offscreen ----------------------------------------------------------------------------------

/// Offscreen tests share one GPU device at a time (see `tests/asset_hook.rs`).
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_serial() -> std::sync::MutexGuard<'static, ()> {
    GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn adapter_required() -> bool {
    matches!(
        std::env::var("GRIMOIRE_REQUIRE_GPU_ADAPTER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true")
    )
}

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;

/// A flat clear colour for the whole stage, so any overlay pixel stands out.
struct Backdrop;

impl GamePlugin for Backdrop {
    fn name(&self) -> &str {
        "backdrop"
    }

    fn extract(&mut self, _world: &World, _alpha: f32, frame: &mut RenderFrame) {
        frame.clear_color = [0.3, 0.35, 0.45, 1.0];
    }
}

#[test]
fn the_overlay_key_toggles_the_overlay_in_the_offscreen_image() {
    let _serial = gpu_serial();
    let mut config = StageRendererConfig::default();
    config.base.vsync = false;
    config.base.allow_software_fallback = true;
    let mut run = OffscreenRun::new(WIDTH, HEIGHT, 4, ONE_TICK);
    run.capture_every = 1;
    let mut images: Vec<Vec<u8>> = Vec::new();
    let result = App::new(WindowConfig::default())
        .stage_renderer_config(config)
        .plugin(Backdrop)
        .run_offscreen(
            run,
            &mut |frame, events| {
                if frame == 1 || frame == 3 {
                    events.push(press(KeyCode::F3));
                }
            },
            &mut |_, rgba| images.push(rgba.to_vec()),
        );
    match result {
        Ok(_) => {}
        Err(GrimoireError::Render(RenderError::NoAdapter)) => {
            assert!(
                !adapter_required(),
                "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
            );
            let _ = writeln!(
                std::io::stdout(),
                "\n::warning title=GPU test skipped::no GPU adapter found; grimoire overlay rendered nothing"
            );
            return;
        }
        Err(error) => panic!("offscreen run failed: {error}"),
    }
    assert_eq!(images.len(), 4);
    let background = images[0][..4].to_vec();
    let covered = |image: &[u8]| {
        image
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[..] != background[..])
            .count()
    };
    assert_eq!(covered(&images[0]), 0, "frame 0: no overlay yet");
    for frame in [1, 2] {
        let pixels = covered(&images[frame]);
        assert!(
            pixels > 500,
            "frame {frame}: the overlay panel covers only {pixels} pixels"
        );
    }
    // The panel sits in the top-left corner: the bottom-right quarter stays untouched.
    let untouched = images[2]
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            let (x, y) = (*index as u32 % WIDTH, *index as u32 / WIDTH);
            x >= 140 && y >= 88
        })
        .all(|(_, pixel)| pixel[..] == background[..]);
    assert!(untouched, "the overlay stays in its panel");
    assert_eq!(covered(&images[3]), 0, "frame 3: hidden again");
}
