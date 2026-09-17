// Instanced sprite pass. One draw call renders every sprite: the quad corners are generated
// from the vertex index, per-sprite data comes from the instance buffer (SpriteInstance).
//
// Colours arrive linear and leave linear; the render target view is sRGB, so the GPU encodes
// on write and blending happens in linear space.

// Layout must match CameraUniform in sprite_pass.rs (128 bytes).
struct Camera {
    view_proj: mat4x4<f32>,
    // Target pixels per world unit (flat 2D camera; unused by `vs_billboard`).
    pixels_per_unit: f32,
    // `vs_billboard` only (plan 0002 WP3.6): world-space directions of the billboard's local +X
    // (camera right) and +Y (camera up), and x, y = render target size in pixels.
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    viewport: vec4<f32>,
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

// Camera-facing billboard on the ground plane (Z = 0) under the tilted 2.5D camera (plan 0002 WP3.6):
// the player marker sits on its ground position like the bullets (`bullet.wgsl`). `position` is the
// ground point, `half_size` the extent in world units along the camera's right and up axes,
// `rotation` turns the sprite within that plane. `vs_main` above stays the flat 2D path unchanged.
@vertex
fn vs_billboard(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let centre = vec3<f32>(instance.position, 0.0);
    let axis_x = camera.axis_x.xyz;
    let axis_y = camera.axis_y.xyz;
    // Pixels per world unit at the sprite's centre, for the anti-aliasing margin.
    var pixels_per_unit = 1.0;
    let c0 = camera.view_proj * vec4<f32>(centre, 1.0);
    let c1 = camera.view_proj * vec4<f32>(centre + axis_y, 1.0);
    if c0.w > 1e-6 && c1.w > 1e-6 {
        let ndc_delta = c1.xy / c1.w - c0.xy / c0.w;
        pixels_per_unit = length(ndc_delta * camera.viewport.xy * 0.5);
    }
    let size_px = max(abs(instance.half_size) * pixels_per_unit, vec2<f32>(1e-6));
    let local = corners[vertex_index % 6u] * (vec2<f32>(1.0) + AA_MARGIN_PX / size_px);
    let scaled = local * instance.half_size;
    let c = cos(instance.rotation);
    let s = sin(instance.rotation);
    let rotated = vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c);
    let world = centre + axis_x * rotated.x + axis_y * rotated.y;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
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
