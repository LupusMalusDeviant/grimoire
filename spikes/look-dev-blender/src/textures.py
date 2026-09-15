"""Procedural PBR texture application for the "realistic_tex" look.

Two independent jobs, both driven by the material key names in common.MATERIALS:

  * generate_uvs() / apply_uvs_to_scene(): world-scaled box/cube-projected UV
    coordinates, written at scene-build time (build_scene.py), before any look
    is applied. One UV unit spans the mapped material's tile_size_m. Floor
    objects (FLOOR_OBJECT_NAMES) always get a fixed planar-XY projection
    instead of per-face dominant-axis selection, so neighbouring floor tiles
    (each individually, slightly tilted) never flip which axis they project
    on and never show a seam.

  * build_realistic_tex() (called from looks.py) / get_images(): the shader
    side, at render time. Same Principled BSDF setup as "realistic", but base
    colour, normal (Normal Map node, tangent space) and roughness/metallic
    (from the ORM texture, G/B channels) are sampled from the texture
    snapshot instead of being flat constants. AO (ORM red channel) is loaded
    nowhere near the shader: Eevee's Principled BSDF has no socket that folds
    a supplied AO map into indirect/ambient light only, and rigging that by
    hand means Shader-to-RGB'ing the whole BSDF (the toon/stylized trick),
    which would defeat the point of a GGX comparison. So AO is left out
    entirely, per the task's explicit fallback, and reported as such. It is
    never multiplied into the base colour.

No bpy-independent guarantees here (unlike common.py/scene_data.py) -- both
halves need bpy.
"""
import json
import os

import bpy

import common as C

# --------------------------------------------------------- material table ---
# common.MATERIALS key -> texture id (folder name under the snapshot's generated/)
MATERIAL_TEXTURE_MAP = {
    "floor_stone": "floor_tiles",
    "grout": "grout",
    "pillar": "pillar_stone",
    "plinth": "pillar_stone",
    "rubble": "pillar_stone",
    "ruin_wall": "ruin_wall_moss",
    "moss": "ruin_wall_moss",          # wall-top and plinth moss patches: no dedicated
                                        # moss-only texture was delivered, closest match
    "altar": "altar_basalt",
    "bronze": "bronze",
    "wax": "candle_wax",
    "cloak": "cloak_fabric",
    "mask": "bone_mask",
    "staff_wood": "staff_wood",
    "imp_skin": "imp_skin",
    "brute_flesh": "brute_flesh",
    "plates": "shoulder_plates",
}
# Kept as the flat "realistic" material: no texture in the delivered set covers them.
UNMAPPED_MATERIALS = ("hood_inside", "horn")

# Mesh objects that make up the ground: fixed planar-XY projection (see module docstring).
FLOOR_OBJECT_NAMES = frozenset({"floor_tiles", "floor_kerb", "floor_grout"})

UV_NAME = "uv_tex"


# --------------------------------------------------------------------- UVs ---
def load_tile_sizes(snapshot_generated_dir):
    """{texture_id: (tile_w_m, tile_h_m)} read from each mapped material's own
    JSON in the snapshot (not from catalog.json, so the frozen snapshot is
    self-sufficient once copied)."""
    sizes = {}
    for tex_id in sorted(set(MATERIAL_TEXTURE_MAP.values())):
        path = os.path.join(snapshot_generated_dir, tex_id, tex_id + ".json")
        with open(path, "r", encoding="utf-8") as fh:
            d = json.load(fh)
        w, h = d["tile_size_m"]
        sizes[tex_id] = (float(w), float(h))
    return sizes


def generate_uvs(ob, tile_lookup, is_floor):
    """World-scaled box/cube-projected UVs for one mesh object.

    tile_lookup: material_index -> (tile_w_m, tile_h_m). Mesh vertex
    coordinates are already world-space in this codebase (every prop/figure
    object keeps an identity object transform; meshgen bakes position and
    rotation straight into the vertex data), so no matrix_world multiply is
    needed for either position or normal.

    is_floor: always project straight down (world X, Y), ignoring the face
    normal, so the floor's per-tile tilt (up to 1.5 degrees) and the kerb's
    stepped profile never flip the dominant axis between neighbouring faces.
    Everywhere else: per-face dominant-axis ("cube") projection.
    """
    me = ob.data
    if UV_NAME in me.uv_layers:
        return
    uv_layer = me.uv_layers.new(name=UV_NAME)
    uv_layer.active = True
    uv_data = uv_layer.data
    for poly in me.polygons:
        tile_w, tile_h = tile_lookup.get(poly.material_index, (1.0, 1.0))
        if is_floor:
            axis = "z"
        else:
            n = poly.normal
            ax, ay, az = abs(n.x), abs(n.y), abs(n.z)
            if az >= ax and az >= ay:
                axis = "z"
            elif ax >= ay:
                axis = "x"
            else:
                axis = "y"
        for li in range(poly.loop_start, poly.loop_start + poly.loop_total):
            vi = me.loops[li].vertex_index
            co = me.vertices[vi].co
            if axis == "z":
                u, v = co.x / tile_w, co.y / tile_h
            elif axis == "x":
                u, v = co.y / tile_w, co.z / tile_h
            else:
                u, v = co.x / tile_w, co.z / tile_h
            uv_data[li].uv = (u, v)


def apply_uvs_to_scene(scene, tile_sizes):
    """Generate UVs (idempotent) for every mesh object in the scene that
    doesn't have the uv_tex layer yet. Safe to call repeatedly as more
    objects (bolts) get added over the course of build_scene.py."""
    n = 0
    for ob in scene.objects:
        if ob.type != 'MESH' or UV_NAME in ob.data.uv_layers:
            continue
        tile_lookup = {idx: tile_sizes.get(MATERIAL_TEXTURE_MAP.get(mat.name), (1.0, 1.0))
                       for idx, mat in enumerate(ob.data.materials)}
        generate_uvs(ob, tile_lookup, is_floor=(ob.name in FLOOR_OBJECT_NAMES))
        n += 1
    return n


# ---------------------------------------------------------------- images ---
_IMAGE_CACHE = {}


def get_images(tex_id, tex_dir):
    """Load (and cache) basecolor/normal/orm for one texture id, with the
    colour spaces the texture author's README specifies: basecolor sRGB,
    normal and ORM Non-Color."""
    if tex_id in _IMAGE_CACHE:
        return _IMAGE_CACHE[tex_id]
    mat_dir = os.path.join(tex_dir, tex_id)

    def load(suffix, colorspace):
        path = os.path.join(mat_dir, "%s_%s.png" % (tex_id, suffix))
        img = bpy.data.images.load(path, check_existing=True)
        img.colorspace_settings.name = colorspace
        return img

    imgs = dict(basecolor=load("basecolor", "sRGB"),
                normal=load("normal", "Non-Color"),
                orm=load("orm", "Non-Color"))
    _IMAGE_CACHE[tex_id] = imgs
    return imgs


# -------------------------------------------------------------- shading ---
def floor_inputs_textured(nb, key, tex_images, mask_image, decal_strength, uv_socket):
    """Textured equivalent of looks.floor_inputs: same engrave/blob decal
    mask machinery (its own procedural position -> [0,1] UV, unrelated to
    uv_tex), but the constant albedo is replaced by the sampled basecolor
    texture. `nb` is the caller's NodeBuilder for the material being built."""
    geo = nb.new("ShaderNodeNewGeometry")
    mask_uv = nb.vmath("SCALE", nb.vmath("ADD", geo.outputs["Position"], (C.FLOOR_EXTENT, C.FLOOR_EXTENT, 0.0)),
                        scale=1.0 / (2 * C.FLOOR_EXTENT))
    tex = nb.new("ShaderNodeTexImage", inputs={"Vector": mask_uv}, image=mask_image,
                 extension='EXTEND', interpolation='Linear')
    sep = nb.new("ShaderNodeSeparateColor", inputs={"Color": tex.outputs["Color"]})
    engrave, blob = sep.outputs["Red"], sep.outputs["Green"]
    k = 1.0 - C.DECAL["engrave_albedo"]
    factor = nb.math("MULTIPLY", nb.math("SUBTRACT", 1.0, nb.math("MULTIPLY", engrave, k)), blob)
    base_tex = nb.new("ShaderNodeTexImage", inputs={"Vector": uv_socket}, image=tex_images["basecolor"])
    albedo = nb.vmath("SCALE", base_tex.outputs["Color"], scale=factor)
    decal_color = tuple(v * decal_strength for v in C.hex_lin(C.DECAL["color"]))
    emission = nb.vmath("SCALE", decal_color, scale=engrave)
    return albedo, emission
