// Light-culling spike: compute-clustering path. One invocation per cluster, each scanning every
// light -- the identical O(clusters x lights) algorithm as src/lights.rs's cpu_assign_clusters,
// run on the GPU instead of the CPU, writing the same fixed-stride cluster-table/index-list
// layout grimoire_render::cluster_layout defines. 3456 clusters / 64 = 54 workgroups exactly.

struct Light {
    position: vec3<f32>,
    range: f32,
    color: vec3<f32>,
    intensity: f32,
}

struct ClusterRange {
    offset: u32,
    count: u32,
}

struct Params {
    light_count: u32,
    light_budget: u32,
    grid_x: u32,
    grid_y: u32,
    grid_z: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0)
var<storage, read> lights: array<Light>;
@group(0) @binding(1)
var<storage, read_write> cluster_table: array<ClusterRange>;
@group(0) @binding(2)
var<storage, read_write> index_list: array<u32>;
@group(0) @binding(3)
var<uniform> params: Params;

// Half-diagonal of a 1x1x1 froxel cell, matching src/lights.rs's FROXEL_HALF_DIAGONAL exactly.
const FROXEL_HALF_DIAGONAL: f32 = 0.8660254;

@compute @workgroup_size(64)
fn cs_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let cluster_index = global_id.x;
    let total_clusters = params.grid_x * params.grid_y * params.grid_z;
    if cluster_index >= total_clusters {
        return;
    }

    let x = cluster_index % params.grid_x;
    let y = (cluster_index / params.grid_x) % params.grid_y;
    let z = cluster_index / (params.grid_x * params.grid_y);
    let centre = vec3<f32>(f32(x) + 0.5, f32(y) + 0.5, f32(z) + 0.5);

    let base = cluster_index * params.light_budget;
    var count: u32 = 0u;
    for (var i: u32 = 0u; i < params.light_count; i = i + 1u) {
        if count >= params.light_budget {
            break;
        }
        let d = lights[i].position - centre;
        let dist_sq = dot(d, d);
        let reach = lights[i].range + FROXEL_HALF_DIAGONAL;
        if dist_sq <= reach * reach {
            index_list[base + count] = i;
            count = count + 1u;
        }
    }
    cluster_table[cluster_index] = ClusterRange(base, count);
}
