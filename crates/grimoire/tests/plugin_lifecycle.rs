//! Plugin call order and arguments in the headless frame loop and the pure headless run.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use grimoire::platform::{PlatformEvent, RawInputEvent};
use grimoire::prelude::*;

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Build {
        plugin: &'static str,
        tick: u64,
        seed: u64,
    },
    Extract {
        plugin: &'static str,
        alpha: f32,
        sprites_before: usize,
    },
    OnFrame {
        plugin: &'static str,
        stats: FrameStats,
    },
    WindowCreated {
        plugin: &'static str,
    },
    Shutdown {
        plugin: &'static str,
    },
}

type Log = Rc<RefCell<Vec<Call>>>;

struct Recorder {
    name: &'static str,
    log: Log,
}

impl GamePlugin for Recorder {
    fn name(&self) -> &str {
        self.name
    }

    fn build(&mut self, sim: &mut Simulation) {
        self.log.borrow_mut().push(Call::Build {
            plugin: self.name,
            tick: sim.tick(),
            seed: sim.seed(),
        });
    }

    fn extract(&mut self, _world: &World, alpha: f32, frame: &mut RenderFrame) {
        self.log.borrow_mut().push(Call::Extract {
            plugin: self.name,
            alpha,
            sprites_before: frame.sprites.len(),
        });
        frame.sprites.push(SpriteInstance::default());
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        self.log.borrow_mut().push(Call::OnFrame {
            plugin: self.name,
            stats: *stats,
        });
    }

    fn window_created(&mut self, _window: &Arc<dyn PlatformWindow>) {
        self.log
            .borrow_mut()
            .push(Call::WindowCreated { plugin: self.name });
    }

    fn shutdown(&mut self) {
        self.log
            .borrow_mut()
            .push(Call::Shutdown { plugin: self.name });
    }
}

/// Asserts that the log ends with one `shutdown` per plugin in registration order and that no
/// other `shutdown` call occurred.
fn assert_shut_down_once_in_order(calls: &[Call]) {
    let shutdowns = calls
        .iter()
        .filter(|call| matches!(call, Call::Shutdown { .. }))
        .count();
    assert_eq!(shutdowns, NAMES.len(), "shutdown runs once per plugin");
    let expected: Vec<Call> = NAMES
        .iter()
        .map(|&plugin| Call::Shutdown { plugin })
        .collect();
    assert_eq!(calls[calls.len() - NAMES.len()..], expected[..]);
}

const NAMES: [&str; 3] = ["first", "second", "third"];

fn app_with_recorders(log: &Log) -> AppBuilder {
    NAMES.iter().fold(
        App::new(WindowConfig::default()).seed(1234),
        |app, &name| {
            app.plugin(Recorder {
                name,
                log: Rc::clone(log),
            })
        },
    )
}

#[test]
fn frame_loop_builds_once_in_order_then_extracts_and_reports_every_frame() {
    const FRAMES: usize = 250;
    let log = Log::default();
    let report = app_with_recorders(&log)
        .run_headless_frames(FRAMES as u64, Duration::from_micros(7_300))
        .expect("headless frame loop runs");
    assert_eq!(report.frames, FRAMES as u64);

    let calls = log.borrow();
    let builds: Vec<&Call> = calls
        .iter()
        .filter(|call| matches!(call, Call::Build { .. }))
        .collect();
    assert_eq!(builds.len(), NAMES.len(), "build runs once per plugin");
    for (index, name) in NAMES.iter().enumerate() {
        assert_eq!(
            calls[index],
            Call::Build {
                plugin: name,
                tick: 0,
                seed: 1234
            }
        );
    }
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, Call::WindowCreated { .. })),
        "headless runs have no window"
    );

    assert_shut_down_once_in_order(&calls);
    let per_frame = &calls[NAMES.len()..calls.len() - NAMES.len()];
    assert_eq!(per_frame.len(), FRAMES * 2 * NAMES.len());
    let mut last_tick = 0;
    for (frame, chunk) in per_frame.chunks(2 * NAMES.len()).enumerate() {
        let (extracts, reports) = chunk.split_at(NAMES.len());
        let mut frame_alpha = None;
        for (index, (call, name)) in extracts.iter().zip(NAMES).enumerate() {
            let Call::Extract {
                plugin,
                alpha,
                sprites_before,
            } = call
            else {
                panic!("frame {frame}: expected extract, got {call:?}");
            };
            assert_eq!(*plugin, name);
            assert!((0.0..1.0).contains(alpha), "alpha {alpha} out of [0, 1)");
            assert_eq!(
                *sprites_before, index,
                "the frame is cleared before extraction"
            );
            assert_eq!(*frame_alpha.get_or_insert(*alpha), *alpha);
        }
        for (call, name) in reports.iter().zip(NAMES) {
            let Call::OnFrame { plugin, stats } = call else {
                panic!("frame {frame}: expected on_frame, got {call:?}");
            };
            assert_eq!(*plugin, name);
            assert_eq!(stats.frame, frame as u64);
            assert_eq!(Some(stats.alpha), frame_alpha);
            assert_eq!(stats.render.sprites_drawn, NAMES.len() as u32);
            assert_eq!(stats.frame_time, Duration::from_micros(7_300));
            assert!(stats.sim_tick >= last_tick);
            last_tick = stats.sim_tick;
        }
    }
    assert_eq!(last_tick, report.final_tick);
}

#[test]
fn pure_headless_run_only_builds() {
    let log = Log::default();
    let report = app_with_recorders(&log).run_headless(50, &mut |_| TickInput::default());
    assert_eq!(report.final_tick, 50);
    let calls = log.borrow();
    assert_eq!(calls.len(), NAMES.len());
    assert!(calls.iter().all(|call| matches!(call, Call::Build { .. })));
}

#[test]
fn max_frames_ends_the_frame_loop() {
    let log = Log::default();
    let report = app_with_recorders(&log)
        .max_frames(5)
        .run_headless_frames(100, Duration::from_millis(16))
        .expect("headless frame loop runs");
    assert_eq!(report.frames, 5);
    assert_shut_down_once_in_order(&log.borrow());

    let log = Log::default();
    let none = app_with_recorders(&log)
        .max_frames(0)
        .run_headless_frames(100, Duration::from_millis(16))
        .expect("headless frame loop runs");
    assert_eq!(none.frames, 0);
    assert_eq!(none.final_tick, 0);
    let calls = log.borrow();
    assert_eq!(calls.len(), 2 * NAMES.len(), "only build and shutdown ran");
    assert_shut_down_once_in_order(&calls);
}

#[test]
fn exit_key_ends_the_frame_loop_with_shutdown() {
    let log = Log::default();
    let mut script = |frame: u64, events: &mut Vec<PlatformEvent>| {
        if frame == 3 {
            events.push(PlatformEvent::Input(RawInputEvent::Key {
                code: KeyCode::Escape,
                pressed: true,
                repeat: false,
            }));
        }
    };
    let report = app_with_recorders(&log)
        .exit_key(KeyCode::Escape)
        .run_headless_frames_with_events(100, Duration::from_millis(16), &mut script)
        .expect("headless frame loop runs");
    assert_eq!(report.frames, 3);
    assert_shut_down_once_in_order(&log.borrow());
}
