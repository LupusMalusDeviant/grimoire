// Depth-only pass for the key-light shadow map (plan 0002 WP2.6, OF-3.2). Uses the same vertex
// buffer as `mesh.wgsl` (`crate::mesh::MeshVertex`; only `position` is read here, `normal`/`uv`
// stay unused attributes at the same stride) but its own, smaller per-instance transform buffer
// (`shadow_pass.rs`'s `ShadowInstanceGpu`, 64 bytes: just the model matrix, no material fields) —
// the shadow map never shades a caster, so it needs none of `mesh.wgsl`'s per-instance material
// data.
//
// No fragment shader: this is a genuine depth-only pipeline (`RenderPipelineDescriptor::fragment
// == None`), which `wgpu` supports directly. Depth bias (constant and slope-scaled) is applied by
// the pipeline's `DepthBiasState` (`shadow_pass.rs`), not in this shader, so it stays a single,
// per-pipeline value rather than per-fragment arithmetic.

struct ShadowUniform {
    // World-to-light-space view-projection matrix (`stage3d::key_light_view_projection`).
    light_view_proj: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> shadow: ShadowUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
}

struct InstanceInput {
    @location(1) transform_0: vec4<f32>,
    @location(2) transform_1: vec4<f32>,
    @location(3) transform_2: vec4<f32>,
    @location(4) transform_3: vec4<f32>,
}

@vertex
fn vs_main(vin: VertexInput, iin: InstanceInput) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(iin.transform_0, iin.transform_1, iin.transform_2, iin.transform_3);
    let world_position = model * vec4<f32>(vin.position, 1.0);
    return shadow.light_view_proj * world_position;
}
