// Clustered forward+ light assignment (plan 0002 WP3.4, engine ADR-0015 "compute clustering"):
// one invocation per froxel (`grimoire_render::cluster_layout`'s 16x9x24 grid, 3456 clusters),
// each testing every uploaded light's sphere (position + range) against its own froxel's
// view-space axis-aligned bounding box and recording the overlapping ones into the fixed-stride
// index list `cluster_layout` reserves (`cluster_index * light_budget`).
//
// Unlike the WP3.2 stress spike this pass is based on (`spikes/wp3.2-light-bullet-stress/src/
// light_cluster.wgsl`, which tested lights already given in abstract froxel-grid coordinates), this
// is the real thing: lights arrive in world space, and this shader projects them into the camera's
// local (right, up, forward) basis itself (`Params`, filled from `stage3d::cluster_camera_basis` on
// the Rust side, `cluster_pass.rs`). Tiles are laid out on a perspective screen-space grid (16x9)
// and depth slices are exponential between `params.proj.z`/`.w` (near/far) — the standard clustered
// shading grid (Olsson & Assarsson 2011) — so near-camera slices stay thin (fine culling close up,
// where most lights matter) and far slices stay coarse.
//
// The per-cluster bound below is a view-space AABB *of the tapered frustum slab*, not the exact
// tapered shape: it takes the min/max of the four screen-space corners at both the near and far
// edge of the depth slice. Because the true frustum slab is entirely contained in that box, a
// sphere that truly overlaps the slab always overlaps the box too — so this test can only ever
// over-include a light in a cluster it does not quite reach, never silently drop one that reaches
// it (the same "reject nothing real, count/bound the worst case" discipline `cluster_layout`'s
// worst-case sizing already assumes). `mesh.wgsl`'s fragment shader still re-checks the exact
// per-light distance against `range` before shading (unchanged from the pre-WP3.4 loop), so an
// over-included light costs a wasted index-list slot and a cheap distance check, never a wrong
// pixel.
//
// Grid constants and the flat cluster-index formula are copied by hand from
// `grimoire_render::cluster_layout` (no shared codegen between WGSL and Rust anywhere in this
// crate, matching `mesh_pass.rs::MAX_POINT_LIGHTS`'s existing convention) — kept in sync with
// `cluster_layout::{CLUSTER_GRID_X, CLUSTER_GRID_Y, CLUSTER_GRID_Z, cluster_index}` and with
// `mesh.wgsl`'s own copy of the same formula (`cluster_index_for`), which must agree with this one
// exactly: a light this shader assigns to cluster `N` must be found by a fragment that computes the
// same `N` for the point it shades.

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
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    // x = f / aspect, y = f (f = 1 / tan(fov_y / 2)), z = near, w = far.
    proj: vec4<f32>,
    light_count: u32,
    light_budget: u32,
    _pad0: u32,
    _pad1: u32,
}

const CLUSTER_GRID_X: u32 = 16u;
const CLUSTER_GRID_Y: u32 = 9u;
const CLUSTER_GRID_Z: u32 = 24u;
const CLUSTER_COUNT: u32 = CLUSTER_GRID_X * CLUSTER_GRID_Y * CLUSTER_GRID_Z;

@group(0) @binding(0)
var<storage, read> lights: array<Light>;
@group(0) @binding(1)
var<storage, read_write> cluster_table: array<ClusterRange>;
@group(0) @binding(2)
var<storage, read_write> index_list: array<u32>;
@group(0) @binding(3)
var<uniform> params: Params;

// Camera-local `u` (right-axis offset) of the point at NDC `ndc_x` and forward-distance `w`:
// inverse of `ndc_x = u * proj.x / w` (see `mesh.wgsl`'s `cluster_index_for`, the same relation
// used in the other direction).
fn tile_u(ndc_x: f32, w: f32) -> f32 {
    return ndc_x * w / params.proj.x;
}

// Camera-local `v` (up-axis offset); see `tile_u`.
fn tile_v(ndc_y: f32, w: f32) -> f32 {
    return ndc_y * w / params.proj.y;
}

@compute @workgroup_size(64)
fn cs_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let cluster_index = global_id.x;
    if cluster_index >= CLUSTER_COUNT {
        return;
    }

    let x = cluster_index % CLUSTER_GRID_X;
    let y = (cluster_index / CLUSTER_GRID_X) % CLUSTER_GRID_Y;
    let z = cluster_index / (CLUSTER_GRID_X * CLUSTER_GRID_Y);

    let near = params.proj.z;
    let far = params.proj.w;
    let ratio = far / near;
    let w_near = near * pow(ratio, f32(z) / f32(CLUSTER_GRID_Z));
    let w_far = near * pow(ratio, f32(z + 1u) / f32(CLUSTER_GRID_Z));

    let ndc_x_min = -1.0 + 2.0 * f32(x) / f32(CLUSTER_GRID_X);
    let ndc_x_max = -1.0 + 2.0 * f32(x + 1u) / f32(CLUSTER_GRID_X);
    let ndc_y_min = -1.0 + 2.0 * f32(y) / f32(CLUSTER_GRID_Y);
    let ndc_y_max = -1.0 + 2.0 * f32(y + 1u) / f32(CLUSTER_GRID_Y);

    // View-space AABB of the tapered frustum slab (see this file's header comment): min/max of the
    // four screen-space corners at each of the slab's two depth bounds.
    let u_near_min = tile_u(ndc_x_min, w_near);
    let u_near_max = tile_u(ndc_x_max, w_near);
    let u_far_min = tile_u(ndc_x_min, w_far);
    let u_far_max = tile_u(ndc_x_max, w_far);
    let u_min = min(min(u_near_min, u_near_max), min(u_far_min, u_far_max));
    let u_max = max(max(u_near_min, u_near_max), max(u_far_min, u_far_max));

    let v_near_min = tile_v(ndc_y_min, w_near);
    let v_near_max = tile_v(ndc_y_max, w_near);
    let v_far_min = tile_v(ndc_y_min, w_far);
    let v_far_max = tile_v(ndc_y_max, w_far);
    let v_min = min(min(v_near_min, v_near_max), min(v_far_min, v_far_max));
    let v_max = max(max(v_near_min, v_near_max), max(v_far_min, v_far_max));

    let base = cluster_index * params.light_budget;
    var count: u32 = 0u;
    for (var i: u32 = 0u; i < params.light_count; i = i + 1u) {
        if count >= params.light_budget {
            break;
        }
        let rel = lights[i].position - params.eye.xyz;
        let lu = dot(params.right.xyz, rel);
        let lv = dot(params.up.xyz, rel);
        let lw = dot(params.forward.xyz, rel);

        // Sphere (light centre + range) vs. the view-space AABB above: clamp the sphere centre
        // into the box, compare the remaining distance to the range. Never a false negative
        // relative to the true tapered frustum (see this file's header comment).
        let cu = clamp(lu, u_min, u_max);
        let cv = clamp(lv, v_min, v_max);
        let cw = clamp(lw, w_near, w_far);
        let du = lu - cu;
        let dv = lv - cv;
        let dw = lw - cw;
        let dist_sq = du * du + dv * dv + dw * dw;
        let reach = lights[i].range;
        if dist_sq <= reach * reach {
            index_list[base + count] = i;
            count = count + 1u;
        }
    }
    cluster_table[cluster_index] = ClusterRange(base, count);
}
