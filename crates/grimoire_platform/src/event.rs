//! Platform events and raw input, independent of any windowing library.

use crate::window::PhysicalSize;

/// Event delivered to [`crate::AppHandler::event`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlatformEvent {
    /// The drawable size changed (physical pixels). May be empty while minimised.
    Resized(PhysicalSize),
    /// The ratio of physical to logical pixels changed.
    ScaleFactorChanged(f64),
    /// The window gained (`true`) or lost (`false`) keyboard focus.
    Focused(bool),
    /// The window became fully hidden (`true`) or visible again (`false`). Not reported on
    /// Windows and Wayland; a minimised window reports an empty [`PlatformEvent::Resized`] there.
    Occluded(bool),
    /// The user asked to close the window. The loop exits after this event is handled.
    CloseRequested,
    /// Raw device input.
    Input(RawInputEvent),
}

/// Raw input as reported by the device. Mapping to game actions happens above this layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawInputEvent {
    /// A physical key changed state.
    Key {
        /// Physical key position (layout-independent).
        code: KeyCode,
        /// `true` when pressed, `false` when released.
        pressed: bool,
        /// `true` for OS key-repeat events.
        repeat: bool,
    },
    /// The cursor moved; position in physical pixels, origin at the top-left of the window.
    CursorMoved {
        /// Horizontal position.
        x: f64,
        /// Vertical position (grows downwards).
        y: f64,
    },
    /// A mouse button changed state.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// `true` when pressed.
        pressed: bool,
    },
    /// Scroll wheel movement in lines (positive `delta_y` scrolls up).
    MouseWheel {
        /// Horizontal scroll.
        delta_x: f32,
        /// Vertical scroll.
        delta_y: f32,
    },
}

/// Mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Wheel button.
    Middle,
    /// "Back" side button.
    Back,
    /// "Forward" side button.
    Forward,
    /// Any other button, by platform index.
    Other(u16),
}

/// Physical key positions, named after the US-QWERTY layout.
///
/// Physical positions keep bindings such as WASD stable across keyboard layouts
/// (on AZERTY the same keys are labelled ZQSD).
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum KeyCode {
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    /// A key without a mapping in this enum.
    Unidentified,
}
