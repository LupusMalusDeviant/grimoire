//! Minimal, self-contained RGBA8 PNG writer for this crate's showcase test
//! (`tests/figure_pack.rs`), mirroring `crates/grimoire_render/tests/wp26_shadow_showcase.rs`'s own
//! private copy rather than depending on `grimoire_render`'s test-only `tests/support/mod.rs`
//! (invisible outside that crate's own test binaries — Cargo test helper modules do not cross
//! crates).
//!
//! No PNG-encoding dependency exists anywhere in this workspace, deliberately (this repo is
//! public, and no other engine code needs one): this writer produces a valid PNG using only
//! uncompressed ("stored") `DEFLATE` blocks, which the zlib/PNG formats allow explicitly.

#![allow(dead_code)]

use std::io::Write as _;
use std::path::Path;

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

/// A tightly packed, top-row-first RGBA8 image, for cropping/compositing before encoding.
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

    /// Places `images` side by side (same height), for an at-a-glance rest-vs-bent comparison.
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
}
