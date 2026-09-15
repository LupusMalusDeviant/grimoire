//! Trait-conformance suite for [`crate::CollisionQuery`] (contract §2 rule 12, §2a), behind the
//! non-default feature `conformance`.
//!
//! Checks only properties every implementation — including [`crate::NullCollision`], which never
//! has any colliders — must satisfy: `out` is cleared before being filled, filled ascending with
//! no duplicates, and [`crate::LayerMask::NONE`] finds nothing. Implementation-specific behaviour
//! (real hits, [`crate::SpatialGrid`] against [`crate::BruteForceQuery`], the golden hash) belongs
//! to that implementation's own tests.

use crate::{ColliderKey, CollisionQuery, GrazeRing, Hit, LayerMask, SOURCE_POOL, Shape};

/// Runs the [`CollisionQuery`] conformance checks against `query`, probing it with `shape` and
/// `ring`. Callers pick `shape`/`ring` so a real implementation returns a non-trivial (but
/// arbitrary) result; the checks hold regardless of what is actually found.
pub fn collision_query(query: &dyn CollisionQuery, shape: &Shape, ring: &GrazeRing) {
    // Intentionally compares `len() == 0` to `is_empty()` instead of using the latter directly:
    // this checks the relationship between the two methods, which `CollisionQuery::is_empty`'s
    // default implementation defines but an override could break.
    #[allow(clippy::len_zero)]
    let len_is_zero = query.len() == 0;
    assert_eq!(
        query.is_empty(),
        len_is_zero,
        "is_empty must match len() == 0"
    );

    check_clears_and_orders(query, shape, ring, LayerMask::ALL);
    check_none_finds_nothing(query, shape, ring);
}

/// A key no real collider in this crate's own tests uses (`SOURCE_POOL` keys there stay far below
/// `u32::MAX`), so its presence after a query proves `out` was not cleared before being filled.
fn sentinel() -> Hit {
    Hit {
        key: ColliderKey {
            source: SOURCE_POOL,
            index: u32::MAX,
            generation: u32::MAX,
        },
        layers: LayerMask::ALL,
    }
}

fn check_clears_and_orders(
    query: &dyn CollisionQuery,
    shape: &Shape,
    ring: &GrazeRing,
    mask: LayerMask,
) {
    let mut out = vec![sentinel()];
    query.overlapping(shape, mask, &mut out);
    assert_cleared_and_sorted(&out, "overlapping");

    let mut out = vec![sentinel()];
    query.graze_ring(ring, mask, &mut out);
    assert_cleared_and_sorted(&out, "graze_ring");
}

fn check_none_finds_nothing(query: &dyn CollisionQuery, shape: &Shape, ring: &GrazeRing) {
    let mut out = vec![sentinel()];
    query.overlapping(shape, LayerMask::NONE, &mut out);
    assert!(
        out.is_empty(),
        "LayerMask::NONE must find nothing (overlapping)"
    );

    let mut out = vec![sentinel()];
    query.graze_ring(ring, LayerMask::NONE, &mut out);
    assert!(
        out.is_empty(),
        "LayerMask::NONE must find nothing (graze_ring)"
    );
}

fn assert_cleared_and_sorted(out: &[Hit], label: &str) {
    assert!(
        !out.contains(&sentinel()),
        "{label} must clear `out` before filling it"
    );
    for pair in out.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{label} result must be strictly ascending by ColliderKey with no duplicates"
        );
    }
}
