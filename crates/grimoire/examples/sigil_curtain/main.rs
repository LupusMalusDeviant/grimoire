//! Plan 0002 WP5.7 milestone showcase (M2): a Sigil pattern, compiled by `sigilc`, run by the
//! interpreter of `grimoire_sigil` inside the facade's main loop and drawn by the bullet pass over
//! the lit 2.5D stage. The pattern (`tests/fixtures/sigil_curtain.sigil`) is the reference pattern
//! `02-spiral-curtain` with a slower counter-turning orb spiral beneath it; the scene lives in
//! `scene.rs` next to this file.
//!
//! **CI only ever builds this example, never runs it** (`.github/workflows/ci.yml`'s "Build
//! examples" step; examples open windows). The job `showcase-gif` renders the same scene
//! offscreen into the M2 showcase GIF through `tests/sigil_curtain_showcase.rs`, which includes
//! `scene.rs`. Run it locally with
//!
//! ```text
//! GRIMOIRE_WINDOW_MONITOR=secondary GRIMOIRE_WINDOW_FOCUS=0 cargo run -p grimoire --example sigil_curtain
//! ```
//!
//! `Escape` exits, `GRIMOIRE_EXAMPLE_MAX_FRAMES=<n>` exits after `n` frames. The window title
//! shows the bullets drawn and the FPS.

mod scene;

use grimoire::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let (app, last_frame) = scene::app(WindowConfig {
        title: String::from(scene::TITLE),
        ..WindowConfig::default()
    });
    let mut app = app.exit_key(KeyCode::Escape);
    if let Ok(value) = std::env::var("GRIMOIRE_EXAMPLE_MAX_FRAMES") {
        app = app.max_frames(value.parse()?);
    }
    app.run()?;
    let frame = last_frame.get();
    log::info!(
        "last frame: {} bullets drawn, {} live, {} spawns dropped",
        frame.bullets.extracted,
        frame.live,
        frame.dropped_spawns
    );
    Ok(())
}
