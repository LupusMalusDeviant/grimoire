//! Minimal hand-rolled camera math for this spike's offscreen render passes.
//!
//! **Deviation from the plan text ("unter der gekippten 2.5D-Kamera"):** the real tilted camera
//! lives at `grimoire_render::Camera25D`, but its `view_projection` helper is `pub(crate)` (not
//! part of the crate's public API — the doc comment on `stage3d::view_projection` says so
//! explicitly), so this throwaway spike cannot call it from outside `grimoire_render`. This module
//! is a small stand-in: a look-at camera tilted down at a ground-plane target, same tilt/FOV
//! vocabulary as `Camera25D` (`tilt_degrees`, `fov_y_degrees`). It is a deliberate simplification,
//! not a claim about the production camera's exact framing — see `README.md`.

/// Column-major 4x4 matrix, matching wgpu/WGSL's `mat4x4<f32>` convention.
pub type Mat4 = [f32; 16];

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize3(a: [f32; 3]) -> [f32; 3] {
    let len = dot3(a, a).sqrt();
    if len <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [a[0] / len, a[1] / len, a[2] / len]
    }
}

fn look_at_rh(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalize3(sub3(target, eye));
    let s = normalize3(cross3(f, up));
    let u = cross3(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.0,
        s[1],
        u[1],
        -f[1],
        0.0,
        s[2],
        u[2],
        -f[2],
        0.0,
        -dot3(s, eye),
        -dot3(u, eye),
        dot3(f, eye),
        1.0,
    ]
}

fn perspective_rh(fov_y_radians: f32, aspect: f32, z_near: f32, z_far: f32) -> Mat4 {
    let f = 1.0 / (fov_y_radians * 0.5).tan();
    let range_inv = 1.0 / (z_near - z_far);
    [
        f / aspect,
        0.0,
        0.0,
        0.0,
        0.0,
        f,
        0.0,
        0.0,
        0.0,
        0.0,
        (z_far + z_near) * range_inv,
        -1.0,
        0.0,
        0.0,
        2.0 * z_far * z_near * range_inv,
        0.0,
    ]
}

fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

/// A tilted, 2.5D-style camera looking at `target` on the `Z = 0` ground plane.
pub struct TiltedCamera {
    pub target: [f32; 2],
    /// Degrees down from the horizon; PRD-0003 configures 60-75 in the real renderer.
    pub tilt_degrees: f32,
    pub distance: f32,
    pub fov_y_degrees: f32,
    pub aspect: f32,
}

impl TiltedCamera {
    fn eye(&self) -> [f32; 3] {
        let tilt = self.tilt_degrees.to_radians();
        [
            self.target[0],
            self.target[1] - self.distance * tilt.cos(),
            self.distance * tilt.sin(),
        ]
    }

    #[must_use]
    pub fn view_projection(&self) -> Mat4 {
        let eye = self.eye();
        let target = [self.target[0], self.target[1], 0.0];
        let view = look_at_rh(eye, target, [0.0, 0.0, 1.0]);
        let proj = perspective_rh(self.fov_y_degrees.to_radians(), self.aspect, 0.1, 500.0);
        mat_mul(&proj, &view)
    }

    /// World-space right/up basis of the camera, for billboard-style extrusion of a camera-facing
    /// quad (the "billboard impostor" variant).
    #[must_use]
    pub fn right_up(&self) -> ([f32; 3], [f32; 3]) {
        let eye = self.eye();
        let target = [self.target[0], self.target[1], 0.0];
        let forward = normalize3(sub3(target, eye));
        let right = normalize3(cross3(forward, [0.0, 0.0, 1.0]));
        let up = cross3(right, forward);
        (right, up)
    }
}

#[cfg(test)]
mod tests {
    use super::TiltedCamera;

    fn default_camera() -> TiltedCamera {
        TiltedCamera {
            target: [0.0, 0.0],
            tilt_degrees: 65.0,
            distance: 20.0,
            fov_y_degrees: 45.0,
            aspect: 16.0 / 9.0,
        }
    }

    #[test]
    fn view_projection_is_finite() {
        let matrix = default_camera().view_projection();
        assert!(matrix.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn right_up_are_unit_and_orthogonal() {
        let (right, up) = default_camera().right_up();
        let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        assert!((len(right) - 1.0).abs() < 1e-4);
        assert!((len(up) - 1.0).abs() < 1e-4);
        let dot = right[0] * up[0] + right[1] * up[1] + right[2] * up[2];
        assert!(dot.abs() < 1e-4, "right/up not orthogonal: dot={dot}");
    }

    #[test]
    fn target_projects_near_the_viewport_centre() {
        let camera = default_camera();
        let matrix = camera.view_projection();
        let target_h = [camera.target[0], camera.target[1], 0.0, 1.0];
        let mut clip = [0.0f32; 4];
        for row in 0..4 {
            let mut sum = 0.0;
            for col in 0..4 {
                sum += matrix[col * 4 + row] * target_h[col];
            }
            clip[row] = sum;
        }
        assert!(clip[3] > 0.0, "target behind the camera");
        let ndc_x = clip[0] / clip[3];
        let ndc_y = clip[1] / clip[3];
        assert!(ndc_x.abs() < 0.05, "ndc_x={ndc_x}");
        assert!(ndc_y.abs() < 0.05, "ndc_y={ndc_y}");
    }
}
