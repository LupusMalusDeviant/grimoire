//! Plan 0002 WP3.7 showcase (M2): many point lights through clustered forward+ at the High budget,
//! hostile bullets with glow on their own layer, and the player marker on the ground plane under the
//! tilted camera, in one window. The scene lives in `scene.rs`, shared with the offscreen GIF of
//! `tests/lights_stage_showcase.rs`.
//!
//! **CI only builds this example, never runs it** (`cargo build --workspace --examples --locked`;
//! every example here opens a window). Run it locally with
//! `cargo run -p grimoire_render --example lights_stage`; `Escape` exits and
//! `GRIMOIRE_EXAMPLE_MAX_FRAMES=<n>` exits after `n` frames (the convention of `pbr_stage.rs`).

mod scene;

use std::sync::Arc;

use grimoire_platform::{
    AppHandler, AppResult, KeyCode, PlatformContext, PlatformEvent, PlatformWindow, RawInputEvent,
    WindowConfig, run_desktop,
};
use grimoire_render::{RenderError, Renderer, RendererConfig, StageFrame, WgpuRenderer};

const TITLE: &str = "Grimoire lights_stage (WP3.7 showcase)";
/// Seconds per loop of the scene.
const LOOP_SECONDS: f32 = 6.0;

struct Stage {
    window: Option<Arc<dyn PlatformWindow>>,
    renderer: Option<WgpuRenderer>,
    meshes: Option<scene::Meshes>,
    frame: StageFrame,
    time: f32,
    frames: u64,
    max_frames: Option<u64>,
}

impl AppHandler for Stage {
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
        let window = ctx
            .window()
            .ok_or("the lights_stage example needs a window")?;
        let config = scene::renderer_config(RendererConfig {
            vsync: true,
            allow_software_fallback: true,
            ..RendererConfig::default()
        });
        let mut renderer = WgpuRenderer::new_for_window_staged(Arc::clone(&window), config)?;
        let size = window.inner_size();
        renderer.resize(size.width, size.height);
        log::info!("renderer backend: {}", renderer.backend_name());
        self.meshes = Some(scene::register_meshes(&mut renderer));
        self.window = Some(window);
        self.renderer = Some(renderer);
        Ok(())
    }

    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent) {
        match event {
            PlatformEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            PlatformEvent::Input(RawInputEvent::Key {
                code: KeyCode::Escape,
                pressed: true,
                ..
            }) => ctx.request_exit(),
            _ => {}
        }
    }

    fn frame(&mut self, ctx: &mut dyn PlatformContext) {
        // A fixed visual step is fine for a showcase, unlike the simulation proper.
        self.time += 1.0 / 60.0;
        let (Some(meshes), Some(renderer)) = (&self.meshes, &mut self.renderer) else {
            return;
        };
        scene::build_frame(meshes, self.time / LOOP_SECONDS, &mut self.frame);
        match renderer.render_stage(&self.frame) {
            Ok(stats) => {
                if self.frames.is_multiple_of(120)
                    && let Some(window) = &self.window
                {
                    window.set_title(&format!(
                        "{TITLE} - {} lights, {} bullets",
                        stats.point_lights_drawn, stats.bullets_drawn
                    ));
                }
            }
            Err(RenderError::SurfaceLost) => ctx.frame_not_presented(),
            Err(error) => {
                log::error!("render failed: {error}");
                ctx.request_exit();
            }
        }
        self.frames += 1;
        if self.max_frames.is_some_and(|max| self.frames >= max) {
            ctx.request_exit();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let max_frames = match std::env::var("GRIMOIRE_EXAMPLE_MAX_FRAMES") {
        Ok(value) => Some(value.parse::<u64>()?),
        Err(_) => None,
    };
    let config = WindowConfig {
        title: String::from(TITLE),
        ..WindowConfig::default()
    };
    run_desktop(
        config,
        Stage {
            window: None,
            renderer: None,
            meshes: None,
            frame: StageFrame::new(),
            time: 0.0,
            frames: 0,
            max_frames,
        },
    )?;
    Ok(())
}
