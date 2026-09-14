//! Deterministic math for simulation code.
//!
//! Floating-point rules for everything that influences simulation state:
//!
//! - IEEE-754 basic operations on `f32` (`+ - * / %`, negation, `sqrt`, `abs`, `floor`, `ceil`,
//!   `round`, `trunc`, `clamp`, casts) are exactly specified and therefore identical on every
//!   platform.
//! - `f32::min`/`max` are not: the zero sign of `min(+0.0, -0.0)` changes even with the
//!   optimisation level, and the state hash distinguishes `-0.0` from `+0.0`. Use
//!   [`dmath::min`]/[`dmath::max`].
//! - Transcendental functions from `std` (`sin`, `cos`, `tan`, `atan2`, `exp`, `ln`, `powf`,
//!   hyperbolic and logarithmic variants, `cbrt`, ...) call the platform's C math library, whose
//!   results differ between operating systems. This holds for `f64` as well. Simulation code uses
//!   [`dmath`] instead, a pure-Rust implementation.
//! - `mul_add` and `powi` (precision documented as non-deterministic) are not used.
//! - NaN must never enter simulation state, and code never inspects NaN sign or payload.
//!
//! The simulation-side crates enforce everything except the NaN rule with clippy
//! (`disallowed-methods` in their `clippy.toml`). Whether plain `f32` with these rules is
//! bit-identical across Windows, Linux and macOS (x86_64 and arm64) is verified by golden tests
//! in CI; see `docs/adr/0004-deterministische-gleitkommaarithmetik.md`.

use core::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

use crate::hash::{StableHash, StableHasher};

pub mod dmath {
    //! Pure-Rust transcendental functions with platform-independent results (backed by `libm`).

    /// Archimedes' constant π.
    pub const PI: f32 = core::f32::consts::PI;
    /// Full turn τ = 2π.
    pub const TAU: f32 = core::f32::consts::TAU;
    /// Quarter turn π/2.
    pub const FRAC_PI_2: f32 = core::f32::consts::FRAC_PI_2;

    /// Sine of `x` (radians).
    #[inline]
    #[must_use]
    pub fn sin(x: f32) -> f32 {
        libm::sinf(x)
    }

    /// Cosine of `x` (radians).
    #[inline]
    #[must_use]
    pub fn cos(x: f32) -> f32 {
        libm::cosf(x)
    }

    /// Tangent of `x` (radians).
    #[inline]
    #[must_use]
    pub fn tan(x: f32) -> f32 {
        libm::tanf(x)
    }

    /// Arcsine of `x`, in radians.
    #[inline]
    #[must_use]
    pub fn asin(x: f32) -> f32 {
        libm::asinf(x)
    }

    /// Arccosine of `x`, in radians.
    #[inline]
    #[must_use]
    pub fn acos(x: f32) -> f32 {
        libm::acosf(x)
    }

    /// Arctangent of `x`, in radians.
    #[inline]
    #[must_use]
    pub fn atan(x: f32) -> f32 {
        libm::atanf(x)
    }

    /// Four-quadrant arctangent of `y / x`, in radians.
    #[inline]
    #[must_use]
    pub fn atan2(y: f32, x: f32) -> f32 {
        libm::atan2f(y, x)
    }

    /// Natural exponential `e^x`.
    #[inline]
    #[must_use]
    pub fn exp(x: f32) -> f32 {
        libm::expf(x)
    }

    /// Natural logarithm of `x`.
    #[inline]
    #[must_use]
    pub fn ln(x: f32) -> f32 {
        libm::logf(x)
    }

    /// `x` raised to the power `y`.
    #[inline]
    #[must_use]
    pub fn powf(x: f32, y: f32) -> f32 {
        libm::powf(x, y)
    }

    /// Euclidean distance `sqrt(x² + y²)` without intermediate overflow.
    #[inline]
    #[must_use]
    pub fn hypot(x: f32, y: f32) -> f32 {
        libm::hypotf(x, y)
    }

    /// Square root. `f32::sqrt` is correctly rounded by IEEE-754 and thus deterministic.
    #[inline]
    #[must_use]
    pub fn sqrt(x: f32) -> f32 {
        x.sqrt()
    }

    /// Minimum of `a` and `b`. If both compare equal (including `+0.0` and `-0.0`), returns `a`.
    ///
    /// Unlike `f32::min`, the zero sign of the result does not depend on the compiler. NaN
    /// behaves as in `std`: a NaN operand is ignored unless both are NaN.
    #[inline]
    #[must_use]
    pub fn min(a: f32, b: f32) -> f32 {
        if b < a || a.is_nan() { b } else { a }
    }

    /// Maximum of `a` and `b`. If both compare equal (including `+0.0` and `-0.0`), returns `a`.
    ///
    /// Unlike `f32::max`, the zero sign of the result does not depend on the compiler. NaN
    /// behaves as in `std`: a NaN operand is ignored unless both are NaN.
    #[inline]
    #[must_use]
    pub fn max(a: f32, b: f32) -> f32 {
        if b > a || a.is_nan() { b } else { a }
    }
}

/// 2D vector on the gameplay plane. X points right, Y points up.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    /// Horizontal component.
    pub x: f32,
    /// Vertical component.
    pub y: f32,
}

impl Vec2 {
    /// `(0, 0)`.
    pub const ZERO: Self = Self::new(0.0, 0.0);
    /// `(1, 1)`.
    pub const ONE: Self = Self::new(1.0, 1.0);
    /// Unit vector along X.
    pub const X: Self = Self::new(1.0, 0.0);
    /// Unit vector along Y.
    pub const Y: Self = Self::new(0.0, 1.0);

    /// Creates a vector from its components.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Creates a vector with both components set to `value`.
    #[must_use]
    pub const fn splat(value: f32) -> Self {
        Self::new(value, value)
    }

    /// Unit vector pointing at `radians` (counter-clockwise from +X).
    #[must_use]
    pub fn from_angle(radians: f32) -> Self {
        Self::new(dmath::cos(radians), dmath::sin(radians))
    }

    /// Dot product.
    #[must_use]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// 2D cross product (z component of the 3D cross product).
    #[must_use]
    pub fn perp_dot(self, other: Self) -> f32 {
        self.x * other.y - self.y * other.x
    }

    /// Squared length.
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// Length.
    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Squared distance to `other`.
    #[must_use]
    pub fn distance_squared(self, other: Self) -> f32 {
        (self - other).length_squared()
    }

    /// Distance to `other`.
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        (self - other).length()
    }

    /// Unit vector in the same direction, or [`Vec2::ZERO`] for a zero-length or non-finite input.
    #[must_use]
    pub fn normalize_or_zero(self) -> Self {
        let length = self.length();
        if length > 0.0 && length.is_finite() {
            self / length
        } else {
            Self::ZERO
        }
    }

    /// Vector rotated by 90° counter-clockwise.
    #[must_use]
    pub const fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// Angle of the vector in radians (counter-clockwise from +X).
    #[must_use]
    pub fn angle(self) -> f32 {
        dmath::atan2(self.y, self.x)
    }

    /// Vector rotated counter-clockwise by `radians`.
    #[must_use]
    pub fn rotate(self, radians: f32) -> Self {
        let (sin, cos) = (dmath::sin(radians), dmath::cos(radians));
        Self::new(self.x * cos - self.y * sin, self.x * sin + self.y * cos)
    }

    /// Linear interpolation: `self` at `t = 0`, `other` at `t = 1`.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        self + (other - self) * t
    }

    /// Components as an array `[x, y]`.
    #[must_use]
    pub const fn to_array(self) -> [f32; 2] {
        [self.x, self.y]
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl MulAssign<f32> for Vec2 {
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl Div<f32> for Vec2 {
    type Output = Self;
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl Neg for Vec2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl StableHash for Vec2 {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_f32(self.x);
        hasher.write_f32(self.y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec2, b: Vec2) -> bool {
        (a.x - b.x).abs() < 1e-5 && (a.y - b.y).abs() < 1e-5
    }

    #[test]
    fn min_max_keep_first_operand_on_equal_zeros() {
        let (pos, neg) = (
            core::hint::black_box(0.0f32),
            core::hint::black_box(-0.0f32),
        );
        assert_eq!(dmath::min(pos, neg).to_bits(), 0x0000_0000);
        assert_eq!(dmath::min(neg, pos).to_bits(), 0x8000_0000);
        assert_eq!(dmath::max(pos, neg).to_bits(), 0x0000_0000);
        assert_eq!(dmath::max(neg, pos).to_bits(), 0x8000_0000);
    }

    #[test]
    fn min_max_order_and_nan() {
        assert_eq!(dmath::min(1.0, -2.0), -2.0);
        assert_eq!(dmath::min(-2.0, 1.0), -2.0);
        assert_eq!(dmath::max(1.0, -2.0), 1.0);
        assert_eq!(dmath::max(-2.0, 1.0), 1.0);
        assert_eq!(dmath::min(f32::NAN, 3.0), 3.0);
        assert_eq!(dmath::min(3.0, f32::NAN), 3.0);
        assert_eq!(dmath::max(f32::NAN, 3.0), 3.0);
        assert_eq!(dmath::max(3.0, f32::NAN), 3.0);
        assert!(dmath::min(f32::NAN, f32::NAN).is_nan());
        assert!(dmath::max(f32::NAN, f32::NAN).is_nan());
        assert_eq!(dmath::min(f32::NEG_INFINITY, f32::MIN), f32::NEG_INFINITY);
        assert_eq!(dmath::max(f32::INFINITY, f32::MAX), f32::INFINITY);
    }

    #[test]
    fn from_angle_zero_points_along_x() {
        assert!(approx(Vec2::from_angle(0.0), Vec2::X));
    }

    #[test]
    fn rotate_quarter_turn() {
        assert!(approx(Vec2::X.rotate(dmath::FRAC_PI_2), Vec2::Y));
    }

    #[test]
    fn normalize_zero_is_zero() {
        assert_eq!(Vec2::ZERO.normalize_or_zero(), Vec2::ZERO);
    }

    #[test]
    fn normalize_has_unit_length() {
        assert!((Vec2::new(3.0, 4.0).normalize_or_zero().length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn angle_roundtrip() {
        let angle = 1.234_f32;
        assert!((Vec2::from_angle(angle).angle() - angle).abs() < 1e-5);
    }

    #[test]
    fn perp_dot_sign() {
        assert!(Vec2::X.perp_dot(Vec2::Y) > 0.0);
    }
}
