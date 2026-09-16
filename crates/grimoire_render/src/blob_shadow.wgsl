// Blob shadow decals (plan 0002 WP2.6, OF-3.2): a soft, dark disc on the ground plane under an
// actor, the stylebook's cheap alternative to the key-light shadow map (preset "Low",
// `docs/art/stilbibel.md`). Drawn by `mesh_pass.rs` in the *same* render pass as the opaque mesh
// draws, right after them, so the already-populated depth buffer both hides a disc behind a wall
// or prop and lets an actor standing on it draw over it normally — the disc sits only slightly
// above the ground (`DECAL_HEIGHT`), so at the same screen pixel an opaque object's own geometry
// (which stands taller, and — under this tilted top-down camera — projects to a smaller depth than
// the ground beneath it) passes the depth test first. Fully vertex-pulled (`@builtin(vertex_index)`
// picks a quad corner from `QUAD_CORNERS`): no vertex buffer, only a per-instance buffer.
//
// Reuses the mesh pass's `Camera` bind group (group 0) purely for `view_proj`; this shader
// declares only the prefix of that uniform buffer it actually needs, which `wgpu` allows as long as
// the buffer is at least as large as the declared type (see `mesh_pass.rs`'s pipeline layout reuse
// for the blob pipeline).

struct Camera {
    view_proj: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

// Local-space quad corners (two triangles), matching `crate::BlobShadowInstance`'s convention of
// a disc inscribed in a square of half-extent 1 (i.e. `[-1, 1]^2`).
const QUAD_CORNERS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
);

// Height above the `Z = 0` ground plane the decal is drawn at, world units: enough above the
// floor mesh (also at `Z = 0` in every procedural test mesh) to win the depth test there, small
// enough not to visibly float off the ground from the game's tilted camera angles.
const DECAL_HEIGHT: f32 = 0.02;

struct InstanceInput {
    @location(0) position: vec2<f32>,
    @location(1) radius: f32,
    @location(2) softness: f32,
    @location(3) strength: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) softness: f32,
    @location(2) @interpolate(flat) strength: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, iin: InstanceInput) -> VertexOutput {
    let local = QUAD_CORNERS[vertex_index];
    let world = vec3<f32>(iin.position + local * iin.radius, DECAL_HEIGHT);

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    out.local = local;
    out.softness = iin.softness;
    out.strength = iin.strength;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let distance = length(in.local);
    // Feather from full `strength` at the centre down to `0` at the rim, over the innermost
    // `softness` fraction of the radius counted inward from the rim (`softness == 0` is a hard
    // edge at `distance == 1`, `softness == 1` fades from the very centre).
    let fade_start = max(1.0 - in.softness, 0.0);
    let alpha = in.strength * (1.0 - smoothstep(fade_start, 1.0, distance));
    return vec4<f32>(0.0, 0.0, 0.0, alpha);
}
