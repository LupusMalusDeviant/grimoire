//! Bullet-cloud lights derived from the bullet channel (plan 0002 WP3.5, PRD-0003 rule 5 / FR-15).
//!
//! WP3.4 built the only permitted way to create a bullet light, [`crate::point_light_from_bullet`],
//! and the cap on its contribution to the environment ([`crate::BulletLightCap`], applied in
//! `mesh_pass::build_light_list`). This module routes the actual bullet channel,
//! [`crate::StageFrame::bullets`], through that path: the renderer derives a handful of lights from
//! the bullets it draws and appends them after the frame's own [`crate::StageFrame::point_lights`].
//! A game never builds such a light by hand (PO decision 2026-09-16).
//!
//! **Why clouds, not bullets.** Ten thousand bullets cannot each be a light — the whole clustered
//! forward+ budget is 32 or 256 lights ([`crate::LightBudget`]). The stylebook speaks of
//! "Bullet-Cluster-Lichter": light from bullet *clouds*. So the derivation bins every accepted,
//! glowing bullet into a world-aligned grid of [`BULLET_LIGHT_CELL_SIZE`] cells around the camera
//! target, keeps the [`BULLET_LIGHT_MAX`] cells with the largest summed glow, and turns each of
//! them into one representative [`BulletInstance`] that goes through
//! [`crate::point_light_from_bullet`]:
//!
//! - `position`: the glow-weighted centroid of the cell's bullets;
//! - `glow`: the brightest bullet's glow, so a dense cloud is not brighter than its brightest
//!   member — the cap stays a cap however many bullets pile up;
//! - `radius`: the largest bullet radius, but at least one cell's reach
//!   (`BULLET_LIGHT_CELL_SIZE / BULLET_LIGHT_RANGE_PER_RADIUS`), so the light covers its cloud.
//!
//! **Determinism and cost.** The result depends only on the frame (never on the renderer, its
//! budget or the GPU), so `NullRenderer` and `WgpuRenderer` count identically (contract §2a). Ties
//! break by cell index; the chosen lights are emitted in ascending cell order. One pass over the
//! bullets, fixed-size stack storage, no heap allocation.
//!
//! **Budget interplay.** Derived lights come after the frame's own lights, so when a frame exceeds
//! its light budget the budget clamp drops bullet lights first; they are counted like any other
//! over-budget light ([`crate::StageStats::point_lights_over_budget`]).
//!
//! All numbers here are provisional (look review, P-11), like the constants behind
//! [`crate::point_light_from_bullet`] itself.

use crate::stage::{BULLET_PASS_PALETTE_SPACE, BulletInstance, StageFrame, is_accepted_bullet};
use crate::stage3d::{BULLET_LIGHT_RANGE_PER_RADIUS, PointLight, point_light_from_bullet};

/// Most bullet-cloud lights derived per frame. Provisional: small enough to leave three quarters
/// of the `Low` budget (32) to the scene's own lights.
pub(crate) const BULLET_LIGHT_MAX: usize = 8;

/// Edge length of one grid cell, in world units. Provisional: several bullet radii of the Sigil
/// corpus (0.12 to 0.3 units), roughly a fifth of the default camera's visible ground width.
pub(crate) const BULLET_LIGHT_CELL_SIZE: f32 = 4.0;

/// Cells per grid axis; the grid covers `BULLET_LIGHT_GRID * BULLET_LIGHT_CELL_SIZE` world units
/// around the camera target. Bullets outside it contribute no light (they are far off screen).
pub(crate) const BULLET_LIGHT_GRID: usize = 16;

const CELL_COUNT: usize = BULLET_LIGHT_GRID * BULLET_LIGHT_GRID;

/// The lights derived for one frame; at most [`BULLET_LIGHT_MAX`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct BulletLights {
    lights: [PointLight; BULLET_LIGHT_MAX],
    len: usize,
}

impl BulletLights {
    const EMPTY: Self = Self {
        lights: [PointLight {
            position: [0.0; 3],
            color: [0.0; 3],
            intensity: 0.0,
            range: 1.0,
            is_bullet_light: false,
            casts_shadow: false,
        }; BULLET_LIGHT_MAX],
        len: 0,
    };

    /// The derived lights, in ascending cell order.
    pub(crate) fn as_slice(&self) -> &[PointLight] {
        &self.lights[..self.len]
    }
}

/// Accumulated glow of one grid cell. `f64` sums: ten thousand bullets times glow 255 times a
/// coordinate of a few hundred units exceeds what `f32` holds to the needed precision.
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    weight: f64,
    sum_x: f64,
    sum_y: f64,
    max_glow: u8,
    max_radius: f32,
}

/// Derives the bullet-cloud lights of `frame` (see the module documentation). Only bullets the
/// bullet pass draws ([`is_accepted_bullet`]) with `glow > 0` contribute.
pub(crate) fn derive_bullet_lights(frame: &StageFrame) -> BulletLights {
    let mut out = BulletLights::EMPTY;
    if frame.bullets.is_empty() {
        return out;
    }
    let anchor = frame
        .camera_25d
        .map_or(frame.base.camera.center, |camera| camera.target);
    if !(anchor[0].is_finite() && anchor[1].is_finite()) {
        return out;
    }
    let cell_size = f64::from(BULLET_LIGHT_CELL_SIZE);
    let half = (BULLET_LIGHT_GRID / 2) as f64;
    let origin_x = (f64::from(anchor[0]) / cell_size).floor() - half;
    let origin_y = (f64::from(anchor[1]) / cell_size).floor() - half;
    let grid = BULLET_LIGHT_GRID as f64;

    let mut cells = [Cell::default(); CELL_COUNT];
    for bullet in &frame.bullets {
        if bullet.glow == 0 || !is_accepted_bullet(bullet) {
            continue;
        }
        let x = f64::from(bullet.position[0]);
        let y = f64::from(bullet.position[1]);
        let column = (x / cell_size).floor() - origin_x;
        let row = (y / cell_size).floor() - origin_y;
        if !(0.0..grid).contains(&column) || !(0.0..grid).contains(&row) {
            continue;
        }
        // In range and integral by construction, so the casts are exact.
        let index = row as usize * BULLET_LIGHT_GRID + column as usize;
        let cell = &mut cells[index];
        let weight = f64::from(bullet.glow);
        cell.weight += weight;
        cell.sum_x += weight * x;
        cell.sum_y += weight * y;
        cell.max_glow = cell.max_glow.max(bullet.glow);
        cell.max_radius = cell.max_radius.max(bullet.radius);
    }

    let mut occupied = [0u16; CELL_COUNT];
    let mut occupied_len = 0;
    for (index, cell) in cells.iter().enumerate() {
        if cell.weight > 0.0 {
            occupied[occupied_len] = index as u16;
            occupied_len += 1;
        }
    }
    let occupied = &mut occupied[..occupied_len];
    // Heaviest first, ties by ascending cell index: a total order, so the choice is deterministic.
    occupied.sort_unstable_by(|&a, &b| {
        cells[usize::from(b)]
            .weight
            .total_cmp(&cells[usize::from(a)].weight)
            .then(a.cmp(&b))
    });
    let chosen = &mut occupied[..occupied_len.min(BULLET_LIGHT_MAX)];
    chosen.sort_unstable();

    let min_radius = BULLET_LIGHT_CELL_SIZE / BULLET_LIGHT_RANGE_PER_RADIUS;
    for &index in chosen.iter() {
        let cell = &cells[usize::from(index)];
        let representative = BulletInstance {
            position: [
                (cell.sum_x / cell.weight) as f32,
                (cell.sum_y / cell.weight) as f32,
            ],
            radius: cell.max_radius.max(min_radius),
            rotation: 0.0,
            silhouette: 0,
            palette: 0,
            palette_space: BULLET_PASS_PALETTE_SPACE,
            glow: cell.max_glow,
            flags: 0,
        };
        out.lights[out.len] = point_light_from_bullet(&representative);
        out.len += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::{bullet_palette, palette_space};

    fn glowing_bullet(x: f32, y: f32, glow: u8) -> BulletInstance {
        BulletInstance {
            position: [x, y],
            radius: 0.25,
            rotation: 0.0,
            silhouette: 0,
            palette: 0,
            palette_space: BULLET_PASS_PALETTE_SPACE,
            glow,
            flags: 0,
        }
    }

    fn frame_with(bullets: Vec<BulletInstance>) -> StageFrame {
        let mut frame = StageFrame::new();
        frame.bullets = bullets;
        frame
    }

    #[test]
    fn no_bullets_no_lights() {
        assert!(
            derive_bullet_lights(&StageFrame::new())
                .as_slice()
                .is_empty()
        );
    }

    #[test]
    fn bullets_without_glow_or_outside_the_pass_give_no_light() {
        let frame = frame_with(vec![
            glowing_bullet(1.0, 1.0, 0),
            BulletInstance {
                palette_space: palette_space::FRIENDLY,
                ..glowing_bullet(1.0, 1.0, 255)
            },
            BulletInstance {
                palette: bullet_palette::COUNT,
                ..glowing_bullet(1.0, 1.0, 255)
            },
            glowing_bullet(f32::NAN, 1.0, 255),
        ]);
        assert!(derive_bullet_lights(&frame).as_slice().is_empty());
    }

    #[test]
    fn one_cloud_gives_one_capped_bullet_light_at_its_glow_weighted_centre() {
        let frame = frame_with(vec![
            glowing_bullet(1.0, 1.0, 100),
            glowing_bullet(3.0, 1.0, 100),
            glowing_bullet(2.0, 3.0, 200),
        ]);
        let lights = derive_bullet_lights(&frame);
        let lights = lights.as_slice();
        assert_eq!(lights.len(), 1);
        let light = lights[0];
        assert!(
            light.is_bullet_light,
            "only point_light_from_bullet builds it"
        );
        assert!(light.is_valid());
        // Weighted centroid: x = (100 + 300 + 400) / 400 = 2, y = (100 + 100 + 600) / 400 = 2.
        assert!((light.position[0] - 2.0).abs() < 1e-5);
        assert!((light.position[1] - 2.0).abs() < 1e-5);
        let expected = point_light_from_bullet(&BulletInstance {
            position: [2.0, 2.0],
            radius: BULLET_LIGHT_CELL_SIZE / BULLET_LIGHT_RANGE_PER_RADIUS,
            glow: 200,
            ..glowing_bullet(0.0, 0.0, 0)
        });
        assert_eq!(
            light.intensity, expected.intensity,
            "brightest member's glow, not the sum"
        );
        assert_eq!(light.range, expected.range, "at least one cell of reach");
    }

    #[test]
    fn many_clouds_keep_only_the_heaviest_in_cell_order() {
        // Twelve clouds in twelve different cells along +X, weight rising with x, all inside the
        // grid around the default camera centre (0, 0).
        let mut bullets = Vec::new();
        for i in 0..12 {
            let x = -24.0 + 4.0 * i as f32 + 2.0;
            for _ in 0..=i {
                bullets.push(glowing_bullet(x, 2.0, 50));
            }
        }
        let lights = derive_bullet_lights(&frame_with(bullets));
        let lights = lights.as_slice();
        assert_eq!(lights.len(), BULLET_LIGHT_MAX);
        let xs: Vec<f32> = lights.iter().map(|light| light.position[0]).collect();
        // The eight heaviest are the last eight clouds (i = 4..12), emitted in ascending x.
        let expected: Vec<f32> = (4..12).map(|i| -24.0 + 4.0 * i as f32 + 2.0).collect();
        assert_eq!(xs, expected);
    }

    #[test]
    fn equal_weights_break_ties_by_cell_index() {
        let mut bullets = Vec::new();
        for i in 0..(BULLET_LIGHT_MAX + 2) {
            bullets.push(glowing_bullet(-30.0 + 4.0 * i as f32, 0.5, 80));
        }
        let first = derive_bullet_lights(&frame_with(bullets.clone()));
        bullets.reverse();
        let reversed = derive_bullet_lights(&frame_with(bullets));
        assert_eq!(
            first.as_slice(),
            reversed.as_slice(),
            "bullet order must not matter"
        );
        assert!((first.as_slice()[0].position[0] - -30.0).abs() < 1e-5);
    }

    #[test]
    fn the_grid_follows_the_camera_target_and_ignores_far_bullets() {
        let far = glowing_bullet(500.0, 500.0, 255);
        assert!(
            derive_bullet_lights(&frame_with(vec![far]))
                .as_slice()
                .is_empty()
        );
        let mut frame = frame_with(vec![far]);
        frame.camera_25d = Some(crate::Camera25D {
            target: [498.0, 503.0],
            ..crate::Camera25D::default()
        });
        assert_eq!(derive_bullet_lights(&frame).as_slice().len(), 1);
    }

    #[test]
    fn only_point_light_from_bullet_sets_is_bullet_light_in_crate_code() {
        // PO decision 2026-09-16, structurally: outside test modules, the literal that sets the flag
        // appears exactly once in this crate — inside `point_light_from_bullet`. This module
        // routes the bullet channel through that function instead of building lights itself.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        for entry in std::fs::read_dir(&src).expect("src is readable") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .expect("source is readable")
                .replace("\r\n", "\n");
            // Everything before the file's unit-test module; a stray `#[cfg(test)]` on a single
            // helper item earlier in a file must not hide the production code after it.
            let production = text
                .find("\n#[cfg(test)]\nmod tests")
                .map_or(text.as_str(), |end| &text[..end]);
            for (line_no, line) in production.lines().enumerate() {
                if line.contains("is_bullet_light: true") {
                    hits.push(format!("{}:{}", path.display(), line_no + 1));
                }
            }
        }
        assert_eq!(
            hits.len(),
            1,
            "unexpected bullet-light constructors: {hits:?}"
        );
        assert!(hits[0].contains("stage3d.rs"), "{hits:?}");
    }
}
