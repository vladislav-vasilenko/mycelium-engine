"""Предметы в руках (этап 10): сканы Poly Haven (CC0) → `assets/models/props/<имя>.glb`.

Запуск: `python3 tools/props/build_props.py` (после
`python3 tools/plants/fetch_polyhaven.py`, который скачивает исходники в
`assets/source/polyhaven/`).

Что делает для каждого предмета из `PROPS`:
1. Берёт нужные узлы и материал исходного glTF 1K, применяет трансформации узлов.
2. Переносит начало координат в точку хвата `grip` (метры, оси исходника),
   чтобы предмет крепился к кисти этой точкой.
3. Пишет один примитив (POSITION, NORMAL, TEXCOORD_0, индексы u16/u32) и
   встроенную текстуру цвета (JPEG) в `.glb`.

Как предмет висит в руке, задаёт `assets/models/props/props.json` (поворот и
сдвиг в пространстве глаза относительно кисти в позе `grip`).
"""

import json
import math
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "assets/source/polyhaven"
OUT = ROOT / "assets/models/props"

# Имя → (узлы: None — все с материалом `material`; материал; точка хвата, м).
PROPS = {
    # Плоская корзина: держится за край, точка хвата — середина длинной стороны.
    "wicker_basket_01": (None, 0, (0.0, 0.10, 0.14)),
    # Совок: рукоять — узкий конец к −z (ширина 3 см), лезвие — к +z; хват
    # на середине рукояти (первая версия держала за лезвие — совок смотрел
    # на игрока).
    "trowel_01": (None, 0, (0.0, 0.034, -0.095)),
    # Фонарь-коробка с ручкой сверху: хват за ручку, без стекла (материал 1).
    "vintage_flashlight": (None, 0, (0.0, 0.135, -0.02)),
    # Полевая рация: только микрофон на стойке (материал аксессуаров); узел
    # сдвинут от начала исходника — хват за стойку у его позиции.
    "vintage_radio_transceiver": (["vintage_radio_transceiver_microphone"], 1, (0.193, 0.08, 0.177)),
}

COMP = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}


def quat_mat(q):
    x, y, z, w = q
    return [
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ]


def node_xform(node):
    r = quat_mat(node.get("rotation", [0, 0, 0, 1]))
    s = node.get("scale", [1, 1, 1])
    t = node.get("translation", [0, 0, 0])
    m = [[r[i][j] * s[j] for j in range(3)] for i in range(3)]

    def point(p):
        return [sum(m[i][j] * p[j] for j in range(3)) + t[i] for i in range(3)]

    def normal(n):
        v = [sum(r[i][j] * n[j] for j in range(3)) for i in range(3)]
        l = math.sqrt(sum(c * c for c in v)) or 1.0
        return [c / l for c in v]

    return point, normal


def read(g, binary, idx):
    acc = g["accessors"][idx]
    view = g["bufferViews"][acc["bufferView"]]
    fmt = COMP[acc["componentType"]]
    n = NCOMP[acc["type"]]
    stride = view.get("byteStride", struct.calcsize(fmt) * n)
    off = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    out = []
    for i in range(acc["count"]):
        v = struct.unpack_from("<" + fmt * n, binary, off + i * stride)
        out.append(v if n > 1 else v[0])
    return out


def build(name, nodes_wanted, material, grip):
    src = SRC / name
    g = json.loads((src / f"{name}_1k.gltf").read_text())
    binary = (src / g["buffers"][0]["uri"]).read_bytes()
    pos, nrm, uv, idx = [], [], [], []
    for node in g["nodes"]:
        if "mesh" not in node:
            continue
        if nodes_wanted is not None and node.get("name") not in nodes_wanted:
            continue
        point, normal = node_xform(node)
        for p in g["meshes"][node["mesh"]]["primitives"]:
            if p.get("material") != material:
                continue
            base = len(pos)
            a = p["attributes"]
            pos += [[c - grip[i] for i, c in enumerate(point(v))] for v in read(g, binary, a["POSITION"])]
            nrm += [normal(v) for v in read(g, binary, a["NORMAL"])]
            uv += [list(v) for v in read(g, binary, a["TEXCOORD_0"])]
            idx += [base + i for i in read(g, binary, p["indices"])]
    tex_index = g["materials"][material]["pbrMetallicRoughness"]["baseColorTexture"]["index"]
    image = g["images"][g["textures"][tex_index]["source"]]
    jpeg = (src / image["uri"]).read_bytes()
    write_glb(OUT / f"{name}.glb", pos, nrm, uv, idx, jpeg)
    lo = [min(p[i] for p in pos) for i in range(3)]
    hi = [max(p[i] for p in pos) for i in range(3)]
    print(f"{name}: {len(pos)} вершин, {len(idx) // 3} треугольников, "
          f"габарит {[round(h - l, 3) for l, h in zip(lo, hi)]} м")


def write_glb(path, pos, nrm, uv, idx, jpeg):
    wide = len(pos) > 65535
    chunks = []

    def add(data, target=None):
        off = sum(len(c) for c in chunks)
        pad = (-len(data)) % 4
        chunks.append(data + b"\0" * pad)
        v = {"buffer": 0, "byteOffset": off, "byteLength": len(data)}
        if target:
            v["target"] = target
        views.append(v)
        return len(views) - 1

    views = []
    pv = add(b"".join(struct.pack("<3f", *p) for p in pos), 34962)
    nv = add(b"".join(struct.pack("<3f", *n) for n in nrm), 34962)
    uvv = add(b"".join(struct.pack("<2f", *t) for t in uv), 34962)
    iv = add(struct.pack(f"<{len(idx)}{'I' if wide else 'H'}", *idx), 34963)
    imv = add(jpeg)
    lo = [min(p[i] for p in pos) for i in range(3)]
    hi = [max(p[i] for p in pos) for i in range(3)]
    doc = {
        "asset": {"version": "2.0", "generator": "tools/props/build_props.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": path.stem}],
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2},
            "indices": 3, "material": 0}]}],
        "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
        "textures": [{"source": 0}],
        "images": [{"bufferView": imv, "mimeType": "image/jpeg"}],
        "accessors": [
            {"bufferView": pv, "componentType": 5126, "count": len(pos), "type": "VEC3", "min": lo, "max": hi},
            {"bufferView": nv, "componentType": 5126, "count": len(nrm), "type": "VEC3"},
            {"bufferView": uvv, "componentType": 5126, "count": len(uv), "type": "VEC2"},
            {"bufferView": iv, "componentType": 5125 if wide else 5123, "count": len(idx), "type": "SCALAR"},
        ],
        "bufferViews": views,
        "buffers": [{"byteLength": sum(len(c) for c in chunks)}],
    }
    js = json.dumps(doc, separators=(",", ":")).encode()
    js += b" " * ((-len(js)) % 4)
    blob = b"".join(chunks)
    out = struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(js) + 8 + len(blob))
    out += struct.pack("<II", len(js), 0x4E4F534A) + js
    out += struct.pack("<II", len(blob), 0x004E4942) + blob
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(out)


def main():
    for name, (nodes, material, grip) in PROPS.items():
        build(name, nodes, material, grip)


if __name__ == "__main__":
    main()
