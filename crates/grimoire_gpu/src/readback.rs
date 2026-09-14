//! Pure helpers for texture read-back.

/// Bytes per row of a texture-to-buffer copy for `width` RGBA8 pixels, rounded up to
/// [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
///
/// Returns `None` if the result does not fit into a `u32`.
#[must_use]
pub const fn padded_bytes_per_row(width: u32) -> Option<u32> {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let Some(unpadded) = width.checked_mul(4) else {
        return None;
    };
    unpadded.div_ceil(align).checked_mul(align)
}

/// Copies `height` rows of `width * 4` bytes out of `padded`, whose rows are
/// `padded_row` bytes long, into a tightly packed buffer (row order preserved).
///
/// Rows missing from a too short `padded` buffer are omitted.
#[must_use]
pub fn strip_row_padding(padded: &[u8], width: u32, height: u32, padded_row: u32) -> Vec<u8> {
    let row = width as usize * 4;
    let padded_row = padded_row as usize;
    let mut packed = Vec::with_capacity(row * height as usize);
    if row == 0 || padded_row < row {
        return packed;
    }
    for chunk in padded.chunks(padded_row).take(height as usize) {
        if let Some(pixels) = chunk.get(..row) {
            packed.extend_from_slice(pixels);
        }
    }
    packed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_rounds_up_to_alignment() {
        assert_eq!(padded_bytes_per_row(1), Some(256));
        assert_eq!(padded_bytes_per_row(64), Some(256));
        assert_eq!(padded_bytes_per_row(65), Some(512));
        assert_eq!(padded_bytes_per_row(100), Some(512));
        assert_eq!(padded_bytes_per_row(128), Some(512));
        assert_eq!(padded_bytes_per_row(0), Some(0));
    }

    #[test]
    fn padding_reports_overflow() {
        assert_eq!(padded_bytes_per_row(u32::MAX / 4 + 1), None);
        // Fits unpadded but not after rounding up to the alignment.
        assert_eq!(padded_bytes_per_row(u32::MAX / 4), None);
        assert_eq!(padded_bytes_per_row(1 << 29), Some(1 << 31));
    }

    #[test]
    fn padded_rows_are_multiples_of_alignment_and_large_enough() {
        for width in 1..2048 {
            let padded = padded_bytes_per_row(width).expect("small widths fit");
            assert_eq!(padded % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
            assert!(padded >= width * 4);
            assert!(padded - width * 4 < wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        }
    }

    #[test]
    fn strip_keeps_row_order_and_drops_padding() {
        let width = 3;
        let height = 2;
        let padded_row = padded_bytes_per_row(width).expect("small widths fit");
        let mut padded = vec![0xEE; (padded_row * height) as usize];
        for y in 0..height as usize {
            for byte in 0..12 {
                padded[y * padded_row as usize + byte] = (y * 16 + byte) as u8;
            }
        }
        let packed = strip_row_padding(&padded, width, height, padded_row);
        assert_eq!(packed.len(), 24);
        assert_eq!(&packed[..12], &(0..12).collect::<Vec<u8>>()[..]);
        assert_eq!(&packed[12..], &(16..28).collect::<Vec<u8>>()[..]);
        assert!(!packed.contains(&0xEE));
    }

    #[test]
    fn strip_rejects_inconsistent_row_length() {
        assert!(strip_row_padding(&[0; 16], 8, 1, 16).is_empty());
    }
}
