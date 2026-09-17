//! The M3 showcase (Plan 0002 WP8.5): the `sigil_curtain` scene of the M2 showcase, rendered
//! offscreen through the facade's own main loop while `grimoire-link` hot-swaps its pattern — the
//! before/after GIF of a save reaching a running engine.
//!
//! - [`the_hot_swap_changes_the_rendered_frames`] renders a few small frames and checks that the
//!   picture before and after the swap really differs (skipped without a GPU adapter, failing
//!   instead with `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`);
//! - [`render_hot_swap_png_sequence`] (`#[ignore]`, run explicitly) writes the showcase frame
//!   sequence as numbered PNGs, which the CI job `showcase-gif` assembles into the GIF:
//!   `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_link --test hot_swap_showcase --locked
//!   -- --ignored --nocapture render_hot_swap_png_sequence`. Frames go to
//!   `GRIMOIRE_HOT_SWAP_GIF_OUT_DIR` (default `target/wp85-hot-swap`).
//!
//! The scene is the example's own `scene.rs`, included with `#[path]` exactly as the M2 showcase
//! test includes it, so the GIF shows what the example draws. The tool runs on its own thread and
//! watches a copy of the scene's `.sigil` source under the same content path the checked-in unit
//! was compiled with; saving the edited source over it is an ordinary hot swap.

mod common;

#[path = "../../grimoire/examples/sigil_curtain/scene.rs"]
mod scene;

// The PNG writer and tolerance metric of the WP2.8 scenes, shared rather than copied (the facade's
// `tests/overlay_scene.rs` includes the same module the same way). Its own helpers for the
// reference comparison are unused here, and its tests name temporary files after the system time,
// which this crate does not lint against but the module is written to tolerate.
#[allow(dead_code)]
#[path = "../../grimoire_render/tests/support/mod.rs"]
mod snapshot_support;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Link, TOKEN, Tool, link_ends, watch_swaps};
use grimoire::platform::PlatformEvent;
use grimoire::prelude::*;
use grimoire::render::RenderError;
use grimoire::{GrimoireError, OffscreenRun};
use grimoire_link::watch::{SwapOutcome, WatchOptions};
use snapshot_support::Image;

/// The scene's `.sigil` source, and the edited one a save pushes over the link.
const CURTAIN_SOURCE: &str = include_str!("../../grimoire/tests/fixtures/sigil_curtain.sigil");
const SWAPPED_SOURCE: &str = include_str!("patterns/sigil_curtain_swapped.sigil");
/// The canonical content path the checked-in unit was compiled under: the swap needs the same one.
const CURTAIN_PATH: &str = "fixtures/sigil_curtain.sigil";

/// One frame of a 24 frames-per-second GIF, as in the M2 showcase: 2.5 ticks at 60 Hz.
const GIF_FRAME: Duration = Duration::from_nanos(41_666_667);

/// A content root with `fixtures/sigil_curtain.sigil`, and the file inside it.
fn content(name: &str) -> (PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&root);
    let fixtures = root.join("fixtures");
    std::fs::create_dir_all(&fixtures).expect("content root");
    let file = fixtures.join("sigil_curtain.sigil");
    std::fs::write(&file, CURTAIN_SOURCE).expect("write the scene's source");
    (root, file)
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

/// What one showcase run produced.
struct Showcase {
    /// The swap the tool pushed, if the run got that far.
    swap: Option<SwapOutcome>,
    /// The tool's log, for the test output.
    log: String,
    /// Frames the loop completed.
    frames: u64,
}

/// Renders `frames` frames of the scene offscreen at `width` × `height`, saving the edited pattern
/// once the tool is connected and at least `warm_up` frames have run, and hands every captured
/// frame to `capture`. `None` without a GPU adapter.
fn run_showcase(
    name: &str,
    width: u32,
    height: u32,
    frames: u64,
    warm_up: u64,
    capture: &mut dyn FnMut(u64, &[u8]),
) -> Option<Showcase> {
    let (root, file) = content(name);
    let mut options = WatchOptions::new(&root, &file);
    options.poll_interval = Duration::from_millis(5);

    let (engine_transport, open_tool) = link_ends(Link::InProcess);
    let tool = Tool::spawn(open_tool, 0, watch_swaps(options, 1));

    let (app, _last_frame) = scene::app(WindowConfig::default());
    let mut run = OffscreenRun::new(width, height, frames, GIF_FRAME);
    run.capture_every = 1;
    let mut saved = false;
    let result = app
        .debug_link(engine_transport)
        .debug_link_token(TOKEN)
        .run_offscreen(
            run,
            &mut |frame: u64, _events: &mut Vec<PlatformEvent>| {
                if !saved && frame >= warm_up && tool.connected() {
                    std::fs::write(&file, SWAPPED_SOURCE).expect("save the edited pattern");
                    saved = true;
                }
                // Rendering a frame already takes milliseconds, so the tool has room to compile
                // and push; this only keeps the loop from racing ahead before it connected.
                if !tool.connected() {
                    std::thread::sleep(Duration::from_millis(1));
                }
            },
            &mut |frame, rgba| capture(frame, rgba),
        );
    let report = match result {
        Ok(report) => report,
        Err(GrimoireError::Render(RenderError::NoAdapter)) => {
            assert!(
                !adapter_required(),
                "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
            );
            let _ = writeln!(
                std::io::stdout(),
                "\n::warning title=GPU test skipped::no GPU adapter found; grimoire_link hot_swap_showcase rendered nothing"
            );
            return None;
        }
        Err(error) => panic!("offscreen run failed: {error}"),
    };
    let tool = tool.join();
    assert!(saved, "the edited pattern was saved during the run");
    let swap = tool.swaps.into_iter().next();
    Some(Showcase {
        swap,
        log: tool.log,
        frames: report.frames,
    })
}

/// Number of pixels whose colour differs by more than `threshold` in any channel.
fn changed_pixels(a: &[u8], b: &[u8], threshold: u8) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(pa, pb)| {
            pa.iter()
                .zip(pb.iter())
                .any(|(x, y)| x.abs_diff(*y) > threshold)
        })
        .count()
}

#[test]
fn the_hot_swap_changes_the_rendered_frames() {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 90;
    const WARM_UP: u64 = 12;
    const FRAMES: u64 = 40;

    let mut before: Option<Vec<u8>> = None;
    let mut after: Option<Vec<u8>> = None;
    let Some(showcase) = run_showcase(
        "hot-swap-small",
        WIDTH,
        HEIGHT,
        FRAMES,
        WARM_UP,
        &mut |frame, rgba| {
            if frame == WARM_UP - 1 {
                before = Some(rgba.to_vec());
            }
            if frame == FRAMES - 1 {
                after = Some(rgba.to_vec());
            }
        },
    ) else {
        return;
    };
    assert_eq!(showcase.frames, FRAMES);

    let Some(swap) = showcase.swap else {
        panic!("the tool pushed the edited pattern:\n{}", showcase.log);
    };
    assert!(swap.applied(), "{}", swap.summary());
    assert_eq!(swap.unit_path, CURTAIN_PATH);
    println!("hot swap in the showcase: {}", swap.summary());

    let (before, after) = (
        before.expect("a frame before the swap"),
        after.expect("a frame after the swap"),
    );
    let changed = changed_pixels(&before, &after, 40);
    assert!(
        changed > 200,
        "only {changed} of {} pixels changed across the swap",
        WIDTH * HEIGHT
    );
}

fn out_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_HOT_SWAP_GIF_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target").join("wp85-hot-swap"))
}

#[test]
#[ignore = "writes a PNG frame sequence; run explicitly (see this file's module docs)"]
fn render_hot_swap_png_sequence() {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 360;
    /// Frames of the original pattern before the save: a second of curtain at 24 fps.
    const WARM_UP: u64 = 24;
    /// Three seconds in total, so two thirds of the GIF show the reloaded pattern.
    const FRAMES: u64 = 72;

    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("output directory");
    let showcase = run_showcase(
        "hot-swap-gif",
        WIDTH,
        HEIGHT,
        FRAMES,
        WARM_UP,
        &mut |frame, rgba| {
            let image = Image::from_offscreen(WIDTH, HEIGHT, rgba.to_vec());
            let path = dir.join(format!("frame_{frame:04}.png"));
            image.write_png(&path).expect("write PNG");
        },
    )
    .expect("the PNG sequence needs a GPU or software adapter");
    assert_eq!(showcase.frames, FRAMES);
    let swap = showcase
        .swap
        .expect("the sequence shows a swap the engine applied");
    assert!(swap.applied(), "{}", swap.summary());
    let _ = writeln!(
        std::io::stdout(),
        "hot swap after frame {WARM_UP} of {FRAMES}: {}\n{}",
        swap.summary(),
        showcase.log
    );
}
