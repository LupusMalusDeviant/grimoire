//! Shared helpers for the plan 0002 WP2.8 render-test-scene harness (`snapshot_scenes.rs`) and its
//! milestone-showcase sibling (`pbr_stage_showcase.rs`): a self-contained, dependency-free RGBA8
//! PNG encoder/decoder, an [`Image`] type with the crop/compose helpers a comparison or a GIF
//! frame sequence needs, and the snapshot tolerance metric ([`compare`]).
//!
//! Pulled in via `#[path = "support/mod.rs"] mod support;` from each test file directly under
//! `tests/` (this file is *not* itself a `tests/*.rs` integration-test binary — Cargo only globs
//! direct children of `tests/`, so a `tests/support/` subdirectory is invisible to it — the
//! standard "shared test helper module" idiom). `tests/wp26_shadow_showcase.rs` (WP2.6, merged
//! earlier) keeps its own private, near-identical `png_writer` module rather than depending on
//! this one: it predates this file and touching already-merged test code is out of scope here.
//!
//! No PNG dependency exists anywhere in this workspace, deliberately (this repo is public, and no
//! other engine code needs one): this writer produces a valid PNG using only uncompressed
//! ("stored") `DEFLATE` blocks, which the zlib/PNG formats allow explicitly, and the matching
//! reader only ever has to understand its own output — not arbitrary PNGs (see [`decode_png`]'s
//! doc comment).

use std::io::Write as _;
use std::path::Path;

// --- Minimal PNG writer (RGBA8, uncompressed DEFLATE blocks only) ------------------------------

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Wraps `data` in a valid zlib stream made only of uncompressed ("stored", `BTYPE = 00`)
/// `DEFLATE` blocks (max 65535 bytes each).
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 8);
    out.push(0x78);
    out.push(0x01);
    let mut i = 0;
    if data.is_empty() {
        out.push(1); // BFINAL = 1, BTYPE = 00, one empty final block.
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    }
    while i < data.len() {
        let remaining = data.len() - i;
        let block_len = remaining.min(65535);
        let is_final = i + block_len >= data.len();
        out.push(u8::from(is_final));
        let len = block_len as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&data[i..i + block_len]);
        i += block_len;
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Encodes tightly packed, top-row-first RGBA8 `rgba` (`width * height * 4` bytes) as a PNG
/// (8-bit depth, colour type 6, filter type `None` on every scanline).
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * 4,
        "rgba length"
    );
    let row_bytes = width as usize * 4;
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in 0..height as usize {
        raw.push(0u8); // Filter type 0 (None) for every scanline.
        raw.extend_from_slice(&rgba[row * row_bytes..(row + 1) * row_bytes]);
    }
    let compressed = zlib_stored(&raw);

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit depth, colour type 6 (RGBA).
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &compressed);
    push_chunk(&mut out, b"IEND", &[]);
    out
}

/// Decodes a PNG produced by [`encode_png`] back into tightly packed, top-row-first RGBA8 bytes.
///
/// This is **not** a general-purpose PNG decoder: it understands exactly the subset [`encode_png`]
/// writes (a single `IHDR` describing 8-bit RGBA, one or more `IDAT` chunks whose concatenated
/// zlib stream contains only uncompressed `DEFLATE` blocks, filter type `None` on every scanline)
/// and returns a plain `String` error for anything else, including a real-world PNG produced by an
/// image editor or `pngcrush`. That is a deliberate, documented restriction (see this module's doc
/// comment): the only PNGs this harness ever needs to read back are ones it wrote itself, either
/// as a checked-in reference image or as a freshly rendered candidate.
fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return Err("not a PNG file (bad signature)".to_string());
    }
    let mut offset = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut idat = Vec::new();
    let mut saw_ihdr = false;
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let data_start = offset + 8;
        let data_end = data_start
            .checked_add(length)
            .ok_or("chunk length overflow")?;
        if data_end + 4 > bytes.len() {
            return Err("truncated chunk".to_string());
        }
        let data = &bytes[data_start..data_end];
        match kind {
            b"IHDR" => {
                if length != 13 {
                    return Err("unexpected IHDR length".to_string());
                }
                width = u32::from_be_bytes(data[0..4].try_into().unwrap());
                height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                let (bit_depth, color_type) = (data[8], data[9]);
                if bit_depth != 8 || color_type != 6 {
                    return Err(format!(
                        "unsupported IHDR: bit_depth={bit_depth} color_type={color_type} \
                         (only 8-bit RGBA is supported)"
                    ));
                }
                saw_ihdr = true;
            }
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        offset = data_end + 4; // Skip the trailing CRC.
    }
    if !saw_ihdr {
        return Err("missing IHDR chunk".to_string());
    }
    if idat.len() < 6 {
        return Err("IDAT too short for a zlib stream".to_string());
    }
    // Skip the 2-byte zlib header; the trailing 4-byte Adler-32 is not re-verified (this decoder
    // trusts its own writer and a checked-in reference file has not been transcoded since).
    let mut raw = Vec::new();
    let mut pos = 2usize;
    let deflate_end = idat.len() - 4;
    while pos < deflate_end {
        let is_final = idat[pos] & 1 != 0;
        let block_type = (idat[pos] >> 1) & 0b11;
        if block_type != 0 {
            return Err(format!(
                "unsupported DEFLATE block type {block_type} (only stored blocks are supported)"
            ));
        }
        pos += 1;
        if pos + 4 > deflate_end {
            return Err("truncated stored-block header".to_string());
        }
        let len = u16::from_le_bytes([idat[pos], idat[pos + 1]]) as usize;
        pos += 4; // LEN + NLEN.
        if pos + len > deflate_end {
            return Err("truncated stored-block data".to_string());
        }
        raw.extend_from_slice(&idat[pos..pos + len]);
        pos += len;
        if is_final {
            break;
        }
    }

    let row_bytes = width as usize * 4;
    let expected = (row_bytes + 1) * height as usize;
    if raw.len() != expected {
        return Err(format!(
            "unexpected decompressed size: got {} bytes, expected {expected}",
            raw.len()
        ));
    }
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * (row_bytes + 1);
        let filter = raw[start];
        if filter != 0 {
            return Err(format!("unsupported filter type {filter} on row {row}"));
        }
        rgba.extend_from_slice(&raw[start + 1..start + 1 + row_bytes]);
    }
    Ok((width, height, rgba))
}

// --- Image: a tightly packed, top-row-first RGBA8 buffer ---------------------------------------

/// A tightly packed, top-row-first RGBA8 image.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn from_offscreen(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), width as usize * height as usize * 4);
        Self {
            width,
            height,
            rgba,
        }
    }

    /// Places `images` side by side (same height, extra rows below the shortest one left as-is),
    /// for an at-a-glance comparison strip or a "columns are frames" GIF-sequence contact sheet.
    ///
    /// `#[allow(dead_code)]`: each integration-test binary that pulls in this shared module via
    /// `#[path = "support/mod.rs"] mod support;` is compiled (and dead-code-checked) on its own, so
    /// a helper only some consumers need — today, only `snapshot_scenes.rs`'s mismatch reporting —
    /// still warns as unused in every consumer that does not call it.
    #[allow(dead_code)]
    pub fn beside(images: &[&Image]) -> Image {
        let height = images.iter().map(|image| image.height).max().unwrap_or(0);
        let width: u32 = images.iter().map(|image| image.width).sum();
        let mut out = vec![0u8; width as usize * height as usize * 4];
        let mut x_offset = 0u32;
        for image in images {
            for row in 0..image.height {
                let src_start = (row * image.width) as usize * 4;
                let dst_start = ((row * width) + x_offset) as usize * 4;
                let len = image.width as usize * 4;
                out[dst_start..dst_start + len]
                    .copy_from_slice(&image.rgba[src_start..src_start + len]);
            }
            x_offset += image.width;
        }
        Image {
            width,
            height,
            rgba: out,
        }
    }

    pub fn write_png(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = encode_png(self.width, self.height, &self.rgba);
        let mut file = std::fs::File::create(path)?;
        file.write_all(&bytes)
    }

    /// Reads back a PNG written by [`Image::write_png`] (or [`encode_png`] directly). See
    /// [`decode_png`]'s doc comment for the (deliberately narrow) format support.
    pub fn read_png(path: &Path) -> Result<Image, String> {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let (width, height, rgba) = decode_png(&bytes)?;
        Ok(Image {
            width,
            height,
            rgba,
        })
    }
}

// --- Snapshot tolerance metric (plan 0002 WP2.8) ------------------------------------------------

/// Result of comparing two same-sized [`Image`]s: the WP2.8 tolerance metric.
///
/// The metric is deliberately simple (a mean and a max, not a perceptual image-similarity score):
/// this harness's job is to catch a *rendering regression* (a shader bug, a broken uniform, a
/// mesh/material wired up wrong) against a *reference* someone already reviewed, not to model
/// human perception. Two independent numbers make the two failure modes it needs to tell apart
/// legible on their own: [`DiffMetric::mean_abs_diff`] catches a small, scene-wide shift (a
/// slightly different ambient term, a rounding change in the tone curve), while
/// [`DiffMetric::max_abs_diff`] catches one badly wrong region (a missing shadow, a flipped
/// normal) that a whole-image mean could otherwise dilute into invisibility.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffMetric {
    /// Mean absolute per-channel difference (R, G, B and A all counted), 0.0 (identical) to 255.0.
    pub mean_abs_diff: f64,
    /// Largest single-channel absolute difference anywhere in the image, 0 (identical) to 255.
    pub max_abs_diff: u8,
}

impl DiffMetric {
    /// Whether this diff is small enough to be within the tolerance every reference is checked
    /// against (see `snapshot_scenes.rs`'s module doc comment for the thresholds' rationale and
    /// for how a mismatch is reported without failing the run while WP2.8's warning mode applies).
    #[must_use]
    pub fn within_tolerance(&self) -> bool {
        self.mean_abs_diff <= MEAN_ABS_DIFF_TOLERANCE && self.max_abs_diff <= MAX_ABS_DIFF_TOLERANCE
    }
}

/// Tolerance for [`DiffMetric::mean_abs_diff`] (see `snapshot_scenes.rs`'s module doc comment):
/// generous enough to absorb the sub-pixel dithering/rounding noise a software rasterizer's own
/// driver version can introduce between two otherwise-identical runs, tight enough that a shader
/// or lighting regression touching a meaningful fraction of the frame still trips it.
pub const MEAN_ABS_DIFF_TOLERANCE: f64 = 3.0;

/// Tolerance for [`DiffMetric::max_abs_diff`] (see `snapshot_scenes.rs`'s module doc comment):
/// looser than the mean, because a single anti-aliased silhouette or shadow edge can legitimately
/// land on a different side of a pixel boundary between driver versions without any real
/// regression; a badly wrong region (a missing shadow, a flipped light) reaches far higher than
/// this on at least one channel.
pub const MAX_ABS_DIFF_TOLERANCE: u8 = 60;

/// Compares two images pixel-by-pixel; `None` if their dimensions differ (not itself a tolerance
/// question — a reference and a candidate of different sizes means the scene or the target
/// resolution changed, which this metric cannot and should not try to quantify).
#[must_use]
pub fn compare(reference: &Image, candidate: &Image) -> Option<DiffMetric> {
    if reference.width != candidate.width || reference.height != candidate.height {
        return None;
    }
    let mut sum_abs_diff: u64 = 0;
    let mut max_abs_diff: u8 = 0;
    for (a, b) in reference.rgba.iter().zip(&candidate.rgba) {
        let diff = a.abs_diff(*b);
        sum_abs_diff += u64::from(diff);
        max_abs_diff = max_abs_diff.max(diff);
    }
    let mean_abs_diff = sum_abs_diff as f64 / reference.rgba.len() as f64;
    Some(DiffMetric {
        mean_abs_diff,
        max_abs_diff,
    })
}

/// The current platform's snapshot-reference directory name, matching the CI matrix
/// (`windows-latest`, `ubuntu-latest`, `macos-latest`) and PRD-0018's per-driver-family adapter
/// table (OF-18.2): each name stands in for the adapter family the plan expects on that runner
/// (WARP, lavapipe, the paravirtual Metal device), not literally the operating system.
#[must_use]
pub fn platform_dir() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Image {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&rgba);
        }
        Image::from_offscreen(width, height, pixels)
    }

    #[test]
    fn png_roundtrip_preserves_pixels() {
        let mut rgba = Vec::new();
        for y in 0..7u32 {
            for x in 0..5u32 {
                rgba.extend_from_slice(&[(x * 37) as u8, (y * 53) as u8, (x + y) as u8, 255]);
            }
        }
        let image = Image::from_offscreen(5, 7, rgba);
        let dir = std::env::temp_dir().join(format!(
            "grimoire-support-roundtrip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("roundtrip.png");
        image.write_png(&path).expect("write png");
        let read_back = Image::read_png(&path).expect("read png");
        assert_eq!(read_back, image);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compare_identical_images_is_zero() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = a.clone();
        let metric = compare(&a, &b).expect("same size");
        assert_eq!(metric.mean_abs_diff, 0.0);
        assert_eq!(metric.max_abs_diff, 0);
        assert!(metric.within_tolerance());
    }

    #[test]
    fn compare_reports_mean_and_max_difference() {
        // A 10x10 image (400 channels total) so a single differing channel barely moves the mean,
        // isolating what each half of the metric actually catches (this module doc comment's
        // "one badly wrong region... a whole-image mean could otherwise dilute into invisibility").
        let mut a = solid(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        // One channel of one pixel differs by 20; the other 399 channels are identical.
        b.rgba[0] = 20;
        let metric = compare(&a, &b).expect("same size");
        assert_eq!(metric.max_abs_diff, 20);
        assert!((metric.mean_abs_diff - 20.0 / 400.0).abs() < 1e-9);
        assert!(metric.within_tolerance(), "well under both thresholds");

        // Push just the max past its tolerance; the mean barely moves on a 400-channel image, so
        // only `max_abs_diff` is responsible for this now failing `within_tolerance`.
        a.rgba[0] = 0;
        b.rgba[0] = 255;
        let metric = compare(&a, &b).expect("same size");
        assert!(
            metric.mean_abs_diff < MEAN_ABS_DIFF_TOLERANCE,
            "mean alone would still pass"
        );
        assert!(metric.max_abs_diff > MAX_ABS_DIFF_TOLERANCE);
        assert!(!metric.within_tolerance());
    }

    #[test]
    fn compare_returns_none_for_mismatched_dimensions() {
        let a = solid(4, 4, [0, 0, 0, 255]);
        let b = solid(4, 5, [0, 0, 0, 255]);
        assert!(compare(&a, &b).is_none());
    }

    #[test]
    fn decode_rejects_a_foreign_png_cleanly() {
        // A real PNG encoder's DEFLATE stream uses dynamic or fixed Huffman blocks (block type 1
        // or 2), never "stored" (block type 0); this decoder must fail cleanly, not panic or
        // silently return garbage pixels, if it is ever pointed at one by mistake.
        let mut fake = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_chunk(&mut fake, b"IHDR", &ihdr);
        // zlib header + one final block with type 2 (fixed Huffman) instead of 0 (stored).
        push_chunk(&mut fake, b"IDAT", &[0x78, 0x01, 0b0000_0111, 0, 0, 0, 0]);
        push_chunk(&mut fake, b"IEND", &[]);
        assert!(decode_png(&fake).is_err());
    }
}
