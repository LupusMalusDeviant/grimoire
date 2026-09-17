//! CPU-side frame preparation of the bullet pass and the clustered lighting, without a GPU, for
//! the benchmark gate (plan 0002 WP3.6: "CPU-Kosten von Clustering und Bullet-Upload im
//! Bench-Crate unter Regressions-Gate").
//!
//! **Not part of the render contract (§6)** — a measurement hook in the spirit of
//! [`crate::WgpuRenderer::render_stage_with_specular_aa`]: it runs exactly the CPU steps
//! [`crate::WgpuRenderer::render_stage`] runs before it hands bullets and lights to the GPU, through
//! the same functions, so `grimoire_bench` can count their instructions on a runner without a GPU.
//! It may change or disappear with the passes it mirrors.
//!
//! - [`CpuFramePreparation::bullet_upload`]: the bullet counters of `StageStats`, the compaction of
//!   accepted bullets the bullet pass performs when any is rejected, and the byte view handed to
//!   `Queue::write_buffer`. The copy into the GPU buffer itself happens inside `wgpu` and is not
//!   included.
//! - [`CpuFramePreparation::light_clustering`]: the frame's point lights followed by the
//!   bullet-cloud lights derived from its bullets, the light budget clamp with the bullet-light cap
//!   and conversion to the GPU layout, and the camera basis of the compute clustering. The froxel
//!   assignment itself runs on the GPU (engine ADR-0015) and is not included.

use crate::bullet_lights::derive_bullet_lights;
use crate::bullet_pass::accepted_bullets;
use crate::stage::stage_stats_from_base;
use crate::{BulletInstance, LightBudget, PointLight, RenderStats, StageFrame, StageStats};

/// Reused buffers of the CPU frame preparation, so a measured frame allocates nothing once they
/// have grown (like the renderer's own).
#[derive(Debug, Clone, Default)]
pub struct CpuFramePreparation {
    staging: Vec<BulletInstance>,
    lights: Vec<PointLight>,
}

impl CpuFramePreparation {
    /// Empty buffers. Identical to [`CpuFramePreparation::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs the bullet pass's CPU preparation for `frame` and returns the stage statistics it
    /// produces and the number of bytes the bullet upload would write.
    pub fn bullet_upload(&mut self, frame: &StageFrame) -> (StageStats, usize) {
        let stats = stage_stats_from_base(RenderStats::default(), frame, None, None, None, &[]);
        let upload = accepted_bullets(&frame.bullets, &mut self.staging);
        (
            stats,
            bytemuck::cast_slice::<BulletInstance, u8>(upload).len(),
        )
    }

    /// Runs the clustered lighting's CPU preparation for `frame` at `budget` and a target of
    /// `aspect` (width / height), and returns the number of lights that would be uploaded.
    pub fn light_clustering(
        &mut self,
        frame: &StageFrame,
        budget: LightBudget,
        aspect: f32,
    ) -> usize {
        let bullet_lights = derive_bullet_lights(frame);
        self.lights.clear();
        self.lights.extend_from_slice(&frame.point_lights);
        self.lights.extend_from_slice(bullet_lights.as_slice());
        // The mesh pass clusters with the camera only while its view-projection is finite.
        let camera = frame.camera_25d.as_ref().filter(|camera| {
            crate::stage3d::view_projection(
                camera,
                aspect,
                crate::mesh_pass::NEAR_PLANE,
                crate::mesh_pass::FAR_PLANE,
            )
            .iter()
            .flatten()
            .all(|value| value.is_finite())
        });
        let upload = crate::mesh_pass::light_upload(
            &self.lights,
            budget.light_count(),
            &frame.bullet_light_cap,
            camera,
            aspect,
        );
        upload.lights.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BULLET_PASS_PALETTE_SPACE, Camera25D};

    fn bullet(x: f32, glow: u8) -> BulletInstance {
        BulletInstance {
            position: [x, 0.0],
            radius: 0.3,
            rotation: 0.0,
            silhouette: 0,
            palette: 0,
            palette_space: BULLET_PASS_PALETTE_SPACE,
            glow,
            flags: 0,
        }
    }

    #[test]
    fn bullet_upload_counts_like_the_renderer_and_compacts_rejected_bullets() {
        let mut frame = StageFrame::new();
        frame.bullets = vec![bullet(0.0, 0), bullet(1.0, 0), bullet(2.0, 0)];
        let mut preparation = CpuFramePreparation::new();
        let (stats, bytes) = preparation.bullet_upload(&frame);
        assert_eq!(stats.bullets_drawn, 3);
        assert_eq!(bytes, 3 * std::mem::size_of::<BulletInstance>());

        frame.bullets[1].radius = 0.0;
        let (stats, bytes) = preparation.bullet_upload(&frame);
        assert_eq!(
            (stats.bullets_drawn, stats.bullets_rejected_invalid),
            (2, 1)
        );
        assert_eq!(bytes, 2 * std::mem::size_of::<BulletInstance>());
    }

    #[test]
    fn light_clustering_clamps_to_the_budget_and_adds_bullet_lights() {
        let mut frame = StageFrame::new();
        frame.camera_25d = Some(Camera25D::default());
        let light = PointLight {
            range: 2.0,
            ..PointLight::default()
        };
        frame.point_lights = vec![light; 40];
        let mut preparation = CpuFramePreparation::new();
        assert_eq!(
            preparation.light_clustering(&frame, LightBudget::Low, 16.0 / 9.0),
            32
        );
        assert_eq!(
            preparation.light_clustering(&frame, LightBudget::High, 16.0 / 9.0),
            40
        );
        frame.bullets = vec![bullet(0.0, 255)];
        assert_eq!(
            preparation.light_clustering(&frame, LightBudget::High, 16.0 / 9.0),
            41,
            "one glowing bullet yields one bullet-cloud light"
        );
    }
}
