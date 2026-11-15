"""Растения Якусимы (02.8, итерация 1): процедурные сетки и атласы листвы.

Запуск: `.venv-geo/bin/python -I tools/plants/build_plants.py`. Детерминирован
(seed в каждом виде). Пишет в `assets/models/plants/`:

- `yakusugi.glb` — криптомерия якусуги: ствол с продольными желобами,
  корни-наплывы, ярусы ветвей, хвоя пучками карточек;
- `broadleaf.glb` — вечнозелёное широколиственное дерево (кастанопсис):
  ствол делится на ветви, округлая крона из карточек листьев;
- `boulder.glb` — валун (мох и камень — в шейдере по нормали);
- `foliage.png` — атлас RGBA 1024 × 512: слева хвоя криптомерии, справа
  листья, по 2 × 2 варианта.

В каждом `.glb` три сетки `lod0`, `lod1`, `lod2`. Оси glTF: Y вверх, метры,
основание ствола в начале координат. Атрибуты вершины: POSITION, NORMAL,
TEXCOORD_0 и COLOR_0 (u8): R — материал (0 кора якусуги, 1 кора лиственного,
2 хвоя, 3 листья, 4 камень), G — сила ветра, B — затенение внутри кроны.

Вместо Blender (спецификация 02.8) — numpy: карточки листвы и трубки ветвей
проще строить напрямую, а детерминизм и проверка числа треугольников — тут же.
"""

import json
import math
import struct
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parents[2] / "assets/models/plants"
BARK_SUGI, BARK_BROAD, NEEDLES, LEAVES, ROCK = 0, 1, 2, 3, 4


class Mesh:
    def __init__(self):
        self.p, self.n, self.uv, self.c, self.i = [], [], [], [], []

    def vert(self, p, n, uv, mat, wind, ao):
        self.p.append(p)
        self.n.append(n / (np.linalg.norm(n) + 1e-9))
        self.uv.append(uv)
        self.c.append((mat, int(np.clip(wind, 0, 1) * 255), int(np.clip(ao, 0, 1) * 255), 255))
        return len(self.p) - 1

    def tri(self, a, b, c):
        self.i += [a, b, c]

    def tris(self):
        return len(self.i) // 3


def frame(d):
    """Два перпендикуляра к направлению d."""
    d = d / np.linalg.norm(d)
    a = np.array([0.0, 0.0, 1.0]) if abs(d[1]) > 0.9 else np.array([0.0, 1.0, 0.0])
    u = np.cross(d, a)
    u /= np.linalg.norm(u)
    return u, np.cross(u, d)


def tube(m, pts, radii, sides, mat, wind_of, ao=1.0, flute=None, u_tiles=1.0):
    """Трубка по ломаной `pts` с радиусами `radii`; `flute(t, θ)` — множитель радиуса."""
    pts = np.asarray(pts, float)
    rings = []
    vlen = 0.0
    for k, (p, r) in enumerate(zip(pts, radii)):
        d = pts[min(k + 1, len(pts) - 1)] - pts[max(k - 1, 0)]
        u, w = frame(d)
        if k > 0:
            vlen += np.linalg.norm(pts[k] - pts[k - 1])
        ring = []
        for s in range(sides + 1):
            th = 2 * math.pi * s / sides
            f = flute(k / max(len(pts) - 1, 1), th) if flute else 1.0
            dirv = u * math.cos(th) + w * math.sin(th)
            q = p + dirv * r * f
            ring.append(m.vert(q, dirv, (s / sides * u_tiles, vlen / 1.5), mat, wind_of(q), ao))
        rings.append(ring)
    for a, b in zip(rings, rings[1:]):
        for s in range(sides):
            m.tri(a[s], b[s], a[s + 1])
            m.tri(a[s + 1], b[s], b[s + 1])


def card(m, center, right, up, normal, rect, mat, wind, ao):
    """Карточка листвы: квадрат со стороной `right`/`up`, uv — прямоугольник атласа."""
    u0, v0, u1, v1 = rect
    vs = [
        m.vert(center - right - up, normal, (u0, v1), mat, wind, ao),
        m.vert(center + right - up, normal, (u1, v1), mat, wind, ao),
        m.vert(center + right + up, normal, (u1, v0), mat, wind, ao),
        m.vert(center - right + up, normal, (u0, v0), mat, wind, ao),
    ]
    m.tri(vs[0], vs[1], vs[2])
    m.tri(vs[0], vs[2], vs[3])


def atlas_rect(side, variant):
    """side 0 — хвоя, 1 — листья; 2 × 2 варианта в половине атласа 1024 × 512."""
    x0 = side * 0.5 + (variant % 2) * 0.25
    y0 = (variant // 2) * 0.5
    e = 0.004
    return (x0 + e, y0 + e, x0 + 0.25 - e, y0 + 0.5 - e)


def bezier(a, b, c, n):
    t = np.linspace(0, 1, n)[:, None]
    return (1 - t) ** 2 * a + 2 * (1 - t) * t * b + t * t * c


# --- Криптомерия якусуги -----------------------------------------------------

def yakusugi(lod, seed=0x5091):
    rng = np.random.default_rng(seed)
    m = Mesh()
    H, R0 = 28.0, 0.9
    sides = [24, 12, 6][lod]
    crown0 = 0.38 * H

    def crown_r(y):
        t = np.clip((y - crown0) / (H - crown0), 0, 1)
        return 0.6 + 4.6 * (1 - t) ** 0.85 * min(1.0, 0.4 + 2.5 * t)

    def wind(q):
        return np.clip((q[1] - crown0 * 0.5) / H, 0, 1) * 0.6 + np.hypot(q[0], q[2]) / 12.0

    # Ствол: изгиб и сужение, наплыв у корней, продольные желоба.
    ys = np.concatenate([np.linspace(0, 3, 8 if lod < 2 else 3), np.linspace(3.8, H, [26, 14, 6][lod])])
    lean = rng.normal(0, 0.25, 2)
    axis = np.stack([lean[0] * (ys / H) ** 1.5 + 0.15 * np.sin(ys * 0.31), ys, lean[1] * (ys / H) ** 1.5 + 0.12 * np.cos(ys * 0.27)], 1)
    radii = R0 * (1 - ys / H * 0.97) ** 0.8 + R0 * 0.9 * np.exp(-ys / 1.3)
    phase = rng.uniform(0, 6.28)

    def flute(t, th):
        if lod == 2:
            return 1.0
        y = t * H
        return 1.0 + (0.10 * math.sin(7 * th + phase + 0.3 * y) + 0.05 * math.sin(13 * th - 0.5 * y)) * (1.2 - y / H)

    tube(m, axis, radii, sides, BARK_SUGI, lambda q: 0.0, 1.0, flute, u_tiles=max(2, round(2 * math.pi * R0 / 1.2)))

    # Корни-наплывы: изогнутые трубки от ствола в землю.
    if lod < 2:
        for k in range(7 if lod == 0 else 5):
            th = 2 * math.pi * (k + rng.uniform(-0.2, 0.2)) / (7 if lod == 0 else 5)
            d = np.array([math.cos(th), 0, math.sin(th)])
            start = np.array([0, rng.uniform(0.9, 1.5), 0]) + d * R0 * 0.55
            mid = d * rng.uniform(1.3, 1.7) + np.array([0, 0.15, 0])
            end = d * rng.uniform(2.0, 2.8) + np.array([0, -0.9, 0])
            pts = bezier(start, mid, end, 10 if lod == 0 else 6)
            rr = np.linspace(R0 * 0.6, 0.12, len(pts))
            tube(m, pts, rr, 8 if lod == 0 else 5, BARK_SUGI, lambda q: 0.0, 0.85)

    # Ветви ярусами по золотому углу, хвоя пучками на внешних 2/3.
    nbr = [64, 34, 22][lod]
    for b in range(nbr):
        t = b / (nbr - 1)
        y = crown0 + (H - crown0 - 0.8) * t ** 0.9
        th = b * 2.39996 + rng.uniform(-0.2, 0.2)
        d = np.array([math.cos(th), 0, math.sin(th)])
        L = crown_r(y) * rng.uniform(0.8, 1.1)
        tilt = math.radians(rng.uniform(10, 32))
        base = np.array([np.interp(y, ys, axis[:, 0]), y, np.interp(y, ys, axis[:, 2])])
        tip = base + d * L * math.cos(tilt) + np.array([0, L * math.sin(tilt), 0])
        mid = (base + tip) / 2 + np.array([0, -0.15 * L, 0])
        if lod < 2:
            pts = bezier(base, mid, tip, 5)
            rb = np.interp(y, ys, radii) * 0.22
            tube(m, pts, np.linspace(rb, 0.03, 5), 5 if lod == 0 else 4, BARK_SUGI, wind, 0.8)
        # Пучки хвои: на вершине ветви звезда из карточек, вдоль ветви — по одной.
        tufts = [5, 3, 2][lod]
        for k in range(tufts):
            s = 1.0 - k * (0.75 / max(tufts - 1, 1))
            c = bezier(base, mid, tip, 11)[int(round(s * 10))]
            size = (1.4 + 0.6 * L / 5.0) * [1.0, 1.3, 2.0][lod] * rng.uniform(0.85, 1.15)
            ao = 0.55 + 0.45 * min(1.0, np.hypot(c[0], c[2]) / max(crown_r(y), 0.5))
            out = np.array([c[0], 0, c[2]])
            out = out / (np.linalg.norm(out) + 1e-6)
            nrm = out * 0.75 + np.array([0, 0.65, 0])
            fwd = (tip - base) / np.linalg.norm(tip - base)
            for j in range([3, 2, 2][lod]):
                rot = j * math.pi / [3, 2, 2][lod] + rng.uniform(0, 0.5)
                u, w = frame(fwd)
                side = u * math.cos(rot) + w * math.sin(rot)
                card(m, c, fwd * size * 0.7, side * size * 0.5, nrm, atlas_rect(0, rng.integers(4)), NEEDLES, wind(c), ao)
    # Вершина — пучок.
    top = np.array([axis[-1, 0], H + 0.3, axis[-1, 2]])
    for j in range(3):
        rot = j * math.pi / 3
        card(m, top, np.array([math.cos(rot), 0, math.sin(rot)]) * 1.0, np.array([0, 1.2, 0]), np.array([0, 1.0, 0]), atlas_rect(0, j), NEEDLES, 1.0, 1.0)
    return m


# --- Широколиственное дерево -------------------------------------------------

def broadleaf(lod, seed=0xB20A):
    rng = np.random.default_rng(seed)
    m = Mesh()
    R0 = 0.38
    crown_c = np.array([0.0, 10.5, 0.0])
    crown_r = np.array([6.0, 4.5, 6.0])

    def wind(q):
        return np.clip((q[1] - 4.0) / 12.0, 0, 1) * 0.7 + np.hypot(q[0], q[2]) / 14.0

    ys = np.linspace(0, 5.0, [10, 6, 3][lod])
    axis = np.stack([0.1 * np.sin(ys), ys, 0.1 * np.cos(ys * 0.7)], 1)
    radii = R0 * (1 - ys / 9.0) + R0 * 0.6 * np.exp(-ys / 0.8)
    tube(m, axis, radii, [16, 10, 6][lod], BARK_BROAD, lambda q: 0.0, 1.0, u_tiles=2)
    tips = []
    limbs = [4, 4, 3][lod]
    for k in range(limbs):
        th = 2 * math.pi * k / limbs + rng.uniform(-0.3, 0.3)
        d = np.array([math.cos(th), 0, math.sin(th)])
        base = axis[-1] + np.array([0, -0.3, 0])
        end = crown_c + d * rng.uniform(2.5, 4.0) + np.array([0, rng.uniform(-0.5, 1.5), 0])
        mid = base + (end - base) * 0.5 + d * 0.8
        pts = bezier(base, mid, end, [7, 5, 3][lod])
        tube(m, pts, np.linspace(R0 * 0.55, 0.08, len(pts)), [8, 6, 4][lod], BARK_BROAD, wind, 0.85)
        if lod < 2:
            for j in range(3):
                th2 = th + rng.uniform(-1.2, 1.2)
                d2 = np.array([math.cos(th2), 0, math.sin(th2)])
                b2 = pts[len(pts) // 2 + j % 2]
                e2 = b2 + d2 * rng.uniform(2.0, 3.5) + np.array([0, rng.uniform(0.5, 2.5), 0])
                tube(m, bezier(b2, (b2 + e2) / 2 + np.array([0, 0.4, 0]), e2, 4), np.linspace(0.08, 0.02, 4), 4, BARK_BROAD, wind, 0.8)
                tips.append(e2)
        tips.append(end)
    # Карточки листвы по оболочке эллипсоида кроны, гуще снаружи.
    n_cards = [150, 60, 28][lod]
    size = [1.3, 1.9, 2.8][lod]
    for k in range(n_cards):
        z = rng.uniform(-0.6, 1.0)
        th = rng.uniform(0, 2 * math.pi)
        r = math.sqrt(max(0.0, 1 - z * z))
        dirv = np.array([r * math.cos(th), z, r * math.sin(th)])
        depth = rng.uniform(0.7, 1.0)
        c = crown_c + dirv * crown_r * depth
        nrm = dirv * 0.8 + np.array([0, 0.4, 0])
        u, w = frame(nrm)
        rot = rng.uniform(0, math.pi)
        right = (u * math.cos(rot) + w * math.sin(rot)) * size * rng.uniform(0.8, 1.2)
        up = np.cross(nrm / np.linalg.norm(nrm), right / np.linalg.norm(right)) * np.linalg.norm(right)
        card(m, c, right, up, nrm, atlas_rect(1, rng.integers(4)), LEAVES, wind(c), 0.5 + 0.5 * depth)
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
    atlas = Image.new("RGBA", (1024, 512), (0, 0, 0, 0))
    ss = 2  # суперсэмплинг
    for side, fn in enumerate((draw_needles, draw_leaves)):
        for v in range(4):
            tile = fn(rng, 256 * ss).resize((256, 256), Image.LANCZOS)
            # Вариант занимает 256 × 256 в своей четверти половины.
            x = side * 512 + (v % 2) * 256
            y = (v // 2) * 256
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
        idx = np.asarray(m.i, np.uint32)
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
    for name, fn in (("yakusugi", yakusugi), ("broadleaf", broadleaf), ("boulder", boulder)):
        lods = [fn(l) for l in range(3)]
        write_glb(OUT / f"{name}.glb", lods)
        print(f"{name}: треугольников по LOD {[m.tris() for m in lods]}")


if __name__ == "__main__":
    main()
