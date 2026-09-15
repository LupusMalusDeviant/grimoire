"""Composites and metrics comparing "realistic" (flat) against "realistic_tex"
(procedural PBR textures), on top of the existing look-comparison spike.

Reuses compose.py's helpers (composite, half_scale, grid, bullet_contrast,
figure_contrast, figure_regions, fmt) instead of re-implementing them, and
extends the existing out/metrics.json / out/metrics.md that compose.py's own
run already wrote (compose.py's own LOOKS tuple only knows toon/stylized/
realistic, so this stays a separate script rather than a rewrite of it).

No bpy (same contract as compose.py): run with a plain Python that has numpy,
e.g. Blender's bundled interpreter (see the look-spike-blender README).

Usage: python compose_textures.py --work <work-dir> --out <out-dir>

Needs from work/: scene.json, classmask.npy, bullets_layer_{calm,busy}.npy,
bullets_{calm,busy}.json, realistic_{calm,busy_dim}_world.npy (existing),
realistic_tex_{calm,busy_dim}_world.npy (new), realistic_busy_1x_world_crop.npy
(existing, busy scene), realistic_tex_busy_1x_world_crop.npy (new, busy_dim
scene -- see the run report for why the two crops come from different scene
variants). Needs out/metrics.json (written by compose.py) to extend.
Writes to out/: realistic_tex_{calm,busy_dim}.png, realistic_tex_busy_1x_crop.png,
composite_textures.png, composite_textures_crops.png; updates metrics.json,
appends a section to metrics.md.
"""
import argparse
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common as C  # noqa: E402
import compose as CP  # noqa: E402

LOOKS2 = ("realistic", "realistic_tex")
VARIANTS2 = ("calm", "busy_dim")

# Mirrors textures.MATERIAL_TEXTURE_MAP / UNMAPPED_MATERIALS. Duplicated (not
# imported) because textures.py needs bpy and this script must not.
MATERIAL_TEXTURE_MAP = {
    "floor_stone": "floor_tiles", "grout": "grout", "pillar": "pillar_stone",
    "plinth": "pillar_stone", "rubble": "pillar_stone", "ruin_wall": "ruin_wall_moss",
    "moss": "ruin_wall_moss", "altar": "altar_basalt", "bronze": "bronze", "wax": "candle_wax",
    "cloak": "cloak_fabric", "mask": "bone_mask", "staff_wood": "staff_wood",
    "imp_skin": "imp_skin", "brute_flesh": "brute_flesh", "plates": "shoulder_plates",
}
UNMAPPED_MATERIALS = ("hood_inside", "horn")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    wk = lambda name: os.path.join(args.work, name)  # noqa: E731
    ot = lambda name: os.path.join(args.out, name)  # noqa: E731

    sc = CP.load(wk("scene.json"))
    mask = CP.load(wk("classmask.npy"))
    layers = {v: CP.load(wk("bullets_layer_%s.npy" % v)).astype(np.float64) for v in ("calm", "busy")}
    brec = {v: CP.load(wk("bullets_%s.json" % v)) for v in ("calm", "busy")}
    regions = CP.figure_regions(mask, sc)
    sel = C.calibration_floor_selection(mask, sc)
    x0, y0, x1, y1 = C.CROP_RECT

    finals, bullet_contrast, figure_contrast, floor_median = {}, {}, {}, {}
    for look in LOOKS2:
        bullet_contrast[look], figure_contrast[look], floor_median[look] = {}, {}, {}
        for var in VARIANTS2:
            world = CP.load(wk("%s_%s_world.npy" % (look, var)))
            lk = CP.LAYER_OF.get(var, var)
            final = CP.composite(world, layers[lk])
            finals[look, var] = final
            if look == "realistic_tex":  # never rewrite the pre-existing "realistic" finals
                C.write_png(ot("%s_%s.png" % (look, var)), final)
            bullet_contrast[look][var] = CP.bullet_contrast(world, brec[lk]["bullets"])
            figure_contrast[look][var] = CP.figure_contrast(world, regions)
            disp = C.linear_to_srgb(C.luminance(C.srgb8_to_linear(world[sel])))
            floor_median[look][var] = float(np.median(disp))

    # Native 1x crops: "realistic" from the pre-existing busy-scene crop, "realistic_tex"
    # from the busy_dim-scene crop rendered for this task (see run report for the caveat).
    crop_world = {"realistic": CP.load(wk("realistic_busy_1x_world_crop.npy")),
                  "realistic_tex": CP.load(wk("realistic_tex_busy_1x_world_crop.npy"))}
    crops = {k: CP.composite(v, layers["busy"][y0:y1, x0:x1]) for k, v in crop_world.items()}
    C.write_png(ot("realistic_tex_busy_1x_crop.png"), crops["realistic_tex"])

    C.write_png(ot("composite_textures.png"),
                CP.grid([[CP.half_scale(finals[k, v]) for k in LOOKS2] for v in VARIANTS2]))
    C.write_png(ot("composite_textures_crops.png"), CP.grid([[crops[k] for k in LOOKS2]]))

    # ---- extend the existing metrics.json / metrics.md written by compose.py ----
    m = C.load_json(ot("metrics.json"))
    m["bullet_contrast"]["realistic_tex"] = bullet_contrast["realistic_tex"]
    m["figure_contrast"]["realistic_tex"] = figure_contrast["realistic_tex"]
    m["calibration"]["gains"]["realistic_tex"] = 1.0
    m["calibration"]["stops"]["realistic_tex"] = 0.0
    m["calibration"]["achieved_display_median"]["realistic_tex"] = floor_median["realistic_tex"]["calm"]
    m["texture_mapping"] = dict(material_to_texture=MATERIAL_TEXTURE_MAP,
                                unmapped_materials=list(UNMAPPED_MATERIALS),
                                uv_approach="World-scaled box/cube projection (per-face dominant axis; "
                                            "1 UV unit = the material's tile_size_m), fixed planar-XY for the "
                                            "floor objects (floor_tiles, floor_kerb, floor_grout) to avoid seams.",
                                ao_handling="ORM red (material AO) is not sampled: Eevee's Principled BSDF has no "
                                            "socket that folds a supplied AO map into indirect light only, and "
                                            "faking it via Shader-to-RGB would defeat the GGX comparison. Left out "
                                            "entirely, never multiplied into base colour.",
                                crop_caveat="composite_textures_crops.png: the 'realistic' crop is the pre-existing "
                                            "busy-scene 1x crop; the 'realistic_tex' crop was rendered from the "
                                            "busy_dim scene per this task's instructions. Composition is identical, "
                                            "cluster-light intensity differs by the busy_dim 25% factor.")
    C.save_json(ot("metrics.json"), m)

    L = ["", "## realistic_tex: PBR-Texturen gegen den flachen \"realistic\"-Look", "",
         "Gleicher Licht-Gain wie realistic (1,0), keine Neukalibrierung. Materialtabelle, UV-Ansatz und "
         "AO-Behandlung stehen unter `texture_mapping` in metrics.json.", "",
         "| Look | Variante | Boden-Median (Anzeige) |", "|---|---|---|"]
    for look in LOOKS2:
        for var in VARIANTS2:
            L.append("| %s | %s | %s |" % (look, var, CP.fmt(floor_median[look][var], 3)))
    L += ["", "| Look | Variante | n | Min | 5. Perzentil | Median | Anteil >= 4,5:1 |", "|---|---|---|---|---|---|---|"]
    for look in LOOKS2:
        for var in VARIANTS2:
            s = bullet_contrast[look][var]
            L.append("| %s | %s | %d | %s | %s | %s | %s %% |" % (
                look, var, s["n"], CP.fmt(s["min"]), CP.fmt(s["p5"]), CP.fmt(s["median"]), CP.fmt(100 * s["share_ge_4_5"], 1)))
    L += ["", "| Look | Variante | Figur-Median | Figur-Min | schwaechste Figur |", "|---|---|---|---|---|"]
    for look in LOOKS2:
        for var in VARIANTS2:
            s = figure_contrast[look][var]
            L.append("| %s | %s | %s | %s | %s |" % (look, var, CP.fmt(s["median"]), CP.fmt(s["min"]), s["weakest"]))
    L += ["", "`composite_textures.png`: Spalten realistic | realistic_tex, Zeilen calm oben / busy_dim unten. "
          "`composite_textures_crops.png`: native 1x-Crops, gleiches Rechteck wie `composite_crops.png`; "
          "die realistic-Seite stammt aus der busy-Szene (bestehende Datei), die realistic_tex-Seite aus der "
          "busy_dim-Szene (fuer diese Aufgabe neu gerendert) -- gleicher Bildausschnitt, Cluster-Lichter unterscheiden "
          "sich um den busy_dim-Faktor.", ""]
    with open(ot("metrics.md"), "a", encoding="utf-8") as fh:
        fh.write("\n".join(L))
    print("COMPOSE_TEXTURES OK", {k: bullet_contrast["realistic_tex"][k]["median"] for k in VARIANTS2})


if __name__ == "__main__":
    main()
