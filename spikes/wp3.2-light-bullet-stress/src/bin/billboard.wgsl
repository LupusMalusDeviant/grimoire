// OF-3.3 spike: billboard-impostor bullet variant. One instanced draw call, quad corners
// generated from the vertex index (same trick as grimoire_render::sprite.wgsl); each instance is
// a BulletInstance (24 bytes) read straight off the real contract type's memory layout. The last
// 8 bytes (silhouette/palette/palette_space/glow/flags) have no matching single-u16/u8 wgpu
// vertex format, so they arrive as two packed u32 words and are unpacked here bitwise — this
// reads the exact same bytes BulletInstance's Rust fields occupy, just grouped differently for
// the vertex-attribute description (little-endian, true of every P1 CI target).

struct Camera {
    view_proj: mat4x4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

struct Instance {
    @location(0) position: vec2<f32>,
    @location(1) radius: f32,
    @location(2) rotation: f32,
    // .x = silhouette (low 16 bits) | palette << 16; .y = palette_space | glow << 8 | flags << 16.
    @location(3) packed: vec2<u32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) palette: u32,
    @location(2) @interpolate(flat) glow: u32,
}

const QUAD = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(1.0, -1.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    let local = QUAD[vertex_index % 6u];
    let c = cos(instance.rotation);
    let s = sin(instance.rotation);
    let rotated = vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);
    let world_centre = vec3<f32>(instance.position, 0.0);
    let offset = (camera.right.xyz * rotated.x + camera.up.xyz * rotated.y) * instance.radius;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world_centre + offset, 1.0);
    out.local = local;
    out.palette = (instance.packed.x >> 16u) & 0xFFFFu;
    out.glow = (instance.packed.y >> 8u) & 0xFFu;
    return out;
}

fn palette_color(hue: f32) -> vec3<f32> {
    // Cheap HSV(hue, 1, 1) -> RGB: a throwaway visual proxy, not the stilbibel's real palette.
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
    // Every silhouette value draws the same disc: a billboard quad cannot change its own outline
    // without a texture atlas (readability regel 3 note, see README/ADR).
    let dist = length(in.local);
    if dist > 1.0 {
        discard;
    }
    let base = palette_color(f32(in.palette % 8u) / 8.0);
    let glow_boost = f32(in.glow) / 255.0;
    return vec4<f32>(base * (0.6 + 0.4 * glow_boost), 1.0);
}
