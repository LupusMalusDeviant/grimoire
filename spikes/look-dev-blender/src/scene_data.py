"""Seeded, bpy-free scene description (geometry parameters, lights, projectiles).

One numpy PCG64 stream with the design seed, consumed in a fixed order:
floor, props, enemies, lights, projectiles (bolts, calm bullets, busy bullets).
Cluster lights are derived from the bullets afterwards and consume no RNG.
Coordinates: X right, Y forward (screen up), Z up, gameplay plane Z = 0.
Light intensities are in design units (see common.UNIT_DEFAULTS).
"""
import math

import numpy as np

from common import SEED, FLOOR_EXTENT, MASK_RES, DECAL

TAU = 2.0 * math.pi
CAMERA = dict(target=(0.0, 1.5, 0.0), distance=34.5, pitch_deg=65.0, vfov_deg=35.0,
              clip=(0.5, 100.0))
NOISE_AMP = 0.08
KERB = dict(r_in=13.6, r_top=15.0, r_bevel=15.25, r_apron=16.7, z_top=0.05, z_low=-0.20)
PLAYER_POS = (0.0, -3.0)
IMPS = [(-3, 5), (3, 4.5), (-8, 1), (8, 0), (-4, 10), (5, 11)]
BRUTES = [(-6, 7), (7, 5)]
BRUTE = dict(radii=(1.1, 0.9, 1.2), center_z=1.2, hunch_deg=20.0)
SHAFT_APOTHEM = 0.7 * math.cos(math.pi / 8)


def smoothstep(e0, e1, x):
    t = np.clip((np.asarray(x, dtype=np.float64) - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def camera_eye():
    tx, ty, tz = CAMERA["target"]
    p = math.radians(CAMERA["pitch_deg"])
    d = CAMERA["distance"]
    return (tx, ty - d * math.cos(p), tz + d * math.sin(p))


# ------------------------------------------------------------------ floor ---
def noise_at(lattice, x, y):
    """Smooth value noise on a 9x9 lattice with 5-unit spacing over the floor."""
    u = np.clip((np.asarray(x) + FLOOR_EXTENT) / 5.0, 0.0, 7.999)
    v = np.clip((np.asarray(y) + FLOOR_EXTENT) / 5.0, 0.0, 7.999)
    i, j = np.floor(u).astype(int), np.floor(v).astype(int)
    fu, fv = smoothstep(0, 1, u - i), smoothstep(0, 1, v - j)
    a = lattice[j, i] * (1 - fu) + lattice[j, i + 1] * fu
    b = lattice[j + 1, i] * (1 - fu) + lattice[j + 1, i + 1] * fu
    return a * (1 - fv) + b * fv


def floor_height(lattice, x, y):
    r = np.hypot(x, y)
    return NOISE_AMP * noise_at(lattice, x, y) * (1.0 - smoothstep(11.0, 13.0, r))


def gen_floor(rng):
    lattice = rng.uniform(-1.0, 1.0, size=(9, 9))
    c = -19.5 + np.arange(40)
    cx, cy = [a.ravel() for a in np.meshgrid(c, c)]
    u = rng.uniform(size=(cx.size, 3))
    rc = np.hypot(cx, cy)
    tiles = []
    for n in range(cx.size):
        if rc[n] <= 14.35:
            level, noise = 0.0, float(floor_height(lattice, cx[n], cy[n]))
        elif rc[n] >= 15.96:
            level, noise = -0.25, 0.0
        else:
            continue
        tiles.append(dict(x=float(cx[n]), y=float(cy[n]), level=level,
                          z_top=level + noise + (u[n, 0] * 2 - 1) * 0.03,
                          tilt=(u[n, 1] * 2 - 1) * math.radians(1.5),
                          tilt_axis=u[n, 2] * TAU))
    return dict(lattice=lattice, tiles=tiles, kerb=KERB, tile_size=0.92, chamfer=0.03)


# ------------------------------------------------------------------ props ---
def plinth_distance(px, py, pillar):
    """Distance from points to a pillar's rotated 2x2 plinth square."""
    ca, sa = math.cos(-pillar["yaw"]), math.sin(-pillar["yaw"])
    qx = (px - pillar["x"]) * ca - (py - pillar["y"]) * sa
    qy = (px - pillar["x"]) * sa + (py - pillar["y"]) * ca
    dx, dy = np.maximum(np.abs(qx) - 1.0, 0), np.maximum(np.abs(qy) - 1.0, 0)
    return np.hypot(dx, dy)


def wall_arc_points(cx, cy, length=5.0, n=41):
    rad, phi = math.hypot(cx, cy), math.atan2(cy, cx)
    s = np.linspace(0.0, length, n)
    ang = phi + (s - length / 2) / rad
    return rad * np.cos(ang), rad * np.sin(ang)


def place_wall(pillars):
    """Smallest shift of the wall centre from (-9, 8) that clears plinths by 1 unit."""
    best = None
    offs = np.arange(-35, 36) * 0.1
    for oy in offs:
        for ox in offs:
            shift = math.hypot(ox, oy)
            if shift > 3.5 or (best is not None and shift >= best[0] - 1e-9):
                continue
            cx, cy = -9.0 + ox, 8.0 + oy
            px, py = wall_arc_points(cx, cy)
            ok = np.hypot(px, py).max() < 14.0
            for p in pillars:
                ok = ok and plinth_distance(px, py, p).min() - 0.3 >= 1.0
            for fx, fy, fr in [(b[0], b[1], 1.1) for b in BRUTES] + [(i[0], i[1], 0.3) for i in IMPS]:
                ok = ok and (np.hypot(px - fx, py - fy).min() - 0.3 - fr) >= 0.5
            if ok:
                best = (shift, cx, cy)
    return best


def gen_props(rng):
    pillars = []
    for k in range(6):
        a = math.radians(30 + 60 * k)
        broken = k in (1, 3)
        height = {1: 2.2, 3: 1.4}.get(k, 4.5)
        jag = rng.uniform(-0.25, 0.25, size=8)
        pillars.append(dict(angle_deg=30 + 60 * k, x=12.5 * math.cos(a), y=12.5 * math.sin(a),
                            yaw=a + math.pi, height=height, broken=broken,
                            jag=(jag if broken else np.zeros(8)).tolist(),
                            sconce_z=min(3.6, height - 0.5),
                            moss=bool(rng.uniform() < 0.5)))
    rubble = []
    for p in pillars:
        vals = rng.uniform(size=(8, 6))
        if not p["broken"]:
            continue
        for v in vals:
            ang, dist = v[3] * TAU, 1.6 + 1.2 * v[4]
            size = (0.3 + 0.5 * v[0], 0.3 + 0.5 * v[1], 0.3 + 0.5 * v[2])
            rubble.append(dict(x=p["x"] + dist * math.cos(ang), y=p["y"] + dist * math.sin(ang),
                               size=size, yaw=v[5] * TAU))
    shift, wcx, wcy = place_wall(pillars)
    segments = [(0.0, 1.5), (2.0, 3.1), (3.7, 5.0)]
    columns = []
    for s0, s1 in segments:
        s = np.round(np.arange(s0, s1 + 1e-6, 0.25), 4)
        if s[-1] < s1 - 1e-6:
            s = np.append(s, s1)
        top = 2.2 - rng.uniform(0.0, 0.5, size=(s.size, 2))
        for idx, sv in enumerate(s):
            near_gap = (sv - s0 < 0.3 and s0 > 0) or (s1 - sv < 0.3 and s1 < 5.0)
            if near_gap:
                top[idx] -= 0.55
        columns.append(dict(s=s.tolist(), top=top.tolist()))
    wall_moss = (rng.uniform(size=64) < 0.4).tolist()
    # Moved from the spec's (10, -6) to (8.5, -4) with the engine spike: the spec
    # position cuts through the plinth of the 330-degree pillar.
    toppled = dict(x=8.5, y=-4.0, length=4.0, radius=0.6, yaw=math.radians(20.0),
                   end_jag=rng.uniform(-0.2, 0.2, size=16).tolist())
    return dict(pillars=pillars, rubble=rubble, toppled=toppled,
                wall=dict(center=(wcx, wcy), shift=shift, length=5.0, thickness=0.6,
                          height=2.2, segments=segments, columns=columns, moss=wall_moss),
                altar=dict(x=0.0, y=6.0, size=(2.4, 1.2, 1.0)),
                candles=[dict(x=-0.8, y=6.0, r=0.06, h=0.25), dict(x=0.8, y=6.0, r=0.06, h=0.25)],
                braziers=[dict(x=-2.2, y=6.0, bowl_r=0.35, bowl_z=0.9),
                          dict(x=2.2, y=6.0, bowl_r=0.35, bowl_z=0.9)])


# ---------------------------------------------------------------- figures ---
def face_yaw(x, y):
    return math.atan2(PLAYER_POS[1] - y, PLAYER_POS[0] - x) - math.pi / 2


def local_to_world(x, y, yaw, p):
    c, s = math.cos(yaw), math.sin(yaw)
    return (x + p[0] * c - p[1] * s, y + p[0] * s + p[1] * c, p[2])


def brute_local(p):
    """Brute body-local point (relative to the body centre, before hunch) -> figure-local."""
    t = math.radians(-BRUTE["hunch_deg"])
    y = p[1] * math.cos(t) - p[2] * math.sin(t)
    z = p[1] * math.sin(t) + p[2] * math.cos(t)
    return (p[0], y, z + BRUTE["center_z"])


def gen_figures(rng):
    player = dict(kind="player", x=PLAYER_POS[0], y=PLAYER_POS[1], yaw=0.0, footprint=0.55,
                  figure_id=1)
    player["orb"] = local_to_world(player["x"], player["y"], 0.0, (0.45, 0.1, 1.79))
    splay = rng.uniform(0.4, 0.6, size=len(IMPS))
    imps = []
    for n, (x, y) in enumerate(IMPS):
        yaw = face_yaw(x, y)
        imps.append(dict(kind="imp", x=x, y=y, yaw=yaw, footprint=0.3, figure_id=2 + n,
                         horn_splay=float(splay[n]),
                         eyes=[local_to_world(x, y, yaw, (sx, 0.27, 0.7)) for sx in (-0.1, 0.1)]))
    brutes = []
    for n, (x, y) in enumerate(BRUTES):
        yaw = face_yaw(x, y)
        brutes.append(dict(kind="brute", x=x, y=y, yaw=yaw, footprint=1.1, figure_id=8 + n,
                           eyes=[local_to_world(x, y, yaw, brute_local((sx, 0.62, 0.85)))
                                 for sx in (-0.28, 0.28)],
                           eye_light=local_to_world(x, y, yaw, brute_local((0.0, 1.2, 0.8)))))
    figures = [player] + imps + brutes
    return dict(player=player, imps=imps, brutes=brutes,
                blobs=[dict(x=f["x"], y=f["y"], radius=1.2 * f["footprint"]) for f in figures])


# ----------------------------------------------------------------- lights ---
def blender_light_placement(pos, radius):
    """Emulate the design falloff 1/(1 + d^2) with Eevee's 1/d^2 on the floor.

    A point light at height h above the floor is placed at h' = sqrt(h^2 + 1), so a
    floor point at horizontal distance x receives 1/(h^2 + x^2 + 1), exactly the
    design's distance term; the cutoff becomes sqrt(r^2 + 1). Without this, low
    lights (candles, rune stones, cluster lights) burn singular hot spots.
    """
    x, y, z = pos
    floor_z = 0.0 if math.hypot(x, y) < KERB["r_top"] else KERB["z_low"]
    h = max(z - floor_z, 0.0)
    return (x, y, floor_z + math.sqrt(h * h + 1.0)), math.sqrt(radius * radius + 1.0)


def light(name, pos, color, intensity, radius, shadow=False):
    return dict(name=name, pos=[float(v) for v in pos], color=color, intensity=intensity,
                radius=radius, shadow=shadow)


def gen_lights(rng, sc):
    calm, torches = [], []
    for k, p in enumerate(sc["pillars"]):
        inward = (-math.cos(math.radians(p["angle_deg"])), -math.sin(math.radians(p["angle_deg"])))
        reach = SHAFT_APOTHEM + 0.35
        base = (p["x"] + inward[0] * reach, p["y"] + inward[1] * reach, p["sconce_z"])
        torches.append(dict(kind="sconce", base=base, pillar=k))
    for b in sc["braziers"]:
        torches.append(dict(kind="brazier", base=(b["x"], b["y"], b["bowl_z"] + 0.02)))
    for n, t in enumerate(torches):
        bx, by, bz = t["base"]
        calm.append(light("torch_%d" % n, (bx, by, bz + 0.15), "#FF9A4A", 6.0, 9.0, True))
    for k in range(4):
        a = math.radians(45 + 90 * k)
        calm.append(light("ritual_%d" % k, (5 * math.cos(a), 5 * math.sin(a), 0.6), "#8A5CFF", 2.5, 6.0))
    for n, c in enumerate(sc["candles"]):
        calm.append(light("candle_%d" % n, (c["x"], c["y"], 1.0 + c["h"] + 0.13), "#FFC07A", 1.2, 3.0))
    calm.append(light("staff_orb", sc["player"]["orb"], "#7FE3FF", 2.0, 5.0, True))
    for n, b in enumerate(sc["brutes"]):
        calm.append(light("brute_eye_%d" % n, b["eye_light"], "#FF6A2A", 1.5, 4.0))
    tp = sc["toppled"]
    # z 0.25 instead of floor contact: with Eevee's 1/d^2 falloff a light at z 0.08
    # would burn a singular hot spot into the floor.
    calm.append(light("ember_crack", (tp["x"], tp["y"], 0.25), "#FF5A1F", 1.2, 4.0))
    extra = []
    jit = rng.uniform(-0.02, 0.02, size=64)
    for k in range(64):
        a = TAU * k / 64 + jit[k]
        extra.append(light("edge_candle_%d" % k, (16 * math.cos(a), 16 * math.sin(a), 0.1), "#FFB066", 0.8, 3.0))
    for k in range(48):
        a = TAU * k / 48
        extra.append(light("rune_%d" % k, (6.2 * math.cos(a), 6.2 * math.sin(a), 0.3), "#8A5CFF", 0.6, 2.5))
    ev = rng.uniform(size=(32, 3))
    for k in range(32):
        p = sc["pillars"][k % 6]
        a, d = ev[k, 0] * TAU, 1.0 + 1.5 * ev[k, 1]
        extra.append(light("ember_%d" % k, (p["x"] + d * math.cos(a), p["y"] + d * math.sin(a),
                                            1.0 + 2.0 * ev[k, 2]), "#FF7A3A", 0.7, 3.0))
    return torches, calm, extra


# ------------------------------------------------------------ projectiles ---
def _pack(lst):
    arr = np.array(lst, dtype=np.float64).reshape(-1, 6)   # x, y, vx, vy, sil, pal
    return arr


def ring(c, radius, n, phase, pal):
    a = phase + TAU * np.arange(n) / n
    return [(c[0] + radius * math.cos(v), c[1] + radius * math.sin(v), math.cos(v), math.sin(v), 0, pal) for v in a]


def spiral(c, arms, n, r0, dr, dtheta, phase, pal, swirl=0.6):
    out = []
    for arm in range(arms):
        for i in range(n):
            a = phase + TAU * arm / arms + i * dtheta
            r = r0 + i * dr
            out.append((c[0] + r * math.cos(a), c[1] + r * math.sin(a),
                        math.cos(a + swirl), math.sin(a + swirl), 1, pal))
    return out


def fans(ndir, rows, spread_deg, d0, dd, pal_fn):
    out = []
    for j, (ix, iy) in enumerate(IMPS):
        base = math.atan2(PLAYER_POS[1] - iy, PLAYER_POS[0] - ix)
        for row in range(rows):
            for k in range(ndir):
                a = base + math.radians(-spread_deg + 2 * spread_deg * k / (ndir - 1))
                d = d0 + dd * row
                out.append((ix + d * math.cos(a), iy + d * math.sin(a), math.cos(a), math.sin(a), 2, pal_fn(j, row)))
    return out


def stress(rng, sc, n_torch, n_ritual, n_player, n_pillar):
    eye = camera_eye()
    u = rng.uniform(size=(n_torch + n_ritual + n_player + n_pillar, 5))
    out, i = [], 0
    bx, by = sc["braziers"][1]["x"], sc["braziers"][1]["y"]   # brightest pool: low brazier light
    tall = [p for p in sc["pillars"] if not p["broken"] and p["y"] > -9.0]
    for group, count in enumerate((n_torch, n_ritual, n_player, n_pillar)):
        for _ in range(count):
            a = u[i, 0] * TAU
            if group == 0:
                r = 2.2 * math.sqrt(u[i, 1]); x, y = bx + r * math.cos(a), by + r * math.sin(a)
            elif group == 1:
                r = 3.7 + 2.4 * u[i, 1]; x, y = r * math.cos(a), r * math.sin(a)
            elif group == 2:
                r = 0.7 + 1.2 * u[i, 1]; x, y = PLAYER_POS[0] + r * math.cos(a), PLAYER_POS[1] + r * math.sin(a)
            else:
                p = tall[i % len(tall)]
                dx, dy = p["x"] - eye[0], p["y"] - eye[1]
                n = math.hypot(dx, dy); s = 1.0 + 1.6 * u[i, 1]
                x, y = p["x"] + dx / n * s, p["y"] + dy / n * s
            va = u[i, 2] * TAU
            out.append((x, y, math.cos(va), math.sin(va), int(u[i, 3] * 3) % 3, int(u[i, 4] * 2) % 2))
            i += 1
    return out


def gen_projectiles(rng, sc):
    orb = np.array(sc["player"]["orb"])
    targets = [(f["x"], f["y"], 0.55) for f in sc["imps"]] + [(f["x"], f["y"], 1.4) for f in sc["brutes"]]
    ub = rng.uniform(size=(40, 2))
    bolts = []
    for i in range(40):
        tgt = np.array(targets[i % 8])
        d = tgt - orb
        t = 0.15 + 0.7 * ub[i, 0]
        side = np.array([-d[1], d[0], 0.0]) / max(1e-6, math.hypot(d[0], d[1])) * (ub[i, 1] - 0.5) * 0.3
        pos = orb + d * t + side
        bolts.append(dict(pos=pos.tolist(), dir=(d / np.linalg.norm(d)).tolist()))
    ph = rng.uniform(0, TAU, size=8)
    calm = ring(BRUTES[0], 3.5, 36, ph[0], 0)
    calm += spiral(BRUTES[1], 3, 12, 1.4, 0.42, 0.32, ph[1], 1)
    calm += fans(5, 3, 28.0, 1.0, 0.8, lambda j, row: j % 2)
    calm += stress(rng, sc, 6, 6, 3, 3)
    ph = rng.uniform(0, TAU, size=16)
    busy = []
    for b, c in enumerate(BRUTES):
        for k, radius in enumerate(np.linspace(2.0, 9.0, 4)):
            busy += ring(c, radius, 48, ph[b * 4 + k], (b + k) % 2)
    busy += spiral(BRUTES[1], 3, 120, 1.2, 0.07, 0.09, ph[8], 1)
    busy += spiral(BRUTES[0], 3, 120, 1.2, 0.07, 0.09, ph[9], 0)
    busy += fans(10, 8, 40.0, 0.9, 0.55, lambda j, row: (j + row) % 2)
    busy += stress(rng, sc, 20, 20, 10, 10)
    ud = rng.uniform(size=(356, 4))
    for v in ud:
        a = v[2] * TAU
        busy.append((-17 + 34 * v[0], -9 + 24 * v[1], math.cos(a), math.sin(a), int(v[3] * 3) % 3, int(v[3] * 6) % 2))
    return bolts, _pack(calm), _pack(busy)


def kmeans(pts, k, iters=30):
    cent = pts[np.linspace(0, len(pts) - 1, k).astype(int)].copy()
    for _ in range(iters):
        lab = np.argmin(((pts[:, None, :] - cent[None]) ** 2).sum(-1), axis=1)
        for c in range(k):
            m = lab == c
            if m.any():
                cent[c] = pts[m].mean(axis=0)
    return cent, lab


def cluster_lights(bullets, k, intensity, radius, prefix):
    cent, lab = kmeans(bullets[:, :2], k)
    out = []
    for c in range(k):
        pal = bullets[lab == c, 5]
        body = "#FF2FB4" if (pal.size == 0 or np.mean(pal) <= 0.5) else "#B6FF2E"
        out.append(light("%s_%d" % (prefix, c), (cent[c, 0], cent[c, 1], 0.5), body, intensity, radius))
    return out


def build(seed=SEED):
    rng = np.random.Generator(np.random.PCG64(seed))
    sc = dict(seed=seed, camera=dict(CAMERA, eye=camera_eye()))
    sc["floor"] = gen_floor(rng)
    sc.update(gen_props(rng))
    sc.update(gen_figures(rng))
    sc["torches"], calm_lights, busy_extra = gen_lights(rng, sc)
    sc["bolts"], sc["bullets_calm"], sc["bullets_busy"] = gen_projectiles(rng, sc)
    calm_lights += cluster_lights(sc["bullets_calm"], 6, 1.5, 5.0, "cluster_calm")
    busy_extra = cluster_lights(sc["bullets_busy"], 64, 1.0, 4.0, "cluster_busy") + busy_extra
    sc["lights_calm"] = calm_lights
    sc["lights_busy"] = calm_lights + busy_extra
    return sc


# ------------------------------------------------------------- floor mask ---
def floor_mask(sc, res=MASK_RES):
    """RGB uint8 mask over the 40x40 floor, row 0 at +Y. R: engraving, G: blob factor."""
    px = 2.0 * FLOOR_EXTENT / res
    mask = np.zeros((res, res, 3), np.uint8)
    mask[..., 1] = 255

    def grid(x0, x1, y0, y1):
        i0, i1 = int((x0 + FLOOR_EXTENT) / px), int(math.ceil((x1 + FLOOR_EXTENT) / px))
        r0, r1 = int((FLOOR_EXTENT - y1) / px), int(math.ceil((FLOOR_EXTENT - y0) / px))
        xs = -FLOOR_EXTENT + (np.arange(i0, i1) + 0.5) * px
        ys = FLOOR_EXTENT - (np.arange(r0, r1) + 0.5) * px
        return (slice(r0, r1), slice(i0, i1)), np.meshgrid(xs, ys)

    sl, (x, y) = grid(-6.3, 6.3, -6.3, 6.3)
    r = np.hypot(x, y)
    d = np.minimum(np.abs(r - 5.8) - 0.2, np.abs(r - 3.9) - 0.1)
    verts = [(3.9 * math.cos(math.radians(90 + 360 * k / 7)), 3.9 * math.sin(math.radians(90 + 360 * k / 7)))
             for k in range(7)]
    for k in range(7):
        (ax, ay), (bx, by) = verts[k], verts[(k + 3) % 7]
        ex, ey = bx - ax, by - ay
        t = np.clip(((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey), 0, 1)
        d = np.minimum(d, np.hypot(x - ax - t * ex, y - ay - t * ey) - 0.04)
    for k in range(12):
        a = math.radians(15 + 30 * k)
        qx = (x * math.cos(a) + y * math.sin(a)) - 4.8
        qy = -x * math.sin(a) + y * math.cos(a)
        ox, oy = np.abs(qx) - 0.125, np.abs(qy) - 0.04
        dr = np.hypot(np.maximum(ox, 0), np.maximum(oy, 0)) + np.minimum(np.maximum(ox, oy), 0)
        d = np.minimum(d, dr)
    mask[sl + (0,)] = np.round(np.clip(0.5 - d / px, 0, 1) * 255).astype(np.uint8)
    for b in sc["blobs"]:
        rb = b["radius"]
        sl, (x, y) = grid(b["x"] - rb, b["x"] + rb, b["y"] - rb, b["y"] + rb)
        f = 0.55 + 0.45 * smoothstep(0.0, rb, np.hypot(x - b["x"], y - b["y"]))
        cur = mask[sl + (1,)].astype(np.float64) / 255.0
        mask[sl + (1,)] = np.round(cur * f * 255).astype(np.uint8)
    return mask


def blob_factor(sc, x, y):
    f = np.ones_like(np.asarray(x, dtype=np.float64))
    for b in sc["blobs"]:
        f *= 0.55 + 0.45 * smoothstep(0.0, b["radius"], np.hypot(x - b["x"], y - b["y"]))
    return f


def to_jsonable(o):
    if isinstance(o, dict):
        return {k: to_jsonable(v) for k, v in o.items()}
    if isinstance(o, (list, tuple)):
        return [to_jsonable(v) for v in o]
    if isinstance(o, np.ndarray):
        return o.tolist()
    if isinstance(o, (np.floating,)):
        return float(o)
    if isinstance(o, (np.integer,)):
        return int(o)
    if isinstance(o, np.bool_):
        return bool(o)
    return o


if __name__ == "__main__":
    s = build()
    print("tiles", len(s["floor"]["tiles"]), "rubble", len(s["rubble"]), "wall", s["wall"]["center"],
          "shift %.2f" % s["wall"]["shift"], "lights", len(s["lights_calm"]), len(s["lights_busy"]),
          "bullets", len(s["bullets_calm"]), len(s["bullets_busy"]), "bolts", len(s["bolts"]),
          "decal", DECAL["calm"])
