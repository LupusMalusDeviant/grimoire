// World layer of the look-dev spike.
//
// One vertex shader and one block of shared code (surface, floor relief, ritual decal, blob
// shadows, light falloff, ambient, the light loop header) serve all three looks. The looks differ
// only in the per-light term of their fragment entry points fs_toon, fs_stylized and
// fs_realistic. Emissive meshes (fs_emissive) and emissive billboards (vs_bolt/fs_emissive_bolt)
// are unlit and identical for every look.
//
// Colours are linear HDR. Output 0 is the HDR colour, output 1 the normal+class MRT
// (view-space geometric normal, alpha = class id + figure index / 16).

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

struct Frame {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    eye: vec3<f32>,
    light_count: u32,
    key_dir: vec3<f32>,
    key_intensity: f32,
    key_color: vec3<f32>,
    ambient_intensity: f32,
    sky: vec3<f32>,
    decal_emission: f32,
    ground: vec3<f32>,
    blob_count: u32,
    cam_right: vec3<f32>,
    near: f32,
    cam_up: vec3<f32>,
    far: f32,
    decal_color: vec3<f32>,
    floor_salt: u32,
    blobs: array<vec4<f32>, 9>,
    // Global light-intensity factor times the look's calibrated light gain. Scales only the direct
    // light (point lights and moon; diffuse and specular), never ambient, rim, emission, decal or bolts.
    light_gain: f32,
    // Global light-intensity factor alone, identical for every look: scales the hemisphere ambient
    // and is the reference scale of the toon ramp.
    light_factor: f32,
    _pad0: f32,
    _pad1: f32,
    // Toon ramp in absolute luminance at light gain 1: threshold 1, threshold 2, mid level, top level.
    toon_ramp: vec4<f32>,
}

struct Light {
    position: vec3<f32>,
    radius: f32,
    // c * I
    color: vec3<f32>,
    _pad: f32,
}

struct Lights {
    data: array<Light, 256>,
}

struct Material {
    albedo: vec4<f32>,
    emissive: vec4<f32>,
    tint: vec4<f32>,
    params: vec4<f32>,
    // rgb metal base colour F0 (equals the albedo for non-metals).
    metal: vec4<f32>,
}

struct Rim {
    color_power: vec4<f32>,
    strength: vec4<f32>,
}

struct Materials {
    mats: array<Material, 32>,
    rims: array<Rim, 4>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<uniform> lights: Lights;
@group(0) @binding(2) var<uniform> materials: Materials;

const CLASS_FLOOR: u32 = 0u;
const CLASS_PLAYER: u32 = 2u;
const CLASS_ENEMY: u32 = 3u;
const CLASS_EMISSIVE: u32 = 4u;
const GROUT: u32 = 1u;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) packed: u32,
    @location(2) normal: vec3<f32>,
    @location(3) figure: u32,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) packed: u32,
    @location(3) @interpolate(flat) figure: u32,
}

struct FragOut {
    @location(0) color: vec4<f32>,
    @location(1) normal_class: vec4<f32>,
}

@vertex
fn vs_world(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = frame.view_proj * vec4<f32>(v.position, 1.0);
    out.world = v.position;
    out.normal = v.normal;
    out.packed = v.packed;
    out.figure = v.figure;
    return out;
}

// ---------------------------------------------------------------------------------------------
// Hashes and floor relief. Must match scene.rs (floor_height) exactly.
// ---------------------------------------------------------------------------------------------

fn hash_u32(x: u32) -> u32 {
    let state = x * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash2(ix: i32, iy: i32, salt: u32) -> u32 {
    return hash_u32((bitcast<u32>(ix) * 1597334677u) ^ (bitcast<u32>(iy) * 3812015801u) ^ salt);
}

fn unit_from(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

fn value_noise(p: vec2<f32>, salt: u32) -> f32 {
    let c = floor(p);
    let f = p - c;
    let ix = i32(c.x);
    let iy = i32(c.y);
    let s = salt ^ 0x00A5A5A5u;
    let c00 = unit_from(hash2(ix, iy, s)) * 2.0 - 1.0;
    let c10 = unit_from(hash2(ix + 1, iy, s)) * 2.0 - 1.0;
    let c01 = unit_from(hash2(ix, iy + 1, s)) * 2.0 - 1.0;
    let c11 = unit_from(hash2(ix + 1, iy + 1, s)) * 2.0 - 1.0;
    let u = f * f * (3.0 - 2.0 * f);
    let a = c00 + (c10 - c00) * u.x;
    let b = c01 + (c11 - c01) * u.x;
    return a + (b - a) * u.y;
}

// (height, grout weight)
fn floor_height(p: vec2<f32>, salt: u32) -> vec2<f32> {
    let c = floor(p);
    let f = p - c;
    let h = hash2(i32(c.x), i32(c.y), salt);
    let offset = (unit_from(h) * 2.0 - 1.0) * 0.03;
    let tilt_x = tan(radians((unit_from(hash_u32(h ^ 0x68E31DA4u)) * 2.0 - 1.0) * 1.5));
    let tilt_y = tan(radians((unit_from(hash_u32(h ^ 0xB5297A4Du)) * 2.0 - 1.0) * 1.5));
    let tile = offset + tilt_x * (f.x - 0.5) + tilt_y * (f.y - 0.5);
    let edge = min(min(f.x, 1.0 - f.x), min(f.y, 1.0 - f.y));
    let s = smoothstep(0.015, 0.055, edge);
    let groove = -0.06 * (1.0 - s);
    let noise = value_noise(p / 5.0, salt) * 0.08;
    let ledge = -0.25 * smoothstep(15.0, 15.25, length(p));
    return vec2<f32>(noise + tile * s + groove + ledge, 1.0 - s);
}

// Per-pixel shading normal of the floor from the height function (central differences).
fn floor_normal(p: vec2<f32>, salt: u32) -> vec3<f32> {
    let e = 0.01;
    let hx = floor_height(p + vec2<f32>(e, 0.0), salt).x - floor_height(p - vec2<f32>(e, 0.0), salt).x;
    let hy = floor_height(p + vec2<f32>(0.0, e), salt).x - floor_height(p - vec2<f32>(0.0, e), salt).x;
    return normalize(vec3<f32>(-hx, -hy, 2.0 * e));
}

// ---------------------------------------------------------------------------------------------
// Ritual circle decal (world XY, floor only) and blob shadows. Layer-1 content, not a look property.
// ---------------------------------------------------------------------------------------------

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

fn decal_distance(p: vec2<f32>) -> f32 {
    let r = length(p);
    // Outer ring 5.6..6.0, inner ring 3.8..4.0.
    var d = min(abs(r - 5.8) - 0.2, abs(r - 3.9) - 0.1);
    // 7-point star polygon {7/3} inscribed in r = 3.9, line width 0.08.
    for (var k = 0; k < 7; k = k + 1) {
        let a0 = PI * 0.5 + TAU * f32(k) / 7.0;
        let a1 = PI * 0.5 + TAU * f32((k + 3) % 7) / 7.0;
        let s = segment_distance(p, 3.9 * vec2<f32>(cos(a0), sin(a0)), 3.9 * vec2<f32>(cos(a1), sin(a1)));
        d = min(d, s - 0.04);
    }
    // 12 rune ticks (0.25 x 0.08) between the rings.
    let sector = TAU / 12.0;
    let a = round(atan2(p.y, p.x) / sector) * sector;
    let q = vec2<f32>(cos(a) * p.x + sin(a) * p.y, -sin(a) * p.x + cos(a) * p.y);
    let bx = abs(q - vec2<f32>(4.8, 0.0)) - vec2<f32>(0.125, 0.04);
    let tick = length(max(bx, vec2<f32>(0.0))) + min(max(bx.x, bx.y), 0.0);
    return min(d, tick);
}

fn blob_shadow(p: vec2<f32>) -> f32 {
    var f = 1.0;
    for (var i = 0u; i < frame.blob_count; i = i + 1u) {
        let b = frame.blobs[i];
        let t = length(p - b.xy) / b.z;
        f = f * mix(0.4, 1.0, smoothstep(0.0, 1.0, t));
    }
    return f;
}

// ---------------------------------------------------------------------------------------------
// Shared surface and lighting helpers.
// ---------------------------------------------------------------------------------------------

struct Surface {
    P: vec3<f32>,
    N: vec3<f32>,
    V: vec3<f32>,
    geo_n: vec3<f32>,
    albedo: vec3<f32>,
    emissive: vec3<f32>,
    tint: vec3<f32>,
    spec_color: vec3<f32>,
    metal_color: vec3<f32>,
    gloss: f32,
    ks: f32,
    roughness: f32,
    metalness: f32,
    // Blob shadow factor, applied to all lighting (not to emission).
    shadow: f32,
    // 1 on floor pixels inside the decal or a blob shadow (excluded from the light gain calibration).
    excluded: f32,
    class_id: u32,
    rim: u32,
    figure: u32,
}

// Branch-free on purpose: derivatives taken after this call stay in uniform control flow.
fn surface(in: VertexOut) -> Surface {
    var s: Surface;
    let mat_id = in.packed & 0xFFu;
    s.class_id = (in.packed >> 8u) & 0xFFu;
    s.rim = (in.packed >> 16u) & 0xFFu;
    s.figure = in.figure;
    let is_floor = s.class_id == CLASS_FLOOR;
    let mat = materials.mats[mat_id];
    let grout = materials.mats[GROUT];
    let world_xy = in.world.xy;
    let footprint = max(length(fwidth(world_xy)), 1e-4);

    let relief = floor_height(world_xy, frame.floor_salt);
    let g = select(0.0, relief.y, is_floor);
    s.P = in.world;
    s.geo_n = normalize(in.normal);
    s.N = select(s.geo_n, floor_normal(world_xy, frame.floor_salt), is_floor);
    s.V = normalize(frame.eye - in.world);

    let decal_d = decal_distance(world_xy);
    let decal = select(0.0, 1.0 - smoothstep(-footprint, footprint, decal_d), is_floor);
    let base = mix(mat.albedo.rgb, grout.albedo.rgb, g) * mix(1.0, 0.55, decal);
    s.albedo = base;
    s.tint = mix(mat.tint.rgb, grout.tint.rgb, g);
    s.gloss = mix(mat.tint.a, grout.tint.a, g);
    s.ks = mix(mat.params.x, grout.params.x, g);
    s.roughness = mix(mat.params.y, grout.params.y, g);
    s.metalness = mix(mat.params.z, grout.params.z, g);
    s.spec_color = mix(vec3<f32>(1.0), mat.metal.rgb, mat.albedo.a);
    s.metal_color = mat.metal.rgb;
    s.emissive = mat.emissive.rgb + frame.decal_color * (frame.decal_emission * decal);
    s.shadow = select(1.0, blob_shadow(world_xy), is_floor);
    s.excluded = select(0.0, 1.0, is_floor && (decal_d < 0.1 || s.shadow < 0.999));
    return s;
}

// Shared falloff: saturate(1 - (d/r)^4)^2 / (1 + d^2).
fn atten(d: f32, r: f32) -> f32 {
    let x = d / r;
    let x2 = x * x;
    let q = saturate(1.0 - x2 * x2);
    return q * q / (1.0 + d * d);
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn ambient(n: vec3<f32>) -> vec3<f32> {
    return mix(frame.ground, frame.sky, 0.5 + 0.5 * n.z) * frame.ambient_intensity;
}

fn output(s: Surface, color: vec3<f32>) -> FragOut {
    var out: FragOut;
    out.color = vec4<f32>(color, 1.0);
    let vn = normalize((frame.view * vec4<f32>(s.geo_n, 0.0)).xyz);
    // alpha = class id + figure index / 16 (figures) or + 0.5 (excluded floor pixels).
    out.normal_class = vec4<f32>(vn, f32(s.class_id) + f32(s.figure) / 16.0 + 0.5 * s.excluded);
    return out;
}

// ---------------------------------------------------------------------------------------------
// Look 1: toon (Lambert irradiance quantised once into 3 bands, shadow tint, hard highlight).
// ---------------------------------------------------------------------------------------------

// Minimum anti-aliasing half-width of a ramp edge as a fraction of the upper threshold, and the
// hard highlight (step of the Blinn-Phong lobe, level = mean of the lobe above the step). materials.rs
// holds the same values.
const TOON_EDGE_MIN: f32 = 0.01;
const TOON_HIGHLIGHT_EDGE: f32 = 0.5;
const TOON_HIGHLIGHT_LEVEL: f32 = 0.72;

fn aa(x: f32, t: f32, w: f32) -> f32 {
    return smoothstep(t - w, t + w, x);
}

// One-step highlight per light. No derivatives inside the light loop: fixed-width step, SSAA smooths it.
fn toon_highlight(s: Surface, L: vec3<f32>, E: vec3<f32>) -> vec3<f32> {
    let nlc = saturate(dot(s.N, L));
    let H = normalize(L + s.V);
    let lobe = pow(max(dot(s.N, H), 1e-4), s.gloss);
    let step_lobe = smoothstep(TOON_HIGHLIGHT_EDGE - 0.05, TOON_HIGHLIGHT_EDGE + 0.05, lobe);
    let level = s.ks * (s.gloss + 8.0) / 8.0 * TOON_HIGHLIGHT_LEVEL;
    return s.spec_color * (level * step_lobe * nlc * smoothstep(0.0, 0.2, nlc)) * E;
}

@fragment
fn fs_toon(in: VertexOut) -> FragOut {
    let s = surface(in);
    let key_e = frame.key_color * frame.key_intensity;
    var irradiance = key_e * saturate(dot(s.N, frame.key_dir));
    var highlight = toon_highlight(s, frame.key_dir, key_e);

    for (var i = 0u; i < frame.light_count; i = i + 1u) {
        let l = lights.data[i];
        let dl = l.position - s.P;
        let d2 = dot(dl, dl);
        if d2 >= l.radius * l.radius {
            continue;
        }
        let d = sqrt(d2);
        let L = dl / d;
        let a = atten(d, l.radius);
        // ---- look-specific term ----
        irradiance = irradiance + l.color * (saturate(dot(s.N, L)) * a);
        highlight = highlight + toon_highlight(s, L, l.color * a);
    }
    // Quantise the accumulated direct irradiance once, in absolute luminance at light gain 1. The loop
    // leaves control flow uniform again, so the derivative is legal here.
    let ramp = frame.toon_ramp;
    let y = luminance(irradiance) * frame.light_factor;
    let w = max(fwidth(y), TOON_EDGE_MIN * ramp.y);
    let level = ramp.z * aa(y, ramp.x, w) + (ramp.w - ramp.z) * aa(y, ramp.y, w);
    let direct = s.albedo * irradiance * (level / max(y, 1e-6)) + highlight;
    // Shadow colour: the unlit band keeps only the ambient, tinted like the stylized look's ambient.
    let amb = ambient(s.N) * s.albedo * s.tint;
    let color = (direct * frame.light_gain + amb * frame.light_factor) * s.shadow + s.emissive;
    return output(s, color);
}

// ---------------------------------------------------------------------------------------------
// Look 2: stylized 3D (wrap diffuse with terminator tint, soft specular, fresnel rim on figures).
// ---------------------------------------------------------------------------------------------

fn stylized_light(s: Surface, L: vec3<f32>, E: vec3<f32>) -> vec3<f32> {
    let nl = dot(s.N, L);
    let d = saturate((nl + 0.45) / 1.45);
    let shade = d * mix(s.tint, vec3<f32>(1.0), smoothstep(0.0, 0.6, d));
    let H = normalize(L + s.V);
    let nlc = saturate(nl);
    let spec = s.ks * (s.gloss + 8.0) / 8.0 * pow(max(dot(s.N, H), 1e-4), s.gloss) * nlc * smoothstep(0.0, 0.2, nlc);
    return s.albedo * E * shade + s.spec_color * (spec * E);
}

fn rim_term(s: Surface) -> vec3<f32> {
    let rim = materials.rims[s.rim];
    let f = pow(max(1.0 - saturate(dot(s.N, s.V)), 1e-4), rim.color_power.w);
    let term = rim.strength.x * f * rim.color_power.rgb * (0.6 + 0.4 * saturate(s.N.z + 0.3));
    let is_figure = s.class_id == CLASS_PLAYER || s.class_id == CLASS_ENEMY;
    return select(vec3<f32>(0.0), term, is_figure);
}

@fragment
fn fs_stylized(in: VertexOut) -> FragOut {
    let s = surface(in);
    var direct = stylized_light(s, frame.key_dir, frame.key_color * frame.key_intensity);

    for (var i = 0u; i < frame.light_count; i = i + 1u) {
        let l = lights.data[i];
        let dl = l.position - s.P;
        let d2 = dot(dl, dl);
        if d2 >= l.radius * l.radius {
            continue;
        }
        let d = sqrt(d2);
        let L = dl / d;
        let a = atten(d, l.radius);
        // ---- look-specific term ----
        direct = direct + stylized_light(s, L, l.color * a);
    }
    let amb = ambient(s.N) * s.albedo * s.tint;
    // The rim is a look property, not a light: it is not scaled by the light gain.
    let color = (direct * frame.light_gain + amb * frame.light_factor) * s.shadow + rim_term(s) + s.emissive;
    return output(s, color);
}

// ---------------------------------------------------------------------------------------------
// Look 3: realistic (GGX / height-correlated Smith / Schlick, analytic ambient).
// ---------------------------------------------------------------------------------------------

// a2 = GGX alpha squared, already widened by the geometric specular anti-aliasing.
fn ggx_light(s: Surface, L: vec3<f32>, E: vec3<f32>, a2: f32) -> vec3<f32> {
    let nl_raw = dot(s.N, L);
    let NL = max(nl_raw, 1e-4);
    let NV = max(dot(s.N, s.V), 1e-4);
    let H = normalize(L + s.V);
    let NH = max(dot(s.N, H), 1e-4);
    let VH = max(dot(s.V, H), 1e-4);
    let dd = NH * NH * (a2 - 1.0) + 1.0;
    let D = a2 / (PI * dd * dd);
    let vis = 0.5 / (NL * sqrt(NV * NV * (1.0 - a2) + a2) + NV * sqrt(NL * NL * (1.0 - a2) + a2));
    let F0 = mix(vec3<f32>(0.04), s.metal_color, s.metalness);
    let m = 1.0 - VH;
    let m5 = m * m * m * m * m;
    let F = F0 + (vec3<f32>(1.0) - F0) * m5;
    let kd = (vec3<f32>(1.0) - F) * (1.0 - s.metalness);
    let lit = select(0.0, 1.0, nl_raw > 0.0);
    return (kd * s.albedo + PI * D * vis * F) * (NL * lit) * E;
}

// Karis' analytic approximation of the split-sum environment BRDF (mobile).
fn env_brdf_approx(F0: vec3<f32>, roughness: f32, NV: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, pow(2.0, -9.28 * NV)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return F0 * ab.x + ab.y;
}

@fragment
fn fs_realistic(in: VertexOut) -> FragOut {
    let s = surface(in);
    // Geometric specular anti-aliasing (Kaplanyan/Filament): widen alpha^2 by the screen-space
    // variance of the shading normal, taken before the light loop.
    let dNx = dpdx(s.N);
    let dNy = dpdy(s.N);
    let variance = 0.15915494 * (dot(dNx, dNx) + dot(dNy, dNy));
    let alpha = s.roughness * s.roughness;
    let a2 = saturate(alpha * alpha + min(2.0 * variance, 0.18));
    var direct = ggx_light(s, frame.key_dir, frame.key_color * frame.key_intensity, a2);

    for (var i = 0u; i < frame.light_count; i = i + 1u) {
        let l = lights.data[i];
        let dl = l.position - s.P;
        let d2 = dot(dl, dl);
        if d2 >= l.radius * l.radius {
            continue;
        }
        let d = sqrt(d2);
        let L = dl / d;
        let a = atten(d, l.radius);
        // ---- look-specific term ----
        direct = direct + ggx_light(s, L, l.color * a, a2);
    }
    let NV = max(dot(s.N, s.V), 1e-4);
    let R = reflect(-s.V, s.N);
    let F0 = mix(vec3<f32>(0.04), s.metal_color, s.metalness);
    let env = env_brdf_approx(F0, s.roughness, NV);
    let amb = ambient(s.N) * s.albedo * (1.0 - s.metalness) * (vec3<f32>(1.0) - env) + ambient(R) * env;
    let color = (direct * frame.light_gain + amb * frame.light_factor) * s.shadow + s.emissive;
    return output(s, color);
}

// ---------------------------------------------------------------------------------------------
// Unlit emissive meshes and billboards (identical in all looks).
// ---------------------------------------------------------------------------------------------

@fragment
fn fs_emissive(in: VertexOut) -> FragOut {
    let mat = materials.mats[in.packed & 0xFFu];
    let n = normalize(in.normal);
    let v = normalize(frame.eye - in.world);
    let facing = saturate(dot(n, v));
    let color = mix(mat.emissive.rgb, mat.tint.rgb, facing * facing);
    var out: FragOut;
    out.color = vec4<f32>(color, 1.0);
    let vn = normalize((frame.view * vec4<f32>(n, 0.0)).xyz);
    out.normal_class = vec4<f32>(vn, f32(CLASS_EMISSIVE) + f32(in.figure) / 16.0);
    return out;
}

struct BoltIn {
    @location(0) center: vec3<f32>,
    @location(1) half_length: f32,
    @location(2) dir: vec2<f32>,
    @location(3) half_width: f32,
    @location(4) alpha: f32,
    @location(5) color: vec3<f32>,
    @location(6) intensity: f32,
}

struct BoltOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) alpha: f32,
    @location(4) intensity: f32,
}

@vertex
fn vs_bolt(@builtin(vertex_index) vertex_index: u32, b: BoltIn) -> BoltOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let d3 = vec3<f32>(b.dir, 0.0);
    let projected = vec2<f32>(dot(d3, frame.cam_right), dot(d3, frame.cam_up));
    let sd = select(vec2<f32>(1.0, 0.0), normalize(projected), length(projected) > 1e-5);
    let perp = vec2<f32>(-sd.y, sd.x);
    let margin = 0.04;
    let local = corners[vertex_index % 6u] * vec2<f32>(b.half_length + margin, b.half_width + margin);
    let screen = sd * local.x + perp * local.y;
    let world = b.center + frame.cam_right * screen.x + frame.cam_up * screen.y;
    var out: BoltOut;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.local = local;
    out.half_size = vec2<f32>(b.half_length, b.half_width);
    out.color = b.color * b.intensity;
    out.alpha = b.alpha;
    out.intensity = b.intensity;
    return out;
}

@fragment
fn fs_emissive_bolt(in: BoltOut) -> FragOut {
    let seg = max(in.half_size.x - in.half_size.y, 0.0);
    let q = vec2<f32>(max(abs(in.local.x) - seg, 0.0), in.local.y);
    let sd = length(q) - in.half_size.y;
    let w = max(fwidth(sd), 1e-5);
    let coverage = saturate(0.5 - sd / w);
    // White-hot core along the centre line.
    let core = 1.0 - smoothstep(0.0, 0.75, saturate((sd + in.half_size.y) / in.half_size.y));
    let color = mix(in.color, vec3<f32>(in.intensity), core);
    var out: FragOut;
    out.color = vec4<f32>(color, in.alpha * coverage);
    out.normal_class = vec4<f32>(0.0, 0.0, 1.0, f32(CLASS_EMISSIVE));
    return out;
}
