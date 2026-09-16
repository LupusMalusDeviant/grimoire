// Mesh pass (plan 0002 WP2.5): PBR shading per ADR-0014 (game repo) — GGX microfacet specular
// with height-correlated Smith visibility, Schlick Fresnel, metallic-roughness parameters from
// `PbrMaterial`, an analytic ambient term (hemisphere diffuse plus a Karis split-sum specular
// approximation) and geometric specular anti-aliasing (OF-3.5: roughness widened by the
// screen-space variance of the shading normal). Ported from the look-dev spike's `fs_realistic`
// (engine branch `p1/wp2-look-dev-spike`, `spikes/look-dev/src/shaders/world.wgsl`), with the
// spike's per-look brightness calibration (`light_gain`/`light_factor`, an extra `PI` on the
// specular term) dropped in favour of a physically normalised Cook-Torrance BRDF, since this pass
// is not compared side-by-side against other looks the way the spike's three shaders were.
//
// The key light, the ambient term and the point lights in `Camera::lights` (contract §6) all use
// the same `ggx_light`/`point_light_falloff` terms, so a point light and the key light shade
// identically for the same incoming radiance. Point lights use the spike's shared falloff
// (`saturate(1 - (d/r)^4)^2 / (1 + d^2)`) with a hard cutoff at `range` (contract §6 "range");
// the light loop is a simple, unclustered P1 loop over `Camera::light_count` (at most
// `MAX_POINT_LIGHTS`, see `mesh_pass.rs`) — clustered forward+ and the `Low 32`/`High 256` count
// budget are WP3.4's job, not this pass's.
//
// `PointLight::is_bullet_light`/`BulletLightCap` (PRD-0003 rule 5 / FR-15) are deliberately not
// applied here: contract §6 assigns *how* the cap limits the shading equation to the stylebook
// (WP2.7) and *implementing* it to the PBR pass at WP3.4/WP3.5, once bullet-cloud lights actually
// exist (WP3.5's bullet pass). This pass shades every valid point light identically regardless of
// that flag.
//
// Textures (base colour, tangent-space normal, occlusion-roughness-metallic) are sampled if the
// renderer has them registered (`WgpuRenderer::register_texture`), otherwise a 1x1 fallback
// texture (white / flat-up-normal / neutral ORM, bound by `mesh_pass.rs`) is bound in their place,
// so the same shader code path always runs: a missing texture is exactly a texture that samples to
// the neutral value, and the result reduces to the material's plain factors. Normal mapping uses a
// derivative-based tangent frame (Lengyel/Schüler) rather than a precomputed per-vertex tangent,
// because `MeshVertex` (mesh.rs) carries none; growing it would touch every procedural test-mesh
// generator for a feature with no authored UV-mapped asset in P1 to validate handedness against
// (OF-3.4/P2 Blender pipeline). The OpenGL (+Y) convention (contract §6) is followed as written;
// its exact handedness against this derivative frame is unverified without such an asset.
//
// Winding is not assumed to be consistent (see procedural.rs); the pipeline disables back-face
// culling, like the sprite pass.
//
// Layout must match `mesh_pass.rs`'s `CameraGpu`/`PointLightGpu` (1264 bytes) and
// `MeshInstanceGpu` (112 bytes) exactly; both sides are hand-kept in sync (no shared codegen).
//
// Shadows (plan 0002 WP2.6, OF-3.2): `camera.light_view_proj`/`camera.shadow_params` (see
// `mesh_pass.rs`'s `CameraGpu` doc comment for the packing) drive a PCF-filtered lookup into the
// key-light shadow map (group 2, `shadow_pass.rs`), applied only to the key light's own
// contribution — point lights and ambient are unaffected, matching how a single key-light shadow
// map behaves. `shadow_params.z <= 0.5` (no shadow map wanted this frame, or no valid key
// light/camera) skips the texture reads entirely rather than sampling a possibly-stale map.
//
// Skinning (P1 addendum, contract §6 changelog 2026-09-16, `mesh_pass.rs`'s module doc comment):
// `vs_skinned` is the second vertex path for a `MeshInstance` with `skin` set. It reads two extra
// per-vertex attributes (`joints`, `weights`, `MeshVertex`'s new fields) and one extra per-instance
// attribute (`bone_offset`), looks up up to four bone matrices in the group-3 storage buffer
// (`bone_matrices`, one entry per `StageFrame::joint_matrices` slot, engine ADR-0013's proven
// `vertex_storage` capability), blends them (linear blend skinning) into one matrix, and applies it
// to the vertex *before* the usual model transform — everything after that point (the `model`
// multiply, the normal matrix, filling `VertexOutput`) is identical to `vs_main`. Both vertex
// entry points feed the *same* `fs_main`: skinning changes only where a vertex's position and
// normal come from, never how a fragment is shaded.

const PI: f32 = 3.14159265358979;
// Mirrors `mesh_pass::MIN_ROUGHNESS` (the Rust side clamps the same way before upload; this clamp
// is the shader's own defence against a texture-sampled roughness of exactly 0, which would
// divide by zero in `visibility_smith_height_correlated`).
const MIN_ROUGHNESS: f32 = 0.045;
// `1 / (2 * PI)`, the geometric specular anti-aliasing normalisation (Kaplanyan 2016, "Stable
// Specular Highlights"; Tokuyoshi's variant used e.g. by Filament).
const INV_TWO_PI: f32 = 0.15915494;
// Cap on the specular-AA roughness widening (`2 * variance`, before the `MIN_ROUGHNESS` floor),
// matching the spike: a screen-aligned, near-mirror surface should not be pushed all the way to
// fully rough.
const MAX_SPECULAR_AA_WIDEN: f32 = 0.18;

struct PointLightGpu {
    position: vec3<f32>,
    range: f32,
    // Already colour * intensity (contract §6 `PointLight`).
    color: vec3<f32>,
    _pad: f32,
}

struct Camera {
    view_proj: mat4x4<f32>,
    // xyz: world-space eye point (for the view vector). w unused.
    eye: vec4<f32>,
    // xyz: unit vector from a lit surface point *towards* the key light (the negated, normalised
    // light travel direction). w unused.
    light_dir: vec4<f32>,
    // rgb: key light colour already multiplied by its intensity. w unused.
    key_light: vec4<f32>,
    // rgb: ambient colour for surfaces facing straight up (normal.z = 1), already multiplied by
    // its intensity. w unused.
    ambient_sky: vec4<f32>,
    // rgb: ambient colour for surfaces facing straight down (normal.z = -1), already multiplied by
    // its intensity. w unused.
    ambient_ground: vec4<f32>,
    // Number of entries in `lights` that are actually lit (0..=MAX_POINT_LIGHTS).
    light_count: u32,
    // 1.0 enables the OF-3.5 geometric specular-AA roughness widening below, 0.0 disables it
    // (`WgpuRenderer::render_stage_with_specular_aa`, a WP2.5/OF-3.5 measurement hook — production
    // rendering through `render_stage` always uses 1.0).
    specular_aa_strength: f32,
    _pad1: u32,
    _pad2: u32,
    // World-to-light-space view-projection for the key-light shadow map
    // (`stage3d::key_light_view_projection`).
    light_view_proj: mat4x4<f32>,
    // x: shadow-map texel size (`1.0 / map_size`). y: PCF kernel radius in texels (float; `0` = a
    // single tap). z: `1.0` to sample the shadow map this frame, `0.0` to skip it entirely. w
    // unused.
    shadow_params: vec4<f32>,
    lights: array<PointLightGpu, 32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0) var base_color_texture: texture_2d<f32>;
@group(1) @binding(1) var normal_texture: texture_2d<f32>;
@group(1) @binding(2) var orm_texture: texture_2d<f32>;
@group(1) @binding(3) var material_sampler: sampler;

// Key-light shadow map (plan 0002 WP2.6): a depth texture rendered by `shadow_pass.rs`, sampled
// here through a comparison sampler for hardware-filtered PCF taps
// (`textureSampleCompareLevel` returns the fraction of nearby samples the surface is closer than,
// i.e. already "how lit is this point", not a raw depth value).
@group(2) @binding(0) var shadow_map: texture_depth_2d;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct InstanceInput {
    @location(3) transform_0: vec4<f32>,
    @location(4) transform_1: vec4<f32>,
    @location(5) transform_2: vec4<f32>,
    @location(6) transform_3: vec4<f32>,
    @location(7) base_color: vec4<f32>,
    @location(8) emissive: vec4<f32>,
    // x = metallic factor, y = roughness factor, z/w reserved (0).
    @location(9) material_params: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) base_color: vec4<f32>,
    @location(4) @interpolate(flat) emissive: vec4<f32>,
    @location(5) @interpolate(flat) material_params: vec4<f32>,
}

// Skinning vertex path (P1 addendum, see this file's header comment): `MeshVertex`'s full layout,
// unlike `VertexInput` above which only reads its first three fields.
struct SkinnedVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
}

struct SkinnedInstanceInput {
    @location(5) transform_0: vec4<f32>,
    @location(6) transform_1: vec4<f32>,
    @location(7) transform_2: vec4<f32>,
    @location(8) transform_3: vec4<f32>,
    @location(9) base_color: vec4<f32>,
    @location(10) emissive: vec4<f32>,
    // x = metallic factor, y = roughness factor, z/w reserved (0).
    @location(11) material_params: vec4<f32>,
    // Index of this instance's first matrix in `bone_matrices` (contract §6
    // `SkinBinding::joint_offset`).
    @location(12) bone_offset: u32,
}

// Bone matrix palette for the whole frame's skinned instances, concatenated in
// `StageFrame::joint_matrices` order (`mesh_pass.rs`'s `render` uploads it verbatim);
// `SkinnedInstanceInput::bone_offset` picks out one instance's slice. Read-only, vertex-stage only
// (engine ADR-0013 measured `vertex_storage` as `true`, with `max_storage_buffer_binding_size` far
// past a single instance's 256-matrix, 16 KiB palette, on every CI target adapter).
@group(3) @binding(0)
var<storage, read> bone_matrices: array<mat4x4<f32>>;

// Inverse-transpose of a 3x3 matrix built from cross products of its columns (Ogre3D / graphitemaster
// "Normals Revisited"): avoids implementing a general 3x3 inverse just for the normal matrix, and
// stays correct under non-uniform scale (the previous WP2.3 shortcut of reusing the model matrix
// directly for normals did not). Falls back to `m` itself (the old, uniform-scale-only behaviour)
// for a degenerate (non-invertible) transform rather than dividing by ~0.
fn normal_matrix(m: mat3x3<f32>) -> mat3x3<f32> {
    let a = m[0];
    let b = m[1];
    let c = m[2];
    let det = dot(a, cross(b, c));
    if abs(det) < 1e-8 {
        return m;
    }
    let inv_det = 1.0 / det;
    return mat3x3<f32>(cross(b, c) * inv_det, cross(c, a) * inv_det, cross(a, b) * inv_det);
}

@vertex
fn vs_main(vin: VertexInput, iin: InstanceInput) -> VertexOutput {
    let model = mat4x4<f32>(iin.transform_0, iin.transform_1, iin.transform_2, iin.transform_3);
    let world_position4 = model * vec4<f32>(vin.position, 1.0);
    let model3 = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz);
    let world_normal = normalize(normal_matrix(model3) * vin.normal);

    var out: VertexOutput;
    out.clip_position = camera.view_proj * world_position4;
    out.world_position = world_position4.xyz;
    out.world_normal = world_normal;
    out.uv = vin.uv;
    out.base_color = iin.base_color;
    out.emissive = iin.emissive;
    out.material_params = iin.material_params;
    return out;
}

// Linear blend skinning: the weighted sum of up to four bone matrices from `bone_matrices`,
// indexed by `joints` relative to `bone_offset` (this instance's slice, contract §6
// `SkinBinding::joint_offset`). Not renormalised: `weights` is already checked to sum to 1.0
// within 1e-3 before a mesh ever reaches the GPU (`MeshData::validate`, and the pack decoder
// before that), so the tiny residual error is negligible next to the software-adapter/driver noise
// every offscreen test already tolerates. wgpu's storage-buffer bounds checking (WebGPU robustness)
// makes an out-of-range `bone_offset + joint` index read zero rather than access undefined memory,
// even though this path trusts the CPU-side range check (`skin_binding_fits`) to keep it in range
// in the first place.
fn skin_matrix(joints: vec4<u32>, weights: vec4<f32>, bone_offset: u32) -> mat4x4<f32> {
    let m0 = bone_matrices[bone_offset + joints.x];
    let m1 = bone_matrices[bone_offset + joints.y];
    let m2 = bone_matrices[bone_offset + joints.z];
    let m3 = bone_matrices[bone_offset + joints.w];
    return mat4x4<f32>(
        m0[0] * weights.x + m1[0] * weights.y + m2[0] * weights.z + m3[0] * weights.w,
        m0[1] * weights.x + m1[1] * weights.y + m2[1] * weights.z + m3[1] * weights.w,
        m0[2] * weights.x + m1[2] * weights.y + m2[2] * weights.z + m3[2] * weights.w,
        m0[3] * weights.x + m1[3] * weights.y + m2[3] * weights.z + m3[3] * weights.w,
    );
}

// Second vertex path (P1 addendum, see this file's header comment): identical to `vs_main` from
// the model transform onward, except the vertex position/normal are first pulled through this
// instance's bone matrix palette. Feeds the same `fs_main` as `vs_main`.
@vertex
fn vs_skinned(vin: SkinnedVertexInput, iin: SkinnedInstanceInput) -> VertexOutput {
    let skin = skin_matrix(vin.joints, vin.weights, iin.bone_offset);
    let skinned_position = skin * vec4<f32>(vin.position, 1.0);
    let skin3 = mat3x3<f32>(skin[0].xyz, skin[1].xyz, skin[2].xyz);
    let skinned_normal = skin3 * vin.normal;

    let model = mat4x4<f32>(iin.transform_0, iin.transform_1, iin.transform_2, iin.transform_3);
    let world_position4 = model * skinned_position;
    let model3 = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz);
    let world_normal = normalize(normal_matrix(model3) * skinned_normal);

    var out: VertexOutput;
    out.clip_position = camera.view_proj * world_position4;
    out.world_position = world_position4.xyz;
    out.world_normal = world_normal;
    out.uv = vin.uv;
    out.base_color = iin.base_color;
    out.emissive = iin.emissive;
    out.material_params = iin.material_params;
    return out;
}

// Derivative-based tangent frame (Lengyel 2001 / Schüler 2006): builds an orthonormal tangent
// basis from screen-space derivatives of world position and UV instead of a precomputed per-vertex
// tangent. See this file's header comment for why.
fn cotangent_frame(n: vec3<f32>, p: vec3<f32>, uv: vec2<f32>) -> mat3x3<f32> {
    let dp1 = dpdx(p);
    let dp2 = dpdy(p);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let inv_max = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-12));
    return mat3x3<f32>(t * inv_max, b * inv_max, n);
}

// GGX (Trowbridge-Reitz) normal distribution term `D`.
fn distribution_ggx(n_dot_h: f32, alpha_squared: f32) -> f32 {
    let d = n_dot_h * n_dot_h * (alpha_squared - 1.0) + 1.0;
    return alpha_squared / (PI * d * d);
}

// Height-correlated Smith visibility term (Heitz 2014): already divided by the `4 * NdotL * NdotV`
// denominator, i.e. the `V` term, not the separate geometry term `G`.
fn visibility_smith_height_correlated(n_dot_l: f32, n_dot_v: f32, alpha_squared: f32) -> f32 {
    let v = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - alpha_squared) + alpha_squared);
    let l = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - alpha_squared) + alpha_squared);
    return 0.5 / max(v + l, 1e-6);
}

// Schlick's Fresnel approximation.
fn fresnel_schlick(v_dot_h: f32, f0: vec3<f32>) -> vec3<f32> {
    let m = clamp(1.0 - v_dot_h, 0.0, 1.0);
    let m5 = m * m * m * m * m;
    return f0 + (vec3<f32>(1.0) - f0) * m5;
}

// The look-dev spike's shared point-light falloff (`atten` in `world.wgsl`):
// `saturate(1 - (d/r)^4)^2 / (1 + d^2)`, reaching exactly `0` at `d == r` (the caller applies the
// hard `d >= r` cutoff itself, contract §6 "range").
fn point_light_falloff(distance: f32, range: f32) -> f32 {
    let x = distance / range;
    let x2 = x * x;
    let q = saturate(1.0 - x2 * x2);
    return q * q / (1.0 + distance * distance);
}

// One light's contribution: physically normalised Cook-Torrance (diffuse `kd * albedo / PI` plus
// GGX specular `D * Vis * F`, no extra calibration factor), height-correlated Smith visibility,
// Schlick Fresnel. `alpha_squared` is the (specular-AA-widened) GGX roughness parameter, shared
// across every light so the anti-aliasing does not need to be recomputed per light.
fn ggx_light(
    n: vec3<f32>,
    v: vec3<f32>,
    l: vec3<f32>,
    albedo: vec3<f32>,
    f0: vec3<f32>,
    metallic: f32,
    alpha_squared: f32,
    radiance: vec3<f32>,
) -> vec3<f32> {
    let n_dot_l_raw = dot(n, l);
    if n_dot_l_raw <= 0.0 {
        return vec3<f32>(0.0);
    }
    let n_l = max(n_dot_l_raw, 1e-4);
    let n_v = max(dot(n, v), 1e-4);
    let h = normalize(l + v);
    let n_h = max(dot(n, h), 0.0);
    let v_h = max(dot(v, h), 0.0);

    let d = distribution_ggx(n_h, alpha_squared);
    let vis = visibility_smith_height_correlated(n_l, n_v, alpha_squared);
    let f = fresnel_schlick(v_h, f0);
    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);

    let diffuse = kd * albedo / PI;
    let specular = d * vis * f;
    return (diffuse + specular) * n_l * radiance;
}

// PCF-filtered key-light shadow factor at `world_position` (plan 0002 WP2.6): `1.0` fully lit,
// `0.0` fully in shadow. Averages a `(2*radius+1)^2` square kernel of hardware-filtered comparison
// taps (`camera.shadow_params.y`, a small integer clamped by `ShadowConfig::is_valid` so the tap
// count stays bounded). `textureSampleCompareLevel` (the explicit-level variant, unlike the
// implicit-derivative `textureSampleCompare`) is legal inside this data-dependent loop because it
// makes no uniformity demand on control flow. A point outside the shadow map's fitted frustum
// (light-space `x`/`y` outside `[0, 1]` after the NDC-to-UV remap, or outside its near/far range)
// is treated as fully lit — the frustum is fitted to cover the visible arena
// (`stage3d::key_light_view_projection`), so this only matters right at its edge. Called only when
// `camera.shadow_params.z > 0.5` (see `fs_main`), so the texture is never sampled on a frame that
// does not want it.
fn key_light_shadow_factor(world_position: vec3<f32>) -> f32 {
    let clip = camera.light_view_proj * vec4<f32>(world_position, 1.0);
    if clip.w <= 0.0 {
        return 1.0; // Degenerate light matrix (`key_light_view_projection`'s identity fallback).
    }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let depth = ndc.z;
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || depth < 0.0 || depth > 1.0 {
        return 1.0;
    }
    let texel = camera.shadow_params.x;
    let radius = i32(camera.shadow_params.y);
    var lit = 0.0;
    var taps = 0.0;
    for (var dy = -radius; dy <= radius; dy = dy + 1) {
        for (var dx = -radius; dx <= radius; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * texel;
            lit = lit + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + offset, depth);
            taps = taps + 1.0;
        }
    }
    return lit / max(taps, 1.0);
}

// Karis' analytic approximation of the split-sum environment BRDF ("Real Shading in Unreal Engine
// 4", mobile appendix): the specular ambient response for a given Fresnel-at-normal-incidence
// `f0`, `roughness` and `n_dot_v`, without an environment map (PRD-0003 FR-01 "einfacher
// Umgebungsterm" — this crate has none in P1).
fn env_brdf_approx(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, pow(2.0, -9.28 * n_dot_v)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n_geo = normalize(in.world_normal);
    let v = normalize(camera.eye.xyz - in.world_position);

    let base_sample = textureSample(base_color_texture, material_sampler, in.uv);
    let normal_sample = textureSample(normal_texture, material_sampler, in.uv);
    let orm_sample = textureSample(orm_texture, material_sampler, in.uv);

    let base_color = in.base_color.rgb * base_sample.rgb;
    let alpha = in.base_color.a * base_sample.a;
    let metallic = saturate(in.material_params.x * orm_sample.b);
    let roughness = clamp(in.material_params.y * orm_sample.g, MIN_ROUGHNESS, 1.0);
    let occlusion = orm_sample.r;

    // Tangent-space normal mapping; the fallback normal texture (`mesh_pass.rs`) samples to
    // (0, 0, 1) in tangent space, which `cotangent_frame`'s third basis vector always maps back to
    // `n_geo` exactly, so an unregistered normal texture reproduces the untextured geometric
    // normal bit-for-bit.
    let tbn = cotangent_frame(n_geo, in.world_position, in.uv);
    let n_ts = normal_sample.xyz * 2.0 - 1.0;
    let n = normalize(tbn * n_ts);

    let n_dot_v = max(dot(n, v), 1e-4);
    let f0 = mix(vec3<f32>(0.04), base_color, metallic);

    // Geometric specular anti-aliasing (OF-3.5): widen alpha^2 by the screen-space variance of the
    // shading normal (Kaplanyan/Tokuyoshi), computed once and shared by every light below. Gated
    // behind a per-draw-call uniform (`camera.specular_aa_strength`, not a per-fragment value), so
    // this stays legal, uniform control flow for `dpdx`/`dpdy` — and skipping the two derivative
    // calls entirely when disabled (rather than always computing them and discarding the result)
    // is what lets the OF-3.5 spike measurement (`tests/offscreen.rs`,
    // `measure_specular_aa_relative_cost_and_shimmer`) actually isolate this technique's own cost.
    let base_alpha = roughness * roughness;
    var alpha_squared = base_alpha * base_alpha;
    if camera.specular_aa_strength > 0.5 {
        let d_nx = dpdx(n);
        let d_ny = dpdy(n);
        let normal_variance = (dot(d_nx, d_nx) + dot(d_ny, d_ny)) * INV_TWO_PI;
        alpha_squared = alpha_squared + min(2.0 * normal_variance, MAX_SPECULAR_AA_WIDEN);
    }
    alpha_squared = clamp(
        alpha_squared,
        MIN_ROUGHNESS * MIN_ROUGHNESS * MIN_ROUGHNESS * MIN_ROUGHNESS,
        1.0,
    );

    // Shadow map (plan 0002 WP2.6): only the key light's own contribution is attenuated — point
    // lights and ambient are unaffected, matching a single key-light shadow map's usual scope.
    var shadow_factor = 1.0;
    if camera.shadow_params.z > 0.5 {
        shadow_factor = key_light_shadow_factor(in.world_position);
    }
    var direct = shadow_factor * ggx_light(n, v, camera.light_dir.xyz, base_color, f0, metallic, alpha_squared, camera.key_light.rgb);

    for (var i = 0u; i < camera.light_count; i = i + 1u) {
        let light = camera.lights[i];
        let to_light = light.position - in.world_position;
        let distance_squared = dot(to_light, to_light);
        if distance_squared >= light.range * light.range {
            continue;
        }
        let distance = sqrt(distance_squared);
        let l = to_light / max(distance, 1e-6);
        let attenuation = point_light_falloff(distance, light.range);
        direct = direct + ggx_light(n, v, l, base_color, f0, metallic, alpha_squared, light.color * attenuation);
    }

    // Analytic ambient (PRD-0003 FR-01): hemisphere diffuse split by the shading normal, plus a
    // Karis split-sum specular approximation sampled along the reflection vector, matching the
    // spike's `fs_realistic`.
    let ambient_dir = mix(camera.ambient_ground.rgb, camera.ambient_sky.rgb, n.z * 0.5 + 0.5);
    let reflect_dir = reflect(-v, n);
    let ambient_reflect = mix(camera.ambient_ground.rgb, camera.ambient_sky.rgb, reflect_dir.z * 0.5 + 0.5);
    let env = env_brdf_approx(f0, roughness, n_dot_v);
    let ambient = (ambient_dir * base_color * (1.0 - metallic) * (vec3<f32>(1.0) - env) + ambient_reflect * env) * occlusion;

    let color = direct + ambient + in.emissive.rgb;
    return vec4<f32>(color, alpha);
}
