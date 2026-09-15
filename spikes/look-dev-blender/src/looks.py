"""The three look setups as Blender node trees (toon, stylized, realistic).

All looks share: the albedo table, the floor mask (engraving + blob shadows +
decal emission), the emissive meshes (built in build_scene.py) and the lights.
The per-look calibration is a LIGHT GAIN that multiplies only light-derived
terms (irradiance, ambient, specular). It never scales rims, emissive meshes,
the decal emission or friendly bolts.

Irradiance-based looks read Eevee's summed lighting through Diffuse BSDF ->
Shader to RGB. Eevee cannot expose individual lights to a material, so toon
bands and the stylized wrap act on the total direct light (total irradiance
minus the analytic hemisphere ambient) instead of per light.
"""
import math

import bpy

import common as C

HULL_SETTINGS = dict(offset=1.0, flip=True)   # verified by the solidify topology probe


class NodeBuilder:
    """Tiny helper to build shader node graphs from sockets and constants."""

    def __init__(self, node_tree):
        self.nt = node_tree
        self.nt.nodes.clear()

    def new(self, kind, inputs=None, **props):
        node = self.nt.nodes.new(kind)
        for k, v in props.items():
            setattr(node, k, v)
        for key, val in (inputs or {}).items():
            self.set(node.inputs[key], val)
        return node

    def set(self, socket, val):
        if isinstance(val, bpy.types.NodeSocket):
            self.nt.links.new(val, socket)
        elif val is not None:
            socket.default_value = val

    def math(self, op, a, b=None, clamp=False):
        n = self.new("ShaderNodeMath", operation=op, use_clamp=clamp)
        self.set(n.inputs[0], a)
        if b is not None:
            self.set(n.inputs[1], b)
        return n.outputs[0]

    def vmath(self, op, a, b=None, scale=None):
        n = self.new("ShaderNodeVectorMath", operation=op)
        self.set(n.inputs[0], a)
        if b is not None:
            self.set(n.inputs[1], b)
        if scale is not None:
            self.set(n.inputs["Scale"], scale)
        return n.outputs["Value"] if op in ("DOT_PRODUCT", "LENGTH", "DISTANCE") else n.outputs["Vector"]

    def smoothstep(self, x, lo, hi):
        n = self.new("ShaderNodeMapRange", interpolation_type='SMOOTHSTEP',
                     inputs={"Value": x, "From Min": lo, "From Max": hi})
        return n.outputs["Result"]

    def mix_rgb(self, fac, a, b):
        n = self.new("ShaderNodeMix", data_type='RGBA', inputs={"Factor": fac, "A": a, "B": b})
        return n.outputs["Result"]

    def output(self, shader):
        out = self.new("ShaderNodeOutputMaterial")
        self.nt.links.new(shader, out.inputs["Surface"])


def rgba(hexv, scale=1.0):
    return tuple(v * scale for v in C.hex_lin(hexv)) + (1.0,)


def vec(hexv, scale=1.0):
    return tuple(v * scale for v in C.hex_lin(hexv))


def normalised_tint(hexv):
    lin = C.hex_lin(hexv)
    lum = float(C.luminance(lin))
    return tuple(v / lum for v in lin) + (1.0,)


# ---------------------------------------------------------- shared blocks ---
def floor_inputs(nb, key, mask_image, decal_strength):
    """Albedo socket and decal emission socket for a floor material."""
    geo = nb.new("ShaderNodeNewGeometry")
    uv = nb.vmath("SCALE", nb.vmath("ADD", geo.outputs["Position"], (C.FLOOR_EXTENT, C.FLOOR_EXTENT, 0.0)),
                  scale=1.0 / (2 * C.FLOOR_EXTENT))
    tex = nb.new("ShaderNodeTexImage", inputs={"Vector": uv}, image=mask_image,
                 extension='EXTEND', interpolation='Linear')
    sep = nb.new("ShaderNodeSeparateColor", inputs={"Color": tex.outputs["Color"]})
    engrave, blob = sep.outputs["Red"], sep.outputs["Green"]
    k = 1.0 - C.DECAL["engrave_albedo"]
    factor = nb.math("MULTIPLY", nb.math("SUBTRACT", 1.0, nb.math("MULTIPLY", engrave, k)), blob)
    albedo = nb.vmath("SCALE", vec(C.MATERIALS[key]["albedo"]), scale=factor)
    emission = nb.vmath("SCALE", vec(C.DECAL["color"], decal_strength), scale=engrave)
    return albedo, emission


def albedo_and_emission(nb, key, ctx):
    if key in C.FLOOR_MATERIALS:
        return floor_inputs(nb, key, ctx["mask_image"], ctx["decal_strength"])
    return vec(C.MATERIALS[key]["albedo"]), None


def irradiance_terms(nb, light_scale):
    """Total irradiance T, analytic ambient A, direct D = max(T - A, 0) and its
    luminance in design units x = lum(D) / light_scale."""
    diff = nb.new("ShaderNodeBsdfDiffuse", inputs={"Color": (1, 1, 1, 1), "Roughness": 0.0})
    s2r = nb.new("ShaderNodeShaderToRGB")
    nb.nt.links.new(diff.outputs[0], s2r.inputs[0])
    geo = nb.new("ShaderNodeNewGeometry")
    nz = nb.new("ShaderNodeSeparateXYZ", inputs={"Vector": geo.outputs["Normal"]}).outputs["Z"]
    fac = nb.math("ADD", nb.math("MULTIPLY", nz, 0.5), 0.5, clamp=True)
    amb_col = nb.mix_rgb(fac, rgba(C.AMBIENT["ground"]), rgba(C.AMBIENT["sky"]))
    ambient = nb.vmath("SCALE", amb_col, scale=light_scale * C.AMBIENT["intensity"])
    direct = nb.vmath("MAXIMUM", nb.vmath("SUBTRACT", s2r.outputs["Color"], ambient), (0.0, 0.0, 0.0))
    y = nb.vmath("DOT_PRODUCT", direct, tuple(float(v) for v in C.LUMA))
    x = nb.math("DIVIDE", y, light_scale)
    return dict(direct=direct, ambient=ambient, y=y, x=x, normal_z=nz)


def rim_term(nb, rim, normal_z):
    lw = nb.new("ShaderNodeLayerWeight", inputs={"Blend": 0.5})
    f = nb.math("MULTIPLY", nb.math("POWER", lw.outputs["Facing"], rim["p"]), rim["k"])
    up = nb.math("ADD", nb.math("MULTIPLY", nb.math("ADD", normal_z, 0.3, clamp=True), 0.4), 0.6)
    return nb.vmath("SCALE", vec(rim["color"]), scale=nb.math("MULTIPLY", f, up))


def emission_shader(nb, color):
    return nb.new("ShaderNodeEmission", inputs={"Color": color, "Strength": 1.0}).outputs[0]


# ------------------------------------------------------------------ looks ---
def build_toon(mat, key, ctx):
    p, g, gain = ctx["params"]["toon"], ctx["light_scale"], ctx["gain"]
    nb = NodeBuilder(mat.node_tree)
    albedo, emission = albedo_and_emission(nb, key, ctx)
    it = irradiance_terms(nb, g)
    l0, l1, l2 = p["band_levels"]
    e = p["edge_rel"]
    b1 = nb.smoothstep(it["x"], p["t1"] * (1 - e), p["t1"] * (1 + e))
    b2 = nb.smoothstep(it["x"], p["t2"] * (1 - e), p["t2"] * (1 + e))
    band = nb.math("ADD", nb.math("ADD", l0, nb.math("MULTIPLY", b1, l1 - l0)), nb.math("MULTIPLY", b2, l2 - l1))
    inv_y = nb.math("DIVIDE", 1.0, nb.math("MAXIMUM", it["y"], 1e-4 * g))
    hue = nb.vmath("MINIMUM", nb.vmath("SCALE", it["direct"], scale=inv_y), (p["hue_clamp"],) * 3)
    banded = nb.vmath("SCALE", hue, scale=nb.math("MULTIPLY", band, p["band_full"] * g))
    light = nb.vmath("SCALE", nb.vmath("ADD", it["ambient"], banded), scale=gain)
    color = nb.vmath("MULTIPLY", albedo, light)
    if emission is not None:
        color = nb.vmath("ADD", color, emission)
    nb.output(emission_shader(nb, color))


def build_stylized(mat, key, ctx):
    p, g, gain = ctx["params"]["stylized"], ctx["light_scale"], ctx["gain"]
    spec = C.MATERIALS[key]
    nb = NodeBuilder(mat.node_tree)
    albedo, emission = albedo_and_emission(nb, key, ctx)
    it = irradiance_terms(nb, g)
    w = p["wrap"]
    xn = nb.math("MAXIMUM", nb.math("MINIMUM", nb.math("DIVIDE", it["x"], p["wrap_ref"]), 1.0), 1e-4)
    xl = nb.math("POWER", xn, 1.0 / (1.0 + w))
    lift = nb.math("DIVIDE", xl, xn)
    tint_fac = nb.smoothstep(xl, p["tint_smooth"][0], p["tint_smooth"][1])
    tint = normalised_tint(spec["tint"])
    tint_col = nb.mix_rgb(tint_fac, tint, (1.0, 1.0, 1.0, 1.0))
    direct = nb.vmath("MULTIPLY", nb.vmath("SCALE", it["direct"], scale=lift), tint_col)
    ambient = nb.vmath("MULTIPLY", it["ambient"], tint[:3])
    light = nb.vmath("SCALE", nb.vmath("ADD", direct, ambient), scale=gain)
    color = nb.vmath("MULTIPLY", albedo, light)
    if emission is not None:
        color = nb.vmath("ADD", color, emission)
    if spec.get("rim"):
        color = nb.vmath("ADD", color, rim_term(nb, p["rim"][spec["rim"]], it["normal_z"]))
    spec_col = C.hex_lin(spec["albedo"]) if spec.get("spec_albedo") else (1.0, 1.0, 1.0)
    ks = spec["ks"] * p["spec_gain"] * gain
    try:
        glossy = nb.new("ShaderNodeBsdfGlossy")
    except RuntimeError:
        glossy = nb.new("ShaderNodeBsdfAnisotropic")
    if hasattr(glossy, "distribution"):
        glossy.distribution = 'GGX'
    glossy.inputs["Color"].default_value = tuple(v * ks for v in spec_col) + (1.0,)
    glossy.inputs["Roughness"].default_value = C.gloss_to_roughness(spec["g"])
    add = nb.new("ShaderNodeAddShader")
    nb.nt.links.new(emission_shader(nb, color), add.inputs[0])
    nb.nt.links.new(glossy.outputs[0], add.inputs[1])
    nb.output(add.outputs[0])


def build_realistic(mat, key, ctx):
    p, gain = ctx["params"]["realistic"], ctx["gain"]
    spec = C.MATERIALS[key]
    nb = NodeBuilder(mat.node_tree)
    albedo, emission = albedo_and_emission(nb, key, ctx)
    bsdf = nb.new("ShaderNodeBsdfPrincipled", inputs={
        "Base Color": albedo if isinstance(albedo, bpy.types.NodeSocket) else albedo + (1.0,),
        "Metallic": spec["m"], "Roughness": max(spec["r"], p["rough_min"])})
    if abs(gain - 1.0) < 1e-6:
        if emission is not None:
            nb.set(bsdf.inputs["Emission Color"], emission)
            bsdf.inputs["Emission Strength"].default_value = 1.0
        nb.output(bsdf.outputs[0])
        return
    # Gain != 1 (only during calibration checks): light-derived part via Shader to RGB.
    s2r = nb.new("ShaderNodeShaderToRGB")
    nb.nt.links.new(bsdf.outputs[0], s2r.inputs[0])
    color = nb.vmath("SCALE", s2r.outputs["Color"], scale=gain)
    if emission is not None:
        color = nb.vmath("ADD", color, emission)
    nb.output(emission_shader(nb, color))


BUILDERS = {"toon": build_toon, "stylized": build_stylized, "realistic": build_realistic}


# ---------------------------------------------------------------- outline ---
def outline_material():
    mat = bpy.data.materials.get("outline") or bpy.data.materials.new("outline")
    mat.use_nodes = True
    nb = NodeBuilder(mat.node_tree)
    nb.output(emission_shader(nb, rgba(C.LOOK_PARAMS["toon"]["outline_color"])))
    mat.use_backface_culling = True
    mat.use_backface_culling_shadow = True
    return mat


def add_outlines(scene, params):
    """Inverted hull: Solidify shell pushed outward with flipped normals and a
    backface-culled near-black material (props and figures only)."""
    mat = outline_material()
    count = 0
    for ob in scene.objects:
        if ob.type != 'MESH' or not ob.get("outline"):
            continue
        n = len(ob.data.materials)
        for _ in range(n):
            ob.data.materials.append(mat)
        md = ob.modifiers.new("outline_hull", 'SOLIDIFY')
        md.thickness = params["outline_thickness"] * float(ob.get("outline_scale", 1.0))
        md.offset = HULL_SETTINGS["offset"]
        md.use_flip_normals = HULL_SETTINGS["flip"]
        md.use_even_offset = True
        md.use_quality_normals = True
        md.use_rim = False
        md.material_offset = n
        count += 1
    return count


# ------------------------------------------------------------------ entry ---
def apply_look(scene, look, params, light_scale, gain, mask_path):
    mask_image = bpy.data.images.load(bpy.path.abspath(mask_path), check_existing=True)
    mask_image.colorspace_settings.name = 'Non-Color'
    ctx = dict(params=params, light_scale=light_scale, gain=gain, mask_image=mask_image,
               decal_strength=float(scene["decal_strength"]))
    for key in C.MATERIALS:
        BUILDERS[look](bpy.data.materials[key], key, ctx)
    outlines = add_outlines(scene, params["toon"]) if look == "toon" else 0
    return dict(look=look, gain=gain, light_scale=light_scale, outlined_objects=outlines)


def classmask_material():
    """View-layer override: R = pass_index*16 + figure_id + 1, G/B = world XY in [0, 1]."""
    mat = bpy.data.materials.new("classmask")
    mat.use_nodes = True
    nb = NodeBuilder(mat.node_tree)
    info = nb.new("ShaderNodeObjectInfo")
    attr = nb.new("ShaderNodeAttribute", attribute_type='OBJECT', attribute_name="figure_id")
    r = nb.math("ADD", nb.math("ADD", nb.math("MULTIPLY", info.outputs["Object Index"], 16.0), attr.outputs["Fac"]), 1.0)
    geo = nb.new("ShaderNodeNewGeometry")
    sep = nb.new("ShaderNodeSeparateXYZ", inputs={"Vector": geo.outputs["Position"]})
    gx = nb.math("DIVIDE", nb.math("ADD", sep.outputs["X"], C.FLOOR_EXTENT), 2 * C.FLOOR_EXTENT)
    gy = nb.math("DIVIDE", nb.math("ADD", sep.outputs["Y"], C.FLOOR_EXTENT), 2 * C.FLOOR_EXTENT)
    comb = nb.new("ShaderNodeCombineColor", inputs={"Red": r, "Green": gx, "Blue": gy})
    nb.output(emission_shader(nb, comb.outputs["Color"]))
    return mat
