"""Build the look-comparison scene in background Blender. Does not render.

Usage:
  blender -b --factory-startup --python build_scene.py -- --out-dir <dir>

Writes <dir>/scene_calm.blend, <dir>/scene_busy.blend, <dir>/scene.json (camera
matrices, figures, lights, projectiles) and <dir>/floor_mask.png (shared floor
engraving + blob-shadow mask). Materials are neutral placeholders; looks.py
rebuilds their node trees per look at render time. Emissive materials are final
here because they are identical in all looks.
"""
import argparse
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bpy  # noqa: E402
import numpy as np  # noqa: E402
from mathutils import Vector  # noqa: E402

import common as C  # noqa: E402
import meshgen as MG  # noqa: E402
import scene_data as SD  # noqa: E402
import textures as TX  # noqa: E402


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--tex-snapshot", help="realistic_tex: path to the texture snapshot's generated/ dir "
                                            "(tile sizes only; without it every UV uses a 1x1 m fallback tile)")
    return ap.parse_args(argv)


def apply_uvs(scene, tile_sizes):
    n = TX.apply_uvs_to_scene(scene, tile_sizes)
    print("UV generation: %d mesh objects got a %r layer" % (n, TX.UV_NAME))


# -------------------------------------------------------------- materials ---
def make_placeholder_materials():
    for key, spec in C.MATERIALS.items():
        mat = bpy.data.materials.new(key)
        mat.use_nodes = True
        nt = mat.node_tree
        nt.nodes.clear()
        d = nt.nodes.new("ShaderNodeBsdfDiffuse")
        d.inputs["Color"].default_value = C.hex_lin(spec["albedo"]) + (1.0,)
        out = nt.nodes.new("ShaderNodeOutputMaterial")
        nt.links.new(d.outputs[0], out.inputs["Surface"])


def make_emissive_material(key):
    spec = C.EMISSIVE[key]
    mat = bpy.data.materials.new(key)
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    em = nt.nodes.new("ShaderNodeEmission")
    em.inputs["Strength"].default_value = spec["mult"]
    body = C.hex_lin(spec["color"]) + (1.0,)
    if spec.get("core"):
        lw = nt.nodes.new("ShaderNodeLayerWeight")
        lw.inputs["Blend"].default_value = 0.5
        mr = nt.nodes.new("ShaderNodeMapRange")
        mr.interpolation_type = 'SMOOTHSTEP'
        mr.inputs["From Min"].default_value = 0.1
        mr.inputs["From Max"].default_value = 0.7
        mix = nt.nodes.new("ShaderNodeMix")
        mix.data_type = 'RGBA'
        mix.inputs["A"].default_value = C.hex_lin(spec["core"]) + (1.0,)
        mix.inputs["B"].default_value = body
        nt.links.new(lw.outputs["Facing"], mr.inputs["Value"])
        nt.links.new(mr.outputs["Result"], mix.inputs["Factor"])
        nt.links.new(mix.outputs["Result"], em.inputs["Color"])
    else:
        em.inputs["Color"].default_value = body
    shader = em.outputs[0]
    if spec.get("alpha") is not None:
        tr = nt.nodes.new("ShaderNodeBsdfTransparent")
        ms = nt.nodes.new("ShaderNodeMixShader")
        ms.inputs["Fac"].default_value = spec["alpha"]
        nt.links.new(tr.outputs[0], ms.inputs[1])
        nt.links.new(em.outputs[0], ms.inputs[2])
        shader = ms.outputs[0]
        mat.surface_render_method = 'BLENDED'
    nt.links.new(shader, out.inputs["Surface"])
    return mat


# ---------------------------------------------------------------- objects ---
def make_object(coll, name, mesh, mat_keys, cls, smooth, figure_id=0, outline=False,
                outline_scale=1.0, casts_shadow=True):
    me = bpy.data.meshes.new(name)
    me.from_pydata(mesh.verts.tolist(), [], mesh.faces)
    for k in mat_keys:
        me.materials.append(bpy.data.materials[k])
    me.polygons.foreach_set("material_index", [int(m) for m in mesh.mats])
    me.update()
    if smooth:
        me.shade_smooth()
    else:
        me.shade_flat()
    ob = bpy.data.objects.new(name, me)
    coll.objects.link(ob)
    ob.pass_index = cls
    ob["figure_id"] = figure_id
    ob["outline"] = int(outline)
    ob["outline_scale"] = float(outline_scale)
    ob.visible_shadow = casts_shadow
    return ob


def sphere_at(radius, pos, segments=10, rings=6):
    return MG.ellipsoid((radius, radius, radius), segments, rings).transformed(None, pos)


def build_floor(coll, sc):
    fl = sc["floor"]
    make_object(coll, "floor_tiles", MG.tiles(fl["tiles"], fl["tile_size"], fl["chamfer"]),
                ["floor_stone"], C.CLASS_FLOOR, smooth=False)
    k = fl["kerb"]
    prof = [(k["r_apron"], k["z_low"] - 0.25), (k["r_apron"], k["z_low"]), (k["r_bevel"], k["z_low"]),
            (k["r_top"], k["z_top"]), (k["r_in"], k["z_top"]), (k["r_in"], -0.3)]
    make_object(coll, "floor_kerb", MG.lathe(prof, 160), ["floor_stone"], C.CLASS_FLOOR, smooth=False)
    lat = np.asarray(fl["lattice"])

    def grout_z(X, Y):
        t = np.clip((np.hypot(X, Y) - 14.0) / 1.2, 0.0, 1.0)
        return (SD.floor_height(lat, X, Y) - 0.07) * (1 - t) - 0.33 * t
    xs = np.linspace(-C.FLOOR_EXTENT, C.FLOOR_EXTENT, 81)
    make_object(coll, "floor_grout", MG.grid(xs, xs, grout_z), ["grout"], C.CLASS_FLOOR, smooth=True)


def build_props(coll, emissive, sc):
    for k, p in enumerate(sc["pillars"]):
        m = MG.Mesh()
        plinth = MG.chamfer_box(1.0, 1.0, -0.1, 0.5, 0.04)
        plinth.mats = [2 if p["moss"] else 1] + [1] * (len(plinth.faces) - 1)
        m.add(plinth)
        m.add(MG.prism(8, 0.7, 0.5, p["height"], top_jag=p["jag"] if p["broken"] else None,
                       phase=math.pi / 8), mat=0)
        if not p["broken"]:
            m.add(MG.chamfer_box(0.9, 0.9, p["height"], p["height"] + 0.35, 0.04), mat=0)
        rot, pos = MG.rot_z(p["yaw"]), (p["x"], p["y"], 0.0)
        make_object(coll, "pillar_%d" % k, m.transformed(rot, pos), ["pillar", "plinth", "moss"],
                    C.CLASS_PROP, smooth=False, outline=True)
        zs, a0, a1 = p["sconce_z"], SD.SHAFT_APOTHEM, SD.SHAFT_APOTHEM + 0.35
        sc_m = MG.box((a1 - a0) / 2, 0.03, 0.03).transformed(None, ((a0 + a1) / 2, 0, zs - 0.1))
        sc_m.add(MG.lathe([(0, zs - 0.1), (0.1, zs - 0.01), (0.1, zs + 0.02), (0.08, zs + 0.02),
                           (0, zs - 0.05)], 10).transformed(None, (a1, 0, 0)))
        make_object(coll, "sconce_%d" % k, sc_m.transformed(rot, pos), ["bronze"], C.CLASS_PROP,
                    smooth=False, outline=True, outline_scale=0.5)
    for n, t in enumerate(sc["torches"]):
        r = 0.1 if t["kind"] == "sconce" else 0.12
        make_object(emissive, "flame_%d" % n, MG.teardrop(r, 0.35).transformed(None, t["base"]),
                    ["em_torch"], C.CLASS_EMISSIVE, smooth=True, casts_shadow=False)
    rub = MG.Mesh()
    for rb in sc["rubble"]:
        sx, sy, sz = rb["size"]
        rub.add(MG.chamfer_box(sx / 2, sy / 2, -0.02, sz, 0.03).transformed(MG.rot_z(rb["yaw"]), (rb["x"], rb["y"], 0)))
    make_object(coll, "rubble", rub, ["rubble"], C.CLASS_PROP, smooth=False, outline=True)
    w = sc["wall"]
    make_object(coll, "ruin_wall", MG.wall_arc(w["center"], w["length"], w["thickness"], w["columns"], w["moss"]),
                ["ruin_wall", "moss"], C.CLASS_PROP, smooth=False, outline=True)
    tp = sc["toppled"]
    jag = tp["end_jag"]
    prism = MG.prism(8, tp["radius"], -tp["length"] / 2, tp["length"] / 2, top_jag=jag[8:], bottom_jag=jag[:8],
                     phase=math.pi / 8, top_cap=True, bottom_cap=True)
    rot = MG.rot_z(tp["yaw"]) @ MG.rot_y(math.pi / 2)
    make_object(coll, "toppled_pillar", prism.transformed(rot, (tp["x"], tp["y"], tp["radius"] * math.cos(math.pi / 8))),
                ["pillar"], C.CLASS_PROP, smooth=False, outline=True)
    al = sc["altar"]
    make_object(coll, "altar", MG.chamfer_box(al["size"][0] / 2, al["size"][1] / 2, -0.05, al["size"][2], 0.05)
                .transformed(None, (al["x"], al["y"], 0)), ["altar"], C.CLASS_PROP, smooth=False, outline=True)
    for n, c in enumerate(sc["candles"]):
        z0, z1 = al["size"][2], al["size"][2] + c["h"]
        cyl = MG.lathe([(0, z0), (c["r"], z0), (c["r"], z1), (0, z1)], 10).transformed(None, (c["x"], c["y"], 0))
        make_object(coll, "candle_%d" % n, cyl, ["wax"], C.CLASS_PROP, smooth=False, outline=True, outline_scale=0.5)
        make_object(emissive, "candle_flame_%d" % n, MG.teardrop(0.03, 0.13).transformed(None, (c["x"], c["y"], z1)),
                    ["em_candle"], C.CLASS_EMISSIVE, smooth=True, casts_shadow=False)
    for n, b in enumerate(sc["braziers"]):
        br, bz = b["bowl_r"], b["bowl_z"]
        th = np.linspace(0, math.pi / 2, 7)
        prof = [(br * math.sin(t), bz - 0.3 * math.cos(t)) for t in th]
        prof += [(br - 0.02, bz + 0.03)]
        prof += [((br - 0.04) * math.sin(t), bz + 0.03 - 0.25 * math.cos(t)) for t in th[::-1]]
        m = MG.lathe(prof, 20)
        for k in range(3):
            a = 2 * math.pi * k / 3
            m.add(MG.cylinder_between((0.22 * math.cos(a), 0.22 * math.sin(a), bz - 0.12),
                                      (0.45 * math.cos(a), 0.45 * math.sin(a), 0.0), 0.03))
        make_object(coll, "brazier_%d" % n, m.transformed(None, (b["x"], b["y"], 0)), ["bronze"], C.CLASS_PROP,
                    smooth=True, outline=True, outline_scale=0.5)


def build_figures(coll, emissive, sc):
    p = sc["player"]
    rot, pos = MG.rot_z(p["yaw"]), (p["x"], p["y"], 0.0)
    m = MG.capsule(0.35, 0.0, 1.3, 16, 6).with_mat(0)
    m.add(MG.lathe([(0.55, 0.0), (0.30, 0.9)], 20), mat=0)
    m.add(sphere_at(0.28, (0, 0, 1.55), 18, 10), mat=0)
    m.add(MG.cone_between((0, -0.18, 1.62), (0, -0.5, 1.42), 0.14), mat=0)
    m.add(MG.ellipsoid((0.17, 0.04, 0.2), 14, 8).transformed(None, (0, 0.26, 1.53)), mat=1)
    m.add(MG.ellipsoid((0.11, 0.035, 0.13), 12, 6).transformed(None, (0, 0.29, 1.52)), mat=2)
    make_object(coll, "player", m.transformed(rot, pos), ["cloak", "hood_inside", "mask"], C.CLASS_PLAYER,
                smooth=True, figure_id=p["figure_id"], outline=True)
    staff = MG.lathe([(0, 0), (0.04, 0), (0.04, 1.7), (0, 1.7)], 8).transformed(None, (0.45, 0.1, 0))
    make_object(coll, "player_staff", staff.transformed(rot, pos), ["staff_wood"], C.CLASS_PLAYER, smooth=True,
                figure_id=p["figure_id"], outline=True, outline_scale=0.5)
    make_object(emissive, "staff_orb", sphere_at(0.09, p["orb"], 14, 8), ["em_orb"], C.CLASS_EMISSIVE, smooth=True,
                figure_id=p["figure_id"], casts_shadow=False)
    for n, f in enumerate(sc["imps"]):
        rot, pos = MG.rot_z(f["yaw"]), (f["x"], f["y"], 0.0)
        m = MG.capsule(0.3, 0.0, 0.9, 16, 6).with_mat(0)
        for sx in (-1, 1):
            base = np.array((0.14 * sx, 0.02, 0.78))
            d = np.array((sx * f["horn_splay"], 0.0, 1.0))
            m.add(MG.cone_between(base, base + 0.3 * d / np.linalg.norm(d), 0.07), mat=1)
        make_object(coll, "imp_%d" % n, m.transformed(rot, pos), ["imp_skin", "horn"], C.CLASS_ENEMY, smooth=True,
                    figure_id=f["figure_id"], outline=True)
        eyes = MG.Mesh()
        for e in f["eyes"]:
            eyes.add(sphere_at(0.045, e, 8, 5))
        make_object(emissive, "imp_eyes_%d" % n, eyes, ["em_eye"], C.CLASS_EMISSIVE, smooth=True,
                    figure_id=f["figure_id"], casts_shadow=False)
    for n, f in enumerate(sc["brutes"]):
        rot, pos = MG.rot_z(f["yaw"]), (f["x"], f["y"], 0.0)
        m = MG.ellipsoid(SD.BRUTE["radii"], 28, 16).transformed(MG.rot_x(math.radians(-SD.BRUTE["hunch_deg"])),
                                                              (0, 0, SD.BRUTE["center_z"])).with_mat(0)
        for sx in (-1, 1):
            m.add(sphere_at(0.45, SD.brute_local((0.95 * sx, 0.05, 0.55)), 16, 10), mat=1)
        make_object(coll, "brute_%d" % n, m.transformed(rot, pos), ["brute_flesh", "plates"], C.CLASS_ENEMY,
                    smooth=True, figure_id=f["figure_id"], outline=True)
        eyes = MG.Mesh()
        for e in f["eyes"]:
            eyes.add(sphere_at(0.07, e, 8, 5))
        make_object(emissive, "brute_eyes_%d" % n, eyes, ["em_eye"], C.CLASS_EMISSIVE, smooth=True,
                    figure_id=f["figure_id"], casts_shadow=False)


def build_bolts(coll, bolts, name):
    m = MG.Mesh()
    for b in bolts:
        m.add(MG.capsule(0.04, -0.225, 0.225, 8, 3).transformed(MG.rot_z_to(b["dir"]), b["pos"]))
    return make_object(coll, name, m, ["em_bolt"], C.CLASS_EMISSIVE, smooth=True, casts_shadow=False)


# ------------------------------------------------------- world and lights ---
def build_world(scene):
    w = bpy.data.worlds.new("world")
    scene.world = w
    w.use_nodes = True
    nt = w.node_tree
    nt.nodes.clear()
    tc = nt.nodes.new("ShaderNodeTexCoord")
    sep = nt.nodes.new("ShaderNodeSeparateXYZ")
    mr = nt.nodes.new("ShaderNodeMapRange")
    mr.inputs["From Min"].default_value = -1.0
    mix = nt.nodes.new("ShaderNodeMix")
    mix.data_type = 'RGBA'
    mix.inputs["A"].default_value = C.hex_lin(C.AMBIENT["ground"]) + (1.0,)
    mix.inputs["B"].default_value = C.hex_lin(C.AMBIENT["sky"]) + (1.0,)
    amb = nt.nodes.new("ShaderNodeBackground")
    amb.name = "ambient_bg"
    void = nt.nodes.new("ShaderNodeBackground")
    void.inputs["Color"].default_value = C.hex_lin(C.VOID) + (1.0,)
    void.inputs["Strength"].default_value = 1.0
    lp = nt.nodes.new("ShaderNodeLightPath")
    ms = nt.nodes.new("ShaderNodeMixShader")
    out = nt.nodes.new("ShaderNodeOutputWorld")
    nt.links.new(tc.outputs["Generated"], sep.inputs[0])
    nt.links.new(sep.outputs["Z"], mr.inputs["Value"])
    nt.links.new(mr.outputs["Result"], mix.inputs["Factor"])
    nt.links.new(mix.outputs["Result"], amb.inputs["Color"])
    nt.links.new(lp.outputs["Is Camera Ray"], ms.inputs["Fac"])
    nt.links.new(amb.outputs[0], ms.inputs[1])
    nt.links.new(void.outputs[0], ms.inputs[2])
    nt.links.new(ms.outputs[0], out.inputs["Surface"])
    w["design_intensity"] = C.AMBIENT["intensity"]


def apply_light_units(scene, units):
    """Set Blender light values from design intensities (also used by render.py)."""
    g = units["light_scale"]
    for ob in scene.objects:
        if ob.type != 'LIGHT' or "design_intensity" not in ob:
            continue
        if ob.data.type == 'SUN':
            ob.data.energy = g * ob["design_intensity"] / units["k_sun"]
        else:
            ob.data.energy = g * ob["design_intensity"] / units["k_point"]
    scene.world.node_tree.nodes["ambient_bg"].inputs["Strength"].default_value = \
        g * scene.world["design_intensity"] / units["k_world"]


def build_lights(coll, lights):
    for L in lights:
        ld = bpy.data.lights.new(L["name"], 'POINT')
        ld.color = C.hex_lin(L["color"])
        ld.shadow_soft_size = C.POINT_SOFT_RADIUS
        pos, cutoff = SD.blender_light_placement(L["pos"], L["radius"])
        ld.use_custom_distance = True
        ld.cutoff_distance = cutoff
        ld.use_shadow = bool(L["shadow"])
        ob = bpy.data.objects.new(L["name"], ld)
        ob.location = pos
        ob["design_pos"] = L["pos"]
        ob["design_intensity"] = L["intensity"]
        coll.objects.link(ob)


def build_sun(coll):
    ld = bpy.data.lights.new("moon", 'SUN')
    ld.color = C.hex_lin(C.MOON["color"])
    ld.angle = math.radians(C.MOON["angle_deg"])
    ld.use_shadow = True
    ob = bpy.data.objects.new("moon", ld)
    ob.rotation_euler = Vector(C.MOON["dir"]).normalized().to_track_quat('-Z', 'Y').to_euler()
    ob["design_intensity"] = C.MOON["intensity"]
    coll.objects.link(ob)


def build_camera(scene, cam_spec):
    cam = bpy.data.cameras.new("camera")
    cam.sensor_fit = 'VERTICAL'
    cam.angle_y = math.radians(cam_spec["vfov_deg"])
    cam.clip_start, cam.clip_end = cam_spec["clip"]
    ob = bpy.data.objects.new("camera", cam)
    ob.location = cam_spec["eye"]
    ob.rotation_euler = (math.radians(90.0 - cam_spec["pitch_deg"]), 0.0, 0.0)
    scene.collection.objects.link(ob)
    scene.camera = ob
    scene.render.resolution_x, scene.render.resolution_y = C.WIDTH, C.HEIGHT
    scene.render.resolution_percentage = 100
    bpy.context.view_layer.update()
    dg = bpy.context.evaluated_depsgraph_get()
    proj = ob.calc_matrix_camera(dg, x=C.WIDTH, y=C.HEIGHT)
    view = ob.matrix_world.inverted()
    return dict(view=[list(r) for r in view], proj=[list(r) for r in proj],
                right=list(ob.matrix_world.to_3x3().col[0]), width=C.WIDTH, height=C.HEIGHT)


def main():
    args = parse_args()
    out_dir = os.path.abspath(args.out_dir)
    os.makedirs(out_dir, exist_ok=True)
    tile_sizes = TX.load_tile_sizes(args.tex_snapshot) if args.tex_snapshot else {}
    sc = SD.build()
    C.write_png(os.path.join(out_dir, "floor_mask.png"), SD.floor_mask(sc))

    for ob in list(bpy.data.objects):
        bpy.data.objects.remove(ob)
    scene = bpy.context.scene
    scene.render.engine = 'BLENDER_EEVEE'
    make_placeholder_materials()
    for key in C.EMISSIVE:
        make_emissive_material(key)
    colls = {}
    for name in ("floor", "props", "figures", "emissive", "lights", "lights_busy", "bolts"):
        colls[name] = bpy.data.collections.new(name)
        scene.collection.children.link(colls[name])
    build_floor(colls["floor"], sc)
    build_props(colls["props"], colls["emissive"], sc)
    build_figures(colls["figures"], colls["emissive"], sc)
    apply_uvs(scene, tile_sizes)
    build_world(scene)
    build_sun(colls["lights"])
    build_lights(colls["lights"], sc["lights_calm"])
    cam = build_camera(scene, sc["camera"])
    scene["floor_mask"] = "//floor_mask.png"

    info = SD.to_jsonable(dict(
        seed=sc["seed"], camera=dict(sc["camera"], **cam), player=sc["player"], imps=sc["imps"],
        brutes=sc["brutes"], blobs=sc["blobs"], torches=sc["torches"], wall=dict(center=sc["wall"]["center"],
        shift=sc["wall"]["shift"]), lights_calm=sc["lights_calm"], lights_busy=sc["lights_busy"],
        bolts=sc["bolts"], bullets_calm=sc["bullets_calm"], bullets_busy=sc["bullets_busy"],
        bullet_columns=["x", "y", "vx", "vy", "silhouette", "palette"], unit_defaults=C.UNIT_DEFAULTS,
        counts=dict(tiles=len(sc["floor"]["tiles"]), lights_calm=len(sc["lights_calm"]),
                    lights_busy=len(sc["lights_busy"]), bullets_calm=len(sc["bullets_calm"]),
                    bullets_busy=len(sc["bullets_busy"]), bolts_calm=12, bolts_busy=40)))
    C.save_json(os.path.join(out_dir, "scene.json"), info)

    calm_bolts = build_bolts(colls["bolts"], sc["bolts"][:12], "bolts_calm")
    apply_uvs(scene, tile_sizes)
    scene["variant"] = "calm"
    scene["decal_strength"] = C.DECAL["calm"]
    apply_light_units(scene, C.UNIT_DEFAULTS)
    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out_dir, "scene_calm.blend"), compress=True)

    bpy.data.objects.remove(calm_bolts)
    build_bolts(colls["bolts"], sc["bolts"], "bolts_busy")
    apply_uvs(scene, tile_sizes)
    build_lights(colls["lights_busy"], sc["lights_busy"][len(sc["lights_calm"]):])
    scene["variant"] = "busy"
    scene["decal_strength"] = C.DECAL["busy"]
    apply_light_units(scene, C.UNIT_DEFAULTS)
    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out_dir, "scene_busy.blend"), compress=True)

    # busy_dim: identical to busy except the bullet-cluster lights at reduced intensity.
    dimmed = 0
    for ob in scene.objects:
        if ob.type == 'LIGHT' and ob.name.startswith(("cluster_calm_", "cluster_busy_")):
            ob["design_intensity"] = ob["design_intensity"] * C.BUSY_DIM_CLUSTER_FACTOR
            dimmed += 1
    scene["variant"] = "busy_dim"
    apply_light_units(scene, C.UNIT_DEFAULTS)
    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out_dir, "scene_busy_dim.blend"), compress=True)
    print("BUSY_DIM dimmed cluster lights:", dimmed)
    n_lights = sum(1 for o in scene.objects if o.type == 'LIGHT')
    print("BUILD OK objects=%d lights=%d proj11=%.4f" % (len(scene.objects), n_lights, cam["proj"][1][1]))


if __name__ == "__main__":
    main()
