//! Procedural mesh builders. Everything is baked into world space (static scene, one draw).

use std::f32::consts::{PI, TAU};

use crate::camera::{Vec3, add, cross, normalize, scale, sub};
use crate::gpu_types::Vertex;

/// Material, class and rim kind of the parts being added.
#[derive(Debug, Clone, Copy)]
pub struct Tag {
    pub material: u32,
    pub class: u32,
    pub rim: u32,
    pub figure: u32,
}

impl Tag {
    pub const fn new(material: u32, class: u32) -> Self {
        Self {
            material,
            class,
            rim: 0,
            figure: 0,
        }
    }

    pub const fn with_material(self, material: u32) -> Self {
        Self { material, ..self }
    }

    fn packed(self) -> u32 {
        self.material | (self.class << 8) | (self.rim << 16)
    }
}

/// Rigid transform: rotation (columns = local axes in world space) plus translation.
#[derive(Debug, Clone, Copy)]
pub struct Xform {
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
    pub t: Vec3,
}

impl Xform {
    pub const IDENTITY: Self = Self {
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
        z: [0.0, 0.0, 1.0],
        t: [0.0, 0.0, 0.0],
    };

    pub fn translate(t: Vec3) -> Self {
        Self { t, ..Self::IDENTITY }
    }

    /// Rotation about +Z by `yaw` radians, then translation.
    pub fn yaw(yaw: f32, t: Vec3) -> Self {
        let (s, c) = yaw.sin_cos();
        Self {
            x: [c, s, 0.0],
            y: [-s, c, 0.0],
            z: [0.0, 0.0, 1.0],
            t,
        }
    }

    /// Frame whose local +Z points along `axis`.
    pub fn along(axis: Vec3, t: Vec3) -> Self {
        let z = normalize(axis);
        let helper = if z[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        let x = normalize(cross(helper, z));
        let y = cross(z, x);
        Self { x, y, z, t }
    }

    pub fn point(&self, p: Vec3) -> Vec3 {
        add(self.t, self.vector(p))
    }

    pub fn vector(&self, v: Vec3) -> Vec3 {
        add(add(scale(self.x, v[0]), scale(self.y, v[1])), scale(self.z, v[2]))
    }

    /// `self` applied after `local` (local coordinates of `local` are expressed in `self`).
    pub fn compose(&self, local: &Xform) -> Xform {
        Xform {
            x: self.vector(local.x),
            y: self.vector(local.y),
            z: self.vector(local.z),
            t: self.point(local.t),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    fn vertex(&mut self, position: Vec3, normal: Vec3, tag: Tag) -> u32 {
        let index = u32::try_from(self.vertices.len()).expect("mesh index fits u32");
        self.vertices.push(Vertex {
            position,
            packed: tag.packed(),
            normal: normalize(normal),
            figure: tag.figure,
        });
        index
    }

    /// Flat-shaded triangle (normal from the winding, counter-clockwise front).
    pub fn flat_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, tag: Tag) {
        let n = normalize(cross(sub(b, a), sub(c, a)));
        let ia = self.vertex(a, n, tag);
        let ib = self.vertex(b, n, tag);
        let ic = self.vertex(c, n, tag);
        self.indices.extend_from_slice(&[ia, ib, ic]);
    }

    /// Flat-shaded quad a-b-c-d (counter-clockwise).
    pub fn flat_quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, tag: Tag) {
        self.flat_triangle(a, b, c, tag);
        self.flat_triangle(a, c, d, tag);
    }

    /// Flat quad whose winding is fixed so that its normal faces `hint`.
    pub fn flat_quad_facing(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, hint: Vec3, tag: Tag) {
        let n = cross(sub(b, a), sub(c, a));
        if crate::camera::dot(n, hint) >= 0.0 {
            self.flat_quad(a, b, c, d, tag);
        } else {
            self.flat_quad(a, d, c, b, tag);
        }
    }

    /// Box with half extents `h`, centred at the local origin, flat-shaded.
    pub fn box_flat(&mut self, xf: &Xform, h: Vec3, tag: Tag, top_tag: Tag) {
        let p = |sx: f32, sy: f32, sz: f32| xf.point([sx * h[0], sy * h[1], sz * h[2]]);
        // +Z (top)
        self.flat_quad(p(-1., -1., 1.), p(1., -1., 1.), p(1., 1., 1.), p(-1., 1., 1.), top_tag);
        // -Z
        self.flat_quad(p(-1., -1., -1.), p(-1., 1., -1.), p(1., 1., -1.), p(1., -1., -1.), tag);
        // +X
        self.flat_quad(p(1., -1., -1.), p(1., 1., -1.), p(1., 1., 1.), p(1., -1., 1.), tag);
        // -X
        self.flat_quad(p(-1., -1., -1.), p(-1., -1., 1.), p(-1., 1., 1.), p(-1., 1., -1.), tag);
        // +Y
        self.flat_quad(p(-1., 1., -1.), p(-1., 1., 1.), p(1., 1., 1.), p(1., 1., -1.), tag);
        // -Y
        self.flat_quad(p(-1., -1., -1.), p(1., -1., -1.), p(1., -1., 1.), p(-1., -1., 1.), tag);
    }

    /// Flat-shaded prism with `sides` facets along local +Z from z0 to the per-vertex top heights.
    pub fn prism_flat(
        &mut self,
        xf: &Xform,
        sides: usize,
        radius: f32,
        z0: f32,
        tops: &[f32],
        tag: Tag,
        cap_tag: Tag,
    ) {
        let offset = PI / sides as f32;
        let ring = |i: usize, z: f32| {
            let a = offset + TAU * (i % sides) as f32 / sides as f32;
            xf.point([radius * a.cos(), radius * a.sin(), z])
        };
        for i in 0..sides {
            let j = (i + 1) % sides;
            self.flat_quad(ring(i, z0), ring(j, z0), ring(j, tops[j]), ring(i, tops[i]), tag);
        }
        let centre_top = tops.iter().sum::<f32>() / sides as f32;
        let ct = xf.point([0.0, 0.0, centre_top]);
        let cb = xf.point([0.0, 0.0, z0]);
        for i in 0..sides {
            let j = (i + 1) % sides;
            self.flat_triangle(ct, ring(i, tops[i]), ring(j, tops[j]), cap_tag);
            self.flat_triangle(cb, ring(j, z0), ring(i, z0), tag);
        }
    }

    /// Smooth surface of revolution around local +Z. `profile` lists (radius, z) from bottom to top;
    /// normals come from the profile tangents. Closed with flat caps where the radius is > 0.
    pub fn lathe(&mut self, xf: &Xform, profile: &[(f32, f32)], segments: usize, tag: Tag) {
        let n = profile.len();
        let base = self.vertices.len() as u32;
        for (k, &(r, z)) in profile.iter().enumerate() {
            let prev = profile[k.saturating_sub(1)];
            let next = profile[(k + 1).min(n - 1)];
            // Tangent in (r, z); outward normal is (dz, -dr).
            let dr = next.0 - prev.0;
            let dz = next.1 - prev.1;
            let len = (dr * dr + dz * dz).sqrt().max(1e-6);
            let (nr, nz) = (dz / len, -dr / len);
            for s in 0..=segments {
                let a = TAU * s as f32 / segments as f32;
                let (sa, ca) = a.sin_cos();
                let position = xf.point([r * ca, r * sa, z]);
                let normal = xf.vector([nr * ca, nr * sa, nz]);
                self.vertex(position, normal, tag);
            }
        }
        let row = segments as u32 + 1;
        for k in 0..(n as u32 - 1) {
            for s in 0..segments as u32 {
                let a = base + k * row + s;
                let b = a + 1;
                let c = a + row + 1;
                let d = a + row;
                self.indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        let (r0, z0) = profile[0];
        if r0 > 1e-4 {
            self.disc(xf, r0, z0, [0.0, 0.0, -1.0], segments, tag);
        }
        let (r1, z1) = profile[n - 1];
        if r1 > 1e-4 {
            self.disc(xf, r1, z1, [0.0, 0.0, 1.0], segments, tag);
        }
    }

    /// Flat disc in the local XY plane at height `z`, facing `facing` (local).
    pub fn disc(&mut self, xf: &Xform, radius: f32, z: f32, facing: Vec3, segments: usize, tag: Tag) {
        let normal = xf.vector(facing);
        let centre = self.vertex(xf.point([0.0, 0.0, z]), normal, tag);
        let first = self.vertices.len() as u32;
        for s in 0..=segments {
            let a = TAU * s as f32 / segments as f32;
            self.vertex(xf.point([radius * a.cos(), radius * a.sin(), z]), normal, tag);
        }
        for s in 0..segments as u32 {
            if facing[2] >= 0.0 {
                self.indices.extend_from_slice(&[centre, first + s, first + s + 1]);
            } else {
                self.indices.extend_from_slice(&[centre, first + s + 1, first + s]);
            }
        }
    }

    /// Ellipsoid with radii `r` centred at the local origin (smooth, analytic normals).
    pub fn ellipsoid(&mut self, xf: &Xform, r: Vec3, rings: usize, segments: usize, tag: Tag) {
        let base = self.vertices.len() as u32;
        for k in 0..=rings {
            let theta = PI * k as f32 / rings as f32; // 0 = bottom pole
            let (st, ct) = theta.sin_cos();
            for s in 0..=segments {
                let phi = TAU * s as f32 / segments as f32;
                let unit = [st * phi.cos(), st * phi.sin(), -ct];
                let p = [unit[0] * r[0], unit[1] * r[1], unit[2] * r[2]];
                let n = [p[0] / (r[0] * r[0]), p[1] / (r[1] * r[1]), p[2] / (r[2] * r[2])];
                self.vertex(xf.point(p), xf.vector(n), tag);
            }
        }
        let row = segments as u32 + 1;
        for k in 0..rings as u32 {
            for s in 0..segments as u32 {
                let a = base + k * row + s;
                self.indices
                    .extend_from_slice(&[a, a + 1, a + row + 1, a, a + row + 1, a + row]);
            }
        }
    }

    pub fn sphere(&mut self, centre: Vec3, radius: f32, tag: Tag) {
        self.ellipsoid(&Xform::translate(centre), [radius; 3], 10, 16, tag);
    }

    /// Capsule standing on local z = 0 with total height `height`.
    pub fn capsule(&mut self, xf: &Xform, radius: f32, height: f32, tag: Tag) {
        let mut profile = Vec::new();
        let cap = 6;
        let z_low = radius;
        let z_high = (height - radius).max(radius);
        for k in 0..=cap {
            let a = -PI / 2.0 + (PI / 2.0) * k as f32 / cap as f32;
            profile.push((radius * a.cos(), z_low + radius * a.sin()));
        }
        for k in 0..=cap {
            let a = (PI / 2.0) * k as f32 / cap as f32;
            profile.push((radius * a.cos(), z_high + radius * a.sin()));
        }
        profile[0].0 = 0.0;
        let last = profile.len() - 1;
        profile[last].0 = 0.0;
        self.lathe(xf, &profile, 20, tag);
    }

    /// Cone along local +Z: base radius at z = 0, apex at z = height.
    pub fn cone(&mut self, xf: &Xform, radius: f32, height: f32, segments: usize, tag: Tag) {
        self.lathe(xf, &[(radius, 0.0), (radius * 0.5, height * 0.5), (0.0, height)], segments, tag);
    }

    pub fn cylinder(&mut self, xf: &Xform, radius: f32, height: f32, segments: usize, tag: Tag) {
        self.lathe(xf, &[(radius, 0.0), (radius, height)], segments, tag);
    }

    /// Teardrop flame of two cones (downward tip of 30 %, upward tip of 70 % of `height`).
    pub fn flame(&mut self, base: Vec3, radius: f32, height: f32, tag: Tag) {
        let lower = 0.3 * height;
        let widest = [base[0], base[1], base[2] + lower];
        self.cone(&Xform::translate(widest), radius, height - lower, 12, tag);
        let flipped = Xform {
            x: [1.0, 0.0, 0.0],
            y: [0.0, -1.0, 0.0],
            z: [0.0, 0.0, -1.0],
            t: widest,
        };
        self.cone(&flipped, radius, lower, 12, tag);
    }

    pub fn append(&mut self, other: &Mesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::dot;

    #[test]
    fn box_normals_point_outwards() {
        let mut mesh = Mesh::default();
        let tag = Tag::new(0, 1);
        mesh.box_flat(&Xform::translate([1.0, 2.0, 3.0]), [0.5, 0.5, 0.5], tag, tag);
        for v in &mesh.vertices {
            let outward = sub(v.position, [1.0, 2.0, 3.0]);
            assert!(dot(outward, v.normal) > 0.0);
        }
    }

    #[test]
    fn lathe_normals_point_outwards() {
        let mut mesh = Mesh::default();
        mesh.capsule(&Xform::IDENTITY, 0.3, 1.0, Tag::new(0, 2));
        for v in &mesh.vertices {
            let axis_point = [0.0, 0.0, v.position[2].clamp(0.3, 0.7)];
            let outward = sub(v.position, axis_point);
            if crate::camera::length(outward) > 1e-3 {
                assert!(dot(outward, v.normal) > -1e-3, "{v:?}");
            }
        }
    }

    #[test]
    fn prism_winding_matches_normals() {
        let mut mesh = Mesh::default();
        let tag = Tag::new(0, 1);
        mesh.prism_flat(&Xform::IDENTITY, 8, 0.7, 0.0, &[2.0; 8], tag, tag);
        for v in &mesh.vertices {
            let outward = sub(v.position, [0.0, 0.0, 1.0]);
            assert!(dot(outward, v.normal) > 0.0);
        }
    }
}
