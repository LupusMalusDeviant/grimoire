//! Mapping of raw device input to the quantised [`InputFrame`] of a simulation tick.

use std::collections::BTreeSet;

use grimoire_platform::{KeyCode, MouseButton, RawInputEvent};
use grimoire_sim::InputFrame;

/// Largest axis magnitude stored in an [`InputFrame`].
pub const AXIS_MAX: i16 = i16::MAX;

/// Number of analogue axes per [`InputFrame`].
pub const AXIS_COUNT: usize = 4;

/// Number of button bits per [`InputFrame`].
pub const BUTTON_COUNT: u8 = 32;

/// A physical input that can be bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InputSource {
    /// A key by physical position.
    Key(KeyCode),
    /// A mouse button.
    Mouse(MouseButton),
}

/// What a held [`InputSource`] contributes to the [`InputFrame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputAction {
    /// Sets bit `0..32` of [`InputFrame::buttons`].
    Button(u8),
    /// Adds `value` to axis `axis` (`0..4`); contributions are summed and clamped to `±32767`.
    Axis {
        /// Axis index.
        axis: usize,
        /// Signed contribution, usually `±32767` for a digital key.
        value: i16,
    },
}

/// Which keys and mouse buttons are held, fed from raw platform events.
///
/// Besides the held set, every press is latched until [`InputState::clear_presses`]. A press that
/// is released again before the next sample therefore still counts as active once, so short taps
/// between two simulation ticks are not lost (PRD-0013: a raw event reaches the input of the next
/// tick). Whoever samples the state clears the latch after the sample has reached a tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    held: BTreeSet<InputSource>,
    latched: BTreeSet<InputSource>,
}

impl InputState {
    /// Creates a state with nothing held and no press latched.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one raw event: a press holds and latches the source, a release only stops holding
    /// it. Key-repeat events and cursor or wheel movement change nothing.
    pub fn apply(&mut self, event: &RawInputEvent) {
        let (source, pressed) = match *event {
            RawInputEvent::Key { repeat: true, .. } => return,
            RawInputEvent::Key { code, pressed, .. } => (InputSource::Key(code), pressed),
            RawInputEvent::MouseButton { button, pressed } => (InputSource::Mouse(button), pressed),
            RawInputEvent::CursorMoved { .. } | RawInputEvent::MouseWheel { .. } => return,
        };
        if pressed {
            self.held.insert(source);
            self.latched.insert(source);
        } else {
            self.held.remove(&source);
        }
    }

    /// Releases everything and drops latched presses, e.g. when the window loses focus and
    /// release events would be missed.
    pub fn release_all(&mut self) {
        self.held.clear();
        self.latched.clear();
    }

    /// Drops the latched presses; held sources stay held. Call it once a sample of this state has
    /// been fed to at least one tick.
    pub fn clear_presses(&mut self) {
        self.latched.clear();
    }

    /// Whether `source` is held right now.
    #[must_use]
    pub fn is_held(&self, source: InputSource) -> bool {
        self.held.contains(&source)
    }

    /// Whether `source` is held or was pressed since the last [`InputState::clear_presses`];
    /// this is what [`InputMap::sample`] reads.
    #[must_use]
    pub fn is_active(&self, source: InputSource) -> bool {
        self.held.contains(&source) || self.latched.contains(&source)
    }

    /// Whether nothing is held and no press is latched.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty() && self.latched.is_empty()
    }
}

/// Bindings from keys and mouse buttons to the buttons and axes of an [`InputFrame`].
///
/// [`InputMap::default`] is the P0 preset:
///
/// | Input | Action |
/// |-------|--------|
/// | `D` / `ArrowRight`, `A` / `ArrowLeft` | axis 0 `+32767` / `-32767` (right is positive) |
/// | `W` / `ArrowUp`, `S` / `ArrowDown` | axis 1 `+32767` / `-32767` (up is positive) |
/// | `Space` | button 0 |
/// | `ShiftLeft` | button 1 |
/// | left mouse button | button 2 |
///
/// Axes 2 and 3 are reserved for aiming and stay 0 in P0: cursor aiming needs the camera, which
/// the input map does not know yet. Diagonal movement is not normalised; the game decides how to
/// combine axes 0 and 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputMap {
    bindings: Vec<(InputSource, InputAction)>,
}

impl Default for InputMap {
    fn default() -> Self {
        let mut map = Self::new();
        let digital_axes = [
            (KeyCode::KeyD, 0, AXIS_MAX),
            (KeyCode::ArrowRight, 0, AXIS_MAX),
            (KeyCode::KeyA, 0, -AXIS_MAX),
            (KeyCode::ArrowLeft, 0, -AXIS_MAX),
            (KeyCode::KeyW, 1, AXIS_MAX),
            (KeyCode::ArrowUp, 1, AXIS_MAX),
            (KeyCode::KeyS, 1, -AXIS_MAX),
            (KeyCode::ArrowDown, 1, -AXIS_MAX),
        ];
        for (code, axis, value) in digital_axes {
            map.bind(InputSource::Key(code), InputAction::Axis { axis, value });
        }
        map.bind(InputSource::Key(KeyCode::Space), InputAction::Button(0));
        map.bind(InputSource::Key(KeyCode::ShiftLeft), InputAction::Button(1));
        map.bind(
            InputSource::Mouse(MouseButton::Left),
            InputAction::Button(2),
        );
        map
    }
}

impl InputMap {
    /// Creates a map without bindings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// Adds a binding. A source may carry several actions; all of them apply.
    ///
    /// # Panics
    /// If a button bit is `>= 32` or an axis index is `>= 4`.
    pub fn bind(&mut self, source: InputSource, action: InputAction) -> &mut Self {
        match action {
            InputAction::Button(bit) => assert!(
                bit < BUTTON_COUNT,
                "button bit {bit} out of range 0..{BUTTON_COUNT}"
            ),
            InputAction::Axis { axis, .. } => {
                assert!(
                    axis < AXIS_COUNT,
                    "axis index {axis} out of range 0..{AXIS_COUNT}"
                );
            }
        }
        self.bindings.push((source, action));
        self
    }

    /// Builder form of [`InputMap::bind`].
    ///
    /// # Panics
    /// Like [`InputMap::bind`].
    #[must_use]
    pub fn with(mut self, source: InputSource, action: InputAction) -> Self {
        self.bind(source, action);
        self
    }

    /// Removes every binding of `source`.
    pub fn unbind(&mut self, source: InputSource) {
        self.bindings.retain(|(bound, _)| *bound != source);
    }

    /// All bindings in insertion order.
    #[must_use]
    pub fn bindings(&self) -> &[(InputSource, InputAction)] {
        &self.bindings
    }

    /// Quantises the active inputs of `state` (held or latched, see [`InputState::is_active`])
    /// into one [`InputFrame`].
    ///
    /// Axis contributions of active sources are summed per axis and clamped to `±32767`, so
    /// opposite keys cancel out and duplicate bindings do not overflow.
    #[must_use]
    pub fn sample(&self, state: &InputState) -> InputFrame {
        let mut sums = [0i32; AXIS_COUNT];
        let mut buttons = 0u32;
        for &(source, action) in &self.bindings {
            if !state.is_active(source) {
                continue;
            }
            match action {
                InputAction::Button(bit) => buttons |= 1 << bit,
                InputAction::Axis { axis, value } => sums[axis] += i32::from(value),
            }
        }
        let limit = i32::from(AXIS_MAX);
        let axes = sums.map(|sum| {
            // The clamp keeps the value inside i16, so the conversion cannot fail.
            i16::try_from(sum.clamp(-limit, limit)).unwrap_or(0)
        });
        InputFrame { axes, buttons }
    }
}
