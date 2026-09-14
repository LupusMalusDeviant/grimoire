//! Opens a window and logs every platform event.
//!
//! `Escape` or closing the window exits. `GRIMOIRE_EXAMPLE_MAX_FRAMES=<n>` exits after `n`
//! frames, which smoke tests use. Log verbosity follows `RUST_LOG` (default `info`).
//!
//! ```text
//! cargo run -p grimoire_platform --example window
//! ```

use std::time::Duration;

use grimoire_platform::{
    AppHandler, AppResult, KeyCode, PlatformContext, PlatformEvent, RawInputEvent, WindowConfig,
    run_desktop,
};

const MAX_FRAMES_VAR: &str = "GRIMOIRE_EXAMPLE_MAX_FRAMES";

/// Without a GPU swap chain there is no vsync, so the example paces itself.
const FRAME_TIME: Duration = Duration::from_micros(16_667);

struct WindowExample {
    max_frames: Option<u64>,
    frames: u64,
    last_title_update: Duration,
    frames_since_title_update: u64,
}

impl AppHandler for WindowExample {
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
        let window = ctx.window().ok_or("desktop runner provided no window")?;
        log::info!(
            "window ready: {:?} physical pixels, scale factor {}",
            window.inner_size(),
            window.scale_factor()
        );
        if let Some(max) = self.max_frames {
            log::info!("{MAX_FRAMES_VAR}={max}: exiting automatically");
        }
        Ok(())
    }

    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent) {
        match event {
            PlatformEvent::Input(RawInputEvent::CursorMoved { .. }) => log::trace!("{event:?}"),
            _ => log::info!("{event:?}"),
        }
        if let PlatformEvent::Input(RawInputEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            ..
        }) = event
        {
            log::info!("escape pressed, exiting");
            ctx.request_exit();
        }
    }

    fn frame(&mut self, ctx: &mut dyn PlatformContext) {
        self.frames += 1;
        self.frames_since_title_update += 1;

        let now = ctx.clock().elapsed();
        let since_update = now.saturating_sub(self.last_title_update);
        if since_update >= Duration::from_secs(1) {
            let fps = self.frames_since_title_update as f64 / since_update.as_secs_f64();
            if let Some(window) = ctx.window() {
                window.set_title(&format!("Grimoire window example - {fps:.0} fps"));
            }
            self.last_title_update = now;
            self.frames_since_title_update = 0;
        }

        if self.max_frames.is_some_and(|max| self.frames >= max) {
            log::info!("reached {} frames, exiting", self.frames);
            ctx.request_exit();
            return;
        }

        let next_frame = FRAME_TIME.saturating_mul(u32::try_from(self.frames).unwrap_or(u32::MAX));
        if let Some(wait) = next_frame.checked_sub(ctx.clock().elapsed()) {
            std::thread::sleep(wait);
        }
    }

    fn shutdown(&mut self) {
        log::info!("shutdown after {} frames", self.frames);
    }
}

fn max_frames_from_env() -> Result<Option<u64>, Box<dyn std::error::Error>> {
    match std::env::var(MAX_FRAMES_VAR) {
        Ok(value) => {
            let frames = value.trim().parse::<u64>().map_err(|error| {
                format!("{MAX_FRAMES_VAR}={value:?} is not a frame count: {error}")
            })?;
            Ok(Some(frames))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("{MAX_FRAMES_VAR}: {error}").into()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let app = WindowExample {
        max_frames: max_frames_from_env()?,
        frames: 0,
        last_title_update: Duration::ZERO,
        frames_since_title_update: 0,
    };
    let config = WindowConfig {
        title: String::from("Grimoire window example"),
        ..WindowConfig::default()
    };
    run_desktop(config, app)?;
    log::info!("event loop finished");
    Ok(())
}
