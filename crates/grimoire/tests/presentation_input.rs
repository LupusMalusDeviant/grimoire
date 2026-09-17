//! Presentation-only input (contract §9.12): a key that reaches `GamePlugin::presentation_input`
//! never reaches a tick, a hash or a recording.
//!
//! The proof is a recording: the same run, once with and once without those keypresses, must
//! produce byte-identical replay data and the same hashes. The last test binds the very same key
//! in the `InputMap` and shows the bytes then *do* differ — without it, the identity above could
//! hold for the trivial reason that the recording never sees any input at all.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::Scenario;
use grimoire::platform::{PlatformEvent, RawInputEvent};
use grimoire::prelude::*;
use grimoire::sim::{InputLog, Replay};
use grimoire::{InputAction, InputSource};

/// One 60 Hz tick per frame (see `tests/frame_loop.rs`).
const ONE_TICK: Duration = Duration::from_nanos(16_666_667);
const FRAMES: u64 = 120;
const SEED: u64 = 0x0009_1211_C0A7_A1A5;
/// The key the prototype uses to cycle camera presets.
const PRESENTATION_KEY: KeyCode = KeyCode::KeyC;

fn key(code: KeyCode, pressed: bool) -> PlatformEvent {
    PlatformEvent::Input(RawInputEvent::Key {
        code,
        pressed,
        repeat: false,
    })
}

/// What one run recorded.
struct Recording {
    /// The replay bytes of the per-tick input, as a recording tool would store them.
    replay: Vec<u8>,
    /// Every state hash of the run.
    hashes: Vec<(u64, u64)>,
    /// Presentation events the camera plugin saw.
    seen: Vec<RawInputEvent>,
    /// Frame and tick at which each of them arrived.
    arrivals: Vec<(u64, u64)>,
    /// Camera preset the plugin ended on.
    preset: u8,
}

/// State the camera plugin keeps outside the world.
#[derive(Default)]
struct CameraState {
    seen: Vec<RawInputEvent>,
    arrivals: Vec<(u64, u64)>,
    preset: u8,
    /// Ticks the simulation had run when the last event arrived, filled by `extract`.
    tick_at_arrival: u64,
    pending: usize,
    frames: u64,
}

/// A plugin like the prototype's camera: it cycles presets on a key and touches nothing else.
struct CameraPresets {
    state: Rc<RefCell<CameraState>>,
}

impl GamePlugin for CameraPresets {
    fn name(&self) -> &str {
        "camera_presets"
    }

    fn presentation_input(&mut self, event: &RawInputEvent) {
        let mut state = self.state.borrow_mut();
        state.seen.push(*event);
        state.pending += 1;
        if matches!(
            event,
            RawInputEvent::Key {
                code: PRESENTATION_KEY,
                pressed: true,
                ..
            }
        ) {
            state.preset = (state.preset + 1) % 3;
        }
    }

    /// Records, for every event of this frame, the frame and the tick the simulation stands at.
    /// `extract` runs after this frame's ticks, so an event delivered before them is recorded with
    /// the tick count the frame ended on.
    fn extract(&mut self, world: &World, _alpha: f32, _frame: &mut RenderFrame) {
        let mut state = self.state.borrow_mut();
        state.tick_at_arrival = world.resource::<Tick>().map_or(0, |tick| tick.0);
        let tick = state.tick_at_arrival;
        let frame = state.frames;
        for _ in 0..state.pending {
            state.arrivals.push((frame, tick));
        }
        state.pending = 0;
        state.frames += 1;
    }
}

/// Records the input of every tick, as a recording harness would (contract §8.1). The log is
/// shared with the test through an `Arc<Mutex<_>>` because a system must be `Send + Sync`.
struct InputRecorder {
    log: Arc<Mutex<Vec<TickInput>>>,
}

impl GamePlugin for InputRecorder {
    fn name(&self) -> &str {
        "input_recorder"
    }

    fn build(&mut self, sim: &mut Simulation) {
        let log = Arc::clone(&self.log);
        sim.schedule_mut()
            .add_system(system_fn("record_input", move |world| {
                let input = world.resource::<TickInput>().copied().unwrap_or_default();
                log.lock()
                    .expect("no test thread panics while holding this")
                    .push(input);
            }));
    }
}

/// Runs the scenario for [`FRAMES`] frames, pressing `keys` (code, frame) as presentation input,
/// and returns what the run recorded. `bind_presentation_key` additionally binds
/// [`PRESENTATION_KEY`] to a gameplay button, which is exactly what this hook exists to avoid.
fn run(keys: &[(KeyCode, u64)], bind_presentation_key: bool) -> Recording {
    let camera = Rc::new(RefCell::new(CameraState::default()));
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut input_map = InputMap::default();
    if bind_presentation_key {
        input_map.bind(InputSource::Key(PRESENTATION_KEY), InputAction::Button(4));
    }
    let report = App::new(WindowConfig::default())
        .seed(SEED)
        .tick_rate(60)
        .hash_every(1)
        .input_map(input_map)
        .plugin(Scenario { movers: 20 })
        .plugin(InputRecorder {
            log: Arc::clone(&log),
        })
        .plugin(CameraPresets {
            state: Rc::clone(&camera),
        })
        .run_headless_frames_with_events(FRAMES, ONE_TICK, &mut |frame, events| {
            for &(code, at) in keys {
                if at == frame {
                    events.push(key(code, true));
                    events.push(key(code, false));
                }
            }
        })
        .expect("headless frame loop runs");

    let replay = Replay {
        header: None,
        log: InputLog {
            seed: SEED,
            tick_rate_hz: 60,
            frames: log
                .lock()
                .expect("no test thread panics while holding this")
                .clone(),
        },
    }
    .to_bytes()
    .expect("the recorded input encodes");
    let camera = camera.borrow();
    Recording {
        replay,
        hashes: report.hashes,
        seen: camera.seen.clone(),
        arrivals: camera.arrivals.clone(),
        preset: camera.preset,
    }
}

/// Frames at which the presentation key is pressed in the "with" run.
fn presses() -> Vec<(KeyCode, u64)> {
    (0..FRAMES)
        .filter(|frame| frame % 7 == 3)
        .map(|frame| (PRESENTATION_KEY, frame))
        .collect()
}

#[test]
fn presentation_keys_change_neither_the_recording_nor_the_hashes() {
    let without = run(&[], false);
    let with = run(&presses(), false);

    assert_eq!(
        with.replay, without.replay,
        "the recorded input must be byte-identical"
    );
    assert_eq!(with.hashes, without.hashes, "every state hash must match");
    assert!(!with.hashes.is_empty(), "the run recorded hashes");

    // The plugin really saw the keys: a press and a release each.
    assert_eq!(with.seen.len(), presses().len() * 2);
    assert!(without.seen.is_empty());
    assert_eq!(with.preset, (presses().len() % 3) as u8);
}

#[test]
fn every_event_arrives_once_before_the_ticks_of_its_frame() {
    let with = run(&presses(), false);
    assert_eq!(with.arrivals.len(), with.seen.len());
    // One tick per frame, and `extract` runs after that tick: an event scripted before frame `n`
    // is delivered in frame `n`, where the simulation has just reached tick `n + 1`.
    for (index, &(frame, tick)) in with.arrivals.iter().enumerate() {
        let pressed_at = presses()[index / 2].1;
        assert_eq!(
            frame, pressed_at,
            "event {index} arrived in the wrong frame"
        );
        assert_eq!(
            tick,
            frame + 1,
            "event {index} arrived after its frame's tick"
        );
    }
}

#[test]
fn a_frame_without_ticks_still_delivers() {
    let camera = Rc::new(RefCell::new(CameraState::default()));
    // Frames far shorter than a tick: the timestep produces no tick at all in most of them.
    let report = App::new(WindowConfig::default())
        .seed(SEED)
        .tick_rate(60)
        .plugin(CameraPresets {
            state: Rc::clone(&camera),
        })
        .run_headless_frames_with_events(3, Duration::from_micros(200), &mut |frame, events| {
            if frame == 1 {
                events.push(key(PRESENTATION_KEY, true));
            }
        })
        .expect("headless frame loop runs");

    assert_eq!(report.final_tick, 0, "no tick ran in these frames");
    let camera = camera.borrow();
    assert_eq!(camera.seen.len(), 1, "the event arrived without any tick");
    assert_eq!(camera.preset, 1);
}

#[test]
fn losing_focus_delivers_nothing() {
    let camera = Rc::new(RefCell::new(CameraState::default()));
    App::new(WindowConfig::default())
        .seed(SEED)
        .plugin(CameraPresets {
            state: Rc::clone(&camera),
        })
        .run_headless_frames_with_events(3, ONE_TICK, &mut |frame, events| {
            if frame == 0 {
                events.push(key(PRESENTATION_KEY, true));
                events.push(PlatformEvent::Focused(false));
            }
        })
        .expect("headless frame loop runs");

    let camera = camera.borrow();
    assert_eq!(
        camera.seen,
        vec![RawInputEvent::Key {
            code: PRESENTATION_KEY,
            pressed: true,
            repeat: false,
        }],
        "only the key event reaches the hook, not the focus change"
    );
}

/// The counter-proof: binding the same key in the `InputMap` — what the prototype had to do
/// before this hook existed — changes the recorded input. Without this test, the identity above
/// could hold for the wrong reason.
#[test]
fn the_same_key_bound_as_gameplay_input_does_change_the_recording() {
    let without = run(&[], true);
    let with = run(&presses(), true);

    assert_ne!(
        with.replay, without.replay,
        "a bound key belongs in the recording"
    );
    // The hook still sees the key: binding it does not take it away from presentation.
    assert_eq!(with.seen.len(), presses().len() * 2);
}
