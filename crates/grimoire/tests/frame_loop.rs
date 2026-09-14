//! Timing and input handling of the real main loop, driven headless.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::Scenario;
use grimoire::platform::{PlatformEvent, RawInputEvent};
use grimoire::prelude::*;

/// Stats of every frame with the input of the frame's last tick.
type Frames = Rc<RefCell<Vec<(FrameStats, TickInput)>>>;

/// Records the stats of every frame and the input the simulation saw last.
#[derive(Default)]
struct Probe {
    frames: Frames,
    last_input: TickInput,
}

impl GamePlugin for Probe {
    fn name(&self) -> &str {
        "probe"
    }

    fn extract(&mut self, world: &World, _alpha: f32, _frame: &mut RenderFrame) {
        self.last_input = world.resource::<TickInput>().copied().unwrap_or_default();
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        self.frames.borrow_mut().push((*stats, self.last_input));
    }
}

fn probe() -> (Probe, Frames) {
    let probe = Probe::default();
    let frames = Rc::clone(&probe.frames);
    (probe, frames)
}

/// One 60 Hz tick per frame (see `tests/headless.rs`).
const ONE_TICK: Duration = Duration::from_nanos(16_666_667);

fn key(code: KeyCode, pressed: bool, repeat: bool) -> PlatformEvent {
    PlatformEvent::Input(RawInputEvent::Key {
        code,
        pressed,
        repeat,
    })
}

#[test]
fn sixty_hertz_ticks_at_144_hertz_frames_give_exactly_600_ticks_in_1440_frames() {
    // 1/144 s is not a whole number of nanoseconds; rounding up keeps 1440 frames ≥ 10 s.
    let frame_delta = Duration::from_nanos(6_944_445);
    let (probe, frames) = probe();
    let report = App::new(WindowConfig::default())
        .tick_rate(60)
        .plugin(Scenario { movers: 50 })
        .plugin(probe)
        .run_headless_frames(1440, frame_delta)
        .expect("headless frame loop runs");

    assert_eq!(report.frames, 1440);
    assert_eq!(report.final_tick, 600);
    assert_eq!(report.dropped_time, Duration::ZERO);

    let frames = frames.borrow();
    assert_eq!(frames.len(), 1440);
    let total: u64 = frames
        .iter()
        .map(|(stats, _)| u64::from(stats.ticks_this_frame))
        .sum();
    assert_eq!(total, 600);
    assert!(frames.iter().all(|(stats, _)| stats.ticks_this_frame <= 1));
    assert!(
        frames
            .iter()
            .all(|(stats, _)| (0.0..1.0).contains(&stats.alpha))
    );
    let (last, _) = frames[1439];
    assert_eq!(last.sim_tick, 600);
    assert!((last.fps - 144.0).abs() < 0.5, "fps = {}", last.fps);
}

#[test]
fn a_huge_frame_delta_clamps_to_the_maximum_ticks_per_frame() {
    let (probe, frames) = probe();
    let report = App::new(WindowConfig::default())
        .plugin(probe)
        .run_headless_frames(10, Duration::from_secs(3_600))
        .expect("headless frame loop runs");
    assert_eq!(report.final_tick, 80);
    assert!(report.dropped_time > Duration::from_secs(35_000));
    assert!(
        frames
            .borrow()
            .iter()
            .all(|(stats, _)| stats.ticks_this_frame == 8)
    );

    let clamped = App::new(WindowConfig::default())
        .max_ticks_per_frame(3)
        .run_headless_frames(10, Duration::from_secs(1))
        .expect("headless frame loop runs");
    assert_eq!(clamped.final_tick, 30);
    // Each 1 s frame is due 60 ticks and runs 3; 57/60 s per frame is dropped.
    assert_eq!(clamped.dropped_time, Duration::from_millis(9_500));
}

#[test]
fn focus_loss_releases_held_input() {
    let (probe, frames) = probe();
    let mut script = |frame: u64, events: &mut Vec<PlatformEvent>| match frame {
        0 => {
            events.push(key(KeyCode::KeyW, true, false));
            events.push(PlatformEvent::Input(RawInputEvent::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            }));
        }
        3 => events.push(key(KeyCode::KeyW, true, true)),
        5 => events.push(PlatformEvent::Focused(false)),
        // The release happened while unfocused; its late arrival must not matter.
        7 => {
            events.push(PlatformEvent::Focused(true));
            events.push(key(KeyCode::KeyW, false, false));
        }
        _ => {}
    };
    let report = App::new(WindowConfig::default())
        .plugin(Scenario { movers: 0 })
        .plugin(probe)
        .run_headless_frames_with_events(10, ONE_TICK, &mut script)
        .expect("headless frame loop runs");
    assert_eq!(report.final_tick, 10);

    let frames = frames.borrow();
    for (index, (stats, input)) in frames.iter().enumerate() {
        assert_eq!(stats.ticks_this_frame, 1, "frame {index}");
        let slot = input.slots[0];
        if index < 5 {
            assert_eq!(slot.axes, [0, i16::MAX, 0, 0], "frame {index}");
            assert_eq!(slot.buttons, 1 << 2, "frame {index}");
        } else {
            assert_eq!(slot, InputFrame::default(), "frame {index}");
        }
        assert!(
            input.slots[1..]
                .iter()
                .all(|other| *other == InputFrame::default())
        );
    }
}

#[test]
fn held_keys_steer_the_simulation() {
    let mut script = |frame: u64, events: &mut Vec<PlatformEvent>| match frame {
        0 => events.push(key(KeyCode::KeyD, true, false)),
        30 => events.push(key(KeyCode::KeyD, false, false)),
        _ => {}
    };
    let steered = App::new(WindowConfig::default())
        .plugin(Scenario { movers: 0 })
        .run_headless_frames_with_events(60, ONE_TICK, &mut script)
        .expect("headless frame loop runs");
    let reference = App::new(WindowConfig::default())
        .plugin(Scenario { movers: 0 })
        .run_headless(60, &mut |tick| {
            let mut input = TickInput::default();
            if tick < 30 {
                input.slots[0].axes[0] = i16::MAX;
            }
            input
        });
    assert_eq!(steered.final_hash, reference.final_hash);
    assert_ne!(
        steered.final_hash,
        App::new(WindowConfig::default())
            .plugin(Scenario { movers: 0 })
            .run_headless(60, &mut |_| TickInput::default())
            .final_hash
    );
}

#[test]
fn a_tap_within_one_frame_reaches_the_next_tick() {
    let (probe, frames) = probe();
    let mut script = |frame: u64, events: &mut Vec<PlatformEvent>| {
        if frame == 2 {
            events.push(key(KeyCode::Space, true, false));
            events.push(key(KeyCode::Space, false, false));
        }
    };
    let report = App::new(WindowConfig::default())
        .plugin(Scenario { movers: 0 })
        .plugin(probe)
        .run_headless_frames_with_events(5, ONE_TICK, &mut script)
        .expect("headless frame loop runs");
    assert_eq!(report.final_tick, 5);

    let frames = frames.borrow();
    assert!(frames.iter().all(|(stats, _)| stats.ticks_this_frame == 1));
    let buttons: Vec<u32> = frames
        .iter()
        .map(|(_, input)| input.slots[0].buttons)
        .collect();
    assert_eq!(buttons, [0, 0, 1 << 0, 0, 0]);
}

#[test]
fn a_tap_during_frames_without_ticks_reaches_the_next_tick() {
    // 144 Hz frames at 60 Hz ticks: frames 0, 1 and 3 run no tick, frames 2 and 4 run one.
    let frame_delta = Duration::from_nanos(6_944_445);
    let (probe, frames) = probe();
    let mut script = |frame: u64, events: &mut Vec<PlatformEvent>| match frame {
        0 => events.push(key(KeyCode::Space, true, false)),
        1 => events.push(key(KeyCode::Space, false, false)),
        _ => {}
    };
    let report = App::new(WindowConfig::default())
        .plugin(Scenario { movers: 0 })
        .plugin(probe)
        .run_headless_frames_with_events(5, frame_delta, &mut script)
        .expect("headless frame loop runs");
    assert_eq!(report.final_tick, 2);

    let frames = frames.borrow();
    let ticks: Vec<u32> = frames
        .iter()
        .map(|(stats, _)| stats.ticks_this_frame)
        .collect();
    assert_eq!(ticks, [0, 0, 1, 0, 1]);
    let (_, first_tick) = frames[2];
    assert_eq!(first_tick.slots[0].buttons, 1 << 0, "tick 0 sees the tap");
    let (_, second_tick) = frames[4];
    assert_eq!(
        second_tick.slots[0].buttons, 0,
        "the latch ends with the tick that consumed it"
    );
}

#[test]
fn exit_key_ends_the_loop_before_the_next_frame() {
    let mut script = |frame: u64, events: &mut Vec<PlatformEvent>| {
        if frame == 4 {
            events.push(key(KeyCode::Escape, true, false));
        }
    };
    let report = App::new(WindowConfig::default())
        .exit_key(KeyCode::Escape)
        .run_headless_frames_with_events(100, ONE_TICK, &mut script)
        .expect("headless frame loop runs");
    assert_eq!(report.frames, 4);
    assert_eq!(report.final_tick, 4);
}
