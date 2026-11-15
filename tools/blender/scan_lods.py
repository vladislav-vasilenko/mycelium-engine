"""Сканы Poly Haven → сетки растений движка (02.11, сессия 2).

Запуск: `blender -b -P tools/blender/scan_lods.py -- [имена…] [--heavy]`.
Без имён — все лёгкие сканы из `assets/source/polyhaven/manifest.json`;
`--heavy` — и деревья-сканы.

Каждый объект скана — свой вариант (наборы камней, папоротников, корней,
кустов дают по несколько вариантов): основание (центр по XZ, низ по Y) — в
начале координат; упрощение Decimate до бюджетов LOD 0–2 по роли; запись
`assets/models/scans/<вариант>.glb` в формате растений движка (сетки
`lod0..lod2`, POSITION, NORMAL, TEXCOORD_0, COLOR_0: R — слот материала
скана, G — ветер, B — затенение 1, A — мох 0). Текстуры материалов (цвет,
нормаль OpenGL, шероховатость/ARM) копируются в `assets/models/scans/<скан>/`,
опись — `assets/models/scans/scans.json`.
"""

import json
import math
import shutil
import struct
import sys
from pathlib import Path

import bmesh
import bpy
import numpy as np

ROOT = Path(bpy.path.abspath("//")) if bpy.data.filepath else Path.cwd()
SRC = ROOT / "assets/source/polyhaven"
OUT = ROOT / "assets/models/scans"

# Бюджеты треугольников LOD 0, 1, 2 по роли (вариант не больше исходника).
BUDGET = {
    "rock": (8000, 2000, 500),
    "wood": (8000, 2000, 500),
    "foliage": (6000, 2500, 900),
    "moss": (4000, 1000, 300),
    "tree": (80000, 16000, 4000),
}


def role_of(name):
    if name in ("island_tree_02", "tree_small_02", "fir_tree_01", "fir_sapling"):
        return "tree"
    if name.startswith(("fern", "shrub", "grass")):
        return "foliage"
    if name.startswith("moss"):
        return "moss"
    if name.startswith(("rock", "boulder", "stone")):
        return "rock"
    return "wood"


def material_textures(gltf_path):
    """Слоты материалов: имя, режим альфы и файлы текстур из glTF."""
    g = json.loads(gltf_path.read_text())
    images = [i.get("uri") for i in g.get("images", [])]
    tex = g.get("textures", [])

    def img(ref):
        return images[tex[ref["index"]]["source"]] if ref else None

    out = []
    for m in g.get("materials", []):
        pbr = m.get("pbrMetallicRoughness", {})
        albedo = img(pbr.get("baseColorTexture"))
        # Альфа-маска рядом с цветом: X_diff_1k.jpg → X_alpha_1k.png (fetch_polyhaven.py).
        mask = albedo.replace("_diff_", "_alpha_").rsplit(".", 1)[0] + ".png" if albedo else None
        out.append({
            "name": m.get("name"),
            "alpha": m.get("alphaMode", "OPAQUE"),
            "mask": mask if mask and (gltf_path.parent / mask).exists() else None,
            "albedo": albedo,
            "normal": img(m.get("normalTexture")),
            "rough": img(pbr.get("metallicRoughnessTexture")),
        })
    return out


def clear_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def triangles(obj):
    return sum(len(p.vertices) - 2 for p in obj.data.polygons)


def decimated(obj, target):
    """Копия объекта, упрощённая до ~target треугольников."""
    c = obj.copy()
    c.data = obj.data.copy()
    bpy.context.collection.objects.link(c)
    # Несколько проходов: на разрывах UV один проход Decimate останавливается рано.
    for _ in range(4):
        n = triangles(c)
        if target >= n * 0.98:
            break
        mod = c.modifiers.new("dec", "DECIMATE")
        mod.decimate_type = "COLLAPSE"
        mod.ratio = max(target / n, 0.002)
        mod.use_collapse_triangulate = True
        bpy.context.view_layer.objects.active = c
        bpy.ops.object.modifier_apply(modifier=mod.name)
        if triangles(c) >= n * 0.98:
            # Не сдвинулось — без границ UV и с объединением по расстоянию.
            bpy.ops.object.mode_set(mode="EDIT")
            bpy.ops.mesh.select_all(action="SELECT")
            bpy.ops.mesh.remove_doubles(threshold=0.0005)
            bpy.ops.object.mode_set(mode="OBJECT")
    return c


def thin_foliage(obj, target, alpha_slots, seed=7):
    """Листва из отдельных карточек не упрощается Decimate: на дальних LOD
    выбрасываем часть островков листвы (связных кусков) случайно."""
    n = triangles(obj)
    if n <= target or not alpha_slots:
        return
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    bm.faces.ensure_lookup_table()
    leaf_tris = sum(len(f.verts) - 2 for f in bm.faces if f.material_index in alpha_slots)
    drop = min(1.0, (n - target) / max(leaf_tris, 1))
    rng = np.random.default_rng(seed)
    seen = set()
    kill = []
    for f in bm.faces:
        if f.index in seen or f.material_index not in alpha_slots:
            continue
        island, stack = [], [f]
        seen.add(f.index)
        while stack:
            g = stack.pop()
            island.append(g)
            for e in g.edges:
                for h in e.link_faces:
                    if h.index not in seen and h.material_index in alpha_slots:
                        seen.add(h.index)
                        stack.append(h)
        if rng.random() < drop:
            kill.extend(island)
    bmesh.ops.delete(bm, geom=kill, context="FACES")
    bm.to_mesh(obj.data)
    bm.free()
    obj.data.update()


def mesh_arrays(obj, slot_map, wind_of_slot, height):
    """Треугольники → массивы вершин (по углам, без слияния) в осях glTF."""
    me = obj.data
    me.calc_loop_triangles()
    tris = me.loop_triangles
    n = len(tris)
    loops = np.empty(n * 3, np.int32)
    tris.foreach_get("loops", loops)
    verts_of_loop = np.empty(len(me.loops), np.int32)
    me.loops.foreach_get("vertex_index", verts_of_loop)
    co = np.empty(len(me.vertices) * 3, np.float32)
    me.vertices.foreach_get("co", co)
    co = co.reshape(-1, 3)
    normals = np.empty(len(me.loops) * 3, np.float32)
    me.corner_normals.foreach_get("vector", normals) if hasattr(me, "corner_normals") else me.loops.foreach_get("normal", normals)
    normals = normals.reshape(-1, 3)
    uv = np.zeros((len(me.loops), 2), np.float32)
    if me.uv_layers.active:
        me.uv_layers.active.data.foreach_get("uv", uv.reshape(-1))
    mats = np.empty(n, np.int32)
    tris.foreach_get("material_index", mats)
    p = co[verts_of_loop[loops]]
    nr = normals[loops]
    t = uv[loops]
    # Blender Z вверх → glTF Y вверх: (x, y, z) → (x, z, −y); v текстуры перевёрнут.
    pos = np.stack([p[:, 0], p[:, 2], -p[:, 1]], 1)
    nrm = np.stack([nr[:, 0], nr[:, 2], -nr[:, 1]], 1)
    t = np.stack([t[:, 0], 1.0 - t[:, 1]], 1)
    slot = np.repeat(np.array([slot_map.get(m, 0) for m in mats], np.uint8), 3)
    wind = np.array([wind_of_slot.get(int(s), 0.0) for s in slot], np.float32)
    wind *= np.clip(pos[:, 1] / max(height, 0.3), 0.0, 1.0)
    col = np.stack([slot, (wind * 255).astype(np.uint8), np.full(len(slot), 255, np.uint8),
                    np.zeros(len(slot), np.uint8)], 1)
    # Слияние одинаковых вершин (позиция, нормаль, uv, атрибуты) — в разы меньше.
    pos, nrm, t = pos.astype(np.float32), nrm.astype(np.float32), t.astype(np.float32)
    rows = np.concatenate([pos.view(np.uint32), nrm.view(np.uint32), t.view(np.uint32),
                           col.astype(np.uint32)], 1)
    uniq, first, inv = np.unique(rows, axis=0, return_index=True, return_inverse=True)
    return pos[first], nrm[first], t[first], col[first], inv.reshape(-1).astype(np.uint32)


def write_glb(path, lods):
    blob = bytearray()
    views, accessors, meshes, nodes = [], [], [], []

    def add(data, target):
        while len(blob) % 4:
            blob.append(0)
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(data), "target": target})
        blob.extend(data)
        return len(views) - 1

    for li, (p, n, t, c, i) in enumerate(lods):
        acc = {}
        for name, arr, typ, comp, norm in (("POSITION", p, "VEC3", 5126, False), ("NORMAL", n, "VEC3", 5126, False),
                                           ("TEXCOORD_0", t, "VEC2", 5126, False), ("COLOR_0", c, "VEC4", 5121, True)):
            a = {"bufferView": add(arr.tobytes(), 34962), "componentType": comp, "count": len(arr), "type": typ}
            if norm:
                a["normalized"] = True
            if name == "POSITION":
                a["min"], a["max"] = p.min(0).tolist(), p.max(0).tolist()
            accessors.append(a)
            acc[name] = len(accessors) - 1
        accessors.append({"bufferView": add(i.tobytes(), 34963), "componentType": 5125, "count": len(i), "type": "SCALAR"})
        meshes.append({"name": f"lod{li}", "primitives": [{"attributes": acc, "indices": len(accessors) - 1}]})
        nodes.append({"name": f"lod{li}", "mesh": li})
    doc = {"asset": {"version": "2.0", "generator": "tools/blender/scan_lods.py"}, "scene": 0,
           "scenes": [{"nodes": list(range(len(nodes)))}], "nodes": nodes, "meshes": meshes,
           "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": len(blob)}]}
    js = json.dumps(doc, separators=(",", ":")).encode()
    js += b" " * (-len(js) % 4)
    while len(blob) % 4:
        blob.append(0)
    with open(path, "wb") as f:
        f.write(struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(js) + 8 + len(blob)))
        f.write(struct.pack("<II", len(js), 0x4E4F534A) + js)
        f.write(struct.pack("<II", len(blob), 0x004E4942) + bytes(blob))


def process(name, catalog):
    gltf = next((SRC / name).glob("*.gltf"))
    mats = material_textures(gltf)
    clear_scene()
    bpy.ops.import_scene.gltf(filepath=str(gltf))
    # Пустые и не нулевые LOD-объекты скана — пропускаем.
    objs = [o for o in bpy.context.scene.objects
            if o.type == "MESH" and len(o.data.vertices) >= 3 and "_LOD" not in o.name.replace("_LOD0", "")]
    role = role_of(name)
    # Объект скана — вариант (у ели и подроста — три дерева a, b, c).
    (OUT / name).mkdir(parents=True, exist_ok=True)
    mat_names = [m["name"] for m in mats]
    for m in mats:
        for key in ("albedo", "normal", "rough", "mask"):
            if m[key]:
                src = gltf.parent / m[key]
                shutil.copyfile(src, OUT / name / Path(m[key]).name)
                m[key] = f"{name}/{Path(m[key]).name}"
    wind_of_slot = {k: (0.7 if m["alpha"] != "OPAQUE" else (0.15 if role == "tree" else 0.0))
                    for k, m in enumerate(mats)}
    for obj in objs:
        # Применить преобразования; основание — в начало координат.
        bpy.context.view_layer.objects.active = obj
        obj.select_set(True)
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
        co = np.empty(len(obj.data.vertices) * 3, np.float32)
        obj.data.vertices.foreach_get("co", co)
        co = co.reshape(-1, 3)
        lo, hi = co.min(0), co.max(0)
        shift = np.array([-(lo[0] + hi[0]) / 2, -(lo[1] + hi[1]) / 2, -lo[2]], np.float32)
        obj.data.vertices.foreach_set("co", (co + shift).reshape(-1))
        obj.data.update()
        height = float(hi[2] - lo[2])
        slot_map = {i: mat_names.index(s.material.name) if s.material and s.material.name in mat_names else 0
                    for i, s in enumerate(obj.material_slots)}
        src_tris = triangles(obj)
        lods, counts = [], []
        for budget in BUDGET[role]:
            # LOD 1 и 2 — не больше 45 % и 35 % предыдущего: дешевеют всегда.
            target = min(budget, src_tris if not counts else int(counts[-1] * (0.45 if len(counts) == 1 else 0.35)))
            d = decimated(obj, target)
            alpha_slots = {i for i, m in slot_map.items() if mats[m]["alpha"] != "OPAQUE"}
            thin_foliage(d, target, alpha_slots)
            lods.append(mesh_arrays(d, slot_map, wind_of_slot, height))
            counts.append(len(lods[-1][4]) // 3)
            bpy.data.objects.remove(d, do_unlink=True)
        variant = obj.name.replace(".", "_")
        if not variant.startswith(name):
            variant = f"{name}_{variant}"
        variant = variant.replace("_LOD0", "")
        write_glb(OUT / f"{variant}.glb", lods)
        size = (hi - lo).tolist()
        catalog[variant] = {"scan": name, "role": role, "source_triangles": src_tris, "lod_triangles": counts,
                            "size_m": [round(size[0], 3), round(size[2], 3), round(size[1], 3)],
                            "materials": mats}
        print(f"{variant}: {src_tris} → {counts}, {size[0]:.2f}×{size[2]:.2f}×{size[1]:.2f} м")


def main():
    args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    heavy = "--heavy" in args
    names = [a for a in args if not a.startswith("--")]
    manifest = json.loads((SRC / "manifest.json").read_text())
    if not names:
        # moss_01 — экземпляры веточки мха (2–3 см), не подушка: свой мох лучше.
        # grass_bermuda_01 — одиночные карточки по 12–19 треугольников.
        names = [n for n, m in manifest.items()
                 if (heavy or not m["heavy"]) and n not in ("moss_01", "grass_bermuda_01")]
    OUT.mkdir(parents=True, exist_ok=True)
    cat_path = OUT / "scans.json"
    catalog = json.loads(cat_path.read_text()) if cat_path.exists() else {}
    for name in names:
        process(name, catalog)
        cat_path.write_text(json.dumps(catalog, ensure_ascii=False, indent=1) + "\n")


main()
