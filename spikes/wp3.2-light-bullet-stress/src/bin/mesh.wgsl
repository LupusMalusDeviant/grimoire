// OF-3.3 spike: instanced low-poly-mesh bullet variant. Shared vertex/index buffers (one
// icosphere, grimoire_render::procedural::icosphere(0, 1.0)) instanced with a per-bullet
// MeshBulletInstance (28 bytes, src/bullets.rs). mesh_index is decoded but unused (this spike only
// ever registers one mesh) -- it exists to prove the layout carries a mesh selector at all, the
// concrete thing BulletInstance's 24-byte, mesh-less layout cannot express (see README/ADR).

struct Camera {
    view_proj: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
}

struct InstanceIn {
    @location(2) position: vec3<f32>,
    @location(3) rotation: f32,
    @location(4) scale: f32,
    // .x = mesh_index (low 16 bits) | palette << 16; .y = palette_space | glow << 8.
    @location(5) mesh_and_palette: u32,
    @location(6) space_and_glow: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) @interpolate(flat) palette: u32,
    @location(2) @interpolate(flat) glow: u32,
}

@vertex
fn vs_main(vin: VertexIn, instance: InstanceIn) -> VertexOutput {
    let c = cos(instance.rotation);
    let s = sin(instance.rotation);
    let rotated_pos = vec3<f32>(
        vin.position.x * c - vin.position.y * s,
        vin.position.x * s + vin.position.y * c,
        vin.position.z,
    );
    let rotated_normal = vec3<f32>(
        vin.normal.x * c - vin.normal.y * s,
        vin.normal.x * s + vin.normal.y * c,
        vin.normal.z,
    );
    let world = instance.position + rotated_pos * instance.scale;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    out.normal = rotated_normal;
    out.palette = (instance.mesh_and_palette >> 16u) & 0xFFFFu;
    out.glow = (instance.space_and_glow >> 8u) & 0xFFu;
    return out;
}

fn palette_color(hue: f32) -> vec3<f32> {
    let h6 = hue * 6.0;
    let x = 1.0 - abs(h6 % 2.0 - 1.0);
    if h6 < 1.0 {
        return vec3<f32>(1.0, x, 0.0);
    } else if h6 < 2.0 {
        return vec3<f32>(x, 1.0, 0.0);
    } else if h6 < 3.0 {
        return vec3<f32>(0.0, 1.0, x);
    } else if h6 < 4.0 {
        return vec3<f32>(0.0, x, 1.0);
    } else if h6 < 5.0 {
        return vec3<f32>(x, 0.0, 1.0);
    }
    return vec3<f32>(1.0, 0.0, x);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let light_dir = normalize(vec3<f32>(0.4, -0.6, 0.7));
    let ndotl = max(dot(n, light_dir), 0.0);
    let base = palette_color(f32(in.palette % 8u) / 8.0);
    let glow_boost = f32(in.glow) / 255.0;
    let shaded = base * (0.3 + 0.7 * ndotl) * (0.6 + 0.4 * glow_boost);
    return vec4<f32>(shaded, 1.0);
}
