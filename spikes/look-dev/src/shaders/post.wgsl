// Shared post stack (identical for all looks, no per-look parameter): 2x2 box downsample, bloom
// (soft threshold + 4-level dual-filter chain from half resolution), Khronos PBR Neutral tonemap.
// The per-look calibration is a light gain inside the world shading, not an exposure here.
// The render target is sRGB, so the resolve writes linear values.

struct Post {
    bloom_intensity: f32,
    threshold: f32,
    knee: f32,
    ss_factor: u32,
    bloom_levels: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var add_tex: texture_2d<f32>;
@group(0) @binding(2) var lin_sampler: sampler;
@group(0) @binding(3) var<uniform> post: Post;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VOut {
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi & 2u) * 2 - 1);
    var out: VOut;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 0.5 - y * 0.5);
    return out;
}

@fragment
fn fs_downsample(in: VOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(in.clip.xy));
    let f = i32(post.ss_factor);
    let o = select(0, 1, f > 1);
    let base = p * f;
    let sum = textureLoad(src_tex, base, 0).rgb
        + textureLoad(src_tex, base + vec2<i32>(o, 0), 0).rgb
        + textureLoad(src_tex, base + vec2<i32>(0, o), 0).rgb
        + textureLoad(src_tex, base + vec2<i32>(o, o), 0).rgb;
    return vec4<f32>(sum * 0.25, 1.0);
}

fn sample_src(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src_tex, lin_sampler, uv, 0.0).rgb;
}

fn soft_threshold(c: vec3<f32>) -> vec3<f32> {
    let br = max(c.r, max(c.g, c.b));
    var rq = clamp(br - post.threshold + post.knee, 0.0, 2.0 * post.knee);
    rq = rq * rq / (4.0 * post.knee + 1e-5);
    let w = max(rq, br - post.threshold) / max(br, 1e-5);
    return c * w;
}

// Half-resolution prefilter: the bilinear tap at the centre of each 2x2 block is an exact box.
@fragment
fn fs_prefilter(in: VOut) -> @location(0) vec4<f32> {
    let c = sample_src(in.uv);
    return vec4<f32>(soft_threshold(c), 1.0);
}

@fragment
fn fs_down(in: VOut) -> @location(0) vec4<f32> {
    let hp = 1.0 / vec2<f32>(textureDimensions(src_tex));
    var sum = sample_src(in.uv) * 4.0;
    sum = sum + sample_src(in.uv - hp);
    sum = sum + sample_src(in.uv + hp);
    sum = sum + sample_src(in.uv + vec2<f32>(hp.x, -hp.y));
    sum = sum + sample_src(in.uv - vec2<f32>(hp.x, -hp.y));
    return vec4<f32>(sum / 8.0, 1.0);
}

@fragment
fn fs_up(in: VOut) -> @location(0) vec4<f32> {
    let hp = 0.5 / vec2<f32>(textureDimensions(src_tex));
    var sum = sample_src(in.uv + vec2<f32>(-hp.x * 2.0, 0.0));
    sum = sum + sample_src(in.uv + vec2<f32>(-hp.x, hp.y)) * 2.0;
    sum = sum + sample_src(in.uv + vec2<f32>(0.0, hp.y * 2.0));
    sum = sum + sample_src(in.uv + vec2<f32>(hp.x, hp.y)) * 2.0;
    sum = sum + sample_src(in.uv + vec2<f32>(hp.x * 2.0, 0.0));
    sum = sum + sample_src(in.uv + vec2<f32>(hp.x, -hp.y)) * 2.0;
    sum = sum + sample_src(in.uv + vec2<f32>(0.0, -hp.y * 2.0));
    sum = sum + sample_src(in.uv + vec2<f32>(-hp.x, -hp.y)) * 2.0;
    let current = textureSampleLevel(add_tex, lin_sampler, in.uv, 0.0).rgb;
    return vec4<f32>(sum / 12.0 + current, 1.0);
}

// Khronos PBR Neutral tone mapper.
fn pbr_neutral(color_in: vec3<f32>) -> vec3<f32> {
    let start_compression = 0.8 - 0.04;
    let desaturation = 0.15;
    var color = color_in;
    let x = min(color.r, min(color.g, color.b));
    let offset = select(0.04, x - 6.25 * x * x, x < 0.08);
    color = color - offset;
    let peak = max(color.r, max(color.g, color.b));
    if peak < start_compression {
        return color;
    }
    let d = 1.0 - start_compression;
    let new_peak = 1.0 - d * d / (peak + d - start_compression);
    color = color * (new_peak / peak);
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(color, vec3<f32>(new_peak), g);
}

@fragment
fn fs_resolve(in: VOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(in.clip.xy));
    let hdr = textureLoad(src_tex, p, 0).rgb;
    let bloom = textureSampleLevel(add_tex, lin_sampler, in.uv, 0.0).rgb / post.bloom_levels;
    let c = hdr + post.bloom_intensity * bloom;
    return vec4<f32>(max(pbr_neutral(c), vec3<f32>(0.0)), 1.0);
}
