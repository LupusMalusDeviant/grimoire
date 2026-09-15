// Toon-only screen-space outline pass, run on the supersampled world targets before the
// downsample. Symmetric cross on linear depth, view normals and class ids; continuous edge strength
// (smoothstep around each threshold); the line is drawn in an absolute colour.

struct Params {
    near: f32,
    far: f32,
    // Tap offsets of the cross in target pixels (2x SSAA: -1 / +1, about 1 px final; 1x: 0 / +1).
    tap_lo: i32,
    tap_hi: i32,
}

// Line colour, linear HDR (about #0A090C after the tonemap).
const OUTLINE_COLOR: vec3<f32> = vec3<f32>(0.0030, 0.0027, 0.0037);

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var nc_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: Params;

struct VOut {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VOut {
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi & 2u) * 2 - 1);
    var out: VOut;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

fn clamp_pixel(p: vec2<i32>) -> vec2<i32> {
    let size = vec2<i32>(textureDimensions(nc_tex));
    return clamp(p, vec2<i32>(0), size - vec2<i32>(1));
}

fn linear_depth(p: vec2<i32>) -> f32 {
    let d = textureLoad(depth_tex, clamp_pixel(p), 0);
    return params.near * params.far / (params.far - d * (params.far - params.near));
}

fn is_solid(c: u32) -> bool {
    return c == 1u || c == 2u || c == 3u;
}

fn is_figure(c: u32) -> bool {
    return c == 2u || c == 3u;
}

fn pair_edge(a: vec2<i32>, b: vec2<i32>) -> f32 {
    let za = linear_depth(a);
    let zb = linear_depth(b);
    let na = textureLoad(nc_tex, clamp_pixel(a), 0);
    let nb = textureLoad(nc_tex, clamp_pixel(b), 0);
    let ca = u32(floor(na.a + 0.001));
    let cb = u32(floor(nb.a + 0.001));
    let differ = ca != cb;

    let depth_edge = smoothstep(0.010, 0.020, abs(za - zb) / max(min(za, zb), 1e-4));
    let tn = select(0.6, 0.35, differ || is_solid(ca) || is_solid(cb));
    let normal_edge = smoothstep(tn - 0.05, tn + 0.05, 1.0 - dot(na.xyz, nb.xyz));
    let class_edge = select(0.0, 1.0, differ && (is_figure(ca) || is_figure(cb)));
    let edge = max(max(depth_edge, normal_edge), class_edge);
    // Emissive class 4 produces no edges (the decal is floor class 0 and has no depth or normal step).
    return select(edge, 0.0, ca == 4u || cb == 4u);
}

@fragment
fn fs_outline(in: VOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(in.clip.xy));
    let lo = params.tap_lo;
    let hi = params.tap_hi;
    let e1 = pair_edge(p + vec2<i32>(lo, lo), p + vec2<i32>(hi, hi));
    let e2 = pair_edge(p + vec2<i32>(hi, lo), p + vec2<i32>(lo, hi));
    let edge = max(e1, e2);
    let c = textureLoad(hdr_tex, p, 0);
    return vec4<f32>(mix(c.rgb, OUTLINE_COLOR, edge), 1.0);
}
