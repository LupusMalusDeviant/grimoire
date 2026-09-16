//! Camera, mesh, material and light channels of the stage frame (contract §6, "Bühnen-Frame,
//! Ebenenreihenfolge und Bullet-Kanal", render contract v1, WP2.2).
//!
//! Purely additive on top of the sibling `stage` module's P1 bullet channel: every type here is new, and
//! [`crate::StageFrame`]/[`crate::StageStats`] (already `#[non_exhaustive]`, contract §2 rule 13)
//! simply grow further fields and counters. Nothing here changes [`crate::BulletInstance`], the
//! palette-space constants or [`crate::RenderLayer`]'s draw order.
//!
//! Coordinate convention for everything here: X right, Y away from the viewer (the same ground
//! plane as [`crate::Camera2D`] and [`crate::BulletInstance`]), Z up. [`Camera25D`] looks down at
//! the ground (`Z = 0`) plane from a tilt-dependent height above [`Camera25D::target`].
//!
//! This module owns the realistic PBR look decided in the game's ADR-0014 (replacing the earlier
//! toon/cel-shading direction): materials follow glTF metallic-roughness, consistent with the
//! game's PRD-0003.
//!
//! **WP2.4 (this work package):** [`CameraFollow`] wires the tilted view-projection into a
//! following camera — a critically damped spring plus look-ahead, both driven render-side (see
//! [`CameraFollow::update`]'s doc comment for why the simulation never runs it and the plan 0002
//! WP2.4 replay-hash gate this guarantees). `grimoire`'s facade (crate `grimoire`, not this one)
//! drives it from the player proxy's interpolated position once per frame and feeds axes 2/3 of
//! `TickInput` from `grimoire::sample_aim`/`grimoire::quantize_aim` (contract §9.4).
//!
//! **WP2.5 (real PBR shading):** `mesh_pass`'s fragment shader (`mesh.wgsl`) now shades every
//! [`MeshInstance`] with GGX microfacet specular, height-correlated Smith visibility, Schlick
//! Fresnel, an analytic ambient term and geometric specular anti-aliasing (OF-3.5), consuming
//! [`PointLight`], [`DirectionalLight`] and [`AmbientLight`] for real; this module still owns only
//! their data contract, plus [`eye_position`]/[`cluster_camera_params`] (added for the shading's
//! view vector and, from WP3.4, its clustering basis) and [`view_projection`] below.
//!
//! **WP3.4 (clustered forward+, engine ADR-0015):** the point-light *count* budget (`Low 32` /
//! `High 256`, `crate::LightBudget`) and [`PointLight::is_bullet_light`]/[`BulletLightCap`]'s
//! actual effect on the shading equation are both wired in by `mesh_pass::build_light_list` and
//! `cluster_pass.rs`/`cluster.wgsl`, replacing the earlier smaller, internal, unclustered
//! pre-WP3.4 light-array limit. [`point_light_from_bullet`] is the PO-decided (2026-09-16) only
//! permitted way to create a bullet-cloud light. Mesh geometry and its GPU upload are WP2.3's job
//! (see [`crate::mesh`], [`crate::procedural`] and [`crate::WgpuRenderer::register_mesh`]); this module
//! still owns only the data contract those steps consume, plus the `view_projection` helper
//! WP2.3's mesh pass builds on ([`Camera25D::screen_to_ground`]/[`Camera25D::ground_to_screen`]
//! stay ray-casts, not a matrix, so both keep working without a GPU) and that [`CameraFollow`] now
//! keeps fed with a followed [`Camera25D::target`] every frame.
//!
//! **WP2.6 (shadows, OF-3.2):** two switchable techniques, selected per frame by
//! [`ShadowConfig::mode`] ([`StageFrame::shadow_config`], persists across `clear()` like the other
//! scene-level fields): a depth-only shadow map for [`StageFrame::key_light`], fitted with an
//! orthographic frustum around the camera's ground target ([`key_light_view_projection`], consumed
//! by `mesh_pass`/a new `shadow_pass` module), and cheap blob shadows
//! ([`BlobShadowInstance`], drawn as soft darkening decals on the ground). Both are data contracts
//! only here; rendering them is `mesh_pass`'s/`shadow_pass`'s job, mirroring how this module never
//! rasterises meshes or lights itself. [`PointLight::casts_shadow`] and
//! [`ShadowConfig::max_point_shadow_casters`] prepare the interface for a limited number of
//! point-light shadow casters (PRD-0003 OF-3.2's "begrenzte Punktlicht-Schattenwerfer"); **actually
//! rendering point-light shadows is deferred** past this work package (see the WP2.6 ADR) —
//! [`ShadowMode::KeyLightPlusPoints`] currently shades identically to [`ShadowMode::KeyLight`].

use grimoire_core::math::dmath;

use crate::RenderLayer;

/// Opaque reference to a mesh registered with the renderer. The registry itself (how a
/// [`MeshHandle`] is created and what it points at) is WP2.3's job; P1 cannot yet check whether a
/// given handle is registered, so [`MeshInstance`] validation checks only what this contract can
/// see (`transform`, `layer` and `material`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MeshHandle(pub u32);

/// Index into a frame's [`crate::StageFrame::materials`] table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MaterialHandle(pub u32);

/// Opaque reference to a texture registered with the renderer, used by [`PbrMaterial`]'s optional
/// texture slots. Like [`MeshHandle`], the registry is WP2.3's job; a handle's validity cannot yet
/// be checked against a real texture table in P1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TextureHandle(pub u32);

/// Largest number of joints one [`SkinBinding`] may reference (P1 skinning addendum, contract §6
/// changelog 2026-09-16). Matches engine ADR-0013's measured headroom (256 bones x 64 bytes = 16
/// KiB, far under every measured `max_storage_buffer_binding_size`) and the pack format's own
/// `FNP_SKELETON::joint_count` limit, so a mesh decoded from a conforming pack can never exceed
/// it.
pub const MAX_SKIN_JOINTS: u32 = 256;

/// A [`MeshInstance`]'s bone matrix palette: a contiguous range of
/// [`crate::StageFrame::joint_matrices`], analogous to how [`MaterialHandle`] indexes
/// [`crate::StageFrame::materials`]. `joint_matrices` holds already-composed
/// (`bone_local_pose * inverse_bind`) skinning matrices for *every* skinned instance in the frame,
/// concatenated; this binding says which slice belongs to one instance. Computing those matrices
/// from a skeleton and a pose (animation curves, blending) is explicitly out of this package's
/// scope — the caller supplies the final matrices already multiplied.
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`] (`joint_offset: 0, joint_count: 0`, which [`MeshInstance::skin`] never sets on its
/// own — a `None` skin binding is the "no skeleton" case, not a zero-length one).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkinBinding {
    /// Index of this instance's first matrix in [`crate::StageFrame::joint_matrices`].
    pub joint_offset: u32,
    /// Number of matrices belonging to this instance, `1..=MAX_SKIN_JOINTS`.
    /// [`crate::MeshVertex::joints`] indices for this mesh must all be `< joint_count` (checked by
    /// the pack decoder against the source skeleton, not re-checked per frame here).
    pub joint_count: u32,
}

/// Column-major 4x4 identity matrix, in the same convention as [`crate::Camera2D::view_projection`].
const IDENTITY_TRANSFORM: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Converts an angle from degrees to radians using only basic arithmetic (no transcendental call),
/// so it stays within the arithmetic rule this module follows for [`Camera25D`] (see the
/// "Determinism" note on [`Camera25D::screen_to_ground`]).
fn radians(degrees: f32) -> f32 {
    degrees * (dmath::PI / 180.0)
}

/// Tilted perspective camera for the 2.5D stage (contract §6, PRD-0003 FR-03).
///
/// The camera never yaws or rolls: it always looks along `+Y` (world "away from the viewer") and
/// down towards the ground plane `Z = 0`, pitched by [`Camera25D::tilt_degrees`] below horizontal.
/// This matches the non-goal "no freely rotatable camera mode" (PRD-0003): only [`Camera25D::target`]
/// and the derived look-ahead (WP2.4) move the view.
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera25D {
    /// Ground point (world/ground units, contract §6 `BulletInstance::position` convention) at
    /// the centre of the view, before any look-ahead offset is applied.
    pub target: [f32; 2],
    /// Pitch of the view direction below horizontal, in degrees. `90.0` looks straight down;
    /// smaller values look more towards the horizon. The shipped look (ADR-0014, PRD-0003 FR-03)
    /// uses `60.0..=75.0`; this type accepts any finite value; extreme or non-finite values simply
    /// make [`Camera25D::screen_to_ground`] and [`Camera25D::ground_to_screen`] return `None` more
    /// often; they never produce NaN (see the "Determinism" note below).
    pub tilt_degrees: f32,
    /// Vertical field of view, in degrees.
    pub fov_y_degrees: f32,
    /// Distance from [`Camera25D::target`] to the camera's eye point, along the view direction (in
    /// world units). Controls perceived zoom.
    pub distance: f32,
    /// Maximum offset the WP2.4 following system may apply ahead of the player's movement, in
    /// world units. Carried here so the follow system and any renderer that visualises the camera
    /// rig read one shared value; WP2.2 does not compute or apply a look-ahead offset itself.
    pub look_ahead_max: f32,
    /// Time constant (seconds) of the WP2.4 critically damped look-ahead spring. Carried here for
    /// the same reason as [`Camera25D::look_ahead_max`]; WP2.2 does not run the spring itself.
    pub look_ahead_smoothing: f32,
}

impl Default for Camera25D {
    fn default() -> Self {
        Self {
            target: [0.0, 0.0],
            // Midpoint of the shipped 60-75 degree range (ADR-0014, PRD-0003 FR-03).
            tilt_degrees: 67.5,
            fov_y_degrees: 50.0,
            distance: 20.0,
            look_ahead_max: 4.0,
            look_ahead_smoothing: 0.25,
        }
    }
}

/// Orthonormal camera basis and derived projection quantities, shared by
/// [`Camera25D::screen_to_ground`] and [`Camera25D::ground_to_screen`] so both stay consistent by
/// construction.
struct CameraBasis {
    eye: [f32; 3],
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    tan_half_fov_y: f32,
}

/// Builds the camera basis. Only basic arithmetic, `dmath::sin`/`dmath::cos`/`dmath::tan` and
/// `dmath::sqrt`-free vector algebra are used here, per the arithmetic rule referenced by contract
/// §9.4: no `std` transcendental functions, no `mul_add`/`powi`, no `f32::min`/`max`.
fn camera_basis(camera: &Camera25D) -> CameraBasis {
    let tilt = radians(camera.tilt_degrees);
    let (sin_t, cos_t) = (dmath::sin(tilt), dmath::cos(tilt));
    // Orthonormal for every finite `tilt`: `forward` and `up` are a rotation of `(0, 1, 0)` and
    // `(0, 0, 1)` around the fixed `right = (1, 0, 0)` axis, so no singularity occurs (unlike a
    // basis built from yaw *and* pitch, this camera never yaws).
    let forward = [0.0, cos_t, -sin_t];
    let right = [1.0, 0.0, 0.0];
    let up = [0.0, sin_t, cos_t];
    let eye = [
        camera.target[0],
        camera.target[1] - camera.distance * cos_t,
        camera.distance * sin_t,
    ];
    let half_fov_y = radians(camera.fov_y_degrees) * 0.5;
    CameraBasis {
        eye,
        forward,
        right,
        up,
        tan_half_fov_y: dmath::tan(half_fov_y),
    }
}

/// A ray from `eye` is parallel to the ground plane, or the intersection parameter would divide by
/// a value this small, whenever the ray's Z (up-axis) component's magnitude is at or below this
/// bound. Guards the division in [`Camera25D::screen_to_ground`] and the depth check in
/// [`Camera25D::ground_to_screen`] against producing `inf`/NaN instead of a clean `None`.
const GROUND_PLANE_EPSILON: f32 = f32::EPSILON;

/// Whether `value` is a real (finite), strictly positive number. Used instead of the more direct
/// `!(value > 0.0)` because that pattern trips `clippy::neg_cmp_op_on_partial_ord` (negating a
/// `PartialOrd` comparison reads as "not greater", which for `f32` is subtly different from "less
/// than or equal" once NaN is possible) even though this crate deliberately relies on NaN failing
/// the comparison here — this helper says explicitly what is intended: NaN and non-positive values
/// both fail.
fn is_positive_and_finite(value: f32) -> bool {
    value > 0.0 && value.is_finite()
}

/// World-space eye point of `camera` (plan 0002 WP2.5): the same basis
/// [`view_projection`]/[`Camera25D::screen_to_ground`] use, exposed so the mesh pass's PBR shading
/// can compute a view vector for specular lighting without duplicating the camera basis maths.
/// Not part of the crate's public API, for the same reason as [`view_projection`].
///
/// Never NaN for a well-formed camera, by construction of [`camera_basis`]; a non-finite `camera`
/// (for example `tilt_degrees: f32::NAN`) may still produce a non-finite eye, which the mesh pass
/// treats the same as "no camera" (it already discards a non-finite `view_projection` from the
/// same camera).
pub(crate) fn eye_position(camera: &Camera25D) -> [f32; 3] {
    camera_basis(camera).eye
}

/// Clustered forward+ camera basis (plan 0002 WP3.4, engine ADR-0015): the same orthonormal basis
/// [`view_projection`]/[`eye_position`] use, flattened into
/// [`crate::cluster_pass::ClusterCameraParams`] so `mesh_pass::render` can feed one shared value
/// into both the compute cluster-assignment pass and `mesh.wgsl`'s per-fragment froxel lookup —
/// built once per frame, from the same [`camera_basis`] call, so both always agree on every
/// froxel. `near`/`far` are the mesh pass's own clip planes (`mesh_pass::NEAR_PLANE`/`FAR_PLANE`),
/// passed through unchanged; not part of the crate's public API, for the same reason as
/// [`view_projection`].
pub(crate) fn cluster_camera_params(
    camera: &Camera25D,
    aspect: f32,
    near: f32,
    far: f32,
) -> crate::cluster_pass::ClusterCameraParams {
    let basis = camera_basis(camera);
    let f = 1.0 / basis.tan_half_fov_y;
    crate::cluster_pass::ClusterCameraParams {
        eye: basis.eye,
        right: basis.right,
        up: basis.up,
        forward: basis.forward,
        f_over_aspect: f / aspect,
        f,
        near,
        far,
    }
}

/// Builds the column-major view-projection matrix for `camera` (plan 0002 WP2.3): a perspective
/// projection from `camera`'s tilt/FOV/distance, using the same orthonormal basis as
/// [`Camera25D::screen_to_ground`]/[`Camera25D::ground_to_screen`] so the mesh pass's depth-tested
/// geometry lines up with the ground ray-cast used for mouse aiming. `near`/`far` are the mesh
/// pass's own clip planes (not part of the [`Camera25D`] contract, which has no such fields); both
/// must be finite and `0.0 < near < far`.
///
/// Not part of the crate's public API: `grimoire_render` keeps `wgpu` and its clip-space
/// conventions (zero-to-one depth, right-handed) out of the [`Camera25D`] contract itself,
/// matching engine ADR-0002. Callers that need the matrix outside this crate build their own from
/// [`Camera25D`]'s public fields.
///
/// Returns a matrix with only finite entries if `camera`'s basis is well-formed (finite `target`,
/// `tilt_degrees`, `fov_y_degrees`, `distance`) and `aspect`/`near`/`far` are finite and positive;
/// the mesh pass treats a non-finite result the same as "no camera" (skip drawing, still clear).
pub(crate) fn view_projection(
    camera: &Camera25D,
    aspect: f32,
    near: f32,
    far: f32,
) -> [[f32; 4]; 4] {
    let basis = camera_basis(camera);
    let (ex, ey, ez) = (basis.eye[0], basis.eye[1], basis.eye[2]);
    let (rx, ry, rz) = (basis.right[0], basis.right[1], basis.right[2]);
    let (ux, uy, uz) = (basis.up[0], basis.up[1], basis.up[2]);
    let (fx, fy, fz) = (basis.forward[0], basis.forward[1], basis.forward[2]);
    // World-to-camera-space view matrix: camera space X/Y are `right`/`up`, camera space Z is
    // `-forward` (so points in front of the camera have negative view-space Z, the right-handed
    // "looking down -Z" convention the projection matrix below assumes).
    let view: [[f32; 4]; 4] = [
        [rx, ux, -fx, 0.0],
        [ry, uy, -fy, 0.0],
        [rz, uz, -fz, 0.0],
        [
            -(rx * ex + ry * ey + rz * ez),
            -(ux * ex + uy * ey + uz * ez),
            fx * ex + fy * ey + fz * ez,
            1.0,
        ],
    ];
    // Right-handed perspective projection with wgpu's zero-to-one clip-space depth (the standard
    // `perspectiveRH_ZO` construction), reusing the same `tan_half_fov_y` as the ray casts above.
    let f = 1.0 / basis.tan_half_fov_y;
    let range = far - near;
    let proj: [[f32; 4]; 4] = [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, -far / range, -1.0],
        [0.0, 0.0, -far * near / range, 0.0],
    ];
    multiply(proj, view)
}

/// Column-major 4x4 matrix product `a * b` (`a` applied after `b`), matching the storage
/// convention of [`crate::Camera2D::view_projection`] and [`MeshInstance::transform`].
fn multiply(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.0f32; 4]; 4];
    for (col, row_out) in result.iter_mut().enumerate() {
        for (row, value) in row_out.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    result
}

impl Camera25D {
    /// Casts a ray from the camera through a pixel and intersects it with the ground plane
    /// (`Z = 0`), the inverse of [`Camera25D::ground_to_screen`].
    ///
    /// `pixel` has its origin at the top-left with Y down (like
    /// [`crate::Camera2D::screen_to_world`]); `viewport` is the viewport size in the same units.
    /// Returns ground coordinates (X right, Y away from the viewer, the same convention as
    /// [`crate::BulletInstance::position`]).
    ///
    /// Returns `None` — never NaN — when no finite ground point exists for this pixel: the ray is
    /// parallel to the ground plane (exactly at the horizon), points above the horizon (would
    /// intersect the plane behind the camera), `viewport` is empty or negative, or any input or
    /// intermediate value is non-finite.
    ///
    /// # Determinism
    /// This function and the view/projection quantities it derives (the camera's basis vectors)
    /// only use operations engine-ADR-0004 allows on every platform: basic arithmetic, `f32::sqrt`
    /// (not used here) and [`grimoire_core::math::dmath`] — never `std` transcendental functions,
    /// `mul_add`, `powi`, or `f32::min`/`max` — even though `grimoire_render` carries no
    /// determinism `clippy.toml` itself (contract §9.4).
    #[must_use]
    pub fn screen_to_ground(&self, pixel: [f32; 2], viewport: [f32; 2]) -> Option<[f32; 2]> {
        if !is_positive_and_finite(viewport[0]) || !is_positive_and_finite(viewport[1]) {
            return None;
        }
        let basis = camera_basis(self);
        let ndc_x = pixel[0] / viewport[0] * 2.0 - 1.0;
        let ndc_y = 1.0 - pixel[1] / viewport[1] * 2.0;
        let aspect = viewport[0] / viewport[1];
        let sx = ndc_x * basis.tan_half_fov_y * aspect;
        let sy = ndc_y * basis.tan_half_fov_y;
        let dir = [
            basis.forward[0] + basis.right[0] * sx + basis.up[0] * sy,
            basis.forward[1] + basis.right[1] * sx + basis.up[1] * sy,
            basis.forward[2] + basis.right[2] * sx + basis.up[2] * sy,
        ];
        if dir[2].abs() <= GROUND_PLANE_EPSILON {
            return None; // Parallel to the ground plane: exactly at the horizon.
        }
        let t = -basis.eye[2] / dir[2];
        if !is_positive_and_finite(t) {
            return None; // `t <= 0`, infinite (above the horizon) or NaN (degenerate camera).
        }
        let ground = [basis.eye[0] + t * dir[0], basis.eye[1] + t * dir[1]];
        if ground[0].is_finite() && ground[1].is_finite() {
            Some(ground)
        } else {
            None
        }
    }

    /// Projects a ground-plane point (`Z = 0`, same convention as
    /// [`Camera25D::screen_to_ground`]'s result) to a pixel position, the inverse of
    /// [`Camera25D::screen_to_ground`]. Both derive the same camera basis, so
    /// `screen_to_ground(ground_to_screen(g, vp)?, vp) == Some(g)` up to floating-point rounding
    /// for any `g` the camera can see.
    ///
    /// Returns `None` — never NaN — when the point lies at or behind the camera's view plane, when
    /// `viewport` is empty or negative, or any intermediate value is non-finite (for example an
    /// almost edge-on field of view).
    ///
    /// # Determinism
    /// Same rule as [`Camera25D::screen_to_ground`].
    #[must_use]
    pub fn ground_to_screen(&self, ground: [f32; 2], viewport: [f32; 2]) -> Option<[f32; 2]> {
        if !is_positive_and_finite(viewport[0]) || !is_positive_and_finite(viewport[1]) {
            return None;
        }
        let basis = camera_basis(self);
        let v = [
            ground[0] - basis.eye[0],
            ground[1] - basis.eye[1],
            -basis.eye[2],
        ];
        let depth = v[0] * basis.forward[0] + v[1] * basis.forward[1] + v[2] * basis.forward[2];
        if !depth.is_finite() || depth <= GROUND_PLANE_EPSILON {
            return None; // At or behind the camera's view plane, or a non-finite input.
        }
        let right_component = v[0] * basis.right[0] + v[1] * basis.right[1] + v[2] * basis.right[2];
        let up_component = v[0] * basis.up[0] + v[1] * basis.up[1] + v[2] * basis.up[2];
        let aspect = viewport[0] / viewport[1];
        let ndc_x = right_component / (depth * basis.tan_half_fov_y * aspect);
        let ndc_y = up_component / (depth * basis.tan_half_fov_y);
        if !ndc_x.is_finite() || !ndc_y.is_finite() {
            return None;
        }
        let pixel = [
            (ndc_x + 1.0) * 0.5 * viewport[0],
            (1.0 - ndc_y) * 0.5 * viewport[1],
        ];
        if pixel[0].is_finite() && pixel[1].is_finite() {
            Some(pixel)
        } else {
            None
        }
    }
}

/// One axis of a critically damped spring-damper toward `target`: the closed-form update used by
/// [`CameraFollow`], not a numerical integration of the underlying differential equation (so it
/// stays stable for any `dt`, not just small ones). `smoothing_time` is the approximate real time
/// (seconds) the spring takes to settle on a stationary target.
///
/// A non-finite `smoothing_time`, `value` or `target` snaps straight to `target` with zero
/// velocity instead of propagating NaN; a non-positive or non-finite `dt` leaves `value`/`velocity`
/// unchanged (no time has passed, so nothing should move). Only basic arithmetic is used, in
/// keeping with the spirit of engine-ADR-0004 even though this function's output does not feed
/// [`Camera25D::screen_to_ground`]'s basis directly (only [`Camera25D::target`], an input value to
/// it) and so is not itself bound by contract §9.4's determinism rule.
fn critically_damped_step(
    value: f32,
    velocity: f32,
    target: f32,
    smoothing_time: f32,
    dt: f32,
) -> (f32, f32) {
    if !is_positive_and_finite(smoothing_time) || !value.is_finite() || !target.is_finite() {
        return (target, 0.0);
    }
    if !dt.is_finite() || dt <= 0.0 {
        return (value, velocity);
    }
    // Closed-form critically damped spring (damping ratio 1), using the standard third-order
    // rational approximation of the exponential decay `exp(-omega * dt)` (Ryan Juckett,
    // "Critically Damped Ease-In/Ease-Out Smoothing", Game Programming Gems 4; the same
    // approximation widely known from Unity's `SmoothDamp`). `omega` is the spring's natural
    // angular frequency; `exp` approximates `e^(-omega * dt)`.
    let omega = 2.0 / smoothing_time;
    let x = omega * dt;
    let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = value - target;
    let temp = (velocity + omega * change) * dt;
    let new_velocity = (velocity - omega * temp) * exp;
    let new_value = target + (change + temp) * exp;
    if new_value.is_finite() && new_velocity.is_finite() {
        (new_value, new_velocity)
    } else {
        (target, 0.0)
    }
}

/// `v` scaled down to at most `max_length` (direction preserved), or `v` unchanged if it is
/// already shorter. Never NaN: a non-finite `v` or a non-finite/non-positive `max_length` returns
/// `[0.0, 0.0]`.
fn clamp_length(v: [f32; 2], max_length: f32) -> [f32; 2] {
    if !v[0].is_finite() || !v[1].is_finite() || !is_positive_and_finite(max_length) {
        return [0.0, 0.0];
    }
    let length_squared = v[0] * v[0] + v[1] * v[1];
    if !length_squared.is_finite() {
        return [0.0, 0.0];
    }
    if length_squared <= max_length * max_length {
        return v;
    }
    let length = dmath::sqrt(length_squared);
    if !is_positive_and_finite(length) {
        return [0.0, 0.0];
    }
    [v[0] / length * max_length, v[1] / length * max_length]
}

/// Render-side camera following (plan 0002 WP2.4): a critically damped spring that drives
/// [`Camera25D::target`] toward a game's focus point, plus look-ahead extrapolated from the
/// focus's own recent motion, capped at [`Camera25D::look_ahead_max`] and smoothed with the same
/// spring at time constant [`Camera25D::look_ahead_smoothing`].
///
/// Lives entirely on the render/facade side and is driven once per rendered frame with the real
/// frame time — the fixed-timestep simulation never runs this and never reads its output; only a
/// game's presentation code (`grimoire`'s main loop, or a game's own `extract_stage`) calls
/// [`CameraFollow::update`]. This keeps the simulation's state hash (`grimoire_sim::Simulation`,
/// not a dependency of this crate) independent of every [`Camera25D`] parameter, however the
/// camera follows (plan 0002 WP2.4 gate: replaying a fixed, recorded `TickInput` sequence with
/// different camera parameters yields identical hashes).
///
/// # Look-ahead formula (implementation choice, not fixed by contract §6)
/// The contract carries [`Camera25D::look_ahead_max`] and [`Camera25D::look_ahead_smoothing`] but
/// leaves the exact look-ahead law to whoever wires the spring up (WP2.4). This type extrapolates
/// one second of the focus point's most recent frame-to-frame velocity, clamped to
/// `look_ahead_max`, and lets the same critically damped spring (time constant
/// `look_ahead_smoothing`) settle [`CameraFollow::position`] on `focus + look_ahead` — so a
/// stationary focus eventually gives `position == focus` and a fast-moving focus leads by at most
/// `look_ahead_max` world units. Flagged as a V-20 candidate (clarification) for the PO: a
/// different lead time or a speed-based (rather than capped-linear) falloff would also satisfy the
/// contract's wording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraFollow {
    /// Current smoothed ground point, suitable for [`Camera25D::target`].
    position: [f32; 2],
    /// Spring velocity driving `position` toward the look-ahead target.
    velocity: [f32; 2],
    /// Focus ground point of the previous [`CameraFollow::update`] call, used to estimate the
    /// focus's own velocity for look-ahead.
    previous_focus: [f32; 2],
}

impl CameraFollow {
    /// Starts a follow spring already settled on `focus` (no initial velocity or look-ahead), so
    /// the very first frame does not snap in from an arbitrary origin.
    #[must_use]
    pub fn new(focus: [f32; 2]) -> Self {
        Self {
            position: focus,
            velocity: [0.0, 0.0],
            previous_focus: focus,
        }
    }

    /// Current smoothed ground point (the last value [`CameraFollow::update`] returned, or the
    /// construction focus if it has never been called).
    #[must_use]
    pub fn position(&self) -> [f32; 2] {
        self.position
    }

    /// Advances the spring by `dt` real seconds toward `focus`, using `camera`'s
    /// [`Camera25D::look_ahead_max`] and [`Camera25D::look_ahead_smoothing`], and returns the new
    /// [`CameraFollow::position`].
    ///
    /// Call this once per rendered frame (never inside the simulation) with the same focus point
    /// the loop already interpolated with `alpha` (contract §9.3 step 5) and the real elapsed time
    /// since the previous frame. A non-finite `focus` is ignored for this call (the position does
    /// not move, as if the frame had `dt == 0`) rather than propagating NaN into the camera.
    pub fn update(&mut self, camera: &Camera25D, focus: [f32; 2], dt: f32) -> [f32; 2] {
        if !focus[0].is_finite() || !focus[1].is_finite() {
            return self.position;
        }
        let safe_dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        let focus_velocity = if safe_dt > 0.0 {
            [
                (focus[0] - self.previous_focus[0]) / safe_dt,
                (focus[1] - self.previous_focus[1]) / safe_dt,
            ]
        } else {
            [0.0, 0.0]
        };
        self.previous_focus = focus;

        let look_ahead_max = if is_positive_and_finite(camera.look_ahead_max) {
            camera.look_ahead_max
        } else {
            0.0
        };
        // One second of lead time, see the type's doc comment.
        let look_ahead = clamp_length(focus_velocity, look_ahead_max);
        let desired = [focus[0] + look_ahead[0], focus[1] + look_ahead[1]];

        let (x, vx) = critically_damped_step(
            self.position[0],
            self.velocity[0],
            desired[0],
            camera.look_ahead_smoothing,
            safe_dt,
        );
        let (y, vy) = critically_damped_step(
            self.position[1],
            self.velocity[1],
            desired[1],
            camera.look_ahead_smoothing,
            safe_dt,
        );
        self.position = [x, y];
        self.velocity = [vx, vy];
        self.position
    }
}

/// glTF-compatible alpha coverage mode of a [`PbrMaterial`] (glTF 2.0 `alphaMode`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlphaMode {
    /// Fully opaque; the alpha channel of [`PbrMaterial::base_color_factor`] is ignored.
    Opaque,
    /// Alpha-tested: pixels with alpha below `cutoff` are discarded, the rest are opaque.
    Mask {
        /// Discard threshold, valid range `0.0..=1.0`.
        cutoff: f32,
    },
    /// Alpha-blended (glTF `BLEND`).
    Blend,
}

/// Physically based material, glTF metallic-roughness compatible (glTF 2.0
/// `pbrMetallicRoughness`), consequence of the game's ADR-0014 (realistic PBR look instead of
/// toon/cel-shading). Referenced by [`MeshInstance::material`] as an index into
/// [`crate::StageFrame::materials`].
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PbrMaterial {
    /// Linear RGBA base colour factor, multiplied with [`PbrMaterial::base_color_texture`] if
    /// present. Each component's valid range is `0.0..=1.0`.
    pub base_color_factor: [f32; 4],
    /// Metalness, `0.0` (dielectric) to `1.0` (metal); glTF default is `1.0`.
    pub metallic_factor: f32,
    /// Perceptual roughness, `0.0` (mirror) to `1.0` (fully rough); glTF default is `1.0`.
    pub roughness_factor: f32,
    /// Linear RGB emissive colour, added regardless of lighting. Each component's valid range is
    /// `0.0..=1.0` (glTF core; no HDR emissive-strength extension in P1).
    pub emissive_factor: [f32; 3],
    /// Alpha coverage mode (glTF `alphaMode`).
    pub alpha_mode: AlphaMode,
    /// Base colour (albedo) texture, glTF `baseColorTexture`. Its pixels are sRGB-encoded and are
    /// linearised on sampling (an sRGB texture format), unlike the two data textures below.
    pub base_color_texture: Option<TextureHandle>,
    /// Tangent-space normal map, glTF `normalTexture` (OpenGL convention, +Y up). Linear data, never
    /// sRGB-decoded.
    pub normal_texture: Option<TextureHandle>,
    /// Combined occlusion/roughness/metallic texture in glTF channel order (R = occlusion,
    /// G = roughness, B = metallic), glTF `occlusionTexture` + `metallicRoughnessTexture` packed
    /// into one image as the Blender-Skript-Pipeline (PRD-0016) produces it. Linear data, never
    /// sRGB-decoded.
    pub occlusion_roughness_metallic_texture: Option<TextureHandle>,
}

impl Default for PbrMaterial {
    fn default() -> Self {
        Self {
            base_color_factor: [1.0, 1.0, 1.0, 1.0],
            metallic_factor: 1.0,
            roughness_factor: 1.0,
            emissive_factor: [0.0, 0.0, 0.0],
            alpha_mode: AlphaMode::Opaque,
            base_color_texture: None,
            normal_texture: None,
            occlusion_roughness_metallic_texture: None,
        }
    }
}

impl PbrMaterial {
    /// Whether every field is within its documented range and finite (contract §2 rule 9 style
    /// validation; never panics). Invalid materials are rejected and counted, never drawn
    /// (`StageStats::materials_rejected_invalid`).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let unit_range = |c: &f32| c.is_finite() && (0.0..=1.0).contains(c);
        self.base_color_factor.iter().all(unit_range)
            && unit_range(&self.metallic_factor)
            && unit_range(&self.roughness_factor)
            && self.emissive_factor.iter().all(unit_range)
            && match self.alpha_mode {
                AlphaMode::Mask { cutoff } => unit_range(&cutoff),
                AlphaMode::Opaque | AlphaMode::Blend => true,
            }
    }
}

/// One mesh instance drawn on [`MeshInstance::layer`]. In P1 the bullet channel
/// ([`crate::BulletInstance`]) is untouched and separate; meshes carry player, enemy, level and
/// prop geometry on [`RenderLayer::World`] (contract §6 layer order).
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshInstance {
    /// Which mesh to draw. The mesh registry is WP2.3's job; P1 cannot check whether a handle is
    /// registered.
    pub mesh: MeshHandle,
    /// Index into [`crate::StageFrame::materials`]; validated against that frame's table length
    /// (`StageStats::meshes_rejected_invalid` counts an out-of-range index).
    pub material: MaterialHandle,
    /// Column-major model-to-world transform (translation, rotation and scale combined), same
    /// matrix convention as [`crate::Camera2D::view_projection`].
    pub transform: [[f32; 4]; 4],
    /// Layer this instance is drawn on. P1 accepts only [`RenderLayer::World`] (contract §6: the
    /// mesh pass shares layers 1-3 with world sprites); any other value is rejected and counted
    /// (`StageStats::meshes_rejected_layer`), analogous to the bullet pass's palette-space check.
    pub layer: RenderLayer,
    /// Bone matrix palette for a skinned mesh (P1 skinning addendum, contract §6 changelog
    /// 2026-09-16), `None` for a rigid mesh (the only case before this field was added). A
    /// `Some(binding)` whose range does not fit entirely inside
    /// [`crate::StageFrame::joint_matrices`], or whose `joint_count` is `0` or exceeds
    /// [`MAX_SKIN_JOINTS`], is rejected like any other structurally invalid instance
    /// (`StageStats::meshes_rejected_invalid`) — the mesh pass costs nothing extra for an instance
    /// with `skin == None`, per the second vertex path added alongside this field.
    pub skin: Option<SkinBinding>,
}

impl Default for MeshInstance {
    fn default() -> Self {
        Self {
            mesh: MeshHandle::default(),
            material: MaterialHandle::default(),
            transform: IDENTITY_TRANSFORM,
            layer: RenderLayer::World,
            skin: None,
        }
    }
}

/// One point light (contract §6, PRD-0003 FR-02). In P1 nothing yet enforces the `>= 256` visible
/// count from PRD-0003 FR-02 or a count budget — that is WP3.4's clustered forward+ pass and its
/// `Low 32` / `High 256` budget, deliberately not part of this contract (plan 0002 places the
/// budget in WP3.4). This type only carries per-light data and its own value validation.
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    /// World position (X right, Y away from the viewer, Z up).
    pub position: [f32; 3],
    /// Linear RGB colour. Unbounded above (intensity carries the exposure scale); components must
    /// be finite and non-negative.
    pub color: [f32; 3],
    /// Brightness scale; must be finite and non-negative.
    pub intensity: f32,
    /// Cutoff distance in world units; must be finite and strictly positive.
    pub range: f32,
    /// Bullet-light cap hook (PRD-0003 rule 5 / FR-15): `true` for a light that originates from
    /// the bullet channel (a bullet or bullet cloud), rather than the environment or an actor. The
    /// PBR pass (WP3.4/WP3.5) looks this flag up to apply
    /// [`crate::StageFrame::bullet_light_cap`] to this light's contribution to the environment;
    /// the light's contribution to the bullet's own glow is unaffected. The shading itself is not
    /// part of this contract.
    pub is_bullet_light: bool,
    /// Reserved interface for point-light shadow casters (plan 0002 WP2.6, OF-3.2): whether this
    /// light should cast a shadow once [`ShadowMode::KeyLightPlusPoints`] actually renders
    /// point-light shadow maps. **Not yet consumed**: WP2.6 ships the key-light shadow map and
    /// blob shadows only (see this crate's WP2.6 ADR for what is deferred and why); the mesh pass
    /// validates and would, in a future work package, clamp the number of lights with this flag
    /// set to [`ShadowConfig::max_point_shadow_casters`], but it does not render point-light
    /// shadows yet. Defaults to `false`.
    pub casts_shadow: bool,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            range: 1.0,
            is_bullet_light: false,
            casts_shadow: false,
        }
    }
}

impl PointLight {
    /// Whether every field is finite and within its documented range (never panics). Invalid
    /// lights are rejected and counted, never drawn
    /// (`StageStats::point_lights_rejected_invalid`).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.position.iter().all(|c| c.is_finite())
            && self.color.iter().all(|c| c.is_finite() && *c >= 0.0)
            && self.intensity.is_finite()
            && self.intensity >= 0.0
            && self.range.is_finite()
            && self.range > 0.0
    }
}

/// Directional "key light" (contract §6, PRD-0003 FR-01). At most one per frame
/// ([`crate::StageFrame::key_light`] is `Option`); shadow casting is WP2.6's job.
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLight {
    /// Direction the light travels (from the light towards the scene), in the same axes as
    /// [`PointLight::position`]. Need not be pre-normalised; consumers normalise it. Must be
    /// finite and non-zero.
    pub direction: [f32; 3],
    /// Linear RGB colour; components must be finite and non-negative.
    pub color: [f32; 3],
    /// Brightness scale; must be finite and non-negative.
    pub intensity: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            direction: [0.0, 0.0, -1.0],
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        }
    }
}

impl DirectionalLight {
    /// Whether every field is finite, within range and `direction` is non-zero (never panics).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let direction_finite = self.direction.iter().all(|c| c.is_finite());
        let direction_length_squared = self.direction.iter().map(|c| c * c).sum::<f32>();
        direction_finite
            && direction_length_squared > 0.0
            && self.color.iter().all(|c| c.is_finite() && *c >= 0.0)
            && self.intensity.is_finite()
            && self.intensity >= 0.0
    }
}

/// Ambient/environment term (contract §6, PRD-0003 FR-01 "einfacher Umgebungsterm"). Exactly one
/// per frame ([`crate::StageFrame::ambient`] is not optional; use zero intensity for "no ambient").
///
/// Not `#[non_exhaustive]`: contract §2 rule 13 requires that only for structs with public fields
/// and error enums, not for a plain data enum like this one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AmbientLight {
    /// Single flat ambient colour applied from every direction.
    Flat {
        /// Linear RGB colour; components must be finite and non-negative.
        color: [f32; 3],
        /// Brightness scale; must be finite and non-negative.
        intensity: f32,
    },
    /// Two-colour hemisphere term (sky above, ground below), cheap analytic ambient with more
    /// depth than a flat term.
    Hemisphere {
        /// Linear RGB colour for surfaces facing up.
        sky_color: [f32; 3],
        /// Linear RGB colour for surfaces facing down.
        ground_color: [f32; 3],
        /// Brightness scale; must be finite and non-negative.
        intensity: f32,
    },
}

impl Default for AmbientLight {
    fn default() -> Self {
        AmbientLight::Flat {
            color: [1.0, 1.0, 1.0],
            intensity: 0.1,
        }
    }
}

impl AmbientLight {
    /// Whether every field is finite and within its documented range (never panics).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let channel = |c: &f32| c.is_finite() && *c >= 0.0;
        match self {
            AmbientLight::Flat { color, intensity } => {
                color.iter().all(channel) && channel(intensity)
            }
            AmbientLight::Hemisphere {
                sky_color,
                ground_color,
                intensity,
            } => {
                sky_color.iter().all(channel)
                    && ground_color.iter().all(channel)
                    && channel(intensity)
            }
        }
    }
}

/// Bullet-light cap hook (PRD-0003 rule 5 / FR-15, ADR-0014): the render vertrag's structural
/// enforcement point for "light from bullets or bullet clouds is limited in its contribution to
/// the ground and environment". One per frame ([`crate::StageFrame::bullet_light_cap`]).
///
/// This type only carries the parameter through the contract; *how* it is mixed into the shading
/// equation is decided by the stylebook (WP2.7, `docs/art/stilbibel.md`) and implemented (plan
/// 0002 WP3.4) in `mesh_pass::build_light_list`: a bullet light's uploaded intensity is multiplied
/// by [`BulletLightCap::clamped_floor_contribution`] before it ever reaches the shading equation
/// mesh.wgsl runs for the environment; the light's contribution to the bullet's own glow (WP3.5's
/// bullet pass, not built yet) is unaffected, since that pass is not this one.
///
/// The point-light *count* budget (`Low 32` / `High 256`, PRD-0003 FR-11) is a separate concept
/// (how many lights the frame may contain in total) and belongs to WP3.4 per plan 0002, not here.
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BulletLightCap {
    /// Upper bound on how much a [`PointLight`] with `is_bullet_light == true` may contribute to
    /// the environment's (non-bullet) shading, as a fraction in `0.0..=1.0`. `0.0` mutes bullet
    /// lights on the environment entirely; `1.0` applies no extra cap (same as any other light).
    pub floor_contribution: f32,
}

impl Default for BulletLightCap {
    fn default() -> Self {
        Self {
            // Stilbibel v0's proposed value ("Bullet-Licht-Obergrenze", `docs/art/stilbibel.md`):
            // 25% of full cluster-light intensity, the strongest single lever the game's look-dev
            // comparison found for keeping bullet contrast above the WCAG-style 4.5:1 target
            // (13-22% of bullets reached it uncapped, 33% at this cap in the Blender comparison).
            // Explicitly a *proposal*, not a freeze ("Vorschlag, keine Festlegung — Bestätigung am
            // Look-Review, P-11"): a game or a future default may still override it per frame via
            // `StageFrame::bullet_light_cap`; only *this* inert fallback changed, from the earlier
            // "no cap" placeholder chosen before the stylebook had a number.
            floor_contribution: 0.25,
        }
    }
}

impl BulletLightCap {
    /// Whether [`BulletLightCap::floor_contribution`] is finite and within `0.0..=1.0` (never
    /// panics).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.floor_contribution.is_finite() && (0.0..=1.0).contains(&self.floor_contribution)
    }

    /// [`BulletLightCap::floor_contribution`] clamped into the valid shading range `0.0..=1.0`.
    /// Falls back to `0.0` (mute bullet lights on the environment, the safe side of PRD-0003 rule
    /// 5) when the configured value is not finite, rather than propagating NaN into the shading
    /// pass.
    #[must_use]
    pub fn clamped_floor_contribution(&self) -> f32 {
        if self.floor_contribution.is_finite() {
            self.floor_contribution.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// Linear-RGB colour every bullet-cloud light uses (`docs/art/stilbibel.md`, "Bullet-Licht-
/// Obergrenze"): a desaturated warm glow, `#B07850` sRGB, deliberately **not** the bullet's own
/// protected palette colour (magenta/lime, contract §6 `palette_space`). Tinting the environment
/// toward the bullet colour space would itself lower bullet-vs-background contrast — the same
/// failure mode [`BulletLightCap`] exists to limit — even if `BulletInstance::palette_space`
/// itself stays formally untouched. `bullet_light_color_matches_stilbibel_hex_value` (below)
/// cross-checks these literals against the standard sRGB transfer function so the hex source stays
/// verifiable, not just asserted in a comment.
const BULLET_LIGHT_COLOR: [f32; 3] = [0.434_154, 0.187_907, 0.080_202];

/// Bullet-cloud light intensity at full glow (`BulletInstance::glow == 255`); scales linearly with
/// `glow` below. Provisional, like [`BULLET_LIGHT_RANGE_PER_RADIUS`] and the stylebook's own
/// floor-contribution cap ([`BulletLightCap::default`]'s doc comment) — no look-dev measurement
/// yet ties a specific radiance to a given glow byte; flagged for confirmation at the same
/// look-review (P-11).
const BULLET_LIGHT_BASE_INTENSITY: f32 = 2.0;

/// Bullet-cloud light range as a multiple of `BulletInstance::radius`: how far past the visible
/// glow the light reaches. Provisional, see [`BULLET_LIGHT_BASE_INTENSITY`].
const BULLET_LIGHT_RANGE_PER_RADIUS: f32 = 6.0;

/// The **only** permitted way to create a bullet-cloud [`PointLight`] (PO decision, 2026-09-16,
/// resolving the "how binding is `is_bullet_light` for bullet-cloud lights" question contract §6
/// left open before WP3.4/WP3.5): bullet-cloud lights arise exclusively in the engine, from the
/// bullet channel, and always through this function. A game never constructs one by hand, and
/// nothing else in this crate ever sets [`PointLight::is_bullet_light`] to `true` — a property
/// `point_light_from_bullet_always_sets_is_bullet_light` (this module's tests) fixes for arbitrary
/// inputs. WP3.5's bullet pass (plan 0002, not built yet) is expected to route every bullet that
/// contributes a light to the ground through this function, so the flag the PO decision requires
/// is structural, not a convention a caller must remember to apply.
///
/// The bullet's ground position becomes the light's `position`, lifted by its own `radius` (a
/// small, deliberately simple height offset — this crate has no other notion of a bullet's visual
/// centre height); `range` scales with `radius` and `intensity` with `glow`
/// (`BulletInstance::glow / 255`); `color` is always this module's fixed bullet-light glow colour,
/// never derived from the bullet's own palette. A structurally degenerate bullet (for example
/// `radius <= 0`) produces a `PointLight` that fails [`PointLight::is_valid`] downstream and is
/// rejected and counted like any other invalid light (contract §6) — this function itself never
/// panics or validates.
///
/// Every numeric constant here besides the flag itself is provisional (see this module's private
/// `BULLET_LIGHT_BASE_INTENSITY` doc comment); only *that* [`PointLight::is_bullet_light`] is
/// always `true` is the firm part of today's PO decision.
#[must_use]
pub fn point_light_from_bullet(bullet: &crate::stage::BulletInstance) -> PointLight {
    PointLight {
        position: [bullet.position[0], bullet.position[1], bullet.radius],
        color: BULLET_LIGHT_COLOR,
        intensity: BULLET_LIGHT_BASE_INTENSITY * (f32::from(bullet.glow) / 255.0),
        range: bullet.radius * BULLET_LIGHT_RANGE_PER_RADIUS,
        is_bullet_light: true,
        casts_shadow: false,
    }
}

/// Shadow technique selected for a frame (plan 0002 WP2.6, OF-3.2). Switchable at runtime so a
/// graphics preset can pick the technique it can afford; not `#[non_exhaustive]` (contract §2 rule
/// 13 applies only to structs with public fields and error enums, like [`AlphaMode`] above).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShadowMode {
    /// No shadows at all.
    #[default]
    None,
    /// Cheap blob shadows only ([`crate::StageFrame::blob_shadows`]), the stylebook's recommended
    /// preset "Low" technique (`docs/art/stilbibel.md`).
    Blob,
    /// A depth-only shadow map for [`crate::StageFrame::key_light`] (`key_light_view_projection`,
    /// `shadow_pass`), PCF-filtered in `mesh.wgsl`. No blob shadows on top.
    KeyLight,
    /// [`ShadowMode::KeyLight`] plus a limited number of point-light shadow casters
    /// ([`PointLight::casts_shadow`], [`ShadowConfig::max_point_shadow_casters`]).
    ///
    /// **Deferred (plan 0002 WP2.6, see the WP2.6 ADR):** point-light shadow casters are not
    /// rendered yet — this variant currently shades identically to [`ShadowMode::KeyLight`]. The
    /// data-side interface ([`PointLight::casts_shadow`], [`ShadowConfig::max_point_shadow_casters`])
    /// is in place so a later work package can add the rendering without another contract change.
    KeyLightPlusPoints,
}

impl ShadowMode {
    /// Whether this mode wants the key-light shadow map built and sampled.
    #[must_use]
    pub fn wants_key_light_shadow_map(self) -> bool {
        matches!(self, ShadowMode::KeyLight | ShadowMode::KeyLightPlusPoints)
    }

    /// Whether this mode wants [`crate::StageFrame::blob_shadows`] drawn.
    #[must_use]
    pub fn wants_blob_shadows(self) -> bool {
        matches!(self, ShadowMode::Blob)
    }
}

/// Parameters of the WP2.6 shadow techniques for one frame ([`crate::StageFrame::shadow_config`]).
/// Persists across [`crate::StageFrame::clear`], like [`crate::StageFrame::camera_25d`]: it
/// describes the current scene/preset, not a per-frame instance list.
///
/// Growable like every new P1 render type (contract §2 rule 13): `#[non_exhaustive]` with
/// [`Default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowConfig {
    /// Which technique(s) to render this frame.
    pub mode: ShadowMode,
    /// Side length, in texels, of the (square) key-light shadow map. Rebuilt by the renderer
    /// whenever this changes; keep it stable across frames to avoid needless GPU reallocation.
    /// Must be non-zero; validated by [`ShadowConfig::is_valid`].
    pub map_size: u32,
    /// Constant depth bias applied when rendering the key-light shadow map (`wgpu`'s
    /// `DepthBiasState::constant`, in units of the smallest value representable by the shadow
    /// map's depth format), fighting shadow acne on surfaces roughly facing the light.
    pub depth_bias_constant: i32,
    /// Slope-scaled depth bias applied on top of [`ShadowConfig::depth_bias_constant`] (`wgpu`'s
    /// `DepthBiasState::slope_scale`), growing the bias for surfaces at a grazing angle to the
    /// light, where acne is worst. Must be finite.
    pub depth_bias_slope_scale: f32,
    /// PCF kernel radius in shadow-map texels: a radius of `1` samples a `3x3` neighbourhood, `2` a
    /// `5x5` one, and so on. `0` disables filtering (a single hard-edged tap). Clamped to a small
    /// maximum by [`ShadowConfig::is_valid`] to keep the per-fragment tap count bounded.
    pub pcf_radius: u32,
    /// Half-width/-depth, in world units, of the orthographic frustum fitted around the camera's
    /// ground target ([`Camera25D::target`]) for the key-light shadow map. Must be finite and
    /// strictly positive.
    pub frustum_radius: f32,
    /// Vertical extent, in world units above the ground plane (`Z = 0`), the fitted frustum must
    /// cover. Must be finite and strictly positive.
    pub frustum_height: f32,
    /// Upper bound on how many [`PointLight`]s with [`PointLight::casts_shadow`] set may cast a
    /// shadow at once, once [`ShadowMode::KeyLightPlusPoints`] actually renders them (plan 0002
    /// WP2.6, deferred — see [`ShadowMode::KeyLightPlusPoints`]'s doc comment). Carried here so the
    /// interface is complete even though nothing consumes it yet.
    pub max_point_shadow_casters: u32,
}

impl Default for ShadowConfig {
    fn default() -> Self {
        Self {
            mode: ShadowMode::None,
            map_size: 1024,
            // Loosely modelled on common engine defaults for a `Depth32Float` shadow map; the
            // WP2.6 ADR records the measured acne/peter-panning trade-off on the software adapter.
            depth_bias_constant: 3,
            depth_bias_slope_scale: 2.0,
            pcf_radius: 1,
            frustum_radius: 25.0,
            frustum_height: 20.0,
            max_point_shadow_casters: 0,
        }
    }
}

impl ShadowConfig {
    /// Largest accepted [`ShadowConfig::map_size`] (`8192`) and [`ShadowConfig::pcf_radius`] (`4`,
    /// a `9x9` kernel): both bound the per-frame GPU/CPU cost an untrusted or misconfigured value
    /// could otherwise force, without editing this contract. An out-of-range or non-finite
    /// [`ShadowConfig`] falls back to shadows off (`StageStats::shadow_config_invalid`, see
    /// `mesh_pass`), never to clamping silently — the same "invalid data is rejected and counted,
    /// not guessed at" rule contract §6 already applies to [`PbrMaterial`] and the light types.
    const MAX_MAP_SIZE: u32 = 8192;
    /// See [`ShadowConfig::MAX_MAP_SIZE`].
    const MAX_PCF_RADIUS: u32 = 4;

    /// Whether every field is finite and within its documented range (never panics).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.map_size > 0
            && self.map_size <= Self::MAX_MAP_SIZE
            && self.depth_bias_slope_scale.is_finite()
            && self.pcf_radius <= Self::MAX_PCF_RADIUS
            && is_positive_and_finite(self.frustum_radius)
            && is_positive_and_finite(self.frustum_height)
    }
}

mod blob_shadow_instance {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `BulletInstance` elsewhere in
    // this crate.
    #![allow(unsafe_code)]

    /// One blob shadow (plan 0002 WP2.6, OF-3.2): a soft, dark decal on the ground plane
    /// (`Z = 0`) under an actor, the stylebook's cheap alternative to the key-light shadow map
    /// (`docs/art/stilbibel.md`, preset "Low"). Drawn by `mesh_pass` after the opaque meshes, depth
    /// tested against them (so a wall or prop between the disc and the camera still occludes it)
    /// but never depth-written, alpha-blended so it darkens whatever ground colour is already
    /// there. Layout is `#[repr(C)]`, 20 bytes, no padding, uploaded to the GPU verbatim.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct BlobShadowInstance {
        /// Ground-plane centre (`Z = 0`), same convention as [`crate::BulletInstance::position`].
        pub position: [f32; 2],
        /// Outer radius of the disc, world units. Must be finite and strictly positive.
        pub radius: f32,
        /// Fraction of `radius`, counted inward from the rim, over which the disc fades from fully
        /// transparent to [`BlobShadowInstance::strength`]. `0.0` is a hard edge, `1.0` fades from
        /// the very centre. Must be finite and in `0.0..=1.0`.
        pub softness: f32,
        /// Maximum darkening at the disc's centre: `0.0` is invisible, `1.0` is fully opaque black.
        /// Must be finite and in `0.0..=1.0`.
        pub strength: f32,
    }
}
pub use blob_shadow_instance::BlobShadowInstance;

impl BlobShadowInstance {
    /// Whether every field is finite and within its documented range (never panics). Invalid
    /// instances are rejected and counted, never drawn
    /// (`StageStats::blob_shadows_rejected_invalid`).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let unit_range = |c: f32| c.is_finite() && (0.0..=1.0).contains(&c);
        self.position.iter().all(|c| c.is_finite())
            && is_positive_and_finite(self.radius)
            && unit_range(self.softness)
            && unit_range(self.strength)
    }
}

/// Builds a world-to-light-space view matrix looking along `direction` (a directional light's
/// travel direction, same convention as [`DirectionalLight::direction`]) with its eye placed at
/// `eye`. Picks an up hint automatically (world `+Z`, falling back to world `+Y` when `direction`
/// is too close to vertical for `+Z` to give a stable basis), so callers never have to reason about
/// the degenerate case themselves.
///
/// Not part of the crate's public API, for the same reason as [`view_projection`]: `wgpu`'s
/// clip-space convention stays out of this module's data contract (engine ADR-0002).
fn light_view_matrix(direction: [f32; 3], eye: [f32; 3]) -> [[f32; 4]; 4] {
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    let forward = if is_positive_and_finite(length) {
        [
            direction[0] / length,
            direction[1] / length,
            direction[2] / length,
        ]
    } else {
        [0.0, 0.0, -1.0]
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let normalize = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if is_positive_and_finite(l) {
            [v[0] / l, v[1] / l, v[2] / l]
        } else {
            [1.0, 0.0, 0.0]
        }
    };
    // World `+Z` is the natural up hint for a mostly-downward light (the common case: a "moon" or
    // "sun" key light); fall back to world `+Y` once `forward` is close enough to vertical that
    // `+Z` would produce a near-zero cross product.
    let up_hint = if forward[0].abs() < 1e-3 && forward[1].abs() < 1e-3 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let right = normalize(cross(up_hint, forward));
    let up = cross(forward, right);
    let (rx, ry, rz) = (right[0], right[1], right[2]);
    let (ux, uy, uz) = (up[0], up[1], up[2]);
    let (fx, fy, fz) = (forward[0], forward[1], forward[2]);
    [
        [rx, ux, fx, 0.0],
        [ry, uy, fy, 0.0],
        [rz, uz, fz, 0.0],
        [
            -(rx * eye[0] + ry * eye[1] + rz * eye[2]),
            -(ux * eye[0] + uy * eye[1] + uz * eye[2]),
            -(fx * eye[0] + fy * eye[1] + fz * eye[2]),
            1.0,
        ],
    ]
}

/// Builds the column-major light-space view-projection matrix for [`crate::StageFrame::key_light`]
/// (plan 0002 WP2.6, OF-3.2): an orthographic frustum fitted tightly around a box centred on
/// `target` (the camera's ground point, [`Camera25D::target`]), `config.frustum_radius` wide/deep
/// on the ground plane and covering `Z` from `0` to `config.frustum_height`.
///
/// "Fitted" means the box's eight corners are projected into light space and the orthographic
/// bounds are taken from their exact min/max extents, rather than a fixed guessed size — the
/// frustum is only ever as large as the scene volume it must cover, which keeps the shadow map's
/// texel density (and therefore its effective softness/aliasing) stable as the config's frustum
/// size changes.
///
/// Returns a matrix with only finite entries whenever `direction` is finite and non-zero and
/// `config` is valid ([`ShadowConfig::is_valid`]); callers treat a non-finite result the same as
/// "no shadow map" (skip sampling), matching [`view_projection`]'s own contract.
pub(crate) fn key_light_view_projection(
    direction: [f32; 3],
    target: [f32; 2],
    config: &ShadowConfig,
) -> [[f32; 4]; 4] {
    let half_height = config.frustum_height * 0.5;
    let center = [target[0], target[1], half_height];
    let half_extent = [
        config.frustum_radius,
        config.frustum_radius,
        half_height + config.frustum_radius, // generous margin so tall props are never clipped
    ];
    let mut corners = [[0.0f32; 3]; 8];
    let mut i = 0;
    for &sx in &[-1.0f32, 1.0] {
        for &sy in &[-1.0f32, 1.0] {
            for &sz in &[-1.0f32, 1.0] {
                corners[i] = [
                    center[0] + sx * half_extent[0],
                    center[1] + sy * half_extent[1],
                    center[2] + sz * half_extent[2],
                ];
                i += 1;
            }
        }
    }
    // A distant eye along `-direction` from the box centre (far enough to sit outside the box for
    // any `frustum_radius`/`frustum_height` this contract accepts), so every corner ends up with a
    // positive light-space "forward" coordinate below.
    let far_guess = (half_extent[0] * half_extent[0]
        + half_extent[1] * half_extent[1]
        + half_extent[2] * half_extent[2])
        .sqrt()
        * 2.0
        + 1.0;
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    let unit_direction = if is_positive_and_finite(length) {
        [
            direction[0] / length,
            direction[1] / length,
            direction[2] / length,
        ]
    } else {
        [0.0, 0.0, -1.0]
    };
    let eye = [
        center[0] - unit_direction[0] * far_guess,
        center[1] - unit_direction[1] * far_guess,
        center[2] - unit_direction[2] * far_guess,
    ];
    let view = light_view_matrix(direction, eye);

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for corner in corners {
        let world = [corner[0], corner[1], corner[2], 1.0];
        for axis in 0..3 {
            let value: f32 = (0..4).map(|k| view[k][axis] * world[k]).sum();
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }

    // Right-handed orthographic projection onto `wgpu`'s zero-to-one clip-space depth, from the
    // light-space bounding box computed above (`x`/`y` = left/right/bottom/top, `z` = near/far
    // along the light's forward axis).
    let (left, right_bound) = (min[0], max[0]);
    let (bottom, top) = (min[1], max[1]);
    let (near, far) = (min[2], max[2]);
    let dx = right_bound - left;
    let dy = top - bottom;
    let dz = far - near;
    if !(dx.is_finite() && dy.is_finite() && dz.is_finite()) || dx <= 0.0 || dy <= 0.0 || dz <= 0.0
    {
        return IDENTITY_TRANSFORM;
    }
    let proj: [[f32; 4]; 4] = [
        [2.0 / dx, 0.0, 0.0, 0.0],
        [0.0, 2.0 / dy, 0.0, 0.0],
        [0.0, 0.0, 1.0 / dz, 0.0],
        [
            -(right_bound + left) / dx,
            -(top + bottom) / dy,
            -near / dz,
            1.0,
        ],
    ];
    multiply(proj, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Camera25D: projection and ray round trip -------------------------------------------

    #[test]
    fn screen_to_ground_center_pixel_is_target() {
        let camera = Camera25D {
            target: [5.0, 7.0],
            ..Camera25D::default()
        };
        let viewport = [800.0, 600.0];
        let center = [viewport[0] * 0.5, viewport[1] * 0.5];
        let ground = camera
            .screen_to_ground(center, viewport)
            .expect("the centre pixel always hits the ground under the target");
        assert!((ground[0] - 5.0).abs() < 1e-4);
        assert!((ground[1] - 7.0).abs() < 1e-4);
    }

    #[test]
    fn ground_to_screen_and_back_round_trips() {
        let camera = Camera25D {
            target: [3.0, -2.0],
            tilt_degrees: 65.0,
            fov_y_degrees: 50.0,
            distance: 15.0,
            ..Camera25D::default()
        };
        let viewport = [1920.0, 1080.0];
        for ground in [[0.0, 0.0], [3.0, -2.0], [10.0, 5.0], [-8.0, 20.0]] {
            let pixel = camera
                .ground_to_screen(ground, viewport)
                .unwrap_or_else(|| panic!("{ground:?} should be in front of the camera"));
            let back = camera
                .screen_to_ground(pixel, viewport)
                .unwrap_or_else(|| panic!("round trip of {ground:?} via {pixel:?} failed"));
            assert!((back[0] - ground[0]).abs() < 1e-2, "{back:?} vs {ground:?}");
            assert!((back[1] - ground[1]).abs() < 1e-2, "{back:?} vs {ground:?}");
        }
    }

    #[test]
    fn steeper_tilt_moves_the_eye_higher_and_closer() {
        // Sanity check on the camera basis: a steeper (more top-down) tilt raises the eye and
        // pulls it closer to being directly above the target, at the same `distance`.
        let shallow = camera_basis(&Camera25D {
            tilt_degrees: 60.0,
            ..Camera25D::default()
        });
        let steep = camera_basis(&Camera25D {
            tilt_degrees: 75.0,
            ..Camera25D::default()
        });
        assert!(steep.eye[2] > shallow.eye[2], "steeper tilt is higher");
        assert!(
            steep.eye[1] > shallow.eye[1],
            "steeper tilt sits closer to the target's Y"
        );
    }

    // --- Camera25D: horizon and parallel-ray edge cases, never NaN --------------------------

    #[test]
    fn screen_to_ground_never_produces_nan_across_extreme_pixels() {
        let camera = Camera25D::default();
        let viewport = [800.0, 600.0];
        for y in [
            -1.0e6, -1.0e3, -600.0, -1.0, 0.0, 300.0, 600.0, 1200.0, 1.0e3, 1.0e6,
        ] {
            let result = camera.screen_to_ground([400.0, y], viewport);
            if let Some(ground) = result {
                assert!(
                    ground[0].is_finite() && ground[1].is_finite(),
                    "pixel y={y} produced a non-finite ground point: {ground:?}"
                );
            }
        }
    }

    #[test]
    fn screen_to_ground_rejects_non_finite_and_degenerate_inputs_without_nan() {
        let camera = Camera25D::default();
        assert_eq!(
            camera.screen_to_ground([f32::NAN, 0.0], [800.0, 600.0]),
            None
        );
        assert_eq!(
            camera.screen_to_ground([0.0, f32::INFINITY], [800.0, 600.0]),
            None
        );
        assert_eq!(camera.screen_to_ground([400.0, 300.0], [0.0, 600.0]), None);
        assert_eq!(camera.screen_to_ground([400.0, 300.0], [800.0, -1.0]), None);

        let nan_camera = Camera25D {
            tilt_degrees: f32::NAN,
            ..Camera25D::default()
        };
        assert_eq!(
            nan_camera.screen_to_ground([400.0, 300.0], [800.0, 600.0]),
            None
        );
    }

    #[test]
    fn screen_to_ground_ray_exactly_parallel_to_the_ground_plane_returns_none() {
        // tilt = 0 makes `forward` horizontal ((0, 1, 0)) and `up` vertical ((0, 0, 1)); at the
        // vertical viewport centre (ndc_y = 0) the ray direction has no Z component at all,
        // regardless of tilt or field of view, so this is an exact, non-approximate parallel ray.
        let camera = Camera25D {
            tilt_degrees: 0.0,
            ..Camera25D::default()
        };
        let viewport = [800.0, 600.0];
        let vertical_center = [123.0, viewport[1] * 0.5];
        assert_eq!(camera.screen_to_ground(vertical_center, viewport), None);
    }

    #[test]
    fn screen_to_ground_above_the_horizon_returns_none_not_a_behind_camera_point() {
        // A very wide field of view lets some pixels look above the horizon even at a steep tilt.
        let camera = Camera25D {
            tilt_degrees: 60.0,
            fov_y_degrees: 170.0,
            ..Camera25D::default()
        };
        let viewport = [800.0, 600.0];
        // Top row of pixels: steeply upward relative to the tilted forward direction.
        assert_eq!(camera.screen_to_ground([400.0, 0.0], viewport), None);
    }

    #[test]
    fn ground_to_screen_behind_the_camera_returns_none_not_nan() {
        let camera = Camera25D::default();
        let viewport = [800.0, 600.0];
        let basis = camera_basis(&camera);
        // A ground point exactly at the eye's projection has zero depth; move further "behind"
        // along -forward from the target to guarantee a non-positive depth.
        let behind = [
            camera.target[0] - basis.forward[0] * 1000.0,
            camera.target[1] - basis.forward[1] * 1000.0,
        ];
        assert_eq!(camera.ground_to_screen(behind, viewport), None);
    }

    #[test]
    fn screen_to_ground_near_the_horizon_stays_finite_or_none_never_nan() {
        // Review follow-up from WP2.2 (contract §6, "Offen: Test für Bodenpunkte nahe am
        // Horizont"): approach the horizon row in shrinking steps from both sides. On the "above
        // the horizon" side the ray never reaches the ground plane (`None`); on the "at or below"
        // side the intersection distance grows without bound as the pixel row approaches the
        // horizon, but must stay finite (or cleanly become `None`) rather than ever producing NaN
        // or a silently wrapped/overflowed value.
        let camera = Camera25D::default();
        let viewport = [800.0, 600.0];
        let basis = camera_basis(&camera);
        // The horizon row is where the ray direction's Z component is exactly zero; solve for the
        // pixel Y using the same construction as `screen_to_ground` (ndc_y before the tan/aspect
        // scale), so the steps below are true "distance to the horizon" steps rather than a guess.
        let horizon_ndc_y = -basis.forward[2] / basis.up[2];
        let horizon_pixel_y = (1.0 - horizon_ndc_y) * 0.5 * viewport[1];

        let mut last_finite_ground: Option<[f32; 2]> = None;
        for step in [10.0_f32, 1.0, 1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 0.0] {
            for pixel_y in [horizon_pixel_y - step, horizon_pixel_y + step] {
                let result = camera.screen_to_ground([400.0, pixel_y], viewport);
                match result {
                    None => {} // Above the horizon, or (at step 0.0) exactly on it: both valid.
                    Some(ground) => {
                        assert!(
                            ground[0].is_finite() && ground[1].is_finite(),
                            "step {step} at pixel_y {pixel_y} produced a non-finite ground point: {ground:?}"
                        );
                        last_finite_ground = Some(ground);
                    }
                }
            }
        }
        assert!(
            last_finite_ground.is_some(),
            "expected at least one finite ground point on the below-horizon side"
        );

        // Document the large-distance behaviour explicitly: a camera looking almost exactly at
        // the horizon (a tiny tilt) still returns `None` or a finite point for ordinary viewport
        // pixels, never NaN or infinity, however large the resulting ground distance gets.
        let grazing = Camera25D {
            tilt_degrees: 0.05,
            ..Camera25D::default()
        };
        for pixel_y in [0.0, 1.0, 100.0, 299.0, 300.0, 301.0, 500.0, 599.0] {
            let result = grazing.screen_to_ground([400.0, pixel_y], viewport);
            if let Some(ground) = result {
                assert!(
                    ground[0].is_finite() && ground[1].is_finite(),
                    "grazing tilt at pixel_y {pixel_y} produced a non-finite ground point: {ground:?}"
                );
            }
        }
    }

    // --- eye_position (WP2.5) -----------------------------------------------------------------

    #[test]
    fn eye_position_matches_camera_basis_eye() {
        let camera = Camera25D {
            target: [5.0, 7.0],
            tilt_degrees: 65.0,
            distance: 15.0,
            ..Camera25D::default()
        };
        assert_eq!(eye_position(&camera), camera_basis(&camera).eye);
    }

    #[test]
    fn eye_position_is_finite_for_a_well_formed_camera() {
        let eye = eye_position(&Camera25D::default());
        assert!(eye.iter().all(|c| c.is_finite()), "{eye:?}");
    }

    // --- view_projection: consistency with the ray-cast basis --------------------------------

    #[test]
    fn view_projection_is_finite_for_a_well_formed_camera() {
        let camera = Camera25D::default();
        let matrix = view_projection(&camera, 800.0 / 600.0, 0.05, 2000.0);
        for column in matrix {
            assert!(column.iter().all(|c| c.is_finite()), "{matrix:?}");
        }
    }

    #[test]
    fn view_projection_maps_the_target_ground_point_near_the_viewport_centre() {
        // The centre pixel always hits the ground at `target` (see
        // `screen_to_ground_center_pixel_is_target`); clip-space should agree; after the
        // perspective divide, that point's NDC x/y should be close to the viewport centre (0, 0).
        let camera = Camera25D {
            target: [5.0, 7.0],
            ..Camera25D::default()
        };
        let aspect = 800.0 / 600.0;
        let matrix = view_projection(&camera, aspect, 0.05, 2000.0);
        let world = [camera.target[0], camera.target[1], 0.0, 1.0];
        let clip: [f32; 4] =
            std::array::from_fn(|row| (0..4).map(|col| matrix[col][row] * world[col]).sum());
        assert!(
            clip[3] > 0.0,
            "target must be in front of the camera: {clip:?}"
        );
        let ndc_x = clip[0] / clip[3];
        let ndc_y = clip[1] / clip[3];
        assert!(ndc_x.abs() < 1e-4, "ndc_x {ndc_x}");
        assert!(ndc_y.abs() < 1e-4, "ndc_y {ndc_y}");
    }

    #[test]
    fn view_projection_never_panics_on_a_non_finite_camera() {
        let camera = Camera25D {
            tilt_degrees: f32::NAN,
            ..Camera25D::default()
        };
        // Must not panic; the mesh pass treats a non-finite result like "no camera" (see
        // `view_projection`'s doc comment).
        let _ = view_projection(&camera, 800.0 / 600.0, 0.05, 2000.0);
    }

    // --- CameraFollow: critically damped spring + look-ahead (WP2.4) ------------------------

    #[test]
    fn camera_follow_starts_settled_on_the_initial_focus() {
        let follow = CameraFollow::new([12.0, -4.0]);
        assert_eq!(follow.position(), [12.0, -4.0]);
    }

    #[test]
    fn camera_follow_converges_on_a_stationary_focus() {
        let camera = Camera25D {
            look_ahead_max: 4.0,
            look_ahead_smoothing: 0.25,
            ..Camera25D::default()
        };
        let mut follow = CameraFollow::new([0.0, 0.0]);
        let focus = [10.0, -6.0];
        // A moving focus first pulls in a look-ahead offset; once it stops (from here on) the
        // spring must settle back down onto the focus itself.
        follow.update(&camera, [5.0, -3.0], 1.0 / 60.0);
        for _ in 0..600 {
            follow.update(&camera, focus, 1.0 / 60.0);
        }
        let position = follow.position();
        assert!(
            (position[0] - focus[0]).abs() < 1e-2 && (position[1] - focus[1]).abs() < 1e-2,
            "expected convergence near {focus:?}, got {position:?}"
        );
    }

    #[test]
    fn camera_follow_look_ahead_leads_a_moving_focus_bounded_by_the_configured_maximum() {
        let camera = Camera25D {
            look_ahead_max: 2.0,
            look_ahead_smoothing: 0.05,
            ..Camera25D::default()
        };
        let mut follow = CameraFollow::new([0.0, 0.0]);
        // A focus moving along +X at 10 units/s for two seconds: the look-ahead offset (one
        // second of velocity, clamped) saturates at `look_ahead_max` from the first frame, so the
        // steady-state lead must stay within it (a critically damped spring tracking a ramp never
        // overshoots its target, so the lead approaches but never exceeds `look_ahead_max` by more
        // than floating point slack) while still being strictly ahead of the focus (look-ahead is
        // actually happening, not just clamped away to zero). A critically damped spring tracking
        // a ramp also lags its own (moving) target by `velocity * look_ahead_smoothing` at steady
        // state, so a *fast* focus relative to `look_ahead_smoothing` can net-lag the raw focus
        // even while leading the look-ahead target — 10 units/s here keeps that tracking lag
        // (0.5 world units) well under `look_ahead_max`, so the net lead stays positive.
        let mut focus_x = 0.0f32;
        let mut position = [0.0, 0.0];
        let mut max_lead = f32::MIN;
        for _ in 0..120 {
            focus_x += 10.0 / 60.0;
            position = follow.update(&camera, [focus_x, 0.0], 1.0 / 60.0);
            max_lead = grimoire_core::math::dmath::max(max_lead, position[0] - focus_x);
        }
        let lead = position[0] - focus_x;
        assert!(
            lead > 0.0 && lead <= camera.look_ahead_max + 1e-3,
            "expected 0 < lead <= look_ahead_max ({}), got {lead}",
            camera.look_ahead_max
        );
        assert!(
            max_lead <= camera.look_ahead_max + 1e-3,
            "lead must never exceed look_ahead_max ({}), peaked at {max_lead}",
            camera.look_ahead_max
        );
        assert!((position[1]).abs() < 1e-6, "no motion on Y: {position:?}");
    }

    #[test]
    fn camera_follow_never_produces_nan_for_degenerate_inputs() {
        let degenerate_cameras = [
            Camera25D::default(),
            Camera25D {
                look_ahead_max: f32::NAN,
                ..Camera25D::default()
            },
            Camera25D {
                look_ahead_smoothing: f32::NAN,
                ..Camera25D::default()
            },
            Camera25D {
                look_ahead_max: -1.0,
                look_ahead_smoothing: 0.0,
                ..Camera25D::default()
            },
            Camera25D {
                look_ahead_max: f32::INFINITY,
                look_ahead_smoothing: f32::INFINITY,
                ..Camera25D::default()
            },
        ];
        let dts = [0.0_f32, -1.0, 1e-6, 1.0, 1000.0, f32::NAN, f32::INFINITY];
        let focuses = [
            [0.0, 0.0],
            [f32::NAN, 0.0],
            [f32::INFINITY, -f32::INFINITY],
            [1e30, -1e30],
        ];
        for camera in &degenerate_cameras {
            let mut follow = CameraFollow::new([0.0, 0.0]);
            for &dt in &dts {
                for &focus in &focuses {
                    let position = follow.update(camera, focus, dt);
                    assert!(
                        position[0].is_finite() && position[1].is_finite(),
                        "camera {camera:?}, dt {dt}, focus {focus:?} produced {position:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn camera_follow_feeds_a_finite_view_projection_and_ray_cast() {
        // Ties CameraFollow into the existing projection/ray-cast gate (WP2.4 depends on WP2.2/
        // WP2.3's `view_projection` and `screen_to_ground`, not a new matrix): whatever position
        // the spring settles on must keep both finite and consistent.
        let template = Camera25D {
            look_ahead_max: 4.0,
            look_ahead_smoothing: 0.2,
            ..Camera25D::default()
        };
        let mut follow = CameraFollow::new([0.0, 0.0]);
        let mut target = [0.0, 0.0];
        for tick in 0..120u32 {
            let focus = [tick as f32 * 0.2, (tick as f32 * 0.05).sin() * 3.0];
            target = follow.update(&template, focus, 1.0 / 60.0);
        }
        let camera = Camera25D { target, ..template };
        let viewport = [800.0, 600.0];
        let matrix = view_projection(&camera, viewport[0] / viewport[1], 0.05, 2000.0);
        for column in matrix {
            assert!(column.iter().all(|c| c.is_finite()), "{matrix:?}");
        }
        let center = [viewport[0] * 0.5, viewport[1] * 0.5];
        let ground = camera
            .screen_to_ground(center, viewport)
            .expect("a well-formed followed camera still hits the ground at its centre pixel");
        assert!((ground[0] - target[0]).abs() < 1e-3);
        assert!((ground[1] - target[1]).abs() < 1e-3);
    }

    // --- PbrMaterial validation boundaries ---------------------------------------------------

    #[test]
    fn pbr_material_default_is_valid() {
        assert!(PbrMaterial::default().is_valid());
    }

    #[test]
    fn pbr_material_accepts_unit_range_boundaries() {
        let material = PbrMaterial {
            base_color_factor: [0.0, 1.0, 0.0, 1.0],
            metallic_factor: 0.0,
            roughness_factor: 1.0,
            emissive_factor: [0.0, 1.0, 0.0],
            alpha_mode: AlphaMode::Mask { cutoff: 0.0 },
            ..PbrMaterial::default()
        };
        assert!(material.is_valid());
        let other = PbrMaterial {
            alpha_mode: AlphaMode::Mask { cutoff: 1.0 },
            ..material
        };
        assert!(other.is_valid());
    }

    #[test]
    fn pbr_material_rejects_out_of_range_and_non_finite_fields() {
        let base = PbrMaterial::default();
        assert!(
            !PbrMaterial {
                metallic_factor: 1.000_1,
                ..base
            }
            .is_valid(),
            "metallic slightly above 1.0"
        );
        assert!(
            !PbrMaterial {
                roughness_factor: -0.000_1,
                ..base
            }
            .is_valid(),
            "roughness slightly below 0.0"
        );
        assert!(
            !PbrMaterial {
                base_color_factor: [f32::NAN, 1.0, 1.0, 1.0],
                ..base
            }
            .is_valid(),
            "NaN base colour component"
        );
        assert!(
            !PbrMaterial {
                emissive_factor: [f32::INFINITY, 0.0, 0.0],
                ..base
            }
            .is_valid(),
            "infinite emissive component"
        );
        assert!(
            !PbrMaterial {
                alpha_mode: AlphaMode::Mask { cutoff: 1.5 },
                ..base
            }
            .is_valid(),
            "mask cutoff above 1.0"
        );
        assert!(
            !PbrMaterial {
                alpha_mode: AlphaMode::Mask { cutoff: f32::NAN },
                ..base
            }
            .is_valid(),
            "NaN mask cutoff"
        );
    }

    // --- Light value validation ("light budget clamping") -----------------------------------

    #[test]
    fn point_light_default_is_valid() {
        assert!(PointLight::default().is_valid());
    }

    #[test]
    fn point_light_rejects_invalid_values_without_panic() {
        let base = PointLight::default();
        assert!(
            !PointLight {
                intensity: -0.001,
                ..base
            }
            .is_valid(),
            "negative intensity"
        );
        assert!(
            !PointLight {
                intensity: f32::NAN,
                ..base
            }
            .is_valid(),
            "NaN intensity"
        );
        assert!(!PointLight { range: 0.0, ..base }.is_valid(), "zero range");
        assert!(
            !PointLight {
                range: f32::INFINITY,
                ..base
            }
            .is_valid(),
            "infinite range"
        );
        assert!(
            !PointLight {
                position: [0.0, f32::NAN, 0.0],
                ..base
            }
            .is_valid(),
            "NaN position component"
        );
        assert!(
            !PointLight {
                color: [-1.0, 0.0, 0.0],
                ..base
            }
            .is_valid(),
            "negative colour component"
        );
    }

    #[test]
    fn directional_light_rejects_zero_and_non_finite_direction() {
        let base = DirectionalLight::default();
        assert!(base.is_valid());
        assert!(
            !DirectionalLight {
                direction: [0.0, 0.0, 0.0],
                ..base
            }
            .is_valid(),
            "zero-length direction"
        );
        assert!(
            !DirectionalLight {
                direction: [f32::NAN, 0.0, -1.0],
                ..base
            }
            .is_valid(),
            "NaN direction component"
        );
        assert!(
            !DirectionalLight {
                intensity: -1.0,
                ..base
            }
            .is_valid(),
            "negative intensity"
        );
    }

    #[test]
    fn ambient_light_variants_validate_every_field() {
        assert!(AmbientLight::default().is_valid());
        assert!(
            AmbientLight::Hemisphere {
                sky_color: [0.2, 0.2, 0.3],
                ground_color: [0.05, 0.05, 0.05],
                intensity: 0.2,
            }
            .is_valid()
        );
        assert!(
            !AmbientLight::Flat {
                color: [1.0, 1.0, 1.0],
                intensity: -0.1,
            }
            .is_valid(),
            "negative intensity"
        );
        assert!(
            !AmbientLight::Hemisphere {
                sky_color: [f32::NAN, 0.0, 0.0],
                ground_color: [0.0, 0.0, 0.0],
                intensity: 0.1,
            }
            .is_valid(),
            "NaN sky colour component"
        );
    }

    // --- Bullet-light-cap parameter validation ------------------------------------------------

    #[test]
    fn bullet_light_cap_accepts_boundaries_and_clamps_for_shading() {
        assert!(
            BulletLightCap {
                floor_contribution: 0.0
            }
            .is_valid()
        );
        assert!(
            BulletLightCap {
                floor_contribution: 1.0
            }
            .is_valid()
        );
        assert!(BulletLightCap::default().is_valid());
        assert!(
            (BulletLightCap {
                floor_contribution: 0.4
            }
            .clamped_floor_contribution()
                - 0.4)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn bullet_light_cap_rejects_out_of_range_and_falls_back_to_the_safe_value_when_clamped() {
        let too_high = BulletLightCap {
            floor_contribution: 1.5,
        };
        assert!(!too_high.is_valid());
        assert!((too_high.clamped_floor_contribution() - 1.0).abs() < 1e-6);

        let negative = BulletLightCap {
            floor_contribution: -0.5,
        };
        assert!(!negative.is_valid());
        assert!(negative.clamped_floor_contribution().abs() < 1e-6);

        let nan = BulletLightCap {
            floor_contribution: f32::NAN,
        };
        assert!(!nan.is_valid());
        assert_eq!(
            nan.clamped_floor_contribution(),
            0.0,
            "non-finite cap fails closed to 0.0, the safe side of PRD-0003 rule 5"
        );
    }

    // --- point_light_from_bullet (WP3.4, PO decision 2026-09-16) -----------------------------

    fn sample_bullet() -> crate::stage::BulletInstance {
        crate::stage::BulletInstance {
            position: [3.0, -1.0],
            radius: 0.5,
            rotation: 0.0,
            silhouette: 0,
            palette: 0,
            palette_space: crate::stage::BULLET_PASS_PALETTE_SPACE,
            glow: 128,
            flags: 0,
        }
    }

    #[test]
    fn point_light_from_bullet_always_sets_is_bullet_light() {
        // The one invariant the PO decision actually fixes (this function's doc comment): every
        // `PointLight` this function returns has the flag set, for any input, including
        // structurally degenerate ones (checked separately below for `is_valid`, not this flag).
        for bullet in [
            sample_bullet(),
            crate::stage::BulletInstance::default(),
            crate::stage::BulletInstance {
                glow: 0,
                ..sample_bullet()
            },
            crate::stage::BulletInstance {
                glow: 255,
                ..sample_bullet()
            },
            crate::stage::BulletInstance {
                radius: -1.0, // structurally invalid; still must carry the flag
                ..sample_bullet()
            },
        ] {
            assert!(point_light_from_bullet(&bullet).is_bullet_light);
        }
    }

    #[test]
    fn point_light_from_bullet_places_the_light_above_the_bullets_ground_position() {
        let bullet = sample_bullet();
        let light = point_light_from_bullet(&bullet);
        assert_eq!(light.position, [3.0, -1.0, 0.5]);
    }

    #[test]
    fn point_light_from_bullet_scales_intensity_with_glow() {
        let dim = point_light_from_bullet(&crate::stage::BulletInstance {
            glow: 0,
            ..sample_bullet()
        });
        let bright = point_light_from_bullet(&crate::stage::BulletInstance {
            glow: 255,
            ..sample_bullet()
        });
        assert_eq!(dim.intensity, 0.0);
        assert!(bright.intensity > dim.intensity);
    }

    #[test]
    fn point_light_from_bullet_never_uses_the_bullets_own_palette_colour() {
        // The colour is fixed (`BULLET_LIGHT_COLOR`) regardless of `palette`/`palette_space` —
        // stilbibel.md's whole point is that a bullet-cloud light must not tint the environment
        // towards the protected bullet colour space.
        let light = point_light_from_bullet(&sample_bullet());
        assert_eq!(light.color, BULLET_LIGHT_COLOR);
    }

    #[test]
    fn point_light_from_bullet_produces_a_valid_light_for_a_well_formed_bullet() {
        assert!(point_light_from_bullet(&sample_bullet()).is_valid());
    }

    #[test]
    fn point_light_from_bullet_of_a_degenerate_bullet_fails_is_valid_not_a_panic() {
        // A non-positive radius makes `range <= 0`, which `PointLight::is_valid` rejects — the
        // same "reject and count, never guess or crash" contract discipline every other light
        // source in this crate already follows; this adapter does not special-case it.
        let light = point_light_from_bullet(&crate::stage::BulletInstance {
            radius: 0.0,
            ..sample_bullet()
        });
        assert!(!light.is_valid());
    }

    /// Reference sRGB-to-linear transfer function (IEC 61966-2-1), used only to cross-check
    /// [`BULLET_LIGHT_COLOR`]'s literals against their documented hex source.
    fn srgb_to_linear(channel: f32) -> f32 {
        if channel <= 0.040_45 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    #[test]
    fn bullet_light_color_matches_the_stilbibel_hex_value() {
        let srgb = [0xB0_u8, 0x78_u8, 0x50_u8].map(|byte| f32::from(byte) / 255.0);
        for (linear, srgb) in BULLET_LIGHT_COLOR.into_iter().zip(srgb) {
            assert!(
                (linear - srgb_to_linear(srgb)).abs() < 1e-3,
                "{linear} vs {}",
                srgb_to_linear(srgb)
            );
        }
    }

    // --- MeshInstance defaults ----------------------------------------------------------------

    #[test]
    fn mesh_instance_default_uses_identity_transform_and_world_layer() {
        let mesh = MeshInstance::default();
        assert_eq!(mesh.transform, IDENTITY_TRANSFORM);
        assert_eq!(mesh.layer, RenderLayer::World);
    }

    // --- ShadowMode / ShadowConfig (WP2.6) ----------------------------------------------------

    #[test]
    fn shadow_mode_default_is_none_and_wants_nothing() {
        assert_eq!(ShadowMode::default(), ShadowMode::None);
        assert!(!ShadowMode::None.wants_key_light_shadow_map());
        assert!(!ShadowMode::None.wants_blob_shadows());
    }

    #[test]
    fn shadow_mode_wants_matrix() {
        assert!(!ShadowMode::Blob.wants_key_light_shadow_map());
        assert!(ShadowMode::Blob.wants_blob_shadows());
        assert!(ShadowMode::KeyLight.wants_key_light_shadow_map());
        assert!(!ShadowMode::KeyLight.wants_blob_shadows());
        assert!(ShadowMode::KeyLightPlusPoints.wants_key_light_shadow_map());
        assert!(!ShadowMode::KeyLightPlusPoints.wants_blob_shadows());
    }

    #[test]
    fn shadow_config_default_is_valid() {
        assert!(ShadowConfig::default().is_valid());
    }

    #[test]
    fn shadow_config_rejects_zero_or_oversized_map_size() {
        assert!(
            !ShadowConfig {
                map_size: 0,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
        assert!(
            !ShadowConfig {
                map_size: 8193,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
        assert!(
            ShadowConfig {
                map_size: 8192,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
    }

    #[test]
    fn shadow_config_rejects_excessive_pcf_radius_and_non_finite_or_non_positive_frustum() {
        assert!(
            !ShadowConfig {
                pcf_radius: 5,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
        assert!(
            ShadowConfig {
                pcf_radius: 4,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
        assert!(
            !ShadowConfig {
                frustum_radius: 0.0,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
        assert!(
            !ShadowConfig {
                frustum_radius: f32::NAN,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
        assert!(
            !ShadowConfig {
                frustum_height: -1.0,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
        assert!(
            !ShadowConfig {
                depth_bias_slope_scale: f32::NAN,
                ..ShadowConfig::default()
            }
            .is_valid()
        );
    }

    // --- BlobShadowInstance validation (WP2.6) ------------------------------------------------

    fn valid_blob() -> BlobShadowInstance {
        BlobShadowInstance {
            position: [1.0, -2.0],
            radius: 1.5,
            softness: 0.5,
            strength: 0.6,
        }
    }

    #[test]
    fn blob_shadow_instance_default_is_invalid_zero_radius() {
        // `Default` (all-zero) is deliberately not automatically valid: a `radius <= 0.0` disc
        // draws nothing, matching `BulletInstance`'s own "radius <= 0 is rejected" rule.
        assert!(!BlobShadowInstance::default().is_valid());
    }

    #[test]
    fn blob_shadow_instance_accepts_boundary_values() {
        assert!(
            BlobShadowInstance {
                softness: 0.0,
                strength: 0.0,
                ..valid_blob()
            }
            .is_valid()
        );
        assert!(
            BlobShadowInstance {
                softness: 1.0,
                strength: 1.0,
                ..valid_blob()
            }
            .is_valid()
        );
    }

    #[test]
    fn blob_shadow_instance_rejects_invalid_fields_without_panic() {
        assert!(
            !BlobShadowInstance {
                radius: 0.0,
                ..valid_blob()
            }
            .is_valid(),
            "zero radius"
        );
        assert!(
            !BlobShadowInstance {
                radius: -1.0,
                ..valid_blob()
            }
            .is_valid(),
            "negative radius"
        );
        assert!(
            !BlobShadowInstance {
                softness: 1.0001,
                ..valid_blob()
            }
            .is_valid(),
            "softness above 1.0"
        );
        assert!(
            !BlobShadowInstance {
                strength: -0.0001,
                ..valid_blob()
            }
            .is_valid(),
            "strength below 0.0"
        );
        assert!(
            !BlobShadowInstance {
                position: [f32::NAN, 0.0],
                ..valid_blob()
            }
            .is_valid(),
            "NaN position component"
        );
    }

    // --- PointLight::casts_shadow (WP2.6, prepared interface) ---------------------------------

    #[test]
    fn point_light_default_does_not_cast_a_shadow() {
        assert!(!PointLight::default().casts_shadow);
    }

    // --- key_light_view_projection (WP2.6) ----------------------------------------------------

    #[test]
    fn key_light_view_projection_is_finite_for_a_well_formed_light() {
        let config = ShadowConfig::default();
        let matrix = key_light_view_projection([0.3, 0.2, -1.0], [5.0, -3.0], &config);
        for column in matrix {
            assert!(column.iter().all(|c| c.is_finite()), "{matrix:?}");
        }
    }

    #[test]
    fn key_light_view_projection_never_panics_or_produces_nan_for_degenerate_input() {
        let config = ShadowConfig::default();
        for direction in [
            [0.0, 0.0, 0.0],
            [f32::NAN, 0.0, -1.0],
            [0.0, 0.0, 1.0],  // straight up, parallel to the up hint
            [0.0, 0.0, -1.0], // straight down, parallel to the up hint
            [f32::INFINITY, 0.0, -1.0],
        ] {
            let matrix = key_light_view_projection(direction, [0.0, 0.0], &config);
            for column in matrix {
                assert!(
                    column.iter().all(|c| c.is_finite()),
                    "direction {direction:?} produced {matrix:?}"
                );
            }
        }
    }

    #[test]
    fn key_light_view_projection_maps_the_target_column_near_the_frustum_centre() {
        // The frustum is centred on `target` in X/Y (at half the configured height in Z); after
        // the perspective-free orthographic divide (`w == 1` always), that point's light-space
        // clip x/y must land at (or very near) the centre (0, 0) of the fitted box.
        let config = ShadowConfig::default();
        let target = [5.0, -3.0];
        let matrix = key_light_view_projection([0.2, 0.4, -1.0], target, &config);
        let world = [target[0], target[1], config.frustum_height * 0.5, 1.0];
        let clip: [f32; 4] =
            std::array::from_fn(|row| (0..4).map(|col| matrix[col][row] * world[col]).sum());
        assert!((clip[0]).abs() < 1e-3, "clip x {clip:?}");
        assert!((clip[1]).abs() < 1e-3, "clip y {clip:?}");
    }

    #[test]
    fn key_light_view_projection_covers_a_taller_frustum_without_clipping_the_top() {
        // A point at the top of the configured frustum height, straight above the target, must
        // still land within the [-1, 1] light-space depth range (it must not be clipped away).
        let config = ShadowConfig {
            frustum_height: 30.0,
            ..ShadowConfig::default()
        };
        let matrix = key_light_view_projection([0.0, 0.0, -1.0], [0.0, 0.0], &config);
        let world = [0.0, 0.0, config.frustum_height, 1.0];
        let clip: [f32; 4] =
            std::array::from_fn(|row| (0..4).map(|col| matrix[col][row] * world[col]).sum());
        assert!(
            (-1.0..=1.0).contains(&clip[2]),
            "top of the frustum must stay within depth range: {clip:?}"
        );
    }
}
