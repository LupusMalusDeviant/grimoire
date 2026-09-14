//! `InputMap` bindings, sampling and `InputState` event handling.

use grimoire::platform::RawInputEvent;
use grimoire::prelude::*;
use grimoire::{AXIS_MAX, InputState};

fn press(state: &mut InputState, code: KeyCode) {
    state.apply(&RawInputEvent::Key {
        code,
        pressed: true,
        repeat: false,
    });
}

fn release(state: &mut InputState, code: KeyCode) {
    state.apply(&RawInputEvent::Key {
        code,
        pressed: false,
        repeat: false,
    });
}

fn sample_default(keys: &[KeyCode]) -> InputFrame {
    let mut state = InputState::new();
    for &code in keys {
        press(&mut state, code);
    }
    InputMap::default().sample(&state)
}

#[test]
fn nothing_held_samples_the_default_frame() {
    assert_eq!(sample_default(&[]), InputFrame::default());
}

#[test]
fn default_movement_keys_drive_axes_zero_and_one_with_y_up() {
    let cases = [
        (KeyCode::KeyD, [AXIS_MAX, 0]),
        (KeyCode::ArrowRight, [AXIS_MAX, 0]),
        (KeyCode::KeyA, [-AXIS_MAX, 0]),
        (KeyCode::ArrowLeft, [-AXIS_MAX, 0]),
        (KeyCode::KeyW, [0, AXIS_MAX]),
        (KeyCode::ArrowUp, [0, AXIS_MAX]),
        (KeyCode::KeyS, [0, -AXIS_MAX]),
        (KeyCode::ArrowDown, [0, -AXIS_MAX]),
    ];
    for (code, [x, y]) in cases {
        let frame = sample_default(&[code]);
        assert_eq!(frame.axes, [x, y, 0, 0], "{code:?}");
        assert_eq!(frame.buttons, 0, "{code:?}");
    }
    let up_right = sample_default(&[KeyCode::KeyW, KeyCode::KeyD]);
    assert_eq!(up_right.axis(0), 1.0);
    assert_eq!(up_right.axis(1), 1.0);
}

#[test]
fn default_buttons() {
    assert_eq!(sample_default(&[KeyCode::Space]).buttons, 1 << 0);
    assert_eq!(sample_default(&[KeyCode::ShiftLeft]).buttons, 1 << 1);
    assert_eq!(sample_default(&[KeyCode::ShiftRight]).buttons, 0);

    let mut state = InputState::new();
    state.apply(&RawInputEvent::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    });
    press(&mut state, KeyCode::Space);
    assert_eq!(InputMap::default().sample(&state).buttons, 0b101);
    state.apply(&RawInputEvent::MouseButton {
        button: MouseButton::Left,
        pressed: false,
    });
    assert_eq!(
        InputMap::default().sample(&state).buttons,
        0b101,
        "the released press stays latched until a tick consumed it"
    );
    state.clear_presses();
    assert_eq!(InputMap::default().sample(&state).buttons, 0b001);
}

#[test]
fn contributions_sum_and_clamp() {
    assert_eq!(
        sample_default(&[KeyCode::KeyW, KeyCode::ArrowUp]).axes,
        [0, AXIS_MAX, 0, 0]
    );
    assert_eq!(sample_default(&[KeyCode::KeyW, KeyCode::KeyS]).axes, [0; 4]);
    assert_eq!(
        sample_default(&[KeyCode::KeyA, KeyCode::ArrowLeft, KeyCode::KeyD]).axes,
        [-AXIS_MAX, 0, 0, 0]
    );

    let map = InputMap::new()
        .with(
            InputSource::Key(KeyCode::KeyQ),
            InputAction::Axis {
                axis: 3,
                value: i16::MIN,
            },
        )
        .with(
            InputSource::Key(KeyCode::KeyE),
            InputAction::Axis {
                axis: 3,
                value: i16::MIN,
            },
        )
        .with(
            InputSource::Key(KeyCode::KeyR),
            InputAction::Axis {
                axis: 2,
                value: 20_000,
            },
        );
    let mut state = InputState::new();
    press(&mut state, KeyCode::KeyQ);
    assert_eq!(
        map.sample(&state).axes,
        [0, 0, 0, -AXIS_MAX],
        "never -32768"
    );
    press(&mut state, KeyCode::KeyE);
    press(&mut state, KeyCode::KeyR);
    assert_eq!(map.sample(&state).axes, [0, 0, 20_000, -AXIS_MAX]);
}

#[test]
fn key_repeat_events_do_not_change_state() {
    let mut state = InputState::new();
    state.apply(&RawInputEvent::Key {
        code: KeyCode::KeyW,
        pressed: true,
        repeat: true,
    });
    assert!(state.is_empty(), "a repeat does not start holding a key");

    press(&mut state, KeyCode::KeyW);
    state.apply(&RawInputEvent::Key {
        code: KeyCode::KeyW,
        pressed: false,
        repeat: true,
    });
    assert!(state.is_held(InputSource::Key(KeyCode::KeyW)));
    release(&mut state, KeyCode::KeyW);
    assert!(!state.is_held(InputSource::Key(KeyCode::KeyW)));
    state.clear_presses();
    assert!(state.is_empty());
}

#[test]
fn a_repeat_press_does_not_latch() {
    let mut state = InputState::new();
    state.apply(&RawInputEvent::Key {
        code: KeyCode::Space,
        pressed: true,
        repeat: true,
    });
    assert!(!state.is_active(InputSource::Key(KeyCode::Space)));
    assert_eq!(InputMap::default().sample(&state), InputFrame::default());
}

#[test]
fn a_tap_released_before_sampling_stays_active_until_presses_are_cleared() {
    let space = InputSource::Key(KeyCode::Space);
    let mut state = InputState::new();
    press(&mut state, KeyCode::Space);
    release(&mut state, KeyCode::Space);
    assert!(!state.is_held(space));
    assert!(state.is_active(space));
    assert!(!state.is_empty());
    assert_eq!(InputMap::default().sample(&state).buttons, 1 << 0);

    state.clear_presses();
    assert!(!state.is_active(space));
    assert!(state.is_empty());
    assert_eq!(InputMap::default().sample(&state), InputFrame::default());
}

#[test]
fn clearing_presses_keeps_held_inputs() {
    let mut state = InputState::new();
    press(&mut state, KeyCode::KeyD);
    state.clear_presses();
    assert!(state.is_held(InputSource::Key(KeyCode::KeyD)));
    assert_eq!(InputMap::default().sample(&state).axes, [AXIS_MAX, 0, 0, 0]);
}

#[test]
fn cursor_and_wheel_events_change_nothing() {
    let mut state = InputState::new();
    state.apply(&RawInputEvent::CursorMoved { x: 10.0, y: 20.0 });
    state.apply(&RawInputEvent::MouseWheel {
        delta_x: 0.0,
        delta_y: 1.0,
    });
    assert!(state.is_empty());
}

#[test]
fn release_all_clears_every_held_input() {
    let mut state = InputState::new();
    press(&mut state, KeyCode::KeyW);
    press(&mut state, KeyCode::Space);
    state.apply(&RawInputEvent::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    });
    release(&mut state, KeyCode::Space);
    state.release_all();
    assert!(state.is_empty(), "held inputs and latched presses are gone");
    assert_eq!(InputMap::default().sample(&state), InputFrame::default());
}

#[test]
fn unbind_removes_all_actions_of_a_source() {
    let mut map = InputMap::default();
    let before = map.bindings().len();
    map.unbind(InputSource::Key(KeyCode::KeyW));
    assert_eq!(map.bindings().len(), before - 1);
    assert_eq!(
        map.sample(&{
            let mut state = InputState::new();
            press(&mut state, KeyCode::KeyW);
            state
        }),
        InputFrame::default()
    );
}

#[test]
#[should_panic(expected = "button bit 32 out of range")]
fn binding_button_32_panics() {
    let _ = InputMap::new().with(InputSource::Key(KeyCode::KeyX), InputAction::Button(32));
}

#[test]
#[should_panic(expected = "axis index 4 out of range")]
fn binding_axis_4_panics() {
    let _ = InputMap::new().with(
        InputSource::Key(KeyCode::KeyX),
        InputAction::Axis { axis: 4, value: 1 },
    );
}
