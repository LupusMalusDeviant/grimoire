//! RGBA8 images: PNG output, luminance, crops, downscaling and hashing.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;
use std::sync::OnceLock;

use crate::materials::srgb_to_linear;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Tightly packed sRGB-encoded RGBA8 rows, top row first.
    pub rgba: Vec<u8>,
}

fn linear_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut lut = [0.0; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = srgb_to_linear(i as f32 / 255.0);
        }
        lut
    })
}

/// Linear value of an sRGB-encoded byte.
pub fn byte_to_linear(b: u8) -> f32 {
    linear_lut()[b as usize]
}

/// WCAG relative luminance of an sRGB-encoded RGB triple.
pub fn relative_luminance(rgb: [u8; 3]) -> f32 {
    0.2126 * byte_to_linear(rgb[0]) + 0.7152 * byte_to_linear(rgb[1]) + 0.0722 * byte_to_linear(rgb[2])
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![0; width as usize * height as usize * 4],
        }
    }

    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), width as usize * height as usize * 4, "image buffer size");
        Self { width, height, rgba }
    }

    fn offset(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * 4
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let o = self.offset(x, y);
        [self.rgba[o], self.rgba[o + 1], self.rgba[o + 2], self.rgba[o + 3]]
    }

    pub fn luminance(&self, x: u32, y: u32) -> f32 {
        let p = self.pixel(x, y);
        relative_luminance([p[0], p[1], p[2]])
    }

    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Image {
        assert!(x + width <= self.width && y + height <= self.height, "crop inside the image");
        let mut out = Image::new(width, height);
        for row in 0..height {
            let src = self.offset(x, y + row);
            let dst = out.offset(0, row);
            let len = width as usize * 4;
            out.rgba[dst..dst + len].copy_from_slice(&self.rgba[src..src + len]);
        }
        out
    }

    /// 2x2 box downscale, averaged in linear light.
    pub fn downscale_half(&self) -> Image {
        let (w, h) = (self.width / 2, self.height / 2);
        let mut out = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = self.pixel(2 * x + dx, 2 * y + dy);
                    for c in 0..3 {
                        acc[c] += byte_to_linear(p[c]);
                    }
                    acc[3] += f32::from(p[3]) / 255.0;
                }
                let o = out.offset(x, y);
                for c in 0..3 {
                    let v = crate::materials::linear_to_srgb(acc[c] * 0.25);
                    out.rgba[o + c] = (v * 255.0).round() as u8;
                }
                out.rgba[o + 3] = (acc[3] * 0.25 * 255.0).round() as u8;
            }
        }
        out
    }

    /// Copies `src` with its top-left corner at (`x`, `y`).
    pub fn blit(&mut self, src: &Image, x: u32, y: u32) {
        assert!(x + src.width <= self.width && y + src.height <= self.height, "blit inside the image");
        for row in 0..src.height {
            let s = src.offset(0, row);
            let d = self.offset(x, y + row);
            let len = src.width as usize * 4;
            self.rgba[d..d + len].copy_from_slice(&src.rgba[s..s + len]);
        }
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, width: u32, height: u32, rgba: [u8; 4]) {
        let x1 = (x + width).min(self.width);
        let y1 = (y + height).min(self.height);
        for py in y..y1 {
            for px in x..x1 {
                let o = self.offset(px, py);
                self.rgba[o..o + 4].copy_from_slice(&rgba);
            }
        }
    }

    pub fn write_png(&self, path: &Path) -> Result<(), String> {
        let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        writer.finish().map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Reads an RGBA8 PNG as written by [`Image::write_png`].
    pub fn read_png(path: &Path) -> Result<Image, String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let file = File::open(path).map_err(|e| fail(&e))?;
        let mut reader = png::Decoder::new(BufReader::new(file)).read_info().map_err(|e| fail(&e))?;
        let (color, depth) = reader.output_color_type();
        if color != png::ColorType::Rgba || depth != png::BitDepth::Eight {
            return Err(fail(&format!("expected RGBA8, found {color:?} {depth:?}")));
        }
        let size = reader.output_buffer_size().ok_or_else(|| fail(&"image too large"))?;
        let mut rgba = vec![0; size];
        let info = reader.next_frame(&mut rgba).map_err(|e| fail(&e))?;
        rgba.truncate(info.buffer_size());
        Ok(Image::from_rgba(info.width, info.height, rgba))
    }
}

/// FNV-1a, 64 bit. Used only to compare renders for byte identity.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// IEEE 754 half to single precision.
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let mantissa = f32::from(bits & 0x3FF);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => {
            if mantissa == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x4B80), 15.0);
        assert!((f16_to_f32(0x3600) - 0.375).abs() < 1e-6);
    }

    #[test]
    fn luminance_of_white_and_black() {
        assert!((relative_luminance([255, 255, 255]) - 1.0).abs() < 1e-4);
        assert_eq!(relative_luminance([0, 0, 0]), 0.0);
    }

    #[test]
    fn crop_blit_and_downscale_keep_pixels() {
        let mut img = Image::new(4, 4);
        img.fill_rect(2, 0, 2, 2, [255, 0, 0, 255]);
        let crop = img.crop(2, 0, 2, 2);
        assert!(crop.rgba.chunks(4).all(|p| p == [255, 0, 0, 255]));
        let half = img.downscale_half();
        assert_eq!(half.pixel(1, 0), [255, 0, 0, 255]);
        assert_eq!(half.pixel(0, 1), [0, 0, 0, 0]);
        let mut canvas = Image::new(6, 6);
        canvas.blit(&crop, 4, 4);
        assert_eq!(canvas.pixel(5, 5), [255, 0, 0, 255]);
    }

    #[test]
    fn png_round_trip_is_lossless() {
        let mut img = Image::new(7, 5);
        for (i, byte) in img.rgba.iter_mut().enumerate() {
            *byte = (i * 37 % 256) as u8;
        }
        let path = std::env::temp_dir().join(format!("look-dev-png-test-{}.png", std::process::id()));
        img.write_png(&path).expect("write");
        let back = Image::read_png(&path).expect("read");
        let _ = std::fs::remove_file(&path);
        assert_eq!(back, img);
    }

    #[test]
    fn fnv_distinguishes_bytes() {
        assert_ne!(fnv1a64(&[0, 1]), fnv1a64(&[1, 0]));
        assert_eq!(fnv1a64(b""), 0xCBF2_9CE4_8422_2325);
    }
}
