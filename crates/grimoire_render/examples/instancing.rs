//! Instancing stress test: 10,000 moving circles in one draw call, FPS in the window title.
//!
//! `GRIMOIRE_EXAMPLE_MAX_FRAMES=<n>` exits after `n` frames; `Escape` exits at any time.

use std::sync::Arc;
use std::time::Duration;

use grimoire_platform::{
    AppHandler, AppResult, KeyCode, PlatformContext, PlatformEvent, PlatformWindow, RawInputEvent,
    WindowConfig, run_desktop,
};
use grimoire_render::{
    Camera2D, RenderError, RenderFrame, Renderer, RendererConfig, SpriteInstance, WgpuRenderer,
    shape,
};

const SPRITE_COUNT: usize = 10_000;
const WORLD_HEIGHT: f32 = 200.0;
const TITLE: &str = "Grimoire instancing";

struct Instancing {
    window: Option<Arc<dyn PlatformWindow>>,
    renderer: Option<WgpuRenderer>,
    frame: RenderFrame,
    velocities: Vec<[f32; 2]>,
    aspect: f32,
    last_time: Duration,
    fps_since: Duration,
    fps_frames: u32,
    frames: u64,
    max_frames: Option<u64>,
}

impl Instancing {
    fn new(max_frames: Option<u64>) -> Self {
        Self {
            window: None,
            renderer: None,
            frame: RenderFrame {
                clear_color: [0.01, 0.01, 0.02, 1.0],
                camera: Camera2D {
                    center: [0.0, 0.0],
                    world_height: WORLD_HEIGHT,
                },
                sprites: Vec::with_capacity(SPRITE_COUNT),
            },
            velocities: Vec::with_capacity(SPRITE_COUNT),
            aspect: 16.0 / 9.0,
            last_time: Duration::ZERO,
            fps_since: Duration::ZERO,
            fps_frames: 0,
            frames: 0,
            max_frames,
        }
    }

    fn spawn_sprites(&mut self) {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32
        };
        let half_width = WORLD_HEIGHT * 0.5 * self.aspect;
        let half_height = WORLD_HEIGHT * 0.5;
        for i in 0..SPRITE_COUNT {
            let radius = 0.4 + next() * 1.2;
            let hue = i as f32 / SPRITE_COUNT as f32;
            self.frame.sprites.push(SpriteInstance {
                position: [
                    (next() * 2.0 - 1.0) * half_width,
                    (next() * 2.0 - 1.0) * half_height,
                ],
                half_size: [radius, radius],
                rotation: 0.0,
                shape: shape::CIRCLE,
                color: [hue, 0.3 + 0.5 * next(), 1.0 - hue, 0.9],
            });
            let angle = next() * std::f32::consts::TAU;
            let speed = 10.0 + next() * 40.0;
            self.velocities
                .push([angle.cos() * speed, angle.sin() * speed]);
        }
    }

    fn update(&mut self, dt: f32) {
        let half_width = WORLD_HEIGHT * 0.5 * self.aspect;
        let half_height = WORLD_HEIGHT * 0.5;
        let extents = [half_width, half_height];
        for (sprite, velocity) in self.frame.sprites.iter_mut().zip(&mut self.velocities) {
            let axes = sprite
                .position
                .iter_mut()
                .zip(velocity.iter_mut())
                .zip(sprite.half_size.iter().zip(extents));
            for ((position, speed), (half_size, extent)) in axes {
                // `clamp` panics on an inverted range, e.g. in an extremely narrow window.
                let limit = (extent - half_size).max(0.0);
                *position += *speed * dt;
                if position.abs() > limit {
                    *position = position.clamp(-limit, limit);
                    *speed = -*speed;
                }
            }
        }
    }
}

impl AppHandler for Instancing {
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
        let window = ctx
            .window()
            .ok_or("the instancing example needs a window")?;
        let config = RendererConfig {
            vsync: false,
            allow_software_fallback: true,
            ..RendererConfig::default()
        };
        let mut renderer = WgpuRenderer::new_for_window(Arc::clone(&window), config)?;
        let size = window.inner_size();
        renderer.resize(size.width, size.height);
        if !size.is_empty() {
            self.aspect = size.width as f32 / size.height as f32;
        }
        log::info!("renderer backend: {}", renderer.backend_name());
        self.spawn_sprites();
        self.last_time = ctx.clock().elapsed();
        self.fps_since = self.last_time;
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
                if !size.is_empty() {
                    self.aspect = size.width as f32 / size.height as f32;
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
        let now = ctx.clock().elapsed();
        let dt = now.saturating_sub(self.last_time).as_secs_f32().min(0.1);
        self.last_time = now;
        self.update(dt);

        if let Some(renderer) = &mut self.renderer {
            match renderer.render(&self.frame) {
                Ok(_) => {}
                Err(RenderError::SurfaceLost) => ctx.frame_not_presented(),
                Err(error) => {
                    log::error!("render failed: {error}");
                    ctx.request_exit();
                }
            }
        }

        self.frames += 1;
        self.fps_frames += 1;
        let window_elapsed = now.saturating_sub(self.fps_since);
        if window_elapsed >= Duration::from_secs(1) {
            let fps = f64::from(self.fps_frames) / window_elapsed.as_secs_f64();
            if let Some(window) = &self.window {
                window.set_title(&format!("{TITLE} - {SPRITE_COUNT} sprites - {fps:.0} FPS"));
            }
            self.fps_frames = 0;
            self.fps_since = now;
        }
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
    run_desktop(config, Instancing::new(max_frames))?;
    Ok(())
}
