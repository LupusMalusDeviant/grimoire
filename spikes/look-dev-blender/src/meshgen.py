"""Numpy mesh generators (no bpy). Each returns a Mesh with verts (Nx3),
faces (list of index tuples, counter-clockwise seen from outside) and a
per-face material slot list."""
import math

import numpy as np


class Mesh:
    def __init__(self, verts=None, faces=None, mats=None):
        self.verts = np.zeros((0, 3)) if verts is None else np.asarray(verts, dtype=np.float64)
        self.faces = [] if faces is None else [tuple(int(i) for i in f) for f in faces]
        self.mats = [0] * len(self.faces) if mats is None else list(mats)

    def add(self, other, mat=None):
        off = len(self.verts)
        self.verts = np.vstack([self.verts, other.verts])
        self.faces += [tuple(i + off for i in f) for f in other.faces]
        self.mats += other.mats if mat is None else [mat] * len(other.faces)
        return self

    def transformed(self, rot=None, trans=(0, 0, 0)):
        v = self.verts if rot is None else self.verts @ np.asarray(rot).T
        return Mesh(v + np.asarray(trans), self.faces, self.mats)

    def with_mat(self, mat):
        return Mesh(self.verts, self.faces, [mat] * len(self.faces))


def rot_z(a):
    c, s = math.cos(a), math.sin(a)
    return np.array([[c, -s, 0], [s, c, 0], [0, 0, 1]])


def rot_x(a):
    c, s = math.cos(a), math.sin(a)
    return np.array([[1, 0, 0], [0, c, -s], [0, s, c]])


def rot_y(a):
    c, s = math.cos(a), math.sin(a)
    return np.array([[c, 0, s], [0, 1, 0], [-s, 0, c]])


def rot_axis(axis, a):
    axis = np.asarray(axis, dtype=np.float64)
    axis = axis / np.linalg.norm(axis)
    k = np.array([[0, -axis[2], axis[1]], [axis[2], 0, -axis[0]], [-axis[1], axis[0], 0]])
    return np.eye(3) + math.sin(a) * k + (1 - math.cos(a)) * (k @ k)


def rot_z_to(direction):
    """Rotation that maps +Z onto the given direction."""
    d = np.asarray(direction, dtype=np.float64)
    d = d / np.linalg.norm(d)
    z = np.array([0.0, 0.0, 1.0])
    c = float(np.dot(z, d))
    if c > 0.999999:
        return np.eye(3)
    if c < -0.999999:
        return rot_x(math.pi)
    return rot_axis(np.cross(z, d), math.acos(c))


def lathe(profile, segments=16, phase=0.0):
    """Surface of revolution around Z. profile: [(r, z), ...]; r == 0 makes a pole.
    Traverse the profile so that the outside is on the right when walking
    upward (bottom-outside first) to get outward normals."""
    verts, rings = [], []
    ang = phase + 2 * math.pi * np.arange(segments) / segments
    for r, z in profile:
        if r <= 1e-9:
            rings.append([len(verts)])
            verts.append((0.0, 0.0, z))
        else:
            idx = list(range(len(verts), len(verts) + segments))
            verts += [(r * math.cos(a), r * math.sin(a), z) for a in ang]
            rings.append(idx)
    faces = []
    for a, b in zip(rings[:-1], rings[1:]):
        for j in range(segments):
            k = (j + 1) % segments
            if len(a) == 1 and len(b) == 1:
                continue
            if len(a) == 1:
                faces.append((a[0], b[k], b[j]))
            elif len(b) == 1:
                faces.append((a[j], a[k], b[0]))
            else:
                faces.append((a[j], a[k], b[k], b[j]))
    return Mesh(verts, faces)


def sphere_profile(radius, rings=8, z_center=0.0, scale_z=1.0):
    return [(radius * math.sin(t), z_center - radius * scale_z * math.cos(t))
            for t in np.linspace(0, math.pi, rings + 1)]


def ellipsoid(radii, segments=20, rings=12):
    m = lathe(sphere_profile(1.0, rings), segments)
    return Mesh(m.verts * np.asarray(radii), m.faces)


def capsule(radius, z0, z1, segments=16, rings=6):
    lo = [(radius * math.sin(t), z0 + radius - radius * math.cos(t)) for t in np.linspace(0, math.pi / 2, rings + 1)]
    hi = [(radius * math.cos(t), z1 - radius + radius * math.sin(t)) for t in np.linspace(0, math.pi / 2, rings + 1)]
    return lathe(lo + hi[1:], segments)


def cylinder_between(p0, p1, radius, segments=8):
    p0, p1 = np.asarray(p0, dtype=np.float64), np.asarray(p1, dtype=np.float64)
    length = float(np.linalg.norm(p1 - p0))
    m = lathe([(0, 0), (radius, 0), (radius, length), (0, length)], segments)
    return m.transformed(rot_z_to(p1 - p0), p0)


def cone_between(p0, p1, radius, segments=10):
    p0, p1 = np.asarray(p0, dtype=np.float64), np.asarray(p1, dtype=np.float64)
    length = float(np.linalg.norm(p1 - p0))
    m = lathe([(0, 0), (radius, 0), (0, length)], segments)
    return m.transformed(rot_z_to(p1 - p0), p0)


def teardrop(radius, height, segments=12):
    """Two cones: a short one below the widest ring and a tall one above."""
    return lathe([(0, 0), (radius, 0.3 * height), (0, height)], segments)


def box(hx, hy, hz):
    v = [(-hx, -hy, -hz), (hx, -hy, -hz), (hx, hy, -hz), (-hx, hy, -hz),
         (-hx, -hy, hz), (hx, -hy, hz), (hx, hy, hz), (-hx, hy, hz)]
    f = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    return Mesh(v, f)


def chamfer_box(hx, hy, z_bottom, z_top, chamfer, bottom_face=False):
    """Box with a chamfered top edge ring (flat facets). Face 0 is the top."""
    c = chamfer
    sq = [(-1, -1), (1, -1), (1, 1), (-1, 1)]
    v = [(sx * (hx - c), sy * (hy - c), z_top) for sx, sy in sq]
    v += [(sx * hx, sy * hy, z_top - c) for sx, sy in sq]
    v += [(sx * hx, sy * hy, z_bottom) for sx, sy in sq]
    f = [(0, 1, 2, 3)]
    for j in range(4):
        k = (j + 1) % 4
        f.append((4 + j, 4 + k, k, j))
        f.append((8 + j, 8 + k, 4 + k, 4 + j))
    if bottom_face:
        f.append((8, 11, 10, 9))
    return Mesh(v, f)


def prism(sides, radius, z0, z1, top_jag=None, bottom_jag=None, phase=0.0, top_cap=True, bottom_cap=False):
    """Regular prism around Z with optional per-vertex jag on the end rings."""
    ang = phase + 2 * math.pi * np.arange(sides) / sides
    tj = np.zeros(sides) if top_jag is None else np.asarray(top_jag)
    bj = np.zeros(sides) if bottom_jag is None else np.asarray(bottom_jag)
    v = [(radius * math.cos(a), radius * math.sin(a), z0 + bj[i]) for i, a in enumerate(ang)]
    v += [(radius * math.cos(a), radius * math.sin(a), z1 + tj[i]) for i, a in enumerate(ang)]
    f = [(j, (j + 1) % sides, sides + (j + 1) % sides, sides + j) for j in range(sides)]
    if top_cap:
        v.append((0, 0, z1 + float(np.mean(tj)) - 0.05 * (top_jag is not None)))
        c = len(v) - 1
        f += [(sides + j, sides + (j + 1) % sides, c) for j in range(sides)]
    if bottom_cap:
        v.append((0, 0, z0 + float(np.mean(bj)) + 0.05 * (bottom_jag is not None)))
        c = len(v) - 1
        f += [((j + 1) % sides, j, c) for j in range(sides)]
    return Mesh(v, f)


def grid(xs, ys, zfunc):
    X, Y = np.meshgrid(xs, ys)
    Z = zfunc(X, Y)
    v = np.stack([X.ravel(), Y.ravel(), Z.ravel()], axis=1)
    nx = len(xs)
    f = []
    for j in range(len(ys) - 1):
        for i in range(nx - 1):
            a = j * nx + i
            f.append((a, a + 1, a + nx + 1, a + nx))
    return Mesh(v, f)


def tiles(tile_list, size, chamfer, depth=0.25):
    """All floor tiles merged: chamfered blocks, tilted about a random horizontal axis."""
    h = size / 2.0
    tpl = chamfer_box(h, h, -depth, 0.0, chamfer)
    n = len(tile_list)
    nv = len(tpl.verts)
    verts = np.empty((n * nv, 3))
    faces = []
    for t, tile in enumerate(tile_list):
        ax = (math.cos(tile["tilt_axis"]), math.sin(tile["tilt_axis"]), 0.0)
        r = rot_axis(ax, tile["tilt"])
        verts[t * nv:(t + 1) * nv] = tpl.verts @ r.T + (tile["x"], tile["y"], tile["z_top"])
        faces += [tuple(i + t * nv for i in f) for f in tpl.faces]
    return Mesh(verts, faces)


def wall_arc(center, length, thickness, columns, moss_flags, z_bottom=-0.15):
    """Arc wall tangential to the arena centre, built from jagged column profiles.
    Material slots: 0 wall, 1 moss (hashed on top faces)."""
    cx, cy = center
    rad, phi = math.hypot(cx, cy), math.atan2(cy, cx)
    m = Mesh()
    moss_i = 0
    for seg in columns:
        v, f, mats = [], [], []
        for c, (s, (t_in, t_out)) in enumerate(zip(seg["s"], seg["top"])):
            a = phi + (s - length / 2) / rad
            ri, ro = rad - thickness / 2, rad + thickness / 2
            ca, sa = math.cos(a), math.sin(a)
            v += [(ri * ca, ri * sa, z_bottom), (ri * ca, ri * sa, t_in),
                  (ro * ca, ro * sa, t_out), (ro * ca, ro * sa, z_bottom)]
        ncol = len(seg["s"])
        for c in range(ncol - 1):
            ib, it, ot, ob = 4 * c, 4 * c + 1, 4 * c + 2, 4 * c + 3
            ib2, it2, ot2, ob2 = ib + 4, it + 4, ot + 4, ob + 4
            f += [(ob, ob2, ot2, ot), (ib2, ib, it, it2), (ot, ot2, it2, it)]
            mats += [0, 0, 1 if moss_flags[moss_i % len(moss_flags)] else 0]
            moss_i += 1
        last = 4 * (ncol - 1)
        f += [(3, 2, 1, 0), (last, last + 1, last + 2, last + 3)]
        mats += [0, 0]
        m.add(Mesh(v, f, mats))
    return m
