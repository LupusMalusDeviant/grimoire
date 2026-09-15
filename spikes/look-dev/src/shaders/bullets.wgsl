// Shared hostile bullet pass (layer 6) and player marker (layer 7), drawn in LDR after
// PostFxResolve, without depth test. Look-independent by construction: nothing here reads the
// world targets or any look parameter. Glow is drawn by this pass itself, never by post-FX.

struct Bullets {
    view_proj: mat4x4<f32>,
    cam_right: vec3<f32>,
    plane_z: f32,
    cam_up: vec3<f32>,
    rim_px: f32,
    body: array<vec4<f32>, 2>,
    core: array<vec4<f32>, 2>,
    rim: vec4<f32>,
    marker: vec4<f32>,
    marker_ring: vec4<f32>,
    _pad: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: Bullets;

const SIL_ORB: u32 = 0u;
const SIL_RICE: u32 = 1u;
// Minor/major axis ratios of the silhouette table (rice 0.14 x 0.32, diamond 0.26 x 0.18).
const RICE_RATIO: f32 = 0.4375;
const DIAMOND_RATIO: f32 = 0.7;

struct BulletIn {
    @location(0) position: vec2<f32>,
    @location(1) radius: f32,
    @location(2) rotation: f32,
    @location(3) silhouette: u32,
    @location(4) palette: u32,
    @location(5) glow: f32,
}

struct BulletOut {
    @builtin(position) clip: vec4<f32>,
    // Billboard-local position in world units, +x along the flight direction.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) silhouette: u32,
    @location(2) @interpolate(flat) palette: u32,
    @location(3) radius: f32,
    @location(4) glow: f32,
}

fn corner(vertex_index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    return corners[vertex_index % 6u];
}

// Camera-facing billboard anchored at plane height plane_z; the flight direction on the plane is
// projected into the billboard so rice and diamonds point the way they fly.
fn billboard(b: BulletIn, local: vec2<f32>) -> vec4<f32> {
    let d3 = vec3<f32>(cos(b.rotation), sin(b.rotation), 0.0);
    let projected = vec2<f32>(dot(d3, u.cam_right), dot(d3, u.cam_up));
    let sd = select(vec2<f32>(1.0, 0.0), normalize(projected), length(projected) > 1e-5);
    let perp = vec2<f32>(-sd.y, sd.x);
    let screen = sd * local.x + perp * local.y;
    let world = vec3<f32>(b.position, u.plane_z) + u.cam_right * screen.x + u.cam_up * screen.y;
    return u.view_proj * vec4<f32>(world, 1.0);
}

@vertex
fn vs_glow(@builtin(vertex_index) vertex_index: u32, b: BulletIn) -> BulletOut {
    let local = corner(vertex_index) * (2.5 * b.radius);
    var out: BulletOut;
    out.clip = billboard(b, local);
    out.local = local;
    out.silhouette = b.silhouette;
    out.palette = b.palette;
    out.radius = b.radius;
    out.glow = b.glow;
    return out;
}

@fragment
fn fs_glow(in: BulletOut) -> @location(0) vec4<f32> {
    let t = saturate(length(in.local) / (2.5 * in.radius));
    let m = 1.0 - t;
    let k = m * m * m * (in.glow / 255.0) * 0.35;
    return vec4<f32>(u.body[in.palette].rgb * k, k);
}

@vertex
fn vs_body(@builtin(vertex_index) vertex_index: u32, b: BulletIn) -> BulletOut {
    // 0.06 world units (about 2 px) margin for the anti-aliasing ramp.
    let local = corner(vertex_index) * (b.radius + 0.06);
    var out: BulletOut;
    out.clip = billboard(b, local);
    out.local = local;
    out.silhouette = b.silhouette;
    out.palette = b.palette;
    out.radius = b.radius;
    out.glow = b.glow;
    return out;
}

fn sd_ellipse(p: vec2<f32>, ab: vec2<f32>) -> f32 {
    let k0 = length(p / ab);
    let k1 = length(p / (ab * ab));
    return k0 * (k0 - 1.0) / max(k1, 1e-6);
}

fn ndot(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.x - a.y * b.y;
}

fn sd_rhombus(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let q = abs(p);
    let h = clamp((-2.0 * ndot(q, b) + ndot(b, b)) / dot(b, b), -1.0, 1.0);
    let d = length(q - 0.5 * b * vec2<f32>(1.0 - h, 1.0 + h));
    return d * sign(q.x * b.y + q.y * b.x - b.x * b.y);
}

@fragment
fn fs_body(in: BulletOut) -> @location(0) vec4<f32> {
    let r = in.radius;
    let p = in.local;
    // All silhouettes are evaluated so the derivative below stays in uniform control flow.
    let d_orb = length(p) - r;
    let d_rice = sd_ellipse(p, vec2<f32>(r, r * RICE_RATIO));
    let d_diamond = sd_rhombus(p, vec2<f32>(r, r * DIAMOND_RATIO));
    let sd = select(select(d_diamond, d_rice, in.silhouette == SIL_RICE), d_orb, in.silhouette == SIL_ORB);
    let minor = select(select(r * DIAMOND_RATIO, r * RICE_RATIO, in.silhouette == SIL_RICE), r, in.silhouette == SIL_ORB);
    let px = max(length(fwidth(p)) * 0.70710678, 1e-6);

    let dpx = sd / px;
    let outer = saturate(0.5 - dpx);
    let inner = saturate(0.5 - (dpx + u.rim_px));
    let depth01 = saturate(-sd / minor);
    let core_w = smoothstep(0.35, 0.85, depth01);
    let fill = mix(u.body[in.palette].rgb, u.core[in.palette].rgb, core_w);
    let color = mix(u.rim.rgb, fill, inner);
    let alpha = outer * mix(u.rim.a, 1.0, inner);
    return vec4<f32>(color, alpha);
}

struct MarkerOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
}

@vertex
fn vs_marker(@builtin(vertex_index) vertex_index: u32) -> MarkerOut {
    let extent = u.marker_ring.z + u.marker_ring.w + 0.1;
    let local = corner(vertex_index) * extent;
    let world = vec3<f32>(u.marker_ring.xy + local, 0.02);
    var out: MarkerOut;
    out.clip = u.view_proj * vec4<f32>(world, 1.0);
    out.local = local;
    return out;
}

@fragment
fn fs_marker(in: MarkerOut) -> @location(0) vec4<f32> {
    let d = abs(length(in.local) - u.marker_ring.z) - 0.5 * u.marker_ring.w;
    let w = max(fwidth(d), 1e-6);
    let coverage = saturate(0.5 - d / w);
    return vec4<f32>(u.marker.rgb, u.marker.a * coverage);
}
