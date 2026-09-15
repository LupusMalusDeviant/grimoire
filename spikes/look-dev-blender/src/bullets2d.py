"""Shared hostile-bullet and player-marker layer (numpy only, no bpy).

The layer is drawn ONCE per variant from scene.json, in linear light with
premultiplied alpha: additive glow halos for all bullets first, then the bodies
in index order (dark rim, saturated body, white-hot core), then the player
marker ring. compose.py alpha-composites this same layer over every look's
tonemapped world image, so bullets are identical in all looks by construction.

Usage: python bullets2d.py --work <work-dir> --out <out-dir>
Reads  <work>/scene.json
Writes <work>/bullets_layer_<variant>.npy, <work>/bullets_<variant>.json,
       <out>/bullets_only_busy.png
"""
import argparse
import hashlib
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common as C  # noqa: E402

W, H = C.WIDTH, C.HEIGHT


def project(cam, pts):
    m = np.array(cam["proj"]) @ np.array(cam["view"])
    ph = np.c_[pts, np.ones(len(pts))] @ m.T
    ndc = ph[:, :3] / ph[:, 3:4]
    return (ndc[:, 0] * 0.5 + 0.5) * W, (0.5 - ndc[:, 1] * 0.5) * H


def sd_ellipse(qx, qy, a, b):
    """Approximate signed distance to an ellipse with semi-axes a (x) and b (y)."""
    k0 = np.hypot(qx / a, qy / b)
    k1 = np.maximum(np.hypot(qx / (a * a), qy / (b * b)), 1e-9)
    return k0 * (k0 - 1.0) / k1


def sd_rhombus(qx, qy, a, b):
    """Signed distance to a rhombus with half-diagonals a (x) and b (y)."""
    px, py = np.abs(qx), np.abs(qy)
    h = np.clip((-2.0 * (px * a - py * b) + (a * a - b * b)) / (a * a + b * b), -1.0, 1.0)
    d = np.hypot(px - 0.5 * a * (1.0 - h), py - 0.5 * b * (1.0 + h))
    return d * np.sign(px * b + py * a - a * b)


def smoothstep(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def over(layer, y0, y1, x0, x1, rgb, alpha):
    """Premultiplied 'over' of (rgb, alpha) onto a layer window."""
    win = layer[y0:y1, x0:x1]
    a = alpha[..., None]
    win[..., :3] = rgb * a + (1.0 - a) * win[..., :3]
    win[..., 3:] = a + (1.0 - a) * win[..., 3:]


def window(cx, cy, radius):
    x0, x1 = max(0, int(math.floor(cx - radius))), min(W, int(math.ceil(cx + radius)) + 1)
    y0, y1 = max(0, int(math.floor(cy - radius))), min(H, int(math.ceil(cy + radius)) + 1)
    if x0 >= x1 or y0 >= y1:
        return None
    xs = np.arange(x0, x1) + 0.5 - cx
    ys = np.arange(y0, y1) + 0.5 - cy
    gx, gy = np.meshgrid(xs, ys)
    return (y0, y1, x0, x1), gx, gy


def draw_layer(sc, variant):
    cam = sc["camera"]
    b = np.asarray(sc["bullets_" + variant], dtype=np.float64)
    rejected = int(np.sum(~np.isin(b[:, 5], (0, 1))))      # HOSTILE palette space only
    b = b[np.isin(b[:, 5], (0, 1))]
    n = len(b)
    centre = np.c_[b[:, :2], np.full(n, C.BULLET_Z)]
    px, py = project(cam, centre)
    rx, ry = project(cam, centre + np.asarray(cam["right"]))
    ppu = np.hypot(rx - px, ry - py)
    vx, vy = project(cam, centre + np.c_[b[:, 2:4] * 0.5, np.zeros(n)])
    dx, dy = vx - px, vy - py
    dn = np.maximum(np.hypot(dx, dy), 1e-9)
    dx, dy = dx / dn, dy / dn

    layer = np.zeros((H, W, 4), dtype=np.float64)
    pal = [dict(body=np.array(C.hex_lin(p["body"])), core=np.array(C.hex_lin(p["core"]))) for p in C.HOSTILE]
    rim_lin = np.array(C.hex_lin(C.BULLET_RIM["color"]))
    glow_gain = C.BULLET_GLOW["value"] / 255.0 * C.BULLET_GLOW["gain"]
    records = []
    bound = np.array([max(C.BULLET_SIZES[int(s)]) for s in b[:, 4]]) * ppu
    for i in range(n):                                     # 1) additive glow halos
        gr = C.BULLET_GLOW["radius_mult"] * bound[i]
        win = window(px[i], py[i], gr + 1)
        if win is None:
            continue
        (y0, y1, x0, x1), gx, gy = win
        t = np.clip(np.hypot(gx, gy) / gr, 0.0, 1.0)
        layer[y0:y1, x0:x1, :3] += ((1.0 - t) ** 3 * glow_gain)[..., None] * pal[int(b[i, 5])]["body"]
    rim_w = C.BULLET_RIM["width_px"]
    for i in range(n):                                     # 2) bodies in index order
        sil, p = int(b[i, 4]), pal[int(b[i, 5])]
        a, bb = (v * ppu[i] for v in C.BULLET_SIZES[sil])
        visible = 0 <= px[i] < W and 0 <= py[i] < H
        records.append(dict(px=float(px[i]), py=float(py[i]), r_px=float(bound[i]), silhouette=sil,
                            palette=int(b[i, 5]), visible=bool(visible)))
        win = window(px[i], py[i], bound[i] + 2)
        if win is None:
            continue
        (y0, y1, x0, x1), gx, gy = win
        qx = gx * -dy[i] + gy * dx[i]       # across the velocity
        qy = gx * dx[i] + gy * dy[i]        # along the velocity
        if sil == 0:
            sdf = np.hypot(qx, qy) - a
        elif sil == 1:
            sdf = sd_ellipse(qx, qy, a, bb)
        else:
            sdf = sd_rhombus(qx, qy, a, bb)
        cov_out = np.clip(0.5 - sdf, 0.0, 1.0)
        cov_in = np.clip(0.5 - (sdf + rim_w), 0.0, 1.0)
        core = smoothstep(0.35, 0.8, np.clip(-sdf / min(a, bb), 0.0, 1.0))[..., None]
        body = p["body"] * (1.0 - core) + p["core"] * core
        frac = (cov_in / np.maximum(cov_out, 1e-9))[..., None]
        rgb = rim_lin * (1.0 - frac) + body * frac
        alpha = cov_out * (C.BULLET_RIM["alpha"] * (1.0 - frac[..., 0]) + frac[..., 0])
        over(layer, y0, y1, x0, x1, rgb, alpha)
    pl = sc["player"]                                       # 3) player marker ring
    mc = np.array([[pl["x"], pl["y"], C.BULLET_Z]])
    mx, my = project(cam, mc)
    mrx, mry = project(cam, mc + np.asarray(cam["right"]))
    mppu = float(np.hypot(mrx - mx, mry - my)[0])
    radius, half = C.MARKER["radius"] * mppu, 0.5 * C.MARKER["width"] * mppu
    win = window(float(mx[0]), float(my[0]), radius + half + 2)
    (y0, y1, x0, x1), gx, gy = win
    cov = np.clip(0.5 - (np.abs(np.hypot(gx, gy) - radius) - half), 0.0, 1.0)
    over(layer, y0, y1, x0, x1, np.array(C.hex_lin(C.MARKER["color"])), cov * C.MARKER["alpha"])
    return layer.astype(np.float32), records, rejected


def straight_rgba8(layer):
    """Premultiplied linear layer -> straight-alpha sRGB PNG for viewing on transparency.
    Additive glow (colour without alpha) is shown with alpha = its brightest channel."""
    rgb = layer[..., :3].astype(np.float64)
    alpha = np.clip(np.maximum(layer[..., 3], rgb.max(axis=-1)), 0.0, 1.0)
    straight = rgb / np.maximum(alpha, 1e-9)[..., None]
    out = np.zeros(layer.shape, np.uint8)
    out[..., :3] = C.linear_to_srgb8(straight)
    out[..., 3] = np.round(alpha * 255.0).astype(np.uint8)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    sc = C.load_json(os.path.join(args.work, "scene.json"))
    os.makedirs(args.out, exist_ok=True)
    for variant in ("calm", "busy"):
        layer, records, rejected = draw_layer(sc, variant)
        np.save(os.path.join(args.work, "bullets_layer_%s.npy" % variant), layer)
        digest = hashlib.sha256(layer.tobytes()).hexdigest()
        C.save_json(os.path.join(args.work, "bullets_%s.json" % variant),
                    dict(variant=variant, count=len(records), visible=sum(r["visible"] for r in records),
                         bullets_rejected_palette_space=rejected, layer_sha256=digest, bullets=records))
        if variant == "busy":
            C.write_png(os.path.join(args.out, "bullets_only_busy.png"), straight_rgba8(layer))
        print("BULLETS OK", variant, len(records), "visible", sum(r["visible"] for r in records),
              "rejected", rejected, "sha256", digest[:16])


if __name__ == "__main__":
    main()
