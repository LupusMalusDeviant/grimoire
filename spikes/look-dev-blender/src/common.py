"""Shared constants and helpers for the look comparison. No bpy dependency.

All colours are sRGB hex values from the design palette and are converted to
linear exactly once here. Look parameters also live here, so compose.py can
list them in the metrics report without importing Blender.
"""
import json
import math
import os
import struct
import zlib

import numpy as np

SEED = 0x5EEDF1E9
WIDTH, HEIGHT = 1280, 720
FLOOR_EXTENT = 20.0          # floor spans [-20, 20] in X and Y
MASK_RES = 4096              # shared floor mask resolution
CROP_RECT = (320, 300, 960, 660)  # x0, y0, x1, y1 in pixels, y from the top

# Class ids written to Object.pass_index.
CLASS_FLOOR, CLASS_PROP, CLASS_PLAYER, CLASS_ENEMY, CLASS_EMISSIVE = 0, 1, 2, 3, 4

LUMA = np.array([0.2126, 0.7152, 0.0722])


def hex_to_srgb(h):
    h = h.lstrip("#")
    return np.array([int(h[i:i + 2], 16) / 255.0 for i in (0, 2, 4)])


def srgb_to_linear(c):
    c = np.asarray(c, dtype=np.float64)
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(c):
    c = np.clip(np.asarray(c, dtype=np.float64), 0.0, 1.0)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * np.power(c, 1.0 / 2.4) - 0.055)


def hex_lin(h):
    return tuple(float(v) for v in srgb_to_linear(hex_to_srgb(h)))


def luminance(rgb_lin):
    return np.tensordot(np.asarray(rgb_lin, dtype=np.float64), LUMA, axes=([-1], [0]))


def wcag_ratio(l_a, l_b):
    hi = np.maximum(l_a, l_b)
    lo = np.minimum(l_a, l_b)
    return (hi + 0.05) / (lo + 0.05)


def srgb8_to_linear(arr_u8):
    """uint8 sRGB array -> float64 linear array (exact LUT)."""
    lut = srgb_to_linear(np.arange(256) / 255.0)
    return lut[arr_u8]


def linear_to_srgb8(arr_lin):
    return np.round(linear_to_srgb(arr_lin) * 255.0).astype(np.uint8)


# ---------------------------------------------------------------- palette ---
VOID = "#06070A"
AMBIENT = {"sky": "#1C2438", "ground": "#110D0B", "intensity": 0.35}
MOON = {"dir": (-0.35, 0.5, -0.8), "color": "#9AB0D8", "intensity": 0.35,
        "angle_deg": 2.0}

# Base albedo, identical in all looks, plus the look-specific material values.
# stylized: shadow tint S, Blinn-Phong gloss g, spec strength ks, bronze spec
# tinted by albedo. realistic: roughness r, metalness m. rim: figure family.
MATERIALS = {
    "floor_stone": dict(albedo="#33363D", tint="#3A3F66", g=12, ks=0.06, r=0.85, m=0.0),
    "grout":       dict(albedo="#1A1C21", tint="#3A3F66", g=12, ks=0.06, r=0.95, m=0.0),
    "pillar":      dict(albedo="#3E4047", tint="#3A3F66", g=12, ks=0.06, r=0.75, m=0.0),
    "plinth":      dict(albedo="#2C2E34", tint="#3A3F66", g=12, ks=0.06, r=0.85, m=0.0),
    "ruin_wall":   dict(albedo="#383A3F", tint="#3A3F66", g=12, ks=0.06, r=0.85, m=0.0),
    "moss":        dict(albedo="#2E3A2A", tint="#3A3F66", g=12, ks=0.06, r=0.85, m=0.0),
    "rubble":      dict(albedo="#45464A", tint="#3A3F66", g=12, ks=0.06, r=0.85, m=0.0),
    "altar":       dict(albedo="#2A2528", tint="#2A1830", g=20, ks=0.10, r=0.60, m=0.0),
    "bronze":      dict(albedo="#6B4A2A", tint="#5A3A20", g=32, ks=0.50, r=0.35, m=1.0,
                        spec_albedo=True),
    "wax":         dict(albedo="#D8CDB4", tint="#3A3F66", g=12, ks=0.06, r=0.85, m=0.0),
    "cloak":       dict(albedo="#24505C", tint="#1E2A4A", g=6, ks=0.03, r=0.90, m=0.0,
                        rim="player"),
    "hood_inside": dict(albedo="#0A0C0E", tint="#1E2A4A", g=6, ks=0.03, r=0.90, m=0.0,
                        rim="player"),
    "mask":        dict(albedo="#CFC3A8", tint="#1E2A4A", g=16, ks=0.08, r=0.60, m=0.0,
                        rim="player"),
    "staff_wood":  dict(albedo="#4A3524", tint="#1E2A4A", g=10, ks=0.05, r=0.70, m=0.0,
                        rim="player"),
    "imp_skin":    dict(albedo="#7A7068", tint="#4A3550", g=20, ks=0.12, r=0.55, m=0.0,
                        rim="imp"),
    "horn":        dict(albedo="#C9BBA0", tint="#4A3550", g=16, ks=0.08, r=0.60, m=0.0,
                        rim="imp"),
    "brute_flesh": dict(albedo="#5E3530", tint="#3A1F3A", g=18, ks=0.10, r=0.60, m=0.0,
                        rim="brute"),
    "plates":      dict(albedo="#3B3A3E", tint="#3A1F3A", g=40, ks=0.30, r=0.40, m=0.0,
                        rim="brute"),
}
FLOOR_MATERIALS = ("floor_stone", "grout")

RIM = {
    "player": dict(color="#A8E6FF", p=3.0, k=0.55),
    "imp":    dict(color="#FF9A6A", p=2.5, k=0.35),
    "brute":  dict(color="#FF9A6A", p=2.5, k=0.40),
}

# Emissive meshes, shared by all looks: body colour, HDR multiplier, optional core.
EMISSIVE = {
    "em_torch":  dict(color="#FFB25A", mult=6.0, core="#FFE2B0"),
    "em_candle": dict(color="#FFC07A", mult=4.0, core=None),
    "em_eye":    dict(color="#FF6A2A", mult=3.0, core=None),
    "em_orb":    dict(color="#7FE3FF", mult=4.0, core=None),
    "em_bolt":   dict(color="#8FE8FF", mult=3.0, core="#FFFFFF", alpha=0.75),
}
DECAL = dict(color="#7A4CFF", calm=0.35, busy=0.6, engrave_albedo=0.55)

HOSTILE = [dict(name="H0", body="#FF2FB4", core="#FFE3F4"),
           dict(name="H1", body="#B6FF2E", core="#F6FFE0")]
BULLET_RIM = dict(color="#0A0510", alpha=0.9, width_px=1.5)
BULLET_SIZES = {0: (0.22, 0.22), 1: (0.14, 0.32), 2: (0.26, 0.26)}  # orb, rice, diamond
BULLET_GLOW = dict(value=200, radius_mult=2.5, gain=0.35)
BULLET_Z = 0.5
MARKER = dict(radius=0.8, width=0.05, color="#BFF3FF", alpha=0.8)

# ------------------------------------------------------- light conventions ---
# Design units: pixel = albedo * E with E = I * atten for point lights and
# E = I * N.L for the moon. Blender units per design unit are derived from the
# renderer's convention (k_*: pixel value for a white Lambert surface per unit of
# Blender light). light_scale is the one global factor applied to all design
# intensities (points, moon, ambient); it is chosen so that the realistic look's
# calibrated light gain comes out at 1.0. Emission is never scaled by it.
UNIT_DEFAULTS = dict(
    k_point=1.0 / (4.0 * math.pi ** 2),  # pixel at 1 m per Watt (theory, probe checks)
    k_sun=1.0 / math.pi,                 # pixel per unit sun strength
    k_world=1.0,                         # pixel per unit uniform world radiance
    light_scale=5.5,                     # first estimate, replaced by calibration
)
POINT_SOFT_RADIUS = 0.1
# Follow-up variant "busy_dim": busy with all bullet-cluster lights (calm 6 + busy 64)
# at this fraction of their design intensity. Same gains and bullet layer as busy.
BUSY_DIM_CLUSTER_FACTOR = 0.25

# ------------------------------------------------------------ look params ---
# Irradiance thresholds are in design units (multiplied by light_scale in the
# shader), so they stay valid when the global light factor changes.
LOOK_PARAMS = {
    "toon": dict(
        band_levels=(0.0, 0.45, 1.0),
        t1=0.03, t2=0.22,          # direct-luminance thresholds (design units)
        edge_rel=0.04,             # smoothstep half width relative to threshold (near pixel-width AA)
        band_full=0.27,            # lighting value of the top band (design units)
        hue_clamp=8.0,
        outline_color="#0B0A0D",
        outline_thickness=0.04,    # world units, about 1.3 px at the target
        outline_thin_scale=0.5,    # staff, candles, tripod legs
    ),
    "stylized": dict(
        wrap=0.45,
        wrap_ref=0.15,             # moon irradiance at N.L = 1 (design units)
        tint_smooth=(0.0, 0.6),
        spec_gain=1.0,             # Glossy colour = ks * gain * specColor
        rim=RIM,
    ),
    "realistic": dict(rough_min=0.25),
}

POST = dict(
    view_transform="Khronos PBR Neutral", look="None",
    compositor_exposure=0.0,     # neutral for all looks; calibration is a light gain
    bloom_threshold=1.0, bloom_smoothness=0.5, bloom_strength=0.08,
    bloom_size=0.1, bloom_quality="High",
    samples_final=32, samples_crop=1, filter_final=1.5,
    # Calibration target: median floor luminance of the calm world-only image as a
    # DISPLAY value (sRGB-encoded after the view transform), i.e. linear ~0.027.
    target_display_median=0.18, calib_tolerance=0.05, calib_exclude_radius=6.2,
    calib_erode_px=2,
)


def pbr_neutral(rgb):
    """Khronos PBR Neutral tonemapper (reference formula), rgb: (..., 3) linear."""
    c = np.array(rgb, dtype=np.float64, copy=True)
    start, desat = 0.8 - 0.04, 0.15
    x = c.min(axis=-1, keepdims=True)
    offset = np.where(x < 0.08, x - 6.25 * x * x, 0.04)
    c -= offset
    peak = c.max(axis=-1, keepdims=True)
    d = 1.0 - start
    new_peak = 1.0 - d * d / (peak + d - start)
    comp = c * (new_peak / np.maximum(peak, 1e-12))
    g = 1.0 - 1.0 / (desat * (peak - new_peak) + 1.0)
    comp = comp * (1.0 - g) + new_peak * g
    return np.where(peak < start, c, comp)


def display_luminance(rgb_lin):
    """sRGB-encoded luminance of the tonemapped colour (the calibration measure)."""
    return linear_to_srgb(luminance(pbr_neutral(rgb_lin)))


def decode_classmask(arr):
    """Class-mask render (R = pass_index*16 + figure_id + 1, G/B = world XY in [0, 1])."""
    v = np.round(arr[..., 0]).astype(np.int64)
    bg = v < 1
    return dict(bg=bg, cls=np.where(bg, -1, (v - 1) // 16), fid=np.where(bg, 0, (v - 1) % 16),
                x=arr[..., 1] * 2 * FLOOR_EXTENT - FLOOR_EXTENT,
                y=arr[..., 2] * 2 * FLOOR_EXTENT - FLOOR_EXTENT)


def erode(mask, px):
    out = mask.copy()
    for _ in range(px):
        o = out.copy()
        o[1:, :] &= out[:-1, :]
        o[:-1, :] &= out[1:, :]
        o[:, 1:] &= out[:, :-1]
        o[:, :-1] &= out[:, 1:]
        out = o
    return out


def dilate(mask, px):
    out = mask.copy()
    for i in range(px):
        o = out.copy()
        o[1:, :] |= out[:-1, :]
        o[:-1, :] |= out[1:, :]
        o[:, 1:] |= out[:, :-1]
        o[:, :-1] |= out[:, 1:]
        if i % 2 == 1:   # alternate 4- and 8-neighbourhoods for a rounder footprint
            o[1:, 1:] |= out[:-1, :-1]
            o[:-1, :-1] |= out[1:, 1:]
            o[1:, :-1] |= out[:-1, 1:]
            o[:-1, 1:] |= out[1:, :-1]
        out = o
    return out


def blob_factor(blobs, x, y):
    f = np.ones_like(np.asarray(x, dtype=np.float64))
    for b in blobs:
        t = np.clip(np.hypot(x - b["x"], y - b["y"]) / b["radius"], 0.0, 1.0)
        f *= 0.55 + 0.45 * t * t * (3.0 - 2.0 * t)
    return f


def calibration_floor_selection(mask_arr, scene_info):
    """Class-0 floor pixels outside r = 6.2 around the circle centre and outside
    all blob shadows, eroded by a few pixels away from other classes."""
    d = decode_classmask(mask_arr)
    floor = erode((d["cls"] == 0) & ~d["bg"], POST["calib_erode_px"])
    far = np.hypot(d["x"], d["y"]) > POST["calib_exclude_radius"]
    no_blob = blob_factor(scene_info["blobs"], d["x"], d["y"]) >= 0.999
    return floor & far & no_blob


def gloss_to_roughness(g):
    """Blinn-Phong exponent -> Beckmann-style alpha -> Blender roughness (sqrt alpha)."""
    alpha = math.sqrt(2.0 / (g + 2.0))
    return math.sqrt(alpha)


def load_params_override(path):
    """Merge an optional tuning override JSON into LOOK_PARAMS (returns a copy)."""
    params = json.loads(json.dumps(LOOK_PARAMS))
    if path and os.path.exists(path):
        with open(path, "r", encoding="utf-8") as fh:
            over = json.load(fh)
        for look, vals in over.items():
            params[look].update(vals)
    return params


# ------------------------------------------------------------------- I/O ---
def write_png(path, arr):
    """Deterministic 8-bit PNG writer (filter 0, fixed zlib level). arr: HxWx{3,4} uint8."""
    arr = np.ascontiguousarray(arr, dtype=np.uint8)
    h, w, c = arr.shape
    color_type = {3: 2, 4: 6}[c]
    raw = np.concatenate([np.zeros((h, 1), np.uint8), arr.reshape(h, w * c)], axis=1)
    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, color_type, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw.tobytes(), 6))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as fh:
        fh.write(png)


def save_json(path, obj):
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(obj, fh, indent=1, sort_keys=True)


def load_json(path):
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)
