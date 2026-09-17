//! A 5×7 bitmap font and pixel-exact screen-space sprites for debug text (plan 0002 WP6.4).
//!
//! The glyph atlas is generated in code: every glyph is a 5×7 bit pattern in [`GLYPH_ROWS`], drawn
//! for this engine, so no font file and no font licence are involved. Text is rasterised into
//! [`SpriteInstance`] quads (one per horizontal run of set pixels) and drawn by the unchanged
//! sprite pass; [`ScreenSpace`] places them on whole screen pixels through the stage's
//! [`Camera2D`], so glyphs stay crisp at every integer scale.

use grimoire_render::{Camera2D, SpriteInstance, shape};

/// Width of a glyph in glyph pixels.
pub const GLYPH_WIDTH: u32 = 5;
/// Height of a glyph in glyph pixels.
pub const GLYPH_HEIGHT: u32 = 7;
/// Horizontal advance per character in glyph pixels: the glyph plus one column of spacing.
pub const GLYPH_ADVANCE: u32 = GLYPH_WIDTH + 1;

/// Every glyph of the atlas: the character and its seven rows, top row first. Within a row, bit 4
/// is the leftmost pixel and bit 0 the rightmost. Lowercase letters use the uppercase glyphs.
#[rustfmt::skip]
pub const GLYPH_ROWS: [(char, [u8; 7]); 55] = [
    (' ', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
    ('0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
    ('1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    ('2', [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111]),
    ('3', [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110]),
    ('4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
    ('5', [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
    ('6', [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
    ('7', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000]),
    ('8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
    ('9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100]),
    ('A', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    ('B', [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110]),
    ('C', [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110]),
    ('D', [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100]),
    ('E', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111]),
    ('F', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000]),
    ('G', [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111]),
    ('H', [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    ('I', [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    ('J', [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100]),
    ('K', [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001]),
    ('L', [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111]),
    ('M', [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001]),
    ('N', [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001]),
    ('O', [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
    ('P', [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000]),
    ('Q', [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101]),
    ('R', [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001]),
    ('S', [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110]),
    ('T', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
    ('U', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
    ('V', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100]),
    ('W', [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010]),
    ('X', [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001]),
    ('Y', [0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100]),
    ('Z', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111]),
    ('.', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100]),
    (',', [0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b00100, 0b01000]),
    (':', [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000]),
    ('/', [0b00000, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000]),
    ('-', [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000]),
    ('+', [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000]),
    ('=', [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000]),
    ('~', [0b00000, 0b00000, 0b01000, 0b10101, 0b00010, 0b00000, 0b00000]),
    ('%', [0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011]),
    ('_', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111]),
    ('(', [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010]),
    (')', [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000]),
    ('<', [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010]),
    ('>', [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000]),
    ('!', [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100]),
    ('#', [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010]),
    ('*', [0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000]),
    ('?', [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100]),
];

/// Rows of the glyph for `character`. Lowercase letters use the uppercase glyph; a character
/// without a glyph shows `?`.
#[must_use]
pub fn glyph(character: char) -> [u8; 7] {
    let character = character.to_ascii_uppercase();
    GLYPH_ROWS
        .iter()
        .find(|(known, _)| *known == character)
        .or_else(|| GLYPH_ROWS.iter().find(|(known, _)| *known == '?'))
        .map_or([0; 7], |&(_, rows)| rows)
}

/// Whether `character` (or its uppercase form) has a glyph of its own.
#[must_use]
pub fn has_glyph(character: char) -> bool {
    let character = character.to_ascii_uppercase();
    GLYPH_ROWS.iter().any(|(known, _)| *known == character)
}

/// Width of `text` in glyph pixels: the advance of every character except the trailing spacing
/// column (0 for empty text).
#[must_use]
pub fn text_width(text: &str) -> u32 {
    let characters = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    characters.saturating_mul(GLYPH_ADVANCE).saturating_sub(1)
}

/// Maps whole screen pixels (origin top left, Y down) to world-space sprites of a [`Camera2D`] on
/// a viewport, for debug drawing on `StageFrame::debug_sprites`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenSpace {
    /// World position of the top-left corner of the viewport.
    origin: [f32; 2],
    /// World units per screen pixel.
    world_per_pixel: f32,
}

impl ScreenSpace {
    /// The mapping for `camera` on a viewport of `viewport` physical pixels, or `None` if the
    /// viewport is empty or the camera is degenerate (non-finite centre, non-positive height).
    #[must_use]
    pub fn new(camera: &Camera2D, viewport: [f32; 2]) -> Option<Self> {
        let [width, height] = viewport;
        let usable = width.is_finite()
            && height.is_finite()
            && width >= 1.0
            && height >= 1.0
            && camera.world_height.is_finite()
            && camera.world_height > 0.0
            && camera.center.iter().all(|value| value.is_finite());
        if !usable {
            return None;
        }
        let world_per_pixel = camera.world_height / height;
        Some(Self {
            origin: [
                camera.center[0] - width * 0.5 * world_per_pixel,
                camera.center[1] + height * 0.5 * world_per_pixel,
            ],
            world_per_pixel,
        })
    }

    /// A quad covering exactly the pixels `x..x + width` × `y..y + height`.
    #[must_use]
    pub fn rect(&self, x: u32, y: u32, width: u32, height: u32, color: [f32; 4]) -> SpriteInstance {
        let half = [
            width as f32 * 0.5 * self.world_per_pixel,
            height as f32 * 0.5 * self.world_per_pixel,
        ];
        SpriteInstance {
            position: [
                self.origin[0] + x as f32 * self.world_per_pixel + half[0],
                self.origin[1] - y as f32 * self.world_per_pixel - half[1],
            ],
            half_size: half,
            rotation: 0.0,
            shape: shape::QUAD,
            color,
        }
    }

    /// Appends `text` with its top-left corner at pixel `(x, y)`, every glyph pixel `scale` screen
    /// pixels wide and high, as one quad per horizontal run of set glyph pixels. Returns the width
    /// drawn in screen pixels.
    pub fn push_text(
        &self,
        out: &mut Vec<SpriteInstance>,
        x: u32,
        y: u32,
        scale: u32,
        text: &str,
        color: [f32; 4],
    ) -> u32 {
        let scale = scale.max(1);
        let mut cursor = x;
        for character in text.chars() {
            for (row, bits) in (0u32..).zip(glyph(character)) {
                let mut column = 0;
                while column < GLYPH_WIDTH {
                    if bits & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                        column += 1;
                        continue;
                    }
                    let start = column;
                    while column < GLYPH_WIDTH && bits & (1 << (GLYPH_WIDTH - 1 - column)) != 0 {
                        column += 1;
                    }
                    out.push(self.rect(
                        cursor + start * scale,
                        y + row * scale,
                        (column - start) * scale,
                        scale,
                        color,
                    ));
                }
            }
            cursor = cursor.saturating_add(GLYPH_ADVANCE * scale);
        }
        text_width(text).saturating_mul(scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_unique_uses_five_columns_and_the_digits_and_letters_are_complete() {
        for (index, (character, rows)) in GLYPH_ROWS.iter().enumerate() {
            assert!(
                rows.iter().all(|row| row >> GLYPH_WIDTH == 0),
                "{character:?} uses more than five columns"
            );
            for (other, other_rows) in &GLYPH_ROWS[index + 1..] {
                assert_ne!(character, other, "duplicate glyph entry");
                if *character != ' ' {
                    assert_ne!(
                        rows, other_rows,
                        "{character:?} and {other:?} look identical"
                    );
                }
            }
        }
        for character in ('0'..='9').chain('A'..='Z') {
            assert!(has_glyph(character), "{character:?} has no glyph");
            assert!(
                glyph(character).iter().any(|row| *row != 0),
                "{character:?} is empty"
            );
        }
        assert_eq!(glyph('s'), glyph('S'));
        assert_eq!(glyph('\u{2603}'), glyph('?'));
        assert!(!has_glyph('\u{2603}'));
    }

    fn rasterise(
        sprites: &[SpriteInstance],
        space: &ScreenSpace,
        width: u32,
        height: u32,
    ) -> Vec<bool> {
        let mut mask = vec![false; (width * height) as usize];
        for sprite in sprites {
            for y in 0..height {
                for x in 0..width {
                    // Pixel centre in world space, as the sprite pass samples it.
                    let wx = space.origin[0] + (x as f32 + 0.5) * space.world_per_pixel;
                    let wy = space.origin[1] - (y as f32 + 0.5) * space.world_per_pixel;
                    if (wx - sprite.position[0]).abs() <= sprite.half_size[0]
                        && (wy - sprite.position[1]).abs() <= sprite.half_size[1]
                    {
                        mask[(y * width + x) as usize] = true;
                    }
                }
            }
        }
        mask
    }

    #[test]
    fn text_lands_on_whole_pixels_at_every_scale() {
        let camera = Camera2D {
            center: [12.5, -3.0],
            world_height: 37.0,
        };
        let (width, height) = (40, 20);
        let space = ScreenSpace::new(&camera, [width as f32, height as f32]).expect("usable");
        for scale in 1..=2 {
            let mut sprites = Vec::new();
            let drawn = space.push_text(&mut sprites, 1, 2, scale, "T1", [1.0; 4]);
            assert_eq!(drawn, 11 * scale);
            let mask = rasterise(&sprites, &space, width, height);
            let mut expected = vec![false; (width * height) as usize];
            for (index, character) in "T1".chars().enumerate() {
                let left = 1 + index as u32 * GLYPH_ADVANCE * scale;
                for (row, bits) in glyph(character).iter().enumerate() {
                    for column in 0..GLYPH_WIDTH {
                        if bits & (1 << (GLYPH_WIDTH - 1 - column)) != 0 {
                            for dy in 0..scale {
                                for dx in 0..scale {
                                    let px = left + column * scale + dx;
                                    let py = 2 + row as u32 * scale + dy;
                                    expected[(py * width + px) as usize] = true;
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(mask, expected, "scale {scale}");
        }
    }

    #[test]
    fn runs_of_set_pixels_become_one_quad() {
        let space = ScreenSpace::new(&Camera2D::default(), [100.0, 100.0]).expect("usable");
        let mut sprites = Vec::new();
        space.push_text(&mut sprites, 0, 0, 1, "-", [1.0; 4]);
        assert_eq!(sprites.len(), 1, "the dash is one run of five pixels");
        sprites.clear();
        space.push_text(&mut sprites, 0, 0, 1, "T", [1.0; 4]);
        assert_eq!(sprites.len(), 7, "the bar and six stem pixels");
    }

    #[test]
    fn degenerate_cameras_and_viewports_draw_nothing() {
        let mut camera = Camera2D::default();
        assert!(ScreenSpace::new(&camera, [0.0, 10.0]).is_none());
        assert!(ScreenSpace::new(&camera, [10.0, f32::NAN]).is_none());
        camera.world_height = 0.0;
        assert!(ScreenSpace::new(&camera, [10.0, 10.0]).is_none());
        camera.world_height = 1.0;
        camera.center = [f32::INFINITY, 0.0];
        assert!(ScreenSpace::new(&camera, [10.0, 10.0]).is_none());
        assert_eq!(text_width(""), 0);
        assert_eq!(text_width("AB"), 11);
    }
}
