//! Little-endian byte reader shared by the `InputLog` v1 and `Replay` v2 binary formats.
//!
//! Everything here is `pub(crate)`: it is an implementation detail of [`crate::input`] and
//! [`crate::replay`], not part of the public API.

use crate::error::SimError;
use crate::input::TickInput;

/// Cursor over a byte slice that reports [`SimError::UnexpectedEnd`] instead of panicking.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    /// Starts reading `bytes` from the beginning.
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    /// Current byte offset from the start of the input.
    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    /// Bytes left between the current offset and the end of the input.
    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    /// Reads and consumes the next `N` bytes as a fixed-size array.
    pub(crate) fn take<const N: usize>(&mut self) -> Result<[u8; N], SimError> {
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

    /// Reads and consumes the next `len` bytes as a slice.
    pub(crate) fn take_slice(&mut self, len: usize) -> Result<&'a [u8], SimError> {
        let end = self
            .offset
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(SimError::UnexpectedEnd {
                offset: self.offset,
                needed: len,
                available: self.remaining(),
            })?;
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }
}

/// Byte size of one encoded [`TickInput`]: [`crate::input::MAX_INPUT_SLOTS`] slots of 4 × `i16`
/// axes plus a `u32` button field.
pub(crate) const FRAME_SIZE: usize = crate::input::MAX_INPUT_SLOTS * (4 * 2 + 4);

/// Appends one [`TickInput`] to `bytes` in the shared frame encoding (v1 and v2 alike).
pub(crate) fn write_frame(bytes: &mut Vec<u8>, frame: &TickInput) {
    for slot in &frame.slots {
        for axis in slot.axes {
            bytes.extend_from_slice(&axis.to_le_bytes());
        }
        bytes.extend_from_slice(&slot.buttons.to_le_bytes());
    }
}

/// Reads one [`TickInput`] in the shared frame encoding (v1 and v2 alike).
pub(crate) fn read_frame(reader: &mut Reader<'_>) -> Result<TickInput, SimError> {
    let mut frame = TickInput::default();
    for slot in &mut frame.slots {
        for axis in &mut slot.axes {
            *axis = i16::from_le_bytes(reader.take()?);
        }
        slot.buttons = u32::from_le_bytes(reader.take()?);
    }
    Ok(frame)
}
