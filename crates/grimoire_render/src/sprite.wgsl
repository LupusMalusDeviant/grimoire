// Instanced sprite pass. One draw call renders every sprite: the quad corners are generated
// from the vertex index, per-sprite data comes from the instance buffer (SpriteInstance).
//
// Colours arrive linear and leave linear; the render target view is sRGB, so the GPU encodes
// on write and blending happens in linear space.

struct Camera {
    view_proj: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

// Must match grimoire_render::shape.
const SHAPE_CIRCLE: u32 = 0u;

struct Instance {
    @location(0) position: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) rotation: f32,
    @location(3) shape: u32,
    @location(4) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Quad-local coordinate in [-1, 1]^2.
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) shape: u32,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[vertex_index % 6u];
    let scaled = corner * instance.half_size;
    let c = cos(instance.rotation);
    let s = sin(instance.rotation);
    let rotated = vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c);
    let world = instance.position + rotated;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world, 0.0, 1.0);
    out.local = corner;
    out.color = instance.color;
    out.shape = instance.shape;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives must be taken in uniform control flow, i.e. before branching on the shape.
    let distance = length(in.local) - 1.0;
    let width = max(fwidth(distance), 1e-6);
    let circle_coverage = clamp(0.5 - distance / width, 0.0, 1.0);
    let coverage = select(1.0, circle_coverage, in.shape == SHAPE_CIRCLE);
    if coverage <= 0.0 {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
