//! Light-culling spike data model, built directly on `grimoire_render::cluster_layout` (WP3.1's
//! frozen froxel grid and light/cluster buffer shapes) — reused verbatim, not redefined, per the
//! WP3.2 instruction.
//!
//! **Deviation from a real clustered forward+ pass:** cluster bounds here are unit froxel cells in
//! an abstract `[0, CLUSTER_GRID_X] x [0, CLUSTER_GRID_Y] x [0, CLUSTER_GRID_Z]` grid, and a
//! light's reach is a plain Euclidean sphere test against a cluster's centre plus its half-diagonal
//! — not the perspective view-space frustum slicing WP3.4 will implement. That real geometry
//! affects *which* lights land in *which* cluster, not the CPU-vs-compute cost comparison this
//! spike measures: both paths run the identical `O(clusters x lights)` assignment loop (see
//! `cpu_assign_clusters` here and `light_cluster.wgsl`'s `cs_main`), just on different processors.
//! See `README.md`.

use grimoire_render::cluster_layout::{
    CLUSTER_COUNT, CLUSTER_GRID_X, CLUSTER_GRID_Y, CLUSTER_GRID_Z, GpuClusterLightRange,
    GpuPointLight,
};

use crate::rng::Rng;

/// Deterministic synthetic light set, scattered through the froxel grid volume.
#[must_use]
pub fn synthetic_lights(count: usize, seed: u64) -> Vec<GpuPointLight> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| GpuPointLight {
            position: [
                rng.next_range(0.0, CLUSTER_GRID_X as f32),
                rng.next_range(0.0, CLUSTER_GRID_Y as f32),
                rng.next_range(0.0, CLUSTER_GRID_Z as f32),
            ],
            // Reach of 1-4 froxel cells: wide enough that most clusters see at least a handful of
            // lights (a degenerate all-or-nothing overlap pattern would not stress the assignment
            // loop realistically).
            range: rng.next_range(1.0, 4.0),
            color: [
                rng.next_range(0.2, 1.0),
                rng.next_range(0.2, 1.0),
                rng.next_range(0.2, 1.0),
            ],
            intensity: rng.next_range(0.5, 2.5),
        })
        .collect()
}

/// Half-diagonal of a `1x1x1` froxel cell (`sqrt(3) / 2`), added to a light's own range so a
/// sphere-vs-sphere overlap test against a cluster's centre is a conservative stand-in for
/// sphere-vs-AABB (never *under*-counts a cluster the real test would also mark as touched).
const FROXEL_HALF_DIAGONAL: f32 = 0.866_025_4;

const fn cluster_index(x: u32, y: u32, z: u32) -> usize {
    ((z * CLUSTER_GRID_Y + y) * CLUSTER_GRID_X + x) as usize
}

fn cluster_centre(x: u32, y: u32, z: u32) -> [f32; 3] {
    [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5]
}

/// Result of assigning `lights` to every cluster of the fixed 16x9x24 froxel grid: a
/// `CLUSTER_COUNT`-entry cluster table plus a **fixed-stride** index list (`CLUSTER_COUNT *
/// light_budget` entries, each cluster's slice starting at `cluster_index * light_budget`) —
/// the simplest layout both the CPU path and the compute-shader path (`light_cluster.wgsl`) can
/// write without atomics or a second compaction pass. `grimoire_render::cluster_layout`'s worst
/// case sizing already budgets for exactly this stride (see its module doc comment).
pub struct ClusterAssignment {
    pub cluster_table: Vec<GpuClusterLightRange>,
    pub index_list: Vec<u32>,
}

/// CPU-side cluster assignment: for every cluster, scan all `lights` and record up to
/// `light_budget` overlapping indices. `O(CLUSTER_COUNT * lights.len())` — identical algorithm to
/// `light_cluster.wgsl`'s `cs_main`, run here on the CPU instead of dispatched to the GPU.
#[must_use]
pub fn cpu_assign_clusters(lights: &[GpuPointLight], light_budget: usize) -> ClusterAssignment {
    let mut cluster_table = vec![GpuClusterLightRange::default(); CLUSTER_COUNT];
    let mut index_list = vec![0u32; CLUSTER_COUNT * light_budget];

    for z in 0..CLUSTER_GRID_Z {
        for y in 0..CLUSTER_GRID_Y {
            for x in 0..CLUSTER_GRID_X {
                let cluster = cluster_index(x, y, z);
                let centre = cluster_centre(x, y, z);
                let base = cluster * light_budget;
                let mut count: u32 = 0;
                for (light_index, light) in lights.iter().enumerate() {
                    if count as usize >= light_budget {
                        break;
                    }
                    let d = [
                        light.position[0] - centre[0],
                        light.position[1] - centre[1],
                        light.position[2] - centre[2],
                    ];
                    let dist_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    let reach = light.range + FROXEL_HALF_DIAGONAL;
                    if dist_sq <= reach * reach {
                        index_list[base + count as usize] = u32::try_from(light_index)
                            .expect("light count stays far below u32::MAX");
                        count += 1;
                    }
                }
                cluster_table[cluster] = GpuClusterLightRange {
                    offset: u32::try_from(base).expect("cluster offsets fit u32"),
                    count,
                };
            }
        }
    }

    ClusterAssignment {
        cluster_table,
        index_list,
    }
}

#[cfg(test)]
mod tests {
    use super::{cpu_assign_clusters, synthetic_lights};
    use grimoire_render::cluster_layout::CLUSTER_COUNT;

    #[test]
    fn synthetic_lights_is_deterministic() {
        let a = synthetic_lights(32, 99);
        let b = synthetic_lights(32, 99);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.position, y.position);
            assert_eq!(x.range, y.range);
        }
    }

    #[test]
    fn cpu_assignment_covers_every_cluster_and_respects_the_budget() {
        let lights = synthetic_lights(16, 5);
        let assignment = cpu_assign_clusters(&lights, 8);
        assert_eq!(assignment.cluster_table.len(), CLUSTER_COUNT);
        for (cluster, range) in assignment.cluster_table.iter().enumerate() {
            assert!(range.count as usize <= 8, "cluster {cluster} over budget");
            assert_eq!(range.offset as usize, cluster * 8);
            for slot in &assignment.index_list[range.offset as usize..][..range.count as usize] {
                assert!((*slot as usize) < lights.len());
            }
        }
    }

    #[test]
    fn a_light_at_a_clusters_centre_is_always_assigned() {
        // Regression guard for the sphere-vs-sphere overlap test itself: a light sitting exactly
        // on a cluster centre with a non-negative range must always be found in that cluster.
        let lights = vec![grimoire_render::cluster_layout::GpuPointLight {
            position: [8.0, 4.0, 12.0],
            range: 0.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        }];
        let assignment = cpu_assign_clusters(&lights, 4);
        let cluster = ((12u32 * 9 + 4) * 16 + 8) as usize;
        assert_eq!(assignment.cluster_table[cluster].count, 1);
    }
}
