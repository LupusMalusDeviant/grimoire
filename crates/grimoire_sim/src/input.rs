//! Per-tick input: [`InputFrame`], [`TickInput`] and the serialisable [`InputLog`].

use grimoire_core::impl_stable_hash;

use crate::error::SimError;

/// Number of input slots (players or bots) per tick.
pub const MAX_INPUT_SLOTS: usize = 4;

/// Scale that maps the stored axis value `±32767` to `±1.0`.
const AXIS_SCALE: f32 = 32_767.0;

/// Input of one slot for one tick, already quantised so it hashes and serialises exactly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputFrame {
    /// Analogue axes, normalised to `±32767`. `-32768` is read as `-1.0` as well.
    pub axes: [i16; 4],
    /// Button bit field; bit `n` set means button `n` is held.
    pub buttons: u32,
}

impl_stable_hash!(InputFrame { axes, buttons });

impl InputFrame {
    /// Axis `index` as `f32` in `[-1, 1]` (`value / 32767`, exact IEEE division).
    ///
    /// Returns `0.0` for `index >= 4`; never panics.
    #[must_use]
    pub fn axis(&self, index: usize) -> f32 {
        self.axes.get(index).map_or(0.0, |&value| {
            grimoire_core::math::dmath::max(f32::from(value) / AXIS_SCALE, -1.0)
        })
    }

    /// Whether button `bit` is held.
    ///
    /// Returns `false` for `bit >= 32`; never panics.
    #[must_use]
    pub fn is_pressed(&self, bit: u8) -> bool {
        self.buttons
            .checked_shr(u32::from(bit))
            .is_some_and(|shifted| shifted & 1 == 1)
    }
}

/// Input of all slots for one tick. Also the resource systems read during a step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickInput {
    /// One frame per slot; unused slots stay at their default.
    pub slots: [InputFrame; MAX_INPUT_SLOTS],
}

impl_stable_hash!(TickInput { slots });

/// Recorded run: seed, tick rate and the input of every tick in order. Replays are rebuilt from
/// it (see [`crate::replay`]).
///
/// # Binary format (version 1)
///
/// All integers little-endian:
///
/// | Offset | Size | Field |
/// |--------|------|-------|
/// | 0 | 8 | magic `b"GRIMREPL"` |
/// | 8 | 4 | format version `u32` = 1 |
/// | 12 | 8 | `seed: u64` |
/// | 20 | 4 | `tick_rate_hz: u32` (never 0) |
/// | 24 | 8 | frame count `u64` |
/// | 32 | 48 × count | frames; per frame [`MAX_INPUT_SLOTS`] slots of 4 × `i16` axes, `u32` buttons |
///
/// Trailing bytes are rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputLog {
    /// Seed the recorded simulation was created with.
    pub seed: u64,
    /// Tick rate of the recording.
    pub tick_rate_hz: u32,
    /// Input of each tick, starting with tick 0.
    pub frames: Vec<TickInput>,
}

const HEADER_SIZE: usize = 32;
const SLOT_SIZE: usize = 4 * 2 + 4;
const FRAME_SIZE: usize = MAX_INPUT_SLOTS * SLOT_SIZE;

impl InputLog {
    /// Magic bytes at the start of every encoded log.
    pub const MAGIC: [u8; 8] = *b"GRIMREPL";
    /// Binary format version written by [`InputLog::to_bytes`].
    pub const FORMAT_VERSION: u32 = 1;

    /// Encodes the log in the binary format described on [`InputLog`].
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_SIZE + FRAME_SIZE * self.frames.len());
        bytes.extend_from_slice(&Self::MAGIC);
        bytes.extend_from_slice(&Self::FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.seed.to_le_bytes());
        bytes.extend_from_slice(&self.tick_rate_hz.to_le_bytes());
        bytes.extend_from_slice(&(self.frames.len() as u64).to_le_bytes());
        for frame in &self.frames {
            for slot in &frame.slots {
                for axis in slot.axes {
                    bytes.extend_from_slice(&axis.to_le_bytes());
                }
                bytes.extend_from_slice(&slot.buttons.to_le_bytes());
            }
        }
        bytes
    }

    /// Decodes a log written by [`InputLog::to_bytes`].
    ///
    /// Never panics. The frame count is validated against the actual input length before
    /// anything is allocated, so memory use is bounded by the size of `bytes`.
    ///
    /// # Errors
    ///
    /// [`SimError::UnexpectedEnd`] for a truncated header, [`SimError::BadMagic`],
    /// [`SimError::UnsupportedVersion`], [`SimError::InvalidTickRate`] for 0 Hz and
    /// [`SimError::FrameDataLength`] when the payload is not exactly `count × 48` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SimError> {
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take::<8>()? != Self::MAGIC {
            return Err(SimError::BadMagic);
        }
        let version = u32::from_le_bytes(reader.take()?);
        if version != Self::FORMAT_VERSION {
            return Err(SimError::UnsupportedVersion(version));
        }
        let seed = u64::from_le_bytes(reader.take()?);
        let tick_rate_hz = u32::from_le_bytes(reader.take()?);
        if tick_rate_hz == 0 {
            return Err(SimError::InvalidTickRate);
        }
        let declared = u64::from_le_bytes(reader.take()?);

        let remaining = reader.remaining();
        let expected = usize::try_from(declared)
            .ok()
            .and_then(|count| count.checked_mul(FRAME_SIZE));
        if expected != Some(remaining) {
            return Err(SimError::FrameDataLength {
                frames: declared,
                remaining,
            });
        }

        let count = remaining / FRAME_SIZE;
        let mut frames = Vec::with_capacity(count);
        for _ in 0..count {
            let mut frame = TickInput::default();
            for slot in &mut frame.slots {
                for axis in &mut slot.axes {
                    *axis = i16::from_le_bytes(reader.take()?);
                }
                slot.buttons = u32::from_le_bytes(reader.take()?);
            }
            frames.push(frame);
        }
        Ok(Self {
            seed,
            tick_rate_hz,
            frames,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], SimError> {
        let chunk = self
            .bytes
            .get(self.offset..)
            .and_then(|rest| rest.first_chunk::<N>())
            .ok_or(SimError::UnexpectedEnd {
                offset: self.offset,
                needed: N,
                available: self.remaining(),
            })?;
        self.offset += N;
        Ok(*chunk)
    }
}
