"""Render the class mask, a calibration image, a final world image or a 1x crop.

Run the GPU guard before every invocation. One image per invocation keeps runs short.

  blender -b --factory-startup <work>/scene_<variant>.blend --python render.py -- \
      --mode {classmask,calib,final,crop,draft} --work <work-dir> --out <out-dir> [--look LOOK]
      [--samples N] [--warmup 1] [--timed 3] [--params <override.json>]
      [--retarget-light-scale]

Modes
  classmask  1 sample, no filter, Standard view, material override encoding
             class/figure id and world XY -> <work>/classmask.npy (calm scene)
  calib      look at gain 1, no compositor -> <work>/calib_<look>.npy and the
             look's light gain in <work>/calibration.json. With
             --retarget-light-scale the global light factor is multiplied by the
             found gain instead (used for the realistic look, whose gain is then 1).
  final      calibrated look, shared bloom, warm-up + timed renders ->
             <out>/<look>_<variant>_world.png, <work>/<look>_<variant>_world.npy,
             <work>/timing_<look>_<variant>.json
  crop       busy scene, 1 sample, no filter, border render of the crop rectangle ->
             <work>/<look>_busy_1x_world_crop.npy
  draft      like final with few samples, written to <work>/draft/
"""
import argparse
import math
import os
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bpy  # noqa: E402
import numpy as np  # noqa: E402

import build_scene  # noqa: E402
import common as C  # noqa: E402
import looks  # noqa: E402


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", required=True, choices=["classmask", "calib", "final", "crop", "draft"])
    ap.add_argument("--look", choices=["toon", "stylized", "realistic"])
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--samples", type=int)
    ap.add_argument("--warmup", type=int, default=1)
    ap.add_argument("--timed", type=int, default=3)
    ap.add_argument("--params")
    ap.add_argument("--retarget-light-scale", action="store_true")
    ap.add_argument("--timing-tag", help="final mode: only time the renders, write timing_<look>_<variant>_<tag>.json")
    return ap.parse_args(argv)


# ----------------------------------------------------------- calibration ---
def calibration_path(work):
    return os.path.join(work, "calibration.json")


def load_calibration(work):
    path = calibration_path(work)
    if os.path.exists(path):
        return C.load_json(path)
    return dict(units={k: v for k, v in C.UNIT_DEFAULTS.items() if k != "light_scale"},
                light_scale=C.UNIT_DEFAULTS["light_scale"], gains={}, history=[])


def solve_gain(rgb, sel):
    """Gain g such that median(display_luminance(g * rgb)) over the floor = target."""
    pix = rgb[sel][::4].astype(np.float64)
    target = C.POST["target_display_median"]

    def med(g):
        return float(np.median(C.display_luminance(pix * g)))
    lo, hi = math.log(1e-3), math.log(1e3)
    for _ in range(48):
        mid = 0.5 * (lo + hi)
        if med(math.exp(mid)) < target:
            lo = mid
        else:
            hi = mid
    gain = math.exp(0.5 * (lo + hi))
    return gain, med(1.0), med(gain), int(sel.sum())


# ---------------------------------------------------------------- setup ---
def build_compositor(scene):
    tree = bpy.data.node_groups.new("post_stack", "CompositorNodeTree")
    rl = tree.nodes.new("CompositorNodeRLayers")
    rl.scene = scene
    ex = tree.nodes.new("CompositorNodeExposure")
    ex.inputs["Exposure"].default_value = C.POST["compositor_exposure"]
    gl = tree.nodes.new("CompositorNodeGlare")
    for name in ("Bloom", "BLOOM"):
        try:
            gl.inputs["Type"].default_value = name
            break
        except (TypeError, ValueError):
            continue
    try:
        gl.inputs["Quality"].default_value = C.POST["bloom_quality"]
    except (TypeError, ValueError):
        pass
    gl.inputs["Threshold"].default_value = C.POST["bloom_threshold"]
    gl.inputs["Smoothness"].default_value = C.POST["bloom_smoothness"]
    gl.inputs["Strength"].default_value = C.POST["bloom_strength"]
    gl.inputs["Size"].default_value = C.POST["bloom_size"]
    out = tree.nodes.new("NodeGroupOutput")
    tree.interface.new_socket(name="Image", in_out='OUTPUT', socket_type='NodeSocketColor')
    tree.links.new(rl.outputs["Image"], ex.inputs["Image"])
    tree.links.new(ex.outputs["Image"], gl.inputs["Image"])
    tree.links.new(gl.outputs["Image"], out.inputs[0])
    scene.compositing_node_group = tree
    return dict(glare_type=str(gl.inputs["Type"].default_value))


def setup_render(scene, samples, filter_size, view_transform, compositing):
    scene.render.engine = 'BLENDER_EEVEE'
    ee = scene.eevee
    ee.use_raytracing = False
    ee.use_fast_gi = False
    ee.use_shadows = True
    ee.taa_render_samples = samples
    scene.render.use_motion_blur = False
    scene.render.filter_size = filter_size
    scene.render.dither_intensity = 0.0
    scene.render.resolution_x, scene.render.resolution_y = C.WIDTH, C.HEIGHT
    scene.render.resolution_percentage = 100
    scene.render.use_border = False
    scene.render.use_crop_to_border = False
    scene.display_settings.display_device = 'sRGB'
    scene.view_settings.view_transform = view_transform
    scene.view_settings.look = 'None'
    scene.view_settings.exposure = 0.0
    scene.view_settings.gamma = 1.0
    scene.view_settings.use_curve_mapping = False
    scene.render.use_compositing = compositing
    bpy.context.view_layer.material_override = None
    info = dict(view_transform=scene.view_settings.view_transform, samples=samples, filter=filter_size)
    if compositing:
        info.update(build_compositor(scene))
    return info


def render_timed():
    t0 = time.perf_counter()
    bpy.ops.render.render(write_still=False)
    return time.perf_counter() - t0


def save_result(scene, path, exr=False):
    s = scene.render.image_settings
    if exr:
        s.file_format = 'OPEN_EXR'
        s.color_mode = 'RGBA'
        s.color_depth = '32'
        s.exr_codec = 'ZIP'
    else:
        s.file_format = 'PNG'
        s.color_mode = 'RGB'
        s.color_depth = '8'
        s.compression = 15
    os.makedirs(os.path.dirname(path), exist_ok=True)
    bpy.data.images["Render Result"].save_render(filepath=path, scene=scene)


def load_pixels(path):
    img = bpy.data.images.load(path)
    w, h = img.size
    arr = np.empty(w * h * 4, dtype=np.float32)
    img.pixels.foreach_get(arr)
    bpy.data.images.remove(img)
    return arr.reshape(h, w, 4)[::-1].copy()


def apply_scene_look(scene, args, cal, gain):
    units = dict(cal["units"], light_scale=cal["light_scale"])
    build_scene.apply_light_units(scene, units)
    params_path = args.params or os.path.join(args.work, "look_params_override.json")
    params = C.load_params_override(params_path)
    return looks.apply_look(scene, args.look, params, cal["light_scale"], gain, scene["floor_mask"])


# ----------------------------------------------------------------- modes ---
def mode_classmask(scene, args):
    info = setup_render(scene, 1, 0.0, 'Standard', False)
    bpy.context.view_layer.material_override = looks.classmask_material()
    t = render_timed()
    path = os.path.join(args.work, "classmask.exr")
    save_result(scene, path, exr=True)
    arr = load_pixels(path)[..., :3]
    np.save(os.path.join(args.work, "classmask.npy"), arr)
    cls = C.decode_classmask(arr)
    print("RENDER OK classmask", info, "time %.2f" % t, "class counts",
          {int(k): int((cls["cls"] == k).sum()) for k in range(-1, 5)})


def mode_calib(scene, args):
    cal = load_calibration(args.work)
    look_info = apply_scene_look(scene, args, cal, 1.0)
    info = setup_render(scene, args.samples or C.POST["samples_final"], C.POST["filter_final"],
                        C.POST["view_transform"], False)
    t = render_timed()
    path = os.path.join(args.work, "calib_%s.exr" % args.look)
    save_result(scene, path, exr=True)
    rgb = load_pixels(path)[..., :3]
    np.save(os.path.join(args.work, "calib_%s.npy" % args.look), rgb)
    sc = C.load_json(os.path.join(args.work, "scene.json"))
    mask = np.load(os.path.join(args.work, "classmask.npy"))
    sel = C.calibration_floor_selection(mask, sc)
    gain, med1, medg, npx = solve_gain(rgb, sel)
    entry = dict(look=args.look, light_scale=cal["light_scale"], gain=gain, display_median_at_gain1=med1,
                 display_median_at_gain=medg, floor_pixels=npx, time_s=t)
    if args.retarget_light_scale:
        cal["light_scale"] = cal["light_scale"] * gain
        entry["retargeted_light_scale"] = cal["light_scale"]
    elif args.look == "realistic" and abs(gain - 1.0) <= C.POST["calib_tolerance"]:
        # The global light factor was retargeted on this look: keep native Principled
        # shading (gain exactly 1) when the verification lands within tolerance.
        cal["gains"][args.look] = 1.0
        entry["stored_gain"] = 1.0
    else:
        cal["gains"][args.look] = gain
    cal["history"].append(entry)
    C.save_json(calibration_path(args.work), cal)
    print("RENDER OK calib", args.look, look_info, info, entry)


def mode_final(scene, args, draft=False):
    cal = load_calibration(args.work)
    gain = float(cal["gains"].get(args.look, 1.0))
    look_info = apply_scene_look(scene, args, cal, gain)
    samples = args.samples or (4 if draft else C.POST["samples_final"])
    info = setup_render(scene, samples, C.POST["filter_final"], C.POST["view_transform"], True)
    warm = [render_timed() for _ in range(0 if draft else args.warmup)]
    timed = [render_timed() for _ in range(1 if draft else max(1, args.timed))]
    variant = scene["variant"]
    if args.timing_tag and not draft:
        # Timing-only control pass: no image output.
        rec = dict(look=args.look, variant=variant, samples=samples, warmup_s=warm, timed_s=timed,
                   median_s=statistics.median(timed), gain=gain)
        C.save_json(os.path.join(args.work, "timing_%s_%s_%s.json" % (args.look, variant, args.timing_tag)), rec)
        print("RENDER OK timing", rec)
        return
    base = "%s_%s_world" % (args.look, variant)
    png = os.path.join(args.work, "draft", base + ".png") if draft else os.path.join(args.out, base + ".png")
    save_result(scene, png)
    arr = np.round(load_pixels(png)[..., :3] * 255.0).astype(np.uint8)
    np.save(os.path.join(os.path.dirname(png) if draft else args.work, base + ".npy"), arr)
    rec = dict(look=args.look, variant=variant, samples=samples, warmup_s=warm, timed_s=timed,
               median_s=statistics.median(timed), gain=gain, light_scale=cal["light_scale"], post=info,
               look_info=look_info)
    if not draft:
        C.save_json(os.path.join(args.work, "timing_%s_%s.json" % (args.look, variant)), rec)
    print("RENDER OK", "draft" if draft else "final", rec)


def mode_crop(scene, args):
    cal = load_calibration(args.work)
    gain = float(cal["gains"].get(args.look, 1.0))
    look_info = apply_scene_look(scene, args, cal, gain)
    info = setup_render(scene, C.POST["samples_crop"], 0.0, C.POST["view_transform"], True)
    x0, y0, x1, y1 = C.CROP_RECT
    r = scene.render
    r.use_border, r.use_crop_to_border = True, True
    r.border_min_x, r.border_max_x = x0 / C.WIDTH, x1 / C.WIDTH
    r.border_min_y, r.border_max_y = (C.HEIGHT - y1) / C.HEIGHT, (C.HEIGHT - y0) / C.HEIGHT
    t = render_timed()
    base = "%s_busy_1x_world_crop" % args.look
    png = os.path.join(args.work, base + ".png")
    save_result(scene, png)
    arr = np.round(load_pixels(png)[..., :3] * 255.0).astype(np.uint8)
    np.save(os.path.join(args.work, base + ".npy"), arr)
    print("RENDER OK crop", args.look, arr.shape, "time %.2f" % t, info, look_info)


def main():
    args = parse_args()
    args.work, args.out = os.path.abspath(args.work), os.path.abspath(args.out)
    scene = bpy.context.scene
    if args.mode != "classmask" and not args.look:
        raise SystemExit("--look is required for mode %s" % args.mode)
    if args.mode == "classmask":
        mode_classmask(scene, args)
    elif args.mode == "calib":
        mode_calib(scene, args)
    elif args.mode == "final":
        mode_final(scene, args)
    elif args.mode == "draft":
        mode_final(scene, args, draft=True)
    elif args.mode == "crop":
        mode_crop(scene, args)


if __name__ == "__main__":
    main()
