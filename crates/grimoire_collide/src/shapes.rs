//! Circle and capsule shapes, their axis-aligned bounding boxes, and the exact overlap test
//! (contract §14).

use grimoire_core::math::dmath;
use grimoire_core::{StableHash, StableHasher, Vec2, impl_stable_hash};

/// Absolute-value bound for every coordinate and radius of a valid shape (PO decision V-18).
///
/// Keeps every intermediate value [`overlaps`] and [`Shape::aabb`] compute finite: squared
/// distances, `(r1 + r2)^2`, dot products, a capsule's `|b - a|^2`. Without this bound, two huge
/// but finite shapes could compare `inf <= inf` as an overlap in [`crate::BruteForceQuery`] while
/// [`crate::SpatialGrid`] sorts them into opposite edge cells and never compares them, silently
/// diverging between the two implementations.
pub const MAX_COORD: f32 = 1.0e9;

/// A circle: every point within `radius` of `center`.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Circle {
    /// Center of the circle in world coordinates.
    pub center: Vec2,
    /// Radius. Must be non-negative for a valid circle (contract §14).
    pub radius: f32,
}
impl_stable_hash!(Circle { center, radius });

/// A capsule: the segment from `a` to `b`, widened by `radius`. `a == b` is a circle.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Capsule {
    /// One endpoint of the segment.
    pub a: Vec2,
    /// The other endpoint of the segment.
    pub b: Vec2,
    /// Radius the segment is widened by. Must be non-negative for a valid capsule (contract §14).
    pub radius: f32,
}
impl_stable_hash!(Capsule { a, b, radius });

/// A collision shape: either a [`Circle`] or a [`Capsule`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    /// A circle.
    Circle(Circle),
    /// A capsule.
    Capsule(Capsule),
}

impl StableHash for Shape {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        match self {
            Self::Circle(circle) => {
                hasher.write_u8(0);
                circle.stable_hash(hasher);
            }
            Self::Capsule(capsule) => {
                hasher.write_u8(1);
                capsule.stable_hash(hasher);
            }
        }
    }
}

impl Shape {
    /// Axis-aligned bounding box of this shape.
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        match self {
            Self::Circle(circle) => {
                let r = Vec2::splat(circle.radius);
                Aabb {
                    min: circle.center - r,
                    max: circle.center + r,
                }
            }
            Self::Capsule(capsule) => {
                let min = Vec2::new(
                    dmath::min(capsule.a.x, capsule.b.x),
                    dmath::min(capsule.a.y, capsule.b.y),
                );
                let max = Vec2::new(
                    dmath::max(capsule.a.x, capsule.b.x),
                    dmath::max(capsule.a.y, capsule.b.y),
                );
                let r = Vec2::splat(capsule.radius);
                Aabb {
                    min: min - r,
                    max: max + r,
                }
            }
        }
    }

    /// Whether every coordinate has `|x| <= `[`MAX_COORD`] and the radius is finite and within
    /// `0..=`[`MAX_COORD`] (contract §14). Invalid shapes must never enter simulation state.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Circle(circle) => {
                point_in_bounds(circle.center) && radius_in_bounds(circle.radius)
            }
            Self::Capsule(capsule) => {
                point_in_bounds(capsule.a)
                    && point_in_bounds(capsule.b)
                    && radius_in_bounds(capsule.radius)
            }
        }
    }
}

/// `|x| <= MAX_COORD` for both components; also excludes NaN and infinity, since neither compares
/// `<=` to a finite bound.
fn point_in_bounds(p: Vec2) -> bool {
    p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD
}

fn radius_in_bounds(radius: f32) -> bool {
    (0.0..=MAX_COORD).contains(&radius)
}

/// Axis-aligned bounding box, `min <= max` componentwise.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec2,
    /// Maximum corner.
    pub max: Vec2,
}
impl_stable_hash!(Aabb { min, max });

/// Exact overlap test between two shapes (contract §14): symmetric, no square root, no
/// trigonometry. Always compares squared distances against `(r1 + r2)^2`; touching counts as
/// overlap.
#[must_use]
pub fn overlaps(a: &Shape, b: &Shape) -> bool {
    match (a, b) {
        (Shape::Circle(c1), Shape::Circle(c2)) => circle_circle(*c1, *c2),
        (Shape::Circle(circle), Shape::Capsule(capsule))
        | (Shape::Capsule(capsule), Shape::Circle(circle)) => circle_capsule(*circle, *capsule),
        (Shape::Capsule(c1), Shape::Capsule(c2)) => capsule_capsule(*c1, *c2),
    }
}

fn circle_circle(a: Circle, b: Circle) -> bool {
    let radius_sum = a.radius + b.radius;
    (a.center - b.center).length_squared() <= radius_sum * radius_sum
}

fn circle_capsule(circle: Circle, capsule: Capsule) -> bool {
    let closest = closest_point_on_segment(circle.center, capsule.a, capsule.b);
    let radius_sum = circle.radius + capsule.radius;
    (closest - circle.center).length_squared() <= radius_sum * radius_sum
}

fn capsule_capsule(a: Capsule, b: Capsule) -> bool {
    let radius_sum = a.radius + b.radius;
    segment_segment_distance_squared(a.a, a.b, b.a, b.b) <= radius_sum * radius_sum
}

/// Nearest point on the segment `a..=b` to `p` (contract §14): `t = clamp(dot(p - a, b - a) / |b -
/// a|^2, 0, 1)`, with `t = 0` for a degenerate (zero-length) segment.
fn closest_point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    let t = if len_sq == 0.0 {
        0.0
    } else {
        ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
    };
    a + ab * t
}

/// Squared distance between the nearest points of segments `p1..=q1` and `p2..=q2`.
///
/// Clamp method (Ericson, *Real-Time Collision Detection*, §5.1.9) with explicit branches for
/// degenerate (zero-length) segments; crossing segments have distance 0. Only basic arithmetic
/// and `clamp` — no square root.
fn segment_segment_distance_squared(p1: Vec2, q1: Vec2, p2: Vec2, q2: Vec2) -> f32 {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.length_squared();
    let e = d2.length_squared();

    let (s, t) = if a == 0.0 && e == 0.0 {
        // Both segments are points.
        (0.0, 0.0)
    } else if a == 0.0 {
        // Segment 1 is a point.
        (0.0, (d2.dot(r) / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e == 0.0 {
            // Segment 2 is a point.
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let f = d2.dot(r);
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let s0 = if denom == 0.0 {
                // Parallel segments: any point on segment 1 is equally good before clamping to t.
                0.0
            } else {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            };
            let t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                ((-c / a).clamp(0.0, 1.0), 0.0)
            } else if t0 > 1.0 {
                (((b - c) / a).clamp(0.0, 1.0), 1.0)
            } else {
                (s0, t0)
            }
        }
    };

    let c1 = p1 + d1 * s;
    let c2 = p2 + d2 * t;
    (c1 - c2).length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle(x: f32, y: f32, r: f32) -> Shape {
        Shape::Circle(Circle {
            center: Vec2::new(x, y),
            radius: r,
        })
    }

    fn capsule(ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> Shape {
        Shape::Capsule(Capsule {
            a: Vec2::new(ax, ay),
            b: Vec2::new(bx, by),
            radius: r,
        })
    }

    #[test]
    fn circle_circle_overlap_and_exact_touching() {
        assert!(overlaps(&circle(0.0, 0.0, 1.0), &circle(1.5, 0.0, 1.0)));
        assert!(!overlaps(&circle(0.0, 0.0, 1.0), &circle(2.001, 0.0, 1.0)));
        // Exact touching (distance == sum of radii) counts as overlap.
        assert!(overlaps(&circle(0.0, 0.0, 1.0), &circle(2.0, 0.0, 1.0)));
    }

    #[test]
    fn overlaps_is_symmetric() {
        let a = circle(0.0, 0.0, 1.0);
        let b = capsule(3.0, 0.0, 5.0, 0.0, 1.0);
        assert_eq!(overlaps(&a, &b), overlaps(&b, &a));
    }

    #[test]
    fn radius_zero_shapes_only_overlap_when_touching() {
        assert!(overlaps(&circle(0.0, 0.0, 0.0), &circle(0.0, 0.0, 0.0)));
        assert!(!overlaps(&circle(0.0, 0.0, 0.0), &circle(0.001, 0.0, 0.0)));
        assert!(overlaps(
            &circle(0.0, 0.0, 0.0),
            &capsule(0.0, 0.0, 5.0, 0.0, 0.0)
        ));
    }

    #[test]
    fn degenerate_capsule_behaves_like_a_circle() {
        let point_capsule = capsule(1.0, 2.0, 1.0, 2.0, 1.5);
        let circle_eq = circle(1.0, 2.0, 1.5);
        let probe = circle(2.0, 2.0, 0.4);
        assert_eq!(
            overlaps(&point_capsule, &probe),
            overlaps(&circle_eq, &probe)
        );
        let probe_capsule = capsule(-5.0, 2.0, 5.0, 2.0, 0.2);
        assert_eq!(
            overlaps(&point_capsule, &probe_capsule),
            overlaps(&circle_eq, &probe_capsule)
        );
    }

    #[test]
    fn capsule_capsule_crossing_segments_touch() {
        let a = capsule(-5.0, 0.0, 5.0, 0.0, 0.0);
        let b = capsule(0.0, -5.0, 0.0, 5.0, 0.0);
        assert!(overlaps(&a, &b));
    }

    #[test]
    fn capsule_capsule_parallel_segments() {
        let a = capsule(0.0, 0.0, 10.0, 0.0, 1.0);
        let b = capsule(0.0, 2.0, 10.0, 2.0, 1.0);
        assert!(overlaps(&a, &b));
        let c = capsule(0.0, 2.001, 10.0, 2.001, 1.0);
        assert!(!overlaps(&a, &c));
    }

    #[test]
    fn shape_validity_boundaries() {
        // f32 near 1e9 only resolves increments of about 64, so the "just over the bound" probes
        // below use +1.0e6, not +1.0, to land on a distinguishable value.
        assert!(circle(MAX_COORD, -MAX_COORD, MAX_COORD).is_valid());
        assert!(!circle(MAX_COORD + 1.0e6, 0.0, 0.0).is_valid());
        assert!(!circle(0.0, 0.0, -0.001).is_valid());
        assert!(!circle(0.0, 0.0, MAX_COORD + 1.0e6).is_valid());
        assert!(!circle(f32::NAN, 0.0, 1.0).is_valid());
        assert!(!circle(f32::INFINITY, 0.0, 1.0).is_valid());
        assert!(!circle(0.0, 0.0, f32::NAN).is_valid());
        assert!(capsule(0.0, 0.0, 0.0, 0.0, 0.0).is_valid());
        assert!(!capsule(0.0, 0.0, MAX_COORD + 1.0e6, 0.0, 0.0).is_valid());
    }

    #[test]
    fn aabb_of_circle_and_capsule() {
        let c = circle(1.0, 2.0, 3.0);
        assert_eq!(
            c.aabb(),
            Aabb {
                min: Vec2::new(-2.0, -1.0),
                max: Vec2::new(4.0, 5.0)
            }
        );
        let cap = capsule(1.0, -2.0, -3.0, 4.0, 0.5);
        assert_eq!(
            cap.aabb(),
            Aabb {
                min: Vec2::new(-3.5, -2.5),
                max: Vec2::new(1.5, 4.5)
            }
        );
    }
}
