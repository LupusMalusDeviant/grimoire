//! Procedurally generated test meshes (plan 0002 WP2.3): a floor tile grid, an octagonal pillar,
//! a capsule stand-in for an actor, an altar block and an icosphere. No dependency on the asset
//! pipeline (deferred to P3, plan 0002): every mesh here is pure Rust geometry, generated the same
//! way every time for the same inputs.
//!
//! Coordinate convention matches the rest of the WP2.2/WP2.3 stage: X right, Y away from the
//! viewer, Z up. Every mesh is centred on the model-space origin unless noted otherwise, so a
//! [`crate::MeshInstance::transform`] translation places it directly at the desired world point.
//!
//! Winding is intentionally not guaranteed to be consistently CCW or CW: the mesh pass disables
//! back-face culling (like the existing sprite pass, whose rotation can also flip winding), so a
//! generator here does not have to reason about front-face orientation. Every generator produces
//! finite positions and normals; texture coordinates are simple planar, cylindrical or spherical
//! mappings, good enough for factor-only PBR materials (WP2.5) but not claimed to be seam-free or
//! distortion-free for a textured material — none of these procedural test meshes is UV-unwrapped
//! for real texture authoring (that needs the Blender pipeline, OF-3.4/P2).

use std::f32::consts::TAU;

use crate::mesh::{MeshData, MeshVertex};

/// A flat, `tiles_per_side` x `tiles_per_side` grid of quads on the `Z = 0` plane, normal `+Z`,
/// each tile `tile_size` world units square. Centred on the origin. `tiles_per_side` is clamped to
/// at least `1`.
///
/// Vertex count: `(tiles_per_side + 1)^2`. Index count: `tiles_per_side^2 * 6`.
#[must_use]
pub fn floor_tile_grid(tiles_per_side: u32, tile_size: f32) -> MeshData {
    let tiles = tiles_per_side.max(1);
    let verts_per_side = tiles + 1;
    let half = tiles as f32 * tile_size * 0.5;

    let mut vertices = Vec::with_capacity((verts_per_side * verts_per_side) as usize);
    for j in 0..verts_per_side {
        for i in 0..verts_per_side {
            vertices.push(MeshVertex {
                position: [
                    i as f32 * tile_size - half,
                    j as f32 * tile_size - half,
                    0.0,
                ],
                normal: [0.0, 0.0, 1.0],
                uv: [i as f32 / tiles as f32, j as f32 / tiles as f32],
            });
        }
    }

    let mut indices = Vec::with_capacity((tiles * tiles * 6) as usize);
    for j in 0..tiles {
        for i in 0..tiles {
            let row0 = j * verts_per_side;
            let row1 = (j + 1) * verts_per_side;
            let a = row0 + i;
            let b = row0 + i + 1;
            let c = row1 + i + 1;
            let d = row1 + i;
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }

    MeshData { vertices, indices }
}

/// Appends one flat-shaded quad (two triangles) with a single face normal; `corners` in either
/// winding order (the mesh pass does not cull).
fn push_quad(
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    normal: [f32; 3],
) {
    let base = u32::try_from(vertices.len()).expect("test meshes stay far below u32::MAX");
    const UVS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for (corner, uv) in corners.into_iter().zip(UVS) {
        vertices.push(MeshVertex {
            position: corner,
            normal,
            uv,
        });
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Number of sides of [`octagonal_pillar`].
const PILLAR_SIDES: u32 = 8;

/// An octagonal (8-sided) prism, flat-shaded, standing along `Z` with height centred on the
/// origin: bottom at `Z = -height / 2`, top at `Z = height / 2`. `radius` is the distance from the
/// centre axis to each side vertex.
///
/// Vertex count: `8 * 4` (side faces) `+ 9 + 9` (top and bottom fans) `= 50`. Index count:
/// `8 * 6` (sides) `+ 8 * 3 + 8 * 3` (fans) `= 96`.
#[must_use]
pub fn octagonal_pillar(radius: f32, height: f32) -> MeshData {
    let half_height = height * 0.5;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for side in 0..PILLAR_SIDES {
        let angle0 = side as f32 / PILLAR_SIDES as f32 * TAU;
        let angle1 = (side + 1) as f32 / PILLAR_SIDES as f32 * TAU;
        let (x0, y0) = (radius * angle0.cos(), radius * angle0.sin());
        let (x1, y1) = (radius * angle1.cos(), radius * angle1.sin());
        let mid_angle = (angle0 + angle1) * 0.5;
        let normal = [mid_angle.cos(), mid_angle.sin(), 0.0];
        let u0 = side as f32 / PILLAR_SIDES as f32;
        let u1 = (side + 1) as f32 / PILLAR_SIDES as f32;
        let base = u32::try_from(vertices.len()).expect("test meshes stay far below u32::MAX");
        vertices.push(MeshVertex {
            position: [x0, y0, -half_height],
            normal,
            uv: [u0, 0.0],
        });
        vertices.push(MeshVertex {
            position: [x1, y1, -half_height],
            normal,
            uv: [u1, 0.0],
        });
        vertices.push(MeshVertex {
            position: [x1, y1, half_height],
            normal,
            uv: [u1, 1.0],
        });
        vertices.push(MeshVertex {
            position: [x0, y0, half_height],
            normal,
            uv: [u0, 1.0],
        });
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    push_cap_fan(&mut vertices, &mut indices, radius, half_height, 1.0);
    push_cap_fan(&mut vertices, &mut indices, radius, -half_height, -1.0);

    MeshData { vertices, indices }
}

/// Appends a triangle-fan cap (a centre vertex plus [`PILLAR_SIDES`] rim vertices) at `z`, normal
/// `(0, 0, normal_z)`. Used for both the top (`normal_z = 1.0`) and bottom (`normal_z = -1.0`)
/// caps of [`octagonal_pillar`].
fn push_cap_fan(
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    radius: f32,
    z: f32,
    normal_z: f32,
) {
    let normal = [0.0, 0.0, normal_z];
    let center = u32::try_from(vertices.len()).expect("test meshes stay far below u32::MAX");
    vertices.push(MeshVertex {
        position: [0.0, 0.0, z],
        normal,
        uv: [0.5, 0.5],
    });
    for side in 0..PILLAR_SIDES {
        let angle = side as f32 / PILLAR_SIDES as f32 * TAU;
        vertices.push(MeshVertex {
            position: [radius * angle.cos(), radius * angle.sin(), z],
            normal,
            uv: [0.5 + 0.5 * angle.cos(), 0.5 + 0.5 * angle.sin()],
        });
    }
    for side in 0..PILLAR_SIDES {
        let a = center + 1 + side;
        let b = center + 1 + (side + 1) % PILLAR_SIDES;
        indices.extend_from_slice(&[center, a, b]);
    }
}

/// A rectangular box ("altar block"), centred on the origin, `width` (X) x `depth` (Y) x `height`
/// (Z), with one flat-shaded quad per face (no shared vertices between faces, so every face has
/// its own exact normal).
///
/// Vertex count: `24` (6 faces x 4). Index count: `36` (6 faces x 6).
#[must_use]
pub fn altar_block(width: f32, depth: f32, height: f32) -> MeshData {
    let (hx, hy, hz) = (width * 0.5, depth * 0.5, height * 0.5);
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);

    push_quad(
        &mut vertices,
        &mut indices,
        [[-hx, -hy, hz], [hx, -hy, hz], [hx, hy, hz], [-hx, hy, hz]],
        [0.0, 0.0, 1.0],
    );
    push_quad(
        &mut vertices,
        &mut indices,
        [
            [-hx, hy, -hz],
            [hx, hy, -hz],
            [hx, -hy, -hz],
            [-hx, -hy, -hz],
        ],
        [0.0, 0.0, -1.0],
    );
    push_quad(
        &mut vertices,
        &mut indices,
        [[hx, -hy, -hz], [hx, hy, -hz], [hx, hy, hz], [hx, -hy, hz]],
        [1.0, 0.0, 0.0],
    );
    push_quad(
        &mut vertices,
        &mut indices,
        [
            [-hx, hy, -hz],
            [-hx, -hy, -hz],
            [-hx, -hy, hz],
            [-hx, hy, hz],
        ],
        [-1.0, 0.0, 0.0],
    );
    push_quad(
        &mut vertices,
        &mut indices,
        [[hx, hy, -hz], [-hx, hy, -hz], [-hx, hy, hz], [hx, hy, hz]],
        [0.0, 1.0, 0.0],
    );
    push_quad(
        &mut vertices,
        &mut indices,
        [
            [-hx, -hy, -hz],
            [hx, -hy, -hz],
            [hx, -hy, hz],
            [-hx, -hy, hz],
        ],
        [0.0, -1.0, 0.0],
    );

    MeshData { vertices, indices }
}

/// Appends one smooth-shaded latitude ring of `radial_segments` vertices at spherical angle `phi`
/// (radians from the equator, positive up), sphere radius `radius`, offset by `z_offset` along Z.
/// Returns the index of the ring's first vertex.
fn push_ring(
    vertices: &mut Vec<MeshVertex>,
    radial_segments: u32,
    phi: f32,
    radius: f32,
    z_offset: f32,
) -> u32 {
    let start = u32::try_from(vertices.len()).expect("test meshes stay far below u32::MAX");
    let ring_radius = radius * phi.cos();
    let z = z_offset + radius * phi.sin();
    for j in 0..radial_segments {
        let theta = j as f32 / radial_segments as f32 * TAU;
        let (cos_t, sin_t) = (theta.cos(), theta.sin());
        vertices.push(MeshVertex {
            position: [ring_radius * cos_t, ring_radius * sin_t, z],
            normal: [phi.cos() * cos_t, phi.cos() * sin_t, phi.sin()],
            uv: [
                j as f32 / radial_segments as f32,
                0.5 + phi / std::f32::consts::PI,
            ],
        });
    }
    start
}

/// Connects two consecutive rings of `radial_segments` vertices each (starting at `ring_a` and
/// `ring_b`) into a strip of quads.
fn push_ring_strip(indices: &mut Vec<u32>, radial_segments: u32, ring_a: u32, ring_b: u32) {
    for j in 0..radial_segments {
        let next = (j + 1) % radial_segments;
        let a0 = ring_a + j;
        let a1 = ring_a + next;
        let b0 = ring_b + j;
        let b1 = ring_b + next;
        indices.extend_from_slice(&[a0, a1, b1, a0, b1, b0]);
    }
}

/// A capsule (a cylinder capped by two hemispheres), standing along `Z`, centred on the origin: a
/// stand-in for a humanoid actor's silhouette. `radial_segments` (clamped to at least 3) is the
/// number of vertices around the circumference; `cap_rings` (clamped to at least 1) is the number
/// of latitude steps per hemisphere, excluding the pole and the equator.
///
/// Vertex count: `2 + radial_segments * (2 * cap_rings + 2)`. Index count:
/// `radial_segments * 6 + radial_segments * 6 * (2 * cap_rings + 1)` (two pole fans plus
/// `2 * cap_rings + 1` quad strips between consecutive rings).
#[must_use]
pub fn capsule_actor(
    radius: f32,
    cylinder_height: f32,
    radial_segments: u32,
    cap_rings: u32,
) -> MeshData {
    let radial_segments = radial_segments.max(3);
    let cap_rings = cap_rings.max(1);
    let half_height = cylinder_height * 0.5;
    let half_pi = std::f32::consts::FRAC_PI_2;

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let bottom_pole = u32::try_from(vertices.len()).expect("test meshes stay far below u32::MAX");
    vertices.push(MeshVertex {
        position: [0.0, 0.0, -half_height - radius],
        normal: [0.0, 0.0, -1.0],
        uv: [0.5, 0.0],
    });

    let mut rings = Vec::with_capacity((2 * cap_rings + 2) as usize);
    for i in 1..=cap_rings {
        let phi = -half_pi + i as f32 / (cap_rings + 1) as f32 * half_pi;
        rings.push(push_ring(
            &mut vertices,
            radial_segments,
            phi,
            radius,
            -half_height,
        ));
    }
    rings.push(push_ring(
        &mut vertices,
        radial_segments,
        0.0,
        radius,
        -half_height,
    ));
    rings.push(push_ring(
        &mut vertices,
        radial_segments,
        0.0,
        radius,
        half_height,
    ));
    for i in 1..=cap_rings {
        let phi = i as f32 / (cap_rings + 1) as f32 * half_pi;
        rings.push(push_ring(
            &mut vertices,
            radial_segments,
            phi,
            radius,
            half_height,
        ));
    }

    let top_pole = u32::try_from(vertices.len()).expect("test meshes stay far below u32::MAX");
    vertices.push(MeshVertex {
        position: [0.0, 0.0, half_height + radius],
        normal: [0.0, 0.0, 1.0],
        uv: [0.5, 1.0],
    });

    for j in 0..radial_segments {
        let next = (j + 1) % radial_segments;
        indices.extend_from_slice(&[bottom_pole, rings[0] + j, rings[0] + next]);
    }
    for pair in rings.windows(2) {
        push_ring_strip(&mut indices, radial_segments, pair[0], pair[1]);
    }
    let last_ring = *rings
        .last()
        .expect("cap_rings >= 1, at least 4 rings pushed");
    for j in 0..radial_segments {
        let next = (j + 1) % radial_segments;
        indices.extend_from_slice(&[top_pole, last_ring + next, last_ring + j]);
    }

    MeshData { vertices, indices }
}

/// Base icosahedron vertices (unnormalised), the standard construction from the golden ratio.
fn icosahedron_positions() -> [[f32; 3]; 12] {
    let t = (1.0 + 5.0_f32.sqrt()) * 0.5;
    [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
}

/// Base icosahedron faces (indices into [`icosahedron_positions`]).
const ICOSAHEDRON_FACES: [[u32; 3]; 20] = [
    [0, 11, 5],
    [0, 5, 1],
    [0, 1, 7],
    [0, 7, 10],
    [0, 10, 11],
    [1, 5, 9],
    [5, 11, 4],
    [11, 10, 2],
    [10, 7, 6],
    [7, 1, 8],
    [3, 9, 4],
    [3, 4, 2],
    [3, 2, 6],
    [3, 6, 8],
    [3, 8, 9],
    [4, 9, 5],
    [2, 4, 11],
    [6, 2, 10],
    [8, 6, 7],
    [9, 8, 1],
];

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}

/// Returns the (cached) index of the midpoint of the unit-sphere positions `a` and `b`,
/// normalised back onto the sphere; creates and caches it on first use so a subdivided icosphere
/// shares vertices between adjacent triangles instead of duplicating every edge.
fn midpoint(
    positions: &mut Vec<[f32; 3]>,
    cache: &mut std::collections::HashMap<(u32, u32), u32>,
    a: u32,
    b: u32,
) -> u32 {
    let key = if a < b { (a, b) } else { (b, a) };
    if let Some(&index) = cache.get(&key) {
        return index;
    }
    let (pa, pb) = (positions[a as usize], positions[b as usize]);
    let mid = normalize3([
        (pa[0] + pb[0]) * 0.5,
        (pa[1] + pb[1]) * 0.5,
        (pa[2] + pb[2]) * 0.5,
    ]);
    let index = u32::try_from(positions.len()).expect("test meshes stay far below u32::MAX");
    positions.push(mid);
    cache.insert(key, index);
    index
}

/// An icosphere (a subdivided icosahedron projected onto a sphere), `radius` world units,
/// centred on the origin. `subdivisions = 0` is the bare icosahedron (12 vertices, 20 faces);
/// each further subdivision quadruples the face count.
///
/// Vertex count: `10 * 4^subdivisions + 2`. Index count: `20 * 4^subdivisions * 3`.
#[must_use]
pub fn icosphere(subdivisions: u32, radius: f32) -> MeshData {
    let mut positions: Vec<[f32; 3]> = icosahedron_positions()
        .into_iter()
        .map(normalize3)
        .collect();
    let mut faces: Vec<[u32; 3]> = ICOSAHEDRON_FACES.to_vec();

    for _ in 0..subdivisions {
        let mut cache = std::collections::HashMap::new();
        let mut next_faces = Vec::with_capacity(faces.len() * 4);
        for face in &faces {
            let [a, b, c] = *face;
            let ab = midpoint(&mut positions, &mut cache, a, b);
            let bc = midpoint(&mut positions, &mut cache, b, c);
            let ca = midpoint(&mut positions, &mut cache, c, a);
            next_faces.push([a, ab, ca]);
            next_faces.push([b, bc, ab]);
            next_faces.push([c, ca, bc]);
            next_faces.push([ab, bc, ca]);
        }
        faces = next_faces;
    }

    let vertices = positions
        .iter()
        .map(|&position| MeshVertex {
            position: [
                position[0] * radius,
                position[1] * radius,
                position[2] * radius,
            ],
            normal: position,
            uv: [
                0.5 + position[0].atan2(position[2]) / TAU,
                0.5 - position[1].asin() / std::f32::consts::PI,
            ],
        })
        .collect();
    let indices = faces.into_iter().flatten().collect();
    MeshData { vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_unit_normals(mesh: &MeshData, tolerance: f32) {
        for (index, vertex) in mesh.vertices.iter().enumerate() {
            let length_squared = vertex.normal.iter().map(|c| c * c).sum::<f32>();
            assert!(
                (length_squared.sqrt() - 1.0).abs() < tolerance,
                "vertex {index} normal {:?} is not unit length (|n| = {})",
                vertex.normal,
                length_squared.sqrt()
            );
        }
    }

    fn assert_finite(mesh: &MeshData) {
        for (index, vertex) in mesh.vertices.iter().enumerate() {
            assert!(
                vertex.position.iter().all(|c| c.is_finite()),
                "vertex {index} has a non-finite position: {:?}",
                vertex.position
            );
            assert!(
                vertex.normal.iter().all(|c| c.is_finite()),
                "vertex {index} has a non-finite normal: {:?}",
                vertex.normal
            );
            assert!(
                vertex.uv.iter().all(|c| c.is_finite()),
                "vertex {index} has a non-finite uv: {:?}",
                vertex.uv
            );
        }
    }

    #[test]
    fn floor_tile_grid_has_expected_counts_and_validates() {
        let mesh = floor_tile_grid(4, 2.0);
        assert_eq!(mesh.vertices.len(), 25); // (4+1)^2
        assert_eq!(mesh.indices.len(), 96); // 4*4*6
        assert_finite(&mesh);
        assert_unit_normals(&mesh, 1e-6);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn floor_tile_grid_clamps_zero_tiles_to_one() {
        let mesh = floor_tile_grid(0, 2.0);
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn octagonal_pillar_has_expected_counts_and_validates() {
        let mesh = octagonal_pillar(1.0, 3.0);
        assert_eq!(mesh.vertices.len(), 8 * 4 + 9 + 9);
        assert_eq!(mesh.indices.len(), 8 * 6 + 8 * 3 + 8 * 3);
        assert_finite(&mesh);
        assert_unit_normals(&mesh, 1e-5);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn altar_block_is_24_vertices_36_indices_and_validates() {
        let mesh = altar_block(2.0, 1.5, 3.0);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_finite(&mesh);
        assert_unit_normals(&mesh, 1e-6);
        assert_eq!(mesh.validate(), Ok(()));
        // Every face is exactly at the box's half extent along its own axis.
        for vertex in &mesh.vertices {
            assert!(vertex.position[0].abs() <= 1.000_1);
            assert!(vertex.position[1].abs() <= 0.750_1);
            assert!(vertex.position[2].abs() <= 1.500_1);
        }
    }

    #[test]
    fn capsule_actor_has_expected_counts_and_validates() {
        let radial_segments = 6;
        let cap_rings = 2;
        let mesh = capsule_actor(0.5, 2.0, radial_segments, cap_rings);
        let expected_vertices = 2 + radial_segments * (2 * cap_rings + 2);
        let ring_gaps = 2 * cap_rings + 1;
        let expected_indices = radial_segments * 6 + radial_segments * 6 * ring_gaps;
        assert_eq!(mesh.vertices.len(), expected_vertices as usize);
        assert_eq!(mesh.indices.len(), expected_indices as usize);
        assert_finite(&mesh);
        assert_unit_normals(&mesh, 1e-5);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn capsule_actor_clamps_degenerate_segment_counts() {
        // Zero segments/rings would otherwise divide by zero; clamped to the smallest usable
        // capsule instead of producing NaN or an empty mesh.
        let mesh = capsule_actor(0.5, 1.0, 0, 0);
        assert_finite(&mesh);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn icosphere_base_case_is_the_icosahedron() {
        let mesh = icosphere(0, 2.0);
        assert_eq!(mesh.vertices.len(), 12);
        assert_eq!(mesh.indices.len(), 60); // 20 faces * 3
        assert_finite(&mesh);
        assert_unit_normals(&mesh, 1e-6);
        for vertex in &mesh.vertices {
            let radius = vertex.position.iter().map(|c| c * c).sum::<f32>().sqrt();
            assert!((radius - 2.0).abs() < 1e-4, "radius {radius}");
        }
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn icosphere_subdivision_quadruples_faces() {
        let mesh = icosphere(1, 1.0);
        assert_eq!(mesh.vertices.len(), 42); // 10*4^1+2
        assert_eq!(mesh.indices.len(), 240); // 20*4^1*3
        assert_unit_normals(&mesh, 1e-5);
        assert_eq!(mesh.validate(), Ok(()));

        let mesh2 = icosphere(2, 1.0);
        assert_eq!(mesh2.vertices.len(), 162); // 10*4^2+2
        assert_eq!(mesh2.indices.len(), 960); // 20*4^2*3
        assert_eq!(mesh2.validate(), Ok(()));
    }

    #[test]
    fn icosphere_is_deterministic() {
        assert_eq!(icosphere(2, 3.0), icosphere(2, 3.0));
    }

    #[test]
    fn every_generator_produces_a_valid_mesh() {
        assert_eq!(floor_tile_grid(3, 1.0).validate(), Ok(()));
        assert_eq!(octagonal_pillar(0.5, 2.0).validate(), Ok(()));
        assert_eq!(capsule_actor(0.4, 1.2, 8, 3).validate(), Ok(()));
        assert_eq!(altar_block(1.0, 1.0, 1.0).validate(), Ok(()));
        assert_eq!(icosphere(1, 0.5).validate(), Ok(()));
    }
}
