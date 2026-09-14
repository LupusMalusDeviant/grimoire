// Instanced sprite pass. One draw call renders every sprite: the quad corners are generated
// from the vertex index, per-sprite data comes from the instance buffer (SpriteInstance).
//
// Colours arrive linear and leave linear; the render target view is sRGB, so the GPU encodes
// on write and blending happens in linear space.

// Layout must match CameraUniform in sprite_pass.rs (80 bytes).
struct Camera {
    view_proj: mat4x4<f32>,
    // Target pixels per world unit.
    pixels_per_unit: f32,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

// Must match grimoire_render::shape.
const SHAPE_CIRCLE: u32 = 0u;

// Rasterised margin around every sprite, in pixels. It must cover the outer half of the
// circle's anti-aliasing ramp (half a pixel), which would otherwise be clipped at the tangent
// points of the quad.
const AA_MARGIN_PX: f32 = 1.0;

struct Instance {
    @location(0) position: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) rotation: f32,
    @location(3) shape: u32,
    @location(4) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Sprite-local coordinate; the sprite itself covers [-1, 1]^2, the margin lies beyond.
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
    let size_px = max(abs(instance.half_size) * camera.pixels_per_unit, vec2<f32>(1e-6));
    let local = corners[vertex_index % 6u] * (vec2<f32>(1.0) + AA_MARGIN_PX / size_px);
    let scaled = local * instance.half_size;
    let c = cos(instance.rotation);
    let s = sin(instance.rotation);
    let rotated = vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c);
    let world = instance.position + rotated;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world, 0.0, 1.0);
    out.local = local;
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
    // Quads keep hard edges: exactly the pixels whose centre lies inside the sprite.
    let quad_coverage = select(0.0, 1.0, max(abs(in.local.x), abs(in.local.y)) <= 1.0);
    let coverage = select(quad_coverage, circle_coverage, in.shape == SHAPE_CIRCLE);
    if coverage <= 0.0 {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
