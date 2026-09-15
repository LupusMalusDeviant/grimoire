//! Mouse aiming and its deterministic quantisation into `TickInput` axes 2/3 (contract §9.2,
//! §9.4, plan 0002 WP2.2/WP2.4).
//!
//! The simulation never reads camera, pointer, viewport or interpolation state: it only ever sees
//! the quantised `i16` axes 2/3 of an [`grimoire_sim::InputFrame`], produced once per frame by the
//! main loop from [`sample_aim`] and recorded/replayed like any other input (contract §9.4,
//! "Mauszielen und Quantisierung"). [`quantize_aim`] and [`sample_aim`] below are the exact
//! reference implementation the contract specifies, so replaying a fixed, recorded `TickInput`
//! sequence gives identical simulation hashes no matter what camera produced the axes originally,
//! and (independently) the same pixel, camera, viewport and focus always quantise to the same
//! `i16` pair.

use grimoire_core::Vec2;
use grimoire_core::math::dmath;
use grimoire_platform::RawInputEvent;
use grimoire_render::Camera25D;

/// Smallest Chebyshev distance (world units) between the cursor's ground point and the focus
/// point that still counts as "aiming somewhere": closer than this, [`quantize_aim`] returns
/// `[0, 0]` ("no direction") instead of an arbitrarily noisy unit vector.
pub const AIM_MIN_DISTANCE: f32 = 0.01;

/// Quantises a ground-plane offset (cursor ground point minus focus point) into the `i16` pair
/// stored in `TickInput` axes 2/3 (contract §9.4).
///
/// `[0, 0]` means "no direction": either `offset` is not finite, or its Chebyshev magnitude is
/// below [`AIM_MIN_DISTANCE`]. Otherwise the result is `offset` normalised to a unit vector and
/// scaled to `±32767`, rounding halves away from zero and saturating on the `f32` -> `i16` cast
/// (both properties of Rust's `as` float-to-int cast, not extra code here). Only the direction is
/// encoded, never the distance.
///
/// # Determinism
/// Every operation here (`abs`, `dmath::max`, division, `dmath::sqrt`, `round`, the saturating
/// `as` cast) is exact IEEE-754 arithmetic that agrees bit-for-bit across platforms, so the same
/// `offset` always quantises to the same pair everywhere (contract §9.4).
#[must_use]
pub fn quantize_aim(offset: Vec2) -> [i16; 2] {
    if !offset.x.is_finite() || !offset.y.is_finite() {
        return [0, 0];
    }
    let m = dmath::max(offset.x.abs(), offset.y.abs());
    if m < AIM_MIN_DISTANCE {
        return [0, 0];
    }
    // Components in [-1, 1] first (Chebyshev scaling), so squaring for the unit-vector division
    // below cannot overflow even for huge finite offsets.
    let s = offset / m;
    let n = s / dmath::sqrt(s.length_squared());
    [
        (n.x * 32767.0).round() as i16,
        (n.y * 32767.0).round() as i16,
    ]
}

/// Casts a ray from `camera` through `cursor` (physical pixels, contract §9.2 [`PointerState`]
/// convention) onto the ground plane and quantises the offset from `focus` (contract §9.4).
///
/// Returns `None` — never NaN — exactly when [`Camera25D::screen_to_ground`] does: the ray is
/// parallel to the ground plane (at the horizon) or the pixel would be behind the camera. The main
/// loop leaves the previously sampled axes unchanged when this returns `None` (contract §9.3 step
/// 3).
#[must_use]
pub fn sample_aim(
    camera: &Camera25D,
    cursor: [f32; 2],
    viewport: [f32; 2],
    focus: Vec2,
) -> Option<[i16; 2]> {
    let [gx, gy] = camera.screen_to_ground(cursor, viewport)?;
    Some(quantize_aim(Vec2::new(gx, gy) - focus))
}

/// Last known cursor position, in physical pixels with the origin at the window's top-left and Y
/// growing downwards (contract §9.2).
///
/// Deliberately separate from [`crate::InputState`]: that type stays `Eq` (a frozen P0 contract),
/// which an `f32` position would break.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointerState {
    position: Option<[f32; 2]>,
}

impl PointerState {
    /// A pointer state with no known position yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the position from a raw platform event; every other event variant is ignored.
    /// The last [`RawInputEvent::CursorMoved`] wins and stays current until the next one — a
    /// focus loss does not clear it, and [`RawInputEvent`] has no "cursor left the window"
    /// variant (contract §9.3).
    pub fn apply(&mut self, event: &RawInputEvent) {
        if let RawInputEvent::CursorMoved { x, y } = *event {
            self.position = Some([x as f32, y as f32]);
        }
    }

    /// The last known position, or `None` before the first [`RawInputEvent::CursorMoved`].
    #[must_use]
    pub fn position(&self) -> Option<[f32; 2]> {
        self.position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- quantize_aim: contract §9.4 table test ----------------------------------------------

    #[test]
    fn axis_aligned_offsets_quantise_to_the_axis_extremes() {
        assert_eq!(quantize_aim(Vec2::new(10.0, 0.0)), [32767, 0]);
        assert_eq!(quantize_aim(Vec2::new(-10.0, 0.0)), [-32767, 0]);
        assert_eq!(quantize_aim(Vec2::new(0.0, 10.0)), [0, 32767]);
        assert_eq!(quantize_aim(Vec2::new(0.0, -10.0)), [0, -32767]);
    }

    #[test]
    fn diagonal_offsets_quantise_to_a_unit_vector() {
        let [x, y] = quantize_aim(Vec2::new(1.0, 1.0));
        // A perfect diagonal: both axes carry the same magnitude, `1/sqrt(2) * 32767`.
        assert_eq!(x, y);
        let expected = (32767.0 / dmath::sqrt(2.0)).round() as i16;
        assert_eq!(x, expected);
    }

    #[test]
    fn distance_does_not_change_the_quantised_direction() {
        let near = quantize_aim(Vec2::new(1.0, 2.0));
        let far = quantize_aim(Vec2::new(100.0, 200.0));
        assert_eq!(near, far, "only direction is encoded, never distance");
    }

    #[test]
    fn offsets_at_and_around_aim_min_distance() {
        // Chebyshev magnitude exactly at the threshold is "close enough to aim": `m < AIM_MIN_DISTANCE`
        // is a strict `<`.
        assert_ne!(quantize_aim(Vec2::new(AIM_MIN_DISTANCE, 0.0)), [0, 0]);
        assert_eq!(quantize_aim(Vec2::new(AIM_MIN_DISTANCE * 0.5, 0.0)), [0, 0]);
        assert_eq!(quantize_aim(Vec2::ZERO), [0, 0]);
    }

    #[test]
    fn non_finite_offsets_quantise_to_no_direction() {
        assert_eq!(quantize_aim(Vec2::new(f32::NAN, 0.0)), [0, 0]);
        assert_eq!(quantize_aim(Vec2::new(0.0, f32::NAN)), [0, 0]);
        assert_eq!(quantize_aim(Vec2::new(f32::INFINITY, 0.0)), [0, 0]);
        assert_eq!(quantize_aim(Vec2::new(f32::NEG_INFINITY, 1.0)), [0, 0]);
    }

    #[test]
    fn huge_finite_offsets_do_not_overflow() {
        // A perfect diagonal at an enormous magnitude still quantises to a unit vector, not the
        // huge value itself, and every `i16` result is inherently within range by construction —
        // the point of this test is that neither the Chebyshev scaling nor the unit-vector
        // division panics or produces NaN/infinity for `1e30`.
        let diagonal = quantize_aim(Vec2::new(1.0e30, 1.0e30));
        assert_eq!(diagonal, quantize_aim(Vec2::new(1.0, 1.0)));
        assert_eq!(quantize_aim(Vec2::new(1.0e30, 0.0)), [32767, 0]);
    }

    #[test]
    fn rounding_at_half_steps_goes_away_from_zero() {
        // A direction whose scaled component lands exactly on a `.5` boundary rounds away from
        // zero (`f32::round`'s documented behaviour), not to even.
        let value = 0.5_f32 / 32767.0;
        assert_eq!((value * 32767.0).round(), 1.0);
        let value = -0.5_f32 / 32767.0;
        assert_eq!((value * 32767.0).round(), -1.0);
    }

    // --- sample_aim: roundtrip and quantisation determinism (WP2.4 gate) ---------------------

    fn test_camera() -> Camera25D {
        // `Camera25D` is `#[non_exhaustive]`: adjust by field assignment onto `default()`
        // (contract §2 rule 13), not by struct-literal or functional-update syntax, both
        // forbidden outside the defining crate.
        let mut camera = Camera25D::default();
        camera.target = [0.0, 0.0];
        camera.tilt_degrees = 65.0;
        camera.fov_y_degrees = 50.0;
        camera.distance = 15.0;
        camera
    }

    #[test]
    fn sample_aim_same_pixel_camera_and_focus_always_quantise_the_same() {
        let camera = test_camera();
        let viewport = [1280.0, 720.0];
        let focus = Vec2::new(2.0, 3.0);
        let cursor = [700.0, 500.0];
        let first = sample_aim(&camera, cursor, viewport, focus);
        for _ in 0..50 {
            assert_eq!(sample_aim(&camera, cursor, viewport, focus), first);
        }
    }

    #[test]
    fn sample_aim_table_against_recorded_constants() {
        // Fixed pixel/camera/viewport/focus table (contract §9.4 test list): pins concrete `i16`
        // values, not just "it round-trips", so a change to the projection or the quantisation
        // formula is caught even if it happens to keep round-tripping with itself.
        let camera = test_camera();
        let viewport = [800.0, 600.0];
        let focus = Vec2::ZERO;
        let cases: [([f32; 2], [i16; 2]); 3] = [
            // The centre pixel always hits the ground at `camera.target` (== focus here), so the
            // offset is zero: "no direction".
            ([400.0, 300.0], [0, 0]),
            // Above the vertical centre: further from the camera, straight "up" in ground Y.
            ([400.0, 200.0], [0, 32767]),
            // Below the vertical centre: straight "down" in ground Y (towards the camera).
            ([400.0, 400.0], [0, -32767]),
        ];
        for (cursor, expected) in cases {
            assert_eq!(
                sample_aim(&camera, cursor, viewport, focus),
                Some(expected),
                "cursor {cursor:?}"
            );
        }
    }

    #[test]
    fn sample_aim_horizon_and_parallel_ray_return_none_without_nan() {
        let mut camera = Camera25D::default();
        camera.tilt_degrees = 0.0;
        let viewport = [800.0, 600.0];
        // Vertical centre with zero tilt: the ray is exactly parallel to the ground plane (see
        // `screen_to_ground_ray_exactly_parallel_to_the_ground_plane_returns_none` in
        // `grimoire_render::stage3d`).
        assert_eq!(
            sample_aim(&camera, [400.0, 300.0], viewport, Vec2::ZERO),
            None
        );

        // A very wide field of view lets the top row of pixels look above the horizon even at a
        // moderate tilt (same construction as `grimoire_render::stage3d`'s
        // `screen_to_ground_above_the_horizon_returns_none_not_a_behind_camera_point`).
        let mut wide_fov = Camera25D::default();
        wide_fov.tilt_degrees = 60.0;
        wide_fov.fov_y_degrees = 170.0;
        assert_eq!(
            sample_aim(&wide_fov, [400.0, 0.0], [800.0, 600.0], Vec2::ZERO),
            None
        );
    }

    #[test]
    fn sample_aim_never_panics_or_nans_for_non_finite_inputs() {
        let camera = test_camera();
        let viewport = [800.0, 600.0];
        for cursor in [[f32::NAN, 0.0], [f32::INFINITY, 300.0], [400.0, f32::NAN]] {
            assert_eq!(
                sample_aim(&camera, cursor, viewport, Vec2::ZERO),
                None,
                "cursor {cursor:?} must not yield a ground point"
            );
        }
        assert_eq!(
            sample_aim(&camera, [400.0, 300.0], [0.0, 600.0], Vec2::ZERO),
            None
        );
    }

    // --- PointerState -------------------------------------------------------------------------

    #[test]
    fn pointer_state_starts_with_no_position() {
        assert_eq!(PointerState::new().position(), None);
    }

    #[test]
    fn pointer_state_keeps_the_last_cursor_moved_position() {
        let mut pointer = PointerState::new();
        pointer.apply(&RawInputEvent::CursorMoved { x: 12.0, y: 34.0 });
        assert_eq!(pointer.position(), Some([12.0, 34.0]));
        pointer.apply(&RawInputEvent::Key {
            code: grimoire_platform::KeyCode::Space,
            pressed: true,
            repeat: false,
        });
        assert_eq!(
            pointer.position(),
            Some([12.0, 34.0]),
            "non-cursor events must not clear or move the position"
        );
        pointer.apply(&RawInputEvent::CursorMoved { x: 5.0, y: 6.0 });
        assert_eq!(pointer.position(), Some([5.0, 6.0]));
    }
}
