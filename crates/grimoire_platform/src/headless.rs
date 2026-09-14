//! Headless runner without any window — for tests, CI and the simulation harness.

use std::sync::Arc;
use std::time::Duration;

use crate::app::{AppHandler, PlatformContext};
use crate::clock::{Clock, ManualClock};
use crate::error::PlatformError;
use crate::window::PlatformWindow;

struct HeadlessContext {
    clock: ManualClock,
    exit_requested: bool,
}

impl PlatformContext for HeadlessContext {
    fn window(&self) -> Option<Arc<dyn PlatformWindow>> {
        None
    }

    fn clock(&self) -> &dyn Clock {
        &self.clock
    }

    fn request_exit(&mut self) {
        self.exit_requested = true;
    }

    fn exit_requested(&self) -> bool {
        self.exit_requested
    }
}

/// Drives `app` for exactly `frames` frames without a window.
///
/// Before every frame a [`crate::ManualClock`] advances by `frame_delta`, so runs are fully
/// reproducible. `ctx.window()` returns `None`. The loop ends early if the app requests exit
/// (also from `init`, in which case no frame runs). No platform events are generated.
///
/// # Errors
/// [`PlatformError::AppInit`] if [`AppHandler::init`] fails.
pub fn run_headless<A: AppHandler>(
    app: &mut A,
    frames: u64,
    frame_delta: Duration,
) -> Result<(), PlatformError> {
    let mut ctx = HeadlessContext {
        clock: ManualClock::new(),
        exit_requested: false,
    };

    app.init(&mut ctx)
        .map_err(|error| PlatformError::AppInit(error.to_string()))?;

    for _ in 0..frames {
        if ctx.exit_requested {
            break;
        }
        ctx.clock.advance(frame_delta);
        app.frame(&mut ctx);
    }

    app.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppResult;
    use crate::event::PlatformEvent;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Init,
        Event,
        Frame { elapsed: Duration },
        Shutdown,
    }

    #[derive(Default)]
    struct Recorder {
        calls: Vec<Call>,
        fail_init: bool,
        exit_in_init: bool,
        exit_after_frames: Option<usize>,
    }

    impl Recorder {
        fn frames(&self) -> usize {
            self.calls
                .iter()
                .filter(|call| matches!(call, Call::Frame { .. }))
                .count()
        }

        fn count(&self, wanted: &Call) -> usize {
            self.calls.iter().filter(|call| *call == wanted).count()
        }
    }

    impl AppHandler for Recorder {
        fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
            assert!(ctx.window().is_none());
            assert_eq!(ctx.clock().elapsed(), Duration::ZERO);
            self.calls.push(Call::Init);
            if self.fail_init {
                return Err("no save directory".into());
            }
            if self.exit_in_init {
                ctx.request_exit();
            }
            Ok(())
        }

        fn event(&mut self, _ctx: &mut dyn PlatformContext, _event: &PlatformEvent) {
            self.calls.push(Call::Event);
        }

        fn frame(&mut self, ctx: &mut dyn PlatformContext) {
            assert!(ctx.window().is_none());
            self.calls.push(Call::Frame {
                elapsed: ctx.clock().elapsed(),
            });
            if self.exit_after_frames == Some(self.frames()) {
                ctx.request_exit();
                assert!(ctx.exit_requested());
            }
        }

        fn shutdown(&mut self) {
            self.calls.push(Call::Shutdown);
        }
    }

    #[test]
    fn runs_init_exact_frames_and_shutdown_in_order() {
        let mut app = Recorder::default();
        let delta = Duration::from_micros(16_667);
        run_headless(&mut app, 5, delta).unwrap();

        let mut expected = vec![Call::Init];
        expected.extend((1..=5u32).map(|i| Call::Frame { elapsed: delta * i }));
        expected.push(Call::Shutdown);
        assert_eq!(app.calls, expected);
        assert_eq!(app.count(&Call::Event), 0);
    }

    #[test]
    fn zero_frames_still_inits_and_shuts_down() {
        let mut app = Recorder::default();
        run_headless(&mut app, 0, Duration::from_millis(16)).unwrap();
        assert_eq!(app.calls, [Call::Init, Call::Shutdown]);
    }

    #[test]
    fn request_exit_in_frame_ends_early() {
        let mut app = Recorder {
            exit_after_frames: Some(3),
            ..Recorder::default()
        };
        run_headless(&mut app, 1_000, Duration::from_millis(10)).unwrap();
        assert_eq!(app.frames(), 3);
        assert_eq!(app.count(&Call::Init), 1);
        assert_eq!(app.count(&Call::Shutdown), 1);
        assert_eq!(app.calls.last(), Some(&Call::Shutdown));
    }

    #[test]
    fn request_exit_in_init_skips_frames() {
        let mut app = Recorder {
            exit_in_init: true,
            ..Recorder::default()
        };
        run_headless(&mut app, 10, Duration::from_millis(10)).unwrap();
        assert_eq!(app.calls, [Call::Init, Call::Shutdown]);
    }

    #[test]
    fn init_error_is_app_init_without_shutdown() {
        let mut app = Recorder {
            fail_init: true,
            ..Recorder::default()
        };
        let error = run_headless(&mut app, 10, Duration::from_millis(10)).unwrap_err();
        match error {
            PlatformError::AppInit(message) => assert_eq!(message, "no save directory"),
            other => panic!("expected AppInit, got {other:?}"),
        }
        assert_eq!(app.calls, [Call::Init]);
    }
}
