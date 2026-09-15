//! CPU side of the shared hostile bullet pass (layer 6), following crate contract §6.
//!
//! The pass accepts only `palette_space == BULLET_PASS_PALETTE_SPACE` (HOSTILE). Every other
//! instance is dropped and counted; invalid instances are dropped and counted separately. Nothing
//! here knows about looks.

use crate::gpu_types::{BulletGpu, BulletInstance};
use crate::scene::HOSTILE;

/// Palette space drawn by the bullet pass (contract §6).
pub const BULLET_PASS_PALETTE_SPACE: u8 = HOSTILE;
/// Silhouette table of the pass: orb, rice, diamond.
pub const SILHOUETTE_COUNT: u16 = 3;
/// Palette entries of the hostile space: H0 Hexenmagenta, H1 Giftlimette.
pub const PALETTE_COUNT: u16 = 2;

/// Counters mirroring `StageStats` of contract §6.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BulletStats {
    pub drawn: u32,
    pub rejected_palette_space: u32,
    pub rejected_invalid: u32,
}

/// Filters and converts the instances of one frame.
///
/// # Panics
/// In debug builds, on an instance with a foreign palette space (contract §6).
pub fn accept(bullets: &[BulletInstance]) -> (Vec<BulletGpu>, BulletStats) {
    let mut stats = BulletStats::default();
    let mut out = Vec::with_capacity(bullets.len());
    for (i, b) in bullets.iter().enumerate() {
        if b.palette_space != BULLET_PASS_PALETTE_SPACE {
            debug_assert!(
                false,
                "bullet instance {i} uses palette space {}; the bullet pass accepts only palette space 1",
                b.palette_space
            );
            stats.rejected_palette_space += 1;
            continue;
        }
        let finite = b.position.iter().all(|v| v.is_finite()) && b.radius.is_finite() && b.rotation.is_finite();
        if !finite || b.radius <= 0.0 || b.silhouette >= SILHOUETTE_COUNT || b.palette >= PALETTE_COUNT {
            stats.rejected_invalid += 1;
            continue;
        }
        out.push(BulletGpu {
            position: b.position,
            radius: b.radius,
            rotation: b.rotation,
            silhouette: u32::from(b.silhouette),
            palette: u32::from(b.palette),
            glow: f32::from(b.glow),
            _pad: 0.0,
        });
        stats.drawn += 1;
    }
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::FRIENDLY;

    fn hostile() -> BulletInstance {
        BulletInstance {
            position: [1.0, 2.0],
            radius: 0.2,
            rotation: 0.5,
            silhouette: 1,
            palette: 1,
            palette_space: HOSTILE,
            glow: 200,
            flags: 0,
        }
    }

    #[test]
    fn hostile_instances_are_drawn() {
        let (gpu, stats) = accept(&[hostile(), hostile()]);
        assert_eq!(gpu.len(), 2);
        assert_eq!(stats.drawn, 2);
        assert_eq!(gpu[0].silhouette, 1);
        assert!((gpu[0].glow - 200.0).abs() < f32::EPSILON);
    }

    #[test]
    fn invalid_instances_are_counted_without_panic() {
        let mut nan = hostile();
        nan.position[0] = f32::NAN;
        let mut zero = hostile();
        zero.radius = 0.0;
        let mut sil = hostile();
        sil.silhouette = SILHOUETTE_COUNT;
        let (gpu, stats) = accept(&[nan, zero, sil]);
        assert!(gpu.is_empty());
        assert_eq!(stats.rejected_invalid, 3);
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn foreign_palette_space_is_rejected() {
        let mut friendly = hostile();
        friendly.palette_space = FRIENDLY;
        let (gpu, stats) = accept(&[hostile(), friendly]);
        assert_eq!(gpu.len(), 1);
        assert_eq!(stats.rejected_palette_space, 1);
    }
}
