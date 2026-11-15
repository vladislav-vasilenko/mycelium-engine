"""Растения Якусимы (02.8, итерация 1): процедурные сетки и атласы листвы.

Запуск: `.venv-geo/bin/python -I tools/plants/build_plants.py`. Детерминирован
(seed в каждом виде). Пишет в `assets/models/plants/`:

- деревья, кустарник и валежник — `tools/plants/trees.py` (02.4, итерация 4):
  `sugi_a`, `sugi_b`, `sugi_old`, `broad_a`, `broad_b`, `broad_c`, `shrub`,
  `log`, `stump`, `windfall`; мелочь под ногами — `tools/plants/ground.py`:
  `debris`, `moss_cushion`;
- `boulder.glb` — валун (мох и камень — в шейдере по нормали);
- `foliage.png` — атлас RGBA 1024 × 512: слева хвоя криптомерии, справа
  листья, по 2 × 2 варианта.

В каждом `.glb` три сетки `lod0`, `lod1`, `lod2`. Оси glTF: Y вверх, метры,
основание ствола в начале координат. Атрибуты вершины: POSITION, NORMAL,
TEXCOORD_0 и COLOR_0 (u8): R — материал (0 кора якусуги, 1 кора лиственного,
2 хвоя, 3 листья, 4 камень, 5 папоротник, 6 якудзаса, 7 гриб), G — сила
ветра, B — затенение внутри кроны, A — мох на коре.

Вместо Blender (спецификация 02.8) — numpy: карточки листвы и трубки ветвей
проще строить напрямую, а детерминизм и проверка числа треугольников — тут же.
"""

import json
import math
import struct
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from mesh import (BARK_SUGI, FERN, LEAVES, MUSHROOM, NEEDLES, ROCK, SASA,  # noqa: E402
                  Mesh, atlas_rect, card, frame, tube)
import ground  # noqa: E402
import trees  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "assets/models/plants"



# --- Подлесок -----------------------------------------------------------------

def fern(lod, seed=0xFE27):
    """Розетка вай: каждая вайя — изогнутая полоса карточек вдоль дуги."""
    rng = np.random.default_rng(seed)
    m = Mesh()
    fronds = [11, 7, 5][lod]
    segs = [4, 2, 1][lod]
    for k in range(fronds):
        th = 2 * math.pi * k / fronds + rng.uniform(-0.2, 0.2)
        d = np.array([math.cos(th), 0, math.sin(th)])
        L = rng.uniform(0.9, 1.4)
        lift = rng.uniform(0.5, 0.9)
        side = np.cross(np.array([0, 1.0, 0]), d)
        rect = atlas_rect(2, rng.integers(4))
        u0, v0, u1, v1 = rect
        prev = None
        for s in range(segs + 1):
            t = s / segs
            # Дуга: поднимается и опускается к концу.
            p = d * L * t + np.array([0, lift * math.sin(math.pi * t * 0.8) * L * 0.6, 0])
            w = 0.22 * (1.0 - 0.6 * t) + 0.05
            n = np.array([0, 1.0, 0]) * 0.8 + d * 0.2
            v = v1 + (v0 - v1) * t
            a = m.vert(p - side * w, n, (u0, v), FERN, 0.6 + 0.4 * t, 0.7 + 0.3 * t)
            b = m.vert(p + side * w, n, (u1, v), FERN, 0.6 + 0.4 * t, 0.7 + 0.3 * t)
            if prev:
                m.tri(prev[0], prev[1], b)
                m.tri(prev[0], b, a)
            prev = (a, b)
    return m


def sasa(lod, seed=0x5A5A):
    """Куртина якудзасы: вертикальные карточки с листьями крест-накрест."""
    rng = np.random.default_rng(seed)
    m = Mesh()
    for k in range([10, 6, 3][lod]):
        th = rng.uniform(0, math.pi)
        c = np.array([rng.normal(0, 0.35), 0, rng.normal(0, 0.35)])
        h = rng.uniform(0.7, 1.3) * [1.0, 1.1, 1.3][lod]
        w = rng.uniform(0.35, 0.55) * [1.0, 1.2, 1.6][lod]
        right = np.array([math.cos(th), 0, math.sin(th)]) * w
        up = np.array([0, h * 0.5, 0])
        nrm = np.array([0, 0.7, 0]) + np.array([-math.sin(th), 0, math.cos(th)]) * 0.5
        card(m, c + np.array([0, h * 0.5 - 0.05, 0]), right, up, nrm, atlas_rect(3, rng.integers(4)), SASA, 0.7, 0.8)
    return m


def mushroom(lod):
    """Гриб 10 см: ножка и шляпка; uv.y = 0 — ножка, 1 — шляпка (цвет — в шейдере по виду)."""
    m = Mesh()
    sides = [12, 8, 5][lod]
    stem = [np.array([0, y, 0]) for y in np.linspace(-0.01, 0.065, 4)]
    tube(m, stem, [0.011, 0.01, 0.009, 0.008], sides, MUSHROOM, lambda q: 0.0, 0.8)
    for i in range(len(m.uv)):
        m.uv[i] = (0.0, 0.0)
    first = len(m.p)
    rings = [5, 3, 2][lod]
    for r in range(rings + 1):
        a = (math.pi / 2) * r / rings
        for s in range(sides + 1):
            th = 2 * math.pi * s / sides
            n = np.array([math.cos(th) * math.sin(a), math.cos(a), math.sin(th) * math.sin(a)])
            p = np.array([n[0] * 0.04, 0.06 + n[1] * 0.022, n[2] * 0.04])
            m.vert(p, n, (0.0, 1.0), MUSHROOM, 0.0, 1.0)
    for r in range(rings):
        for s in range(sides):
            a0 = first + r * (sides + 1) + s
            b0 = a0 + sides + 1
            m.tri(a0, a0 + 1, b0)
            m.tri(a0 + 1, b0 + 1, b0)
    # Пластинки снизу: диск чуть ниже края шляпки.
    c = m.vert(np.array([0, 0.058, 0]), np.array([0, -1.0, 0]), (0.5, 1.0), MUSHROOM, 0.0, 0.5)
    ring = [m.vert(np.array([math.cos(2 * math.pi * s / sides) * 0.039, 0.06, math.sin(2 * math.pi * s / sides) * 0.039]),
                   np.array([0, -1.0, 0]), (0.5, 1.0), MUSHROOM, 0.0, 0.5) for s in range(sides + 1)]
    for s in range(sides):
        m.tri(c, ring[s + 1], ring[s])
    return m


# --- Валун -------------------------------------------------------------------

def icosphere(sub):
    t = (1 + 5 ** 0.5) / 2
    v = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t), (0, -1, -t), (0, 1, -t),
         (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    v = [np.array(x, float) / np.linalg.norm(x) for x in v]
    f = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4), (11, 10, 2), (10, 7, 6),
         (7, 1, 8), (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8), (3, 8, 9), (4, 9, 5), (2, 4, 11), (6, 2, 10), (8, 6, 7), (9, 8, 1)]
    for _ in range(sub):
        cache, nf = {}, []
        for a, b, c in f:
            def mid(i, j):
                key = (min(i, j), max(i, j))
                if key not in cache:
                    p = v[i] + v[j]
                    v.append(p / np.linalg.norm(p))
                    cache[key] = len(v) - 1
                return cache[key]
            ab, bc, ca = mid(a, b), mid(b, c), mid(c, a)
            nf += [(a, ab, ca), (b, bc, ab), (c, ca, bc), (ab, bc, ca)]
        f = nf
    return np.array(v), f


def boulder(lod, seed=0xB01D):
    rng = np.random.default_rng(seed)
    m = Mesh()
    v, f = icosphere([3, 2, 1][lod])
    waves = rng.normal(0, 1, (6, 3))
    amp = np.array([0.18, 0.12, 0.08, 0.06, 0.04, 0.03])
    def disp(p):
        return 1.0 + sum(a * math.sin(w @ p * (k + 1.5) + k) for k, (a, w) in enumerate(zip(amp, waves)))
    pos = np.array([p * disp(p) for p in v]) * np.array([1.6, 1.0, 1.25])
    pos[:, 1] = np.maximum(pos[:, 1], -0.6)  # плоское дно, уходит в грунт
    pos[:, 1] += 0.35
    vn = np.zeros_like(pos)
    for a, b, c in f:
        n = np.cross(pos[b] - pos[a], pos[c] - pos[a])
        vn[[a, b, c]] += n
    ids = [m.vert(p, n, (0.0, 0.0), ROCK, 0.0, 1.0) for p, n in zip(pos, vn)]
    for a, b, c in f:
        m.tri(ids[a], ids[c], ids[b])
    return m


# --- Атлас листвы --------------------------------------------------------------

def draw_needles(rng, size):
    """Веточка криптомерии: стебель с боковыми побегами, шиловидная хвоя."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    def shoot(x, y, ang, length, width, depth):
        steps = 22
        px, py = x, y
        for s in range(steps):
            t = s / steps
            a = ang + 0.25 * math.sin(t * 3 + depth)
            nx, ny = px + math.cos(a) * length / steps, py + math.sin(a) * length / steps
            d.line([(px, py), (nx, ny)], fill=(70, 62, 30, 255), width=max(1, int(width * (1 - t * 0.6))))
            # Хвоя: короткие шилья вперёд по обе стороны.
            for side in (-1, 1):
                for _ in range(5):
                    na = a + side * rng.uniform(0.35, 0.95)
                    nl = size * rng.uniform(0.04, 0.065) * (1 - 0.4 * t)
                    g = rng.uniform(0.75, 1.15)
                    col = (int(34 * g + 30 * t), int(66 * g + 25 * t), int(22 * g), 255)
                    d.line([(px, py), (px + math.cos(na) * nl, py + math.sin(na) * nl)], fill=col, width=max(2, size // 100))
            if depth < 2 and s % 3 == 1:
                for side in (-1, 1):
                    shoot(px, py, a + side * rng.uniform(0.6, 0.9), length * 0.32, width * 0.6, depth + 1)
            px, py = nx, ny
    shoot(size * 0.5, size * 0.95, -math.pi / 2 + rng.uniform(-0.1, 0.1), size * 0.68, size / 60, 0)
    return img


def draw_leaves(rng, size):
    """Веточка с эллиптическими глянцевыми листьями."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for _ in range(26):
        cx, cy = rng.uniform(0.15, 0.85) * size, rng.uniform(0.12, 0.88) * size
        ang = rng.uniform(0, 2 * math.pi)
        L, W = size * rng.uniform(0.13, 0.19), size * rng.uniform(0.045, 0.065)
        g = rng.uniform(0.7, 1.2)
        base = np.array([28, 58, 18]) * g
        pts = []
        for k in range(24):
            t = 2 * math.pi * k / 24
            x, y = math.cos(t) * L, math.sin(t) * W * (1 - 0.3 * math.cos(t))
            pts.append((cx + x * math.cos(ang) - y * math.sin(ang), cy + x * math.sin(ang) + y * math.cos(ang)))
        d.polygon(pts, fill=(int(base[0]), int(base[1]), int(base[2]), 255))
        # Жилка и светлая половина листа.
        d.line([(cx - math.cos(ang) * L, cy - math.sin(ang) * L), (cx + math.cos(ang) * L, cy + math.sin(ang) * L)],
               fill=(int(base[0] * 1.6), int(base[1] * 1.5), int(base[2] * 1.3), 255), width=max(1, size // 200))
    return img


def draw_frond(rng, size):
    """Вайя папоротника: стержень снизу вверх, перья попарно, к концу мельче."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    x0, y0, y1 = size * 0.5, size * 0.98, size * 0.04
    bend = rng.uniform(-0.08, 0.08) * size
    n = 22
    for k in range(n):
        t = k / (n - 1)
        x = x0 + bend * t * t
        y = y0 + (y1 - y0) * t
        L = size * 0.42 * (1 - t) ** 0.7 * (0.3 + 0.7 * min(1.0, t * 5))
        g = rng.uniform(0.8, 1.15)
        col = (int(40 * g), int(92 * g), int(26 * g), 255)
        for side in (-1, 1):
            ex, ey = x + side * L, y - L * 0.35
            w = max(2, int(size * 0.035 * (1 - t * 0.6)))
            d.line([(x, y), (ex, ey)], fill=col, width=w)
            # Доли пера.
            for j in range(1, 6):
                px, py = x + (ex - x) * j / 6, y + (ey - y) * j / 6
                d.line([(px, py), (px + side * size * 0.015, py - size * 0.03)], fill=col, width=max(1, w // 2))
    d.line([(x0, y0), (x0 + bend, y1)], fill=(70, 70, 30, 255), width=max(2, size // 120))
    return img


def draw_sasa(rng, size):
    """Листья якудзасы: узкие ланцетные листья веером от стеблей."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for _ in range(14):
        bx = rng.uniform(0.3, 0.7) * size
        ang = -math.pi / 2 + rng.uniform(-0.9, 0.9)
        L = size * rng.uniform(0.35, 0.55)
        W = size * rng.uniform(0.04, 0.06)
        cx, cy = bx + math.cos(ang) * L * 0.6, size * 0.95 + math.sin(ang) * L * 0.6
        g = rng.uniform(0.75, 1.15)
        pts = []
        for k in range(20):
            t = 2 * math.pi * k / 20
            x, y = math.cos(t) * L * 0.5, math.sin(t) * W
            pts.append((cx + x * math.cos(ang) - y * math.sin(ang), cy + x * math.sin(ang) + y * math.cos(ang)))
        d.polygon(pts, fill=(int(48 * g), int(86 * g), int(24 * g), 255))
        # Сухой светлый край — у якудзасы зимой.
        d.line(pts[:6], fill=(int(150 * g), int(140 * g), int(90 * g), 255), width=max(1, size // 250))
    return img


def bleed(img):
    """Цвет прозрачных пикселей — размытый цвет соседей: без тёмной каймы на мипах."""
    a = np.asarray(img).astype(np.float32)
    rgb, al = a[..., :3], a[..., 3:] / 255.0
    acc, w = rgb * al, al.copy()
    for r in (2, 6, 16, 40):
        k = Image.fromarray(np.uint8(np.clip(acc, 0, 255))).filter(ImageFilter.BoxBlur(r))
        kw = Image.fromarray(np.uint8(w[..., 0] * 255)).filter(ImageFilter.BoxBlur(r))
        kw = np.asarray(kw).astype(np.float32)[..., None] / 255.0
        fill = np.asarray(k).astype(np.float32) / np.maximum(kw, 1e-3)
        rgb = np.where(al > 0.5, rgb, np.where(kw > 0.01, fill, rgb))
    out = np.concatenate([np.clip(rgb, 0, 255), a[..., 3:]], -1)
    return Image.fromarray(np.uint8(out), "RGBA")


def build_atlas():
    rng = np.random.default_rng(0xF011)
    atlas = Image.new("RGBA", (1024, 1024), (0, 0, 0, 0))
    ss = 2  # суперсэмплинг
    for side, fn in enumerate((draw_needles, draw_leaves, draw_frond, draw_sasa)):
        for v in range(4):
            tile = fn(rng, 256 * ss).resize((256, 256), Image.LANCZOS)
            # Вариант занимает 256 × 256 в своей четверти 512².
            x = (side % 2) * 512 + (v % 2) * 256
            y = (side // 2) * 512 + (v // 2) * 256
            atlas.paste(tile, (x, y))
    return bleed(atlas)


# --- glTF ----------------------------------------------------------------------

def write_glb(path, lods):
    blob = bytearray()
    views, accessors, meshes, nodes = [], [], [], []

    def add(data, target=None):
        while len(blob) % 4:
            blob.append(0)
        off = len(blob)
        blob.extend(data)
        v = {"buffer": 0, "byteOffset": off, "byteLength": len(data)}
        if target:
            v["target"] = target
        views.append(v)
        return len(views) - 1

    for li, m in enumerate(lods):
        p = np.asarray(m.p, np.float32)
        n = np.asarray(m.n, np.float32)
        uv = np.asarray(m.uv, np.float32)
        c = np.asarray(m.c, np.uint8)
        idx = np.asarray(m.i, np.uint32).reshape(-1, 3)
        # Обход треугольника — против часовой стрелки со стороны его нормалей
        # (лицевая сторона в Metal): иначе `plant_fs` перевернёт нормаль.
        g = np.cross(p[idx[:, 1]] - p[idx[:, 0]], p[idx[:, 2]] - p[idx[:, 0]])
        flip = (g * n[idx].sum(1)).sum(1) < 0
        idx[flip] = idx[flip][:, [0, 2, 1]]
        idx = idx.reshape(-1)
        acc = {}
        for name, arr, typ, comp, norm in (
            ("POSITION", p, "VEC3", 5126, False),
            ("NORMAL", n, "VEC3", 5126, False),
            ("TEXCOORD_0", uv, "VEC2", 5126, False),
            ("COLOR_0", c, "VEC4", 5121, True),
        ):
            a = {"bufferView": add(arr.tobytes(), 34962), "componentType": comp, "count": len(arr), "type": typ}
            if norm:
                a["normalized"] = True
            if name == "POSITION":
                a["min"], a["max"] = p.min(0).tolist(), p.max(0).tolist()
            accessors.append(a)
            acc[name] = len(accessors) - 1
        accessors.append({"bufferView": add(idx.tobytes(), 34963), "componentType": 5125, "count": len(idx), "type": "SCALAR"})
        meshes.append({"name": f"lod{li}", "primitives": [{"attributes": acc, "indices": len(accessors) - 1}]})
        nodes.append({"name": f"lod{li}", "mesh": li})
    doc = {
        "asset": {"version": "2.0", "generator": "tools/plants/build_plants.py"},
        "scene": 0,
        "scenes": [{"nodes": list(range(len(nodes)))}],
        "nodes": nodes,
        "meshes": meshes,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": len(blob)}],
    }
    js = json.dumps(doc, separators=(",", ":")).encode()
    js += b" " * (-len(js) % 4)
    while len(blob) % 4:
        blob.append(0)
    total = 12 + 8 + len(js) + 8 + len(blob)
    with open(path, "wb") as f:
        f.write(struct.pack("<III", 0x46546C67, 2, total))
        f.write(struct.pack("<II", len(js), 0x4E4F534A) + js)
        f.write(struct.pack("<II", len(blob), 0x004E4942) + bytes(blob))


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    build_atlas().save(OUT / "foliage.png", optimize=True)
    species = dict(trees.SPECIES)
    species.update(ground.SPECIES)
    species.update(boulder=boulder, fern=fern, sasa=sasa, mushroom=mushroom)
    for name, fn in species.items():
        lods = [fn(l) for l in range(3)]
        write_glb(OUT / f"{name}.glb", lods)
        print(f"{name}: треугольников по LOD {[m.tris() for m in lods]}")


if __name__ == "__main__":
    main()
