// Mesh pass (plan 0002 WP2.3): depth-tested static meshes with deliberately provisional shading —
// base colour factor, one directional key light and a two-colour (sky/ground) ambient term. No
// GGX, no shadows, no textures: WP2.5 replaces this fragment shader with real PBR shading.
//
// Winding is not assumed to be consistent (see procedural.rs); the pipeline disables back-face
// culling, like the sprite pass.
//
// Layout must match MeshCameraUniform in mesh_pass.rs (128 bytes).
struct Camera {
    view_proj: mat4x4<f32>,
    // xyz: unit vector from a lit surface point *towards* the key light (i.e. the negated,
    // normalised light travel direction). w unused.
    light_dir: vec4<f32>,
    // rgb: key light colour already multiplied by its intensity. w unused.
    key_light: vec4<f32>,
    // rgb: ambient colour for surfaces facing straight up (normal.z = 1), already multiplied by
    // its intensity. w unused.
    ambient_sky: vec4<f32>,
    // rgb: ambient colour for surfaces facing straight down (normal.z = -1), already multiplied by
    // its intensity. w unused.
    ambient_ground: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct InstanceInput {
    @location(3) transform_0: vec4<f32>,
    @location(4) transform_1: vec4<f32>,
    @location(5) transform_2: vec4<f32>,
    @location(6) transform_3: vec4<f32>,
    @location(7) base_color: vec4<f32>,
    @location(8) emissive: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) base_color: vec4<f32>,
    @location(2) emissive: vec3<f32>,
}

@vertex
fn vs_main(vin: VertexInput, iin: InstanceInput) -> VertexOutput {
    let model = mat4x4<f32>(iin.transform_0, iin.transform_1, iin.transform_2, iin.transform_3);
    let world_position = model * vec4<f32>(vin.position, 1.0);
    // Provisional: assumes `model` has no non-uniform scale, so its upper-left 3x3 also carries
    // normals correctly. WP2.5's real PBR pass uses a proper normal (inverse-transpose) matrix.
    let world_normal = normalize((model * vec4<f32>(vin.normal, 0.0)).xyz);

    var out: VertexOutput;
    out.clip_position = camera.view_proj * world_position;
    out.world_normal = world_normal;
    out.base_color = iin.base_color;
    out.emissive = iin.emissive.rgb;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let n_dot_l = max(dot(n, camera.light_dir.xyz), 0.0);
    let ambient = mix(camera.ambient_ground.rgb, camera.ambient_sky.rgb, n.z * 0.5 + 0.5);
    let lit = in.base_color.rgb * (camera.key_light.rgb * n_dot_l + ambient) + in.emissive;
    return vec4<f32>(lit, in.base_color.a);
}
