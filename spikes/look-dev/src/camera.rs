//! Minimal vector math and the shared perspective camera.
//!
//! Coordinates: X right, Y forward (screen up), Z up. The gameplay plane is Z = 0.

pub type Vec3 = [f32; 3];
/// Column-major 4x4 matrix, as WGSL expects.
pub type Mat4 = [[f32; 4]; 4];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

pub fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l > 1e-12 { scale(a, 1.0 / l) } else { [0.0, 0.0, 1.0] }
}

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (c, column) in out.iter_mut().enumerate() {
        for (r, value) in column.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

pub fn transform_point(m: &Mat4, p: Vec3) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (r, value) in out.iter_mut().enumerate() {
        *value = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    out
}

/// Frozen camera parameters of the spike (identical for every look and variant).
pub const PITCH_DEG: f32 = 65.0;
pub const FOV_Y_DEG: f32 = 35.0;
pub const DISTANCE: f32 = 34.5;
pub const TARGET: Vec3 = [0.0, 1.5, 0.0];
pub const NEAR: f32 = 0.5;
pub const FAR: f32 = 100.0;

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub view: Mat4,
    pub view_proj: Mat4,
}

impl Camera {
    /// The shared camera; `yaw_deg` rotates the eye around the target (0 for all stills).
    pub fn new(aspect: f32, yaw_deg: f32) -> Self {
        let pitch = PITCH_DEG.to_radians();
        let yaw = yaw_deg.to_radians();
        // Eye offset (0, -cos pitch, sin pitch), optionally rotated about Z.
        let offset = [
            -(-pitch.cos()) * yaw.sin(),
            -pitch.cos() * yaw.cos(),
            pitch.sin(),
        ];
        let eye = add(TARGET, scale(offset, DISTANCE));
        let forward = normalize(sub(TARGET, eye));
        let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
        let up = cross(right, forward);
        let view = [
            [right[0], up[0], -forward[0], 0.0],
            [right[1], up[1], -forward[1], 0.0],
            [right[2], up[2], -forward[2], 0.0],
            [-dot(right, eye), -dot(up, eye), dot(forward, eye), 1.0],
        ];
        let tan_half_fov = (FOV_Y_DEG.to_radians() * 0.5).tan();
        let f = 1.0 / tan_half_fov;
        // Right-handed perspective with depth 0..1 (standard depth, LessEqual).
        let proj = [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, FAR / (NEAR - FAR), -1.0],
            [0.0, 0.0, NEAR * FAR / (NEAR - FAR), 0.0],
        ];
        let view_proj = mat_mul(&proj, &view);
        Self {
            eye,
            right,
            up,
            view,
            view_proj,
        }
    }

    /// Projects a world point to pixel coordinates (origin top-left) of a `width` x `height`
    /// image. `None` behind the camera.
    pub fn project(&self, p: Vec3, width: f32, height: f32) -> Option<[f32; 2]> {
        let clip = transform_point(&self.view_proj, p);
        if clip[3] <= 1e-6 {
            return None;
        }
        let nx = clip[0] / clip[3];
        let ny = clip[1] / clip[3];
        Some([(nx * 0.5 + 0.5) * width, (0.5 - ny * 0.5) * height])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_matches_the_spec() {
        let camera = Camera::new(16.0 / 9.0, 0.0);
        assert!((camera.eye[0]).abs() < 1e-4);
        assert!((camera.eye[1] + 13.08).abs() < 0.01);
        assert!((camera.eye[2] - 31.27).abs() < 0.01);
    }

    #[test]
    fn target_projects_to_the_centre() {
        let camera = Camera::new(16.0 / 9.0, 0.0);
        let p = camera.project(TARGET, 1280.0, 720.0).expect("in front");
        assert!((p[0] - 640.0).abs() < 0.01 && (p[1] - 360.0).abs() < 0.01);
    }

    #[test]
    fn screen_up_is_world_forward() {
        let camera = Camera::new(16.0 / 9.0, 0.0);
        let near = camera.project([0.0, 0.0, 0.0], 1280.0, 720.0).expect("visible");
        let far = camera.project([0.0, 5.0, 0.0], 1280.0, 720.0).expect("visible");
        assert!(far[1] < near[1]);
        let right = camera.project([5.0, 0.0, 0.0], 1280.0, 720.0).expect("visible");
        assert!(right[0] > near[0]);
    }
}
