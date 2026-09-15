# Look comparison in Blender Eevee (toon | stylized | realistic)

Procedural bullet-hell arena rendered with the three candidate looks, so they can be
compared with a mature real-time renderer (shadow maps, temporal AA). All paths are
command-line arguments; nothing in the scripts is machine-specific.

Requirements: Blender 5.2 (Eevee) and Python 3 with numpy (Blender's bundled Python works).

## Files

| File | Role |
|---|---|
| `common.py` | palette, material table, look parameters, post stack, colour helpers, PNG writer (no bpy) |
| `scene_data.py` | seeded scene description: floor, props, figures, lights, bolts, bullets, floor mask (no bpy) |
| `meshgen.py` | numpy mesh generators (no bpy) |
| `build_scene.py` | builds `scene_calm.blend`, `scene_busy.blend`, `scene.json`, `floor_mask.png` (no rendering) |
| `looks.py` | node trees of the three looks, inverted-hull outlines, class-mask override |
| `render.py` | class mask, calibration, final, 1x crop and draft renders (one image per call) |
| `bullets2d.py` | shared bullet and player-marker layer, drawn once per variant |
| `compose.py` | final frames, composites, `metrics.json`, `metrics.md` |

## Run

Below, `blender` is `blender -b --factory-startup`, `WORK` and `OUT` are directories of your
choice and `SRC` is this directory. If the machine is shared, run your GPU availability check
before every call that renders.

```sh
# 1. Scene, camera JSON and floor mask (no GPU needed)
blender --python SRC/build_scene.py -- --out-dir WORK
# 2. Bullet layers (no GPU needed)
python SRC/bullets2d.py --work WORK --out OUT
# 3. Class mask (renders)
blender WORK/scene_calm.blend --python SRC/render.py -- --mode classmask --work WORK --out OUT
# 4. Calibration (renders). First retarget the global light factor on the realistic look,
#    then verify it (gain within tolerance is stored as exactly 1.0), then the other looks.
blender WORK/scene_calm.blend --python SRC/render.py -- --mode calib --look realistic --retarget-light-scale --work WORK --out OUT
blender WORK/scene_calm.blend --python SRC/render.py -- --mode calib --look realistic --work WORK --out OUT
blender WORK/scene_calm.blend --python SRC/render.py -- --mode calib --look toon --work WORK --out OUT
blender WORK/scene_calm.blend --python SRC/render.py -- --mode calib --look stylized --work WORK --out OUT
# 5. Optional quick drafts (4 samples) into WORK/draft
blender WORK/scene_calm.blend --python SRC/render.py -- --mode draft --look toon --work WORK --out OUT
# 6. Finals: 1 warm-up + 3 timed renders each, for LOOK in toon stylized realistic, VARIANT in calm busy
blender WORK/scene_VARIANT.blend --python SRC/render.py -- --mode final --look LOOK --work WORK --out OUT
# 7. 1x crops (1 sample, no filter), busy scene, for each LOOK
blender WORK/scene_busy.blend --python SRC/render.py -- --mode crop --look LOOK --work WORK --out OUT
# 8. Finals with bullets, composites and metrics
python SRC/compose.py --work WORK --out OUT --notes WORK/notes_de.md
```

## Calibration

`WORK/calibration.json` holds the light-unit constants (`units`: pixel value of a white
Lambert surface per Watt at 1 m, per unit of sun strength and per unit of world radiance),
the global `light_scale` (design intensity units to Blender units) and one light gain per
look. The gain multiplies only light-derived terms (irradiance, ambient, specular) so the
median floor luminance of the calm world image, measured as a display value after the view
transform, is 0.18. Rims, emissive meshes, decal emission and friendly bolts are never scaled.
The compositor exposure stays at 0 for all looks.

## Tuning round

Parameter changes after viewing images go into `WORK/look_params_override.json`
(`{"toon": {"t2": 0.25}}`), and each change is logged in `WORK/tuning_log.json` as
`[{"look": ..., "param": ..., "old": ..., "new": ..., "reason": ...}]`. Re-render all six
finals afterwards; `compose.py` lists both files in `metrics.md`.
