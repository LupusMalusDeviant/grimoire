"""Final frames, composites and metrics for the look comparison (numpy only, no bpy).

Usage: python compose.py --work <work-dir> --out <out-dir> [--notes <notes_de.md>]

Needs from build_scene.py / render.py / bullets2d.py (all in <work>):
  scene.json, classmask.npy, calibration.json, bullets_layer_{calm,busy}.npy,
  bullets_{calm,busy}.json, <look>_<variant>_world.npy, timing_<look>_<variant>.json,
  <look>_busy_1x_world_crop.npy; optional look_params_override.json, tuning_log.json.
Writes to <out>: <look>_<variant>.png, <look>_busy_1x_crop.png,
  composite_side_by_side.png, composite_crops.png, metrics.json, metrics.md (German).
"""
import argparse
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common as C  # noqa: E402

LOOKS = ("toon", "stylized", "realistic")
VARIANTS = ("calm", "busy")
LAYER_OF = {"busy_dim": "busy"}   # busy_dim reuses the busy bullets and bullet layer
GUTTER = 4
TIME_LABEL = "Blender Eevee auf der GPU des Entwicklungsrechners, kein Engine-Budget"

DEVIATIONS_DE = [
    "Ruhige Variante: 3 Spiralarme mit je 12 statt 20 Reis-Geschossen (die Design-Zählung ergibt 204 statt 180). "
    "Die 18 Streukugeln sind die Stress-Platzierungen (6 Fackelpool, 6 Ritualkreis, 3 Spielerrand, 3 Säulen). "
    "Unruhige Variante: die 60 Stress-Geschosse stammen aus dem 416er-Driftfeld.",
    "Toon-Bänder und stilisierter Wrap wirken auf die gesamte direkte Beleuchtung (Eevee liefert über Shader to RGB "
    "nur die Summe aller Lichter), nicht pro Licht. Das Mondlicht wird mitquantisiert statt mit eigenen Schwellen 0,05/0,5.",
    "Kalibrierung als Licht-Gain in der Look-Schattierung (wirkt auf Diffus, Ambient, Glanz; nicht auf Rim, Emissive, "
    "Dekal-Emission, freundliche Geschosse). Compositor-Belichtung neutral (0). Ziel: Median der Boden-Leuchtdichte als "
    "Anzeigewert 0,18 (sRGB nach View-Transform, linear etwa 0,027). Der globale Lichtfaktor ist so gewählt, dass der "
    "realistische Look Gain 1,0 hat.",
    "Punktlicht-Abfall: Eevee rechnet 1/d² mit Fenster (1-(d/r)^4)² statt 1/(1+d²). Um das Design am Boden "
    "nachzubilden, sitzt jedes Punktlicht in der Höhe sqrt(h²+1) über dem Boden darunter (h = Design-Höhe) und "
    "cutoff_distance = sqrt(r²+1); der Boden erhält damit genau den Design-Abstandsterm. Ohne diese Anhebung "
    "brannten Randkerzen, Runensteine und Cluster-Lichter singuläre Hotspots (Bugfix im Entwurf, kein Tuning). "
    "Senkrechte Flächen direkt neben einer Lichtquelle bleiben heller als im Design. Ein globaler Watt-Faktor.",
    "Echte Schattenwürfe (Eevee Shadow Maps) für Mond, 8 Fackeln/Kohlebecken und Stab-Orb in allen Looks; "
    "alle anderen Lichter ohne Schatten.",
    "Geometrie: Plinthe (1,0) und Kapitell (0,9) als Halbmaße; Wandleuchter der gebrochenen Säulen bei "
    "z = min(3,6; Säulenhöhe - 0,5); Mauerbogen tangential, minimal nach (-7,9 | 10,0) verschoben (2,28 Einheiten), "
    "damit er Säule und Plinthe bei 150° um mindestens 1 Einheit freigibt; umgestürzte Säule samt Glutlicht bei "
    "(8,5 | -4,0) wie im Engine-Spike; Glutlicht auf z 0,25 statt Bodenkontakt (sonst singulärer Hotspot).",
    "Toon-Outlines als invertierte Hülle (Solidify nach außen, gespiegelte Normalen, Backface-Culling, #0B0A0D, "
    "Dicke 0,04 Welteinheiten, dünne Teile 0,02) statt Screen-Space-Pass; nicht auf Boden, Dekal und Emissive.",
    "Bloom: Blender-Glare im Bloom-Modus (Schwelle 1,0, Glättung 0,5, Stärke 0,08, Größe 0,1) statt 4-stufiger "
    "Dual-Filter-Kette; in den 1x-Crops wird Bloom nur auf dem Crop-Ausschnitt gerechnet. Kein 2x-SSAA, sondern "
    "32 Eevee-Samples mit 1,5-px-Filter.",
    "Stilisiert: Glanz über GGX Glossy BSDF (Rauheit aus dem Glanzexponenten, Farbe ks) statt normalisiertem "
    "Blinn-Phong; nimmt auch die schwache Welt-Umgebung als Reflexion auf.",
    "Freundliche Geschosse als Kapsel-Meshes (Blended, Alpha 0,75) statt Billboards; die 64 Randkerzen der "
    "unruhigen Variante sind reine Lichter ohne Mesh.",
    "Komposit-Größen 1928x724 und 1928x360 wegen 4-px-Rinnen zwischen den Zellen.",
]


def load(path):
    if not os.path.exists(path):
        raise SystemExit("missing input: %s" % os.path.basename(path))
    return np.load(path) if path.endswith(".npy") else C.load_json(path)


def composite(world_u8, layer):
    lin = C.srgb8_to_linear(world_u8)
    return C.linear_to_srgb8(layer[..., :3] + (1.0 - layer[..., 3:4]) * lin)


def half_scale(img_u8):
    lin = C.srgb8_to_linear(img_u8)
    h, w = lin.shape[0] // 2, lin.shape[1] // 2
    return C.linear_to_srgb8(lin[:2 * h, :2 * w].reshape(h, 2, w, 2, 3).mean(axis=(1, 3)))


def grid(rows):
    ch, cw = rows[0][0].shape[:2]
    nr, nc = len(rows), len(rows[0])
    canvas = np.zeros((nr * ch + (nr - 1) * GUTTER, nc * cw + (nc - 1) * GUTTER, 3), np.uint8)
    for r, row in enumerate(rows):
        for c, cell in enumerate(row):
            y, x = r * (ch + GUTTER), c * (cw + GUTTER)
            canvas[y:y + ch, x:x + cw] = cell
    return canvas


def stats(values, threshold=4.5):
    v = np.asarray(values, dtype=np.float64)
    return dict(n=int(v.size), min=float(v.min()), p5=float(np.percentile(v, 5)), median=float(np.median(v)),
                share_ge_4_5=float(np.mean(v >= threshold)))


def bullet_contrast(world_u8, records):
    lum = C.luminance(C.srgb8_to_linear(world_u8))
    body_l = [float(C.luminance(C.hex_lin(p["body"]))) for p in C.HOSTILE]
    h, w = lum.shape
    out = []
    for r in records:
        if not r["visible"]:
            continue
        rad = r["r_px"] + 6.0
        x0, x1 = max(0, int(r["px"] - rad)), min(w, int(r["px"] + rad) + 2)
        y0, y1 = max(0, int(r["py"] - rad)), min(h, int(r["py"] + rad) + 2)
        gx, gy = np.meshgrid(np.arange(x0, x1) + 0.5 - r["px"], np.arange(y0, y1) + 0.5 - r["py"])
        dist = np.hypot(gx, gy)
        ring = (dist >= r["r_px"] + 3.0) & (dist <= r["r_px"] + 6.0)
        if ring.any():
            out.append(float(C.wcag_ratio(body_l[r["palette"]], lum[y0:y1, x0:x1][ring].mean())))
    return stats(out)


def figure_regions(mask, sc):
    d = C.decode_classmask(mask)
    names = {sc["player"]["figure_id"]: "Spieler"}
    names.update({f["figure_id"]: "Imp %d" % (i + 1) for i, f in enumerate(sc["imps"])})
    names.update({f["figure_id"]: "Brute %d" % (i + 1) for i, f in enumerate(sc["brutes"])})
    regions = []
    for fid, name in sorted(names.items()):
        sil = (d["fid"] == fid) & ~d["bg"]
        if not sil.any():
            regions.append((name, None, None))
            continue
        ys, xs = np.nonzero(sil)
        y0, y1 = max(0, ys.min() - 8), min(sil.shape[0], ys.max() + 9)
        x0, x1 = max(0, xs.min() - 8), min(sil.shape[1], xs.max() + 9)
        s = sil[y0:y1, x0:x1]
        ring = C.dilate(s, 6) & ~C.dilate(s, 2)
        regions.append((name, (y0, x0, np.nonzero(s)), (y0, x0, np.nonzero(ring))))
    return regions


def figure_contrast(world_u8, regions):
    lum = C.luminance(C.srgb8_to_linear(world_u8))
    per = {}
    for name, sil, ring in regions:
        if sil is None:
            continue
        ls = lum[sil[0] + sil[2][0], sil[1] + sil[2][1]].mean()
        lr = lum[ring[0] + ring[2][0], ring[1] + ring[2][1]].mean()
        per[name] = float(C.wcag_ratio(ls, lr))
    vals = list(per.values())
    weakest = min(per, key=per.get)
    return dict(per_figure=per, median=float(np.median(vals)), min=float(min(vals)), weakest=weakest)


def fmt(v, nd=2):
    return ("%%.%df" % nd % v).replace(".", ",")


def write_markdown(path, m, notes):
    L = ["# Look-Vergleich in Blender Eevee: Messwerte", "",
         "Legende der Komposite: Spalten von links nach rechts **toon | stylized | realistic**; "
         "Zeilen **calm oben, busy unten**. `composite_crops.png`: busy, native Ausschnitte x 320-960, y 300-660, "
         "gleiche Spaltenreihenfolge.", "",
         "## Kalibrierung (Licht-Gain je Look)", "",
         "Globaler Lichtfaktor (Design-Einheiten zu Blender): %s. Ziel: Median der Boden-Leuchtdichte als Anzeigewert "
         "%s (±%d %%)." % (fmt(m["calibration"]["light_scale"], 3), fmt(C.POST["target_display_median"]),
                          int(C.POST["calib_tolerance"] * 100)), "",
         "| Look | Licht-Gain | entspricht Blendenstufen | erreichter Anzeige-Median (calm, mit Bloom) |",
         "|---|---|---|---|"]
    for look in LOOKS:
        c = m["calibration"]
        L.append("| %s | %s | %s | %s |" % (look, fmt(c["gains"][look], 3), fmt(c["stops"][look], 2),
                                            fmt(c["achieved_display_median"][look], 3)))
    L += ["", "## Bullet-Kontrast (WCAG, Körperfarbe gegen Ring 3-6 px im Welt-Bild)", "",
          "| Look | Variante | n | Min | 5. Perzentil | Median | Anteil ≥ 4,5:1 |", "|---|---|---|---|---|---|---|"]
    for look in LOOKS:
        for var in m["variants"]:
            s = m["bullet_contrast"][look][var]
            L.append("| %s | %s | %d | %s | %s | %s | %s %% |" % (look, var, s["n"], fmt(s["min"]), fmt(s["p5"]),
                                                              fmt(s["median"]), fmt(100 * s["share_ge_4_5"], 1)))
    L += ["", "bullets_rejected_palette_space: calm %d, busy %d (muss 0 sein). Die Bullet-Ebene wird einmal je Variante "
          "gezeichnet und identisch über alle Looks gelegt (SHA-256 busy: `%s`)." % (
              m["bullets_rejected_palette_space"]["calm"], m["bullets_rejected_palette_space"]["busy"],
              m["bullets_layer_sha256"]["busy"][:16]), "",
          "## Figur-gegen-Hintergrund-Kontrast (Klassenmaske, Ring 2-6 px außerhalb der Silhouette)", "",
          "| Look | Variante | Median | Min | schwächste Figur |", "|---|---|---|---|---|"]
    for look in LOOKS:
        for var in m["variants"]:
            s = m["figure_contrast"][look][var]
            L.append("| %s | %s | %s | %s | %s |" % (look, var, fmt(s["median"]), fmt(s["min"]), s["weakest"]))
    if "busy_dim" in m["variants"]:
        L += ["", "## Variante busy_dim (Cluster-Lichter auf %d %%)" % round(100 * C.BUSY_DIM_CLUSTER_FACTOR), "",
              "Identisch mit busy (Geometrie, Bullets, Bullet-Ebene, Licht-Gains ohne Neukalibrierung); nur die 64 "
              "Bullet-Cluster-Lichter der unruhigen und die 6 der ruhigen Variante leuchten mit %d %% ihrer "
              "Intensität. Test einer Szenen-Eigenschaft, für alle Looks gleich. Komposite: "
              "`composite_busy_dim.png` (oben busy, unten busy_dim) und `composite_crops_busy_dim.png`." % round(
                  100 * C.BUSY_DIM_CLUSTER_FACTOR), "",
              "| Look | Bullet-Median busy → dim | P5 busy → dim | Anteil ≥ 4,5:1 busy → dim | "
              "Figur-Median busy → dim | Figur-Min busy → dim |", "|---|---|---|---|---|---|"]
        for look in LOOKS:
            b, d = m["bullet_contrast"][look]["busy"], m["bullet_contrast"][look]["busy_dim"]
            fb, fd = m["figure_contrast"][look]["busy"], m["figure_contrast"][look]["busy_dim"]
            L.append("| %s | %s → %s | %s → %s | %s %% → %s %% | %s → %s | %s → %s |" % (
                look, fmt(b["median"]), fmt(d["median"]), fmt(b["p5"]), fmt(d["p5"]),
                fmt(100 * b["share_ge_4_5"], 1), fmt(100 * d["share_ge_4_5"], 1),
                fmt(fb["median"]), fmt(fd["median"]), fmt(fb["min"]), fmt(fd["min"])))
    L += ["", "## Renderzeit je Finalbild", "", "Messung: %s. Median aus 3 Läufen nach 1 Aufwärmlauf, "
          "gemessen um `bpy.ops.render.render` (inklusive Compositor, ohne Dateischreiben)." % TIME_LABEL, "",
          "Die Einzelwerte streuen stark (asynchrone Shader-Kompilierung von Eevee reicht über den einen "
          "Aufwärmlauf hinaus). Deshalb steht daneben eine Kontrollmessung mit 3 Aufwärm- und 5 Messläufen im "
          "selben Ablauf; nur sie taugt für einen Vergleich zwischen den Looks, und auch sie ist kein Engine-Budget.", "",
          "| Look | Variante | Median s (1+3) | Einzelwerte s | Kontrolle Median s (3+5) | Kontrolle Einzelwerte s |",
          "|---|---|---|---|---|---|"]
    for look in LOOKS:
        for var in m["variants"]:
            t = m["render_time_s"][look][var]
            c = t.get("control")
            L.append("| %s | %s | %s | %s | %s | %s |" % (
                look, var, fmt(t["median"]), ", ".join(fmt(v) for v in t["timed"]),
                fmt(c["median"]) if c else "-", ", ".join(fmt(v) for v in c["timed"]) if c else "-"))
    L += ["", "## Tuning-Log", ""]
    if m["tuning_log"]:
        L += ["| Look | Parameter | alt | neu | Begründung |", "|---|---|---|---|---|"]
        for e in m["tuning_log"]:
            L.append("| %s | %s | %s | %s | %s |" % (e["look"], e["param"], e["old"], e["new"], e["reason"]))
    else:
        L.append("Keine Tuning-Runde durchgeführt.")
    L += ["", "## Abweichungen vom Design", ""] + ["%d. %s" % (i + 1, d) for i, d in enumerate(m["deviations"])]
    L += ["", "## Look-Parameter (vollständig)", "", "```json", _json_block(m["look_params"]), "```", "",
          "### Materialtabelle (Albedo identisch in allen Looks)", "", "```json", _json_block(m["materials"]), "```", "",
          "### Gemeinsamer Post-Stack und Einheiten", "", "```json",
          _json_block(dict(post=m["post"], calibration=m["calibration"])), "```", ""]
    if notes:
        L += ["## Beobachtungen", "", notes.strip(), ""]
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("\n".join(L))


def _json_block(obj):
    import json
    return json.dumps(obj, indent=1, sort_keys=True, ensure_ascii=False)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--notes")
    args = ap.parse_args()
    wk = lambda name: os.path.join(args.work, name)  # noqa: E731
    sc, mask, cal = load(wk("scene.json")), load(wk("classmask.npy")), load(wk("calibration.json"))
    layers = {v: load(wk("bullets_layer_%s.npy" % v)).astype(np.float64) for v in VARIANTS}
    brec = {v: load(wk("bullets_%s.json" % v)) for v in VARIANTS}
    regions = figure_regions(mask, sc)
    sel = C.calibration_floor_selection(mask, sc)
    x0, y0, x1, y1 = C.CROP_RECT
    finals, crops_1x = {}, {}
    variants = list(VARIANTS)
    if all(os.path.exists(wk("%s_busy_dim_world.npy" % k)) for k in LOOKS):
        variants.append("busy_dim")
    m = dict(legend="Spalten: toon | stylized | realistic; Zeilen: calm oben, busy unten",
             variants=variants,
             bullet_contrast={}, figure_contrast={}, render_time_s={}, render_time_label=TIME_LABEL,
             bullets_rejected_palette_space={v: brec[v]["bullets_rejected_palette_space"] for v in VARIANTS},
             bullets_layer_sha256={v: brec[v]["layer_sha256"] for v in VARIANTS},
             calibration=dict(light_scale=cal["light_scale"], units=cal["units"],
                              gains={k: float(cal["gains"].get(k, 1.0)) for k in LOOKS},
                              stops={k: math.log2(float(cal["gains"].get(k, 1.0))) for k in LOOKS},
                              achieved_display_median={}, history=cal.get("history", [])))
    for look in LOOKS:
        m["bullet_contrast"][look], m["figure_contrast"][look], m["render_time_s"][look] = {}, {}, {}
        for var in m["variants"]:
            world = load(wk("%s_%s_world.npy" % (look, var)))
            lk = LAYER_OF.get(var, var)
            finals[look, var] = composite(world, layers[lk])
            C.write_png(os.path.join(args.out, "%s_%s.png" % (look, var)), finals[look, var])
            m["bullet_contrast"][look][var] = bullet_contrast(world, brec[lk]["bullets"])
            m["figure_contrast"][look][var] = figure_contrast(world, regions)
            t = load(wk("timing_%s_%s.json" % (look, var)))
            m["render_time_s"][look][var] = dict(median=t["median_s"], timed=t["timed_s"], warmup=t["warmup_s"])
            ctrl = wk("timing_%s_%s_control.json" % (look, var))
            if os.path.exists(ctrl):
                tc = C.load_json(ctrl)
                m["render_time_s"][look][var]["control"] = dict(median=tc["median_s"], timed=tc["timed_s"],
                                                                warmup=tc["warmup_s"])
            if var == "calm":
                disp = C.linear_to_srgb(C.luminance(C.srgb8_to_linear(world[sel])))
                m["calibration"]["achieved_display_median"][look] = float(np.median(disp))
        crop_world = load(wk("%s_busy_1x_world_crop.npy" % look))
        crops_1x[look] = composite(crop_world, layers["busy"][y0:y1, x0:x1])
        C.write_png(os.path.join(args.out, "%s_busy_1x_crop.png" % look), crops_1x[look])
    C.write_png(os.path.join(args.out, "composite_side_by_side.png"),
                grid([[half_scale(finals[k, v]) for k in LOOKS] for v in VARIANTS]))
    C.write_png(os.path.join(args.out, "composite_crops.png"),
                grid([[finals[k, "busy"][y0:y1, x0:x1] for k in LOOKS]]))
    if "busy_dim" in variants:
        C.write_png(os.path.join(args.out, "composite_busy_dim.png"),
                    grid([[half_scale(finals[k, v]) for k in LOOKS] for v in ("busy", "busy_dim")]))
        C.write_png(os.path.join(args.out, "composite_crops_busy_dim.png"),
                    grid([[finals[k, "busy_dim"][y0:y1, x0:x1] for k in LOOKS]]))
    tuning = wk("tuning_log.json")
    m["tuning_log"] = C.load_json(tuning) if os.path.exists(tuning) else []
    m["look_params"] = C.load_params_override(wk("look_params_override.json"))
    m["materials"], m["post"], m["deviations"] = C.MATERIALS, C.POST, DEVIATIONS_DE
    C.save_json(os.path.join(args.out, "metrics.json"), m)
    notes = open(args.notes, encoding="utf-8").read() if args.notes and os.path.exists(args.notes) else ""
    write_markdown(os.path.join(args.out, "metrics.md"), m, notes)
    print("COMPOSE OK", {k: m["bullet_contrast"][k]["busy"]["median"] for k in LOOKS})


if __name__ == "__main__":
    main()
