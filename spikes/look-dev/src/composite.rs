//! Side-by-side composites. No text is rendered; the legend and column order live in metrics.md.

use crate::image_io::Image;

pub const CELL_WIDTH: u32 = 640;
pub const CELL_HEIGHT: u32 = 360;
pub const GUTTER: u32 = 4;
const GUTTER_COLOR: [u8; 4] = [0, 0, 0, 255];

/// Native-resolution crop rectangle of `composite_crops.png`: x, y, width, height.
pub const CROP_RECT: (u32, u32, u32, u32) = (320, 300, CELL_WIDTH, CELL_HEIGHT);

/// 3 columns (toon, stylized, realistic) x 2 rows (calm, busy) of half-scale 1280x720 frames,
/// 1920x720 in total. The 4 px gutters are painted over the touching cell edges (2 px of each
/// neighbour), so every cell keeps its position.
pub fn side_by_side(rows: &[[&Image; 3]; 2]) -> Image {
    let mut out = Image::new(3 * CELL_WIDTH, 2 * CELL_HEIGHT);
    for (row, frames) in rows.iter().enumerate() {
        for (column, frame) in frames.iter().enumerate() {
            let half = frame.downscale_half();
            assert_eq!((half.width, half.height), (CELL_WIDTH, CELL_HEIGHT), "frames are 1280x720");
            out.blit(&half, column as u32 * CELL_WIDTH, row as u32 * CELL_HEIGHT);
        }
    }
    paint_column_gutters(&mut out);
    out.fill_rect(0, CELL_HEIGHT - GUTTER / 2, out.width, GUTTER, GUTTER_COLOR);
    out
}

/// 3 native crops (`CROP_RECT`) side by side, 1920x360.
pub fn crops(frames: [&Image; 3]) -> Image {
    let (x, y, w, h) = CROP_RECT;
    let mut out = Image::new(3 * w, h);
    for (column, frame) in frames.iter().enumerate() {
        out.blit(&frame.crop(x, y, w, h), column as u32 * w, 0);
    }
    paint_column_gutters(&mut out);
    out
}

fn paint_column_gutters(out: &mut Image) {
    for column in 1..3 {
        out.fill_rect(column * CELL_WIDTH - GUTTER / 2, 0, GUTTER, out.height, GUTTER_COLOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(value: u8) -> Image {
        let mut img = Image::new(1280, 720);
        img.fill_rect(0, 0, 1280, 720, [value, value, value, 255]);
        img
    }

    #[test]
    fn side_by_side_places_columns_and_rows() {
        let frames: Vec<Image> = (0..6).map(|i| solid(40 * (i + 1))).collect();
        let out = side_by_side(&[[&frames[0], &frames[1], &frames[2]], [&frames[3], &frames[4], &frames[5]]]);
        assert_eq!((out.width, out.height), (1920, 720));
        assert_eq!(out.pixel(100, 100)[0], 40);
        assert_eq!(out.pixel(1000, 100)[0], 80);
        assert_eq!(out.pixel(1800, 600)[0], 240);
        assert_eq!(out.pixel(640, 100), [0, 0, 0, 255]);
        assert_eq!(out.pixel(100, 359), [0, 0, 0, 255]);
    }

    #[test]
    fn crops_keep_native_pixels() {
        let frames: Vec<Image> = (0..3).map(|i| solid(50 * (i + 1))).collect();
        let out = crops([&frames[0], &frames[1], &frames[2]]);
        assert_eq!((out.width, out.height), (1920, 360));
        assert_eq!(out.pixel(1300, 10)[0], 150);
        assert_eq!(out.pixel(1279, 10), [0, 0, 0, 255]);
    }
}
