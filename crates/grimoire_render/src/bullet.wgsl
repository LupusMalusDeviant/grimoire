// Bullet pass, layer 6 (plan 0002 WP3.5, engine ADR-0014 "billboard impostors"). Instanced draws
// over every accepted bullet: the quad corners come from the vertex index, the per-bullet data is
// a `BulletInstance` (24 bytes) read straight off its `#[repr(C)]` layout.
//
// Look (stylebook v0, "Bullets"): each bullet is a camera-facing billboard whose outline comes
// from a signed-distance atlas indexed by `silhouette` (PRD-0003 rule 3 through a texture, not
// geometry, as engine ADR-0014 requires), filled with its palette's body colour fading into a
// white-hot core, outlined by a dark rim 1.5 px wide, and surrounded by a soft halo scaled by
// `glow`. The pass draws its own glow; no post effect ever touches this layer (PRD-0003 rule 1).
//
// Two draws over the same instances: first every halo (`vs_halo`/`fs_halo`), then every rim and
// body (`vs_body`/`fs_body`), so a halo never lies on top of another bullet's outline.
//
// Colours arrive linear and leave premultiplied and linear; the render target view is sRGB, so the
// GPU encodes on write and blending happens in linear space.

// Must match `bullet_palette::COUNT` and `bullet_silhouette::COUNT` (checked by
// `bullet_pass::tests::shader_table_sizes_match_the_rust_tables`).
const PALETTE_COUNT: u32 = 2u;
const SILHOUETTE_COUNT: u32 = 3u;

// Layout must match `BulletCameraGpu` in bullet_pass.rs (112 bytes).
struct Camera {
    view_proj: mat4x4<f32>,
    // World-space direction a billboard's local +X maps to before the heading rotation (camera
    // right), and its local +Y (camera up). Both unit length.
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    // x, y: render target size in pixels.
    viewport: vec4<f32>,
}

// Layout must match `BulletStyleGpu` in bullet_pass.rs (128 bytes).
struct Style {
    body: array<vec4<f32>, PALETTE_COUNT>,
    core: array<vec4<f32>, PALETTE_COUNT>,
    // Linear RGB of the rim, a = rim opacity.
    rim: vec4<f32>,
    // x = rim width in pixels, y = largest rim width in local units, z = glow reach beyond the rim
    // in local units, w = halo opacity at full glow.
    shape: vec4<f32>,
    // x, y = depth inside the silhouette (local units) where the core starts and is complete,
    // z = atlas half extent in local units, w = texels per atlas cell edge.
    core_atlas: vec4<f32>,
    // x = distance encoded as 0, y = distance encoded as 1, z, w unused.
    distance: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;
@group(0) @binding(1)
var<uniform> style: Style;
@group(0) @binding(2)
var silhouette_atlas: texture_2d<f32>;
@group(0) @binding(3)
var atlas_sampler: sampler;

struct Instance {
    @location(0) position: vec2<f32>,
    @location(1) radius: f32,
    @location(2) rotation: f32,
    // silhouette | palette << 16 (little-endian bytes 16..20 of BulletInstance).
    @location(3) ids: u32,
    // palette_space | glow << 8 | flags << 16 (bytes 20..24).
    @location(4) tail: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Silhouette-local coordinate in units of the bullet's radius; +X is the flight direction.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) silhouette: u32,
    @location(2) @interpolate(flat) palette: u32,
    @location(3) @interpolate(flat) glow: f32,
}

const QUAD = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(1.0, -1.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, 1.0),
);

// Extra quad margin beyond the rim, in pixels: the outer half of the anti-aliasing ramp.
const AA_MARGIN_PX: f32 = 1.5;

// Builds one corner of a bullet's camera-facing billboard. `glow_reach` is how far (local units)
// beyond the outline the quad must extend for the halo; `0` for the body pass.
fn billboard(vertex_index: u32, instance: Instance, glow_reach: f32) -> VertexOutput {
    let glow = f32((instance.tail >> 8u) & 0xFFu) / 255.0;
    let axis_x = camera.axis_x.xyz;
    let axis_y = camera.axis_y.xyz;
    // Bullets sit exactly on the ground plane (Z = 0), so the drawn centre is the hit position.
    let centre = vec3<f32>(instance.position, 0.0);

    // The heading lies on the ground; project it into the billboard plane so the silhouette points
    // where the bullet flies on screen, whatever the camera tilt.
    let heading = vec3<f32>(cos(instance.rotation), sin(instance.rotation), 0.0);
    let projected = vec2<f32>(dot(heading, axis_x), dot(heading, axis_y));
    let projected_length = length(projected);
    let h = select(vec2<f32>(1.0, 0.0), projected / max(projected_length, 1e-6), projected_length > 1e-5);

    // Pixels per local unit at the centre, to size the rim and anti-aliasing margin.
    let c0 = camera.view_proj * vec4<f32>(centre, 1.0);
    let c1 = camera.view_proj * vec4<f32>(centre + axis_y * instance.radius, 1.0);
    var margin = 0.0;
    if c0.w > 1e-6 && c1.w > 1e-6 {
        let ndc_delta = c1.xy / c1.w - c0.xy / c0.w;
        let pixels_per_local = length(ndc_delta * camera.viewport.xy * 0.5);
        margin = min((style.shape.x + AA_MARGIN_PX) / max(pixels_per_local, 1e-3), style.core_atlas.z);
    }
    let extent = min(1.0 + glow * glow_reach + margin, style.core_atlas.z);

    let corner = QUAD[vertex_index % 6u] * extent;
    let rotated = vec2<f32>(corner.x * h.x - corner.y * h.y, corner.x * h.y + corner.y * h.x);
    let world = centre + (axis_x * rotated.x + axis_y * rotated.y) * instance.radius;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    out.local = corner;
    out.silhouette = instance.ids & 0xFFFFu;
    out.palette = (instance.ids >> 16u) & 0xFFFFu;
    out.glow = glow;
    return out;
}

// First draw: every halo. A bullet without glow collapses to a zero-area quad and rasterises
// nothing.
@vertex
fn vs_halo(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    var out = billboard(vertex_index, instance, style.shape.z);
    if out.glow <= 0.0 {
        out.clip_position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return out;
}

// Second draw: every rim and body, on top of all halos, so no bullet's glow ever tints another
// bullet's outline or body.
@vertex
fn vs_body(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    return billboard(vertex_index, instance, 0.0);
}

// Signed distance (local units, negative inside) of `silhouette` at `local`, read from the atlas.
fn silhouette_distance(silhouette: u32, local: vec2<f32>) -> f32 {
    let half_texel = 0.5 / style.core_atlas.w;
    let cell = clamp(
        vec2<f32>(local.x, -local.y) / (2.0 * style.core_atlas.z) + vec2<f32>(0.5),
        vec2<f32>(half_texel),
        vec2<f32>(1.0 - half_texel),
    );
    let uv = vec2<f32>((f32(min(silhouette, SILHOUETTE_COUNT - 1u)) + cell.x) / f32(SILHOUETTE_COUNT), cell.y);
    let encoded = textureSampleLevel(silhouette_atlas, atlas_sampler, uv, 0.0).r;
    return mix(style.distance.x, style.distance.y, encoded);
}

// Local units per pixel of the interpolated local coordinate.
fn local_units_per_pixel(local: vec2<f32>) -> f32 {
    return max(max(length(dpdx(local)), length(dpdy(local))), 1e-5);
}

@fragment
fn fs_halo(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow.
    let px = local_units_per_pixel(in.local);
    let d = silhouette_distance(in.silhouette, in.local);
    let palette = min(in.palette, PALETTE_COUNT - 1u);
    let rim_width = min(style.shape.x * px, style.shape.y);
    let fade = 1.0 - clamp((d - rim_width) / style.shape.z, 0.0, 1.0);
    // Only outside the outline: inside, the body pass covers it anyway.
    let outside = clamp(0.5 + d / px, 0.0, 1.0);
    let alpha = in.glow * fade * fade * style.shape.w * outside;
    if alpha <= 0.0 {
        discard;
    }
    return vec4<f32>(style.body[palette].rgb * alpha, alpha);
}

@fragment
fn fs_body(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow.
    let px = local_units_per_pixel(in.local);
    let d = silhouette_distance(in.silhouette, in.local);
    let palette = min(in.palette, PALETTE_COUNT - 1u);

    let rim_width = min(style.shape.x * px, style.shape.y);
    let body_coverage = clamp(0.5 - d / px, 0.0, 1.0);
    let rim_coverage = clamp(0.5 - (d - rim_width) / px, 0.0, 1.0);
    let core = smoothstep(style.core_atlas.x, style.core_atlas.y, -d);
    let fill = mix(style.body[palette].rgb, style.core[palette].rgb, core);

    // Back to front, premultiplied: rim, then body.
    let rim_alpha = style.rim.a * rim_coverage;
    let color = vec4<f32>(fill * body_coverage, body_coverage)
        + vec4<f32>(style.rim.rgb * rim_alpha, rim_alpha) * (1.0 - body_coverage);
    if color.a <= 0.0 {
        discard;
    }
    return color;
}
