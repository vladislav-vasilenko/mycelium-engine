"""Мелочь под ногами (02.4, итерация 4): то, что видно в 1–20 м от глаз.

- `debris` — лесной сор: камешки, сучки, опавшие листья плашмя;
- `moss_cushion` — подушки мха: неровные бугорки почвы (материал 8) с мхом.

Рисуется только рядом с камерой (`CLUTTER_DIST` в `forest.metal`), поэтому
LOD 1 и 2 — упрощения на случай дальних подклеток.
"""

import json
import math
from pathlib import Path

import numpy as np

from mesh import BARK_BROAD, BLADE, LEAVES, ROCK, SOIL, Mesh, atlas_rect, card, tube


def pebble(m, rng, c, r, sub):
    """Камешек: сплюснутый неровный многогранник."""
    sides, rings = [8, 6, 4][sub], [4, 3, 2][sub]
    wob = rng.normal(0, 1, (3, 3))
    first = len(m.p)
    for k in range(rings + 1):
        a = math.pi / 2 * k / rings
        for s in range(sides):
            th = 2 * math.pi * s / sides
            d = np.array([math.cos(th) * math.cos(a), math.sin(a), math.sin(th) * math.cos(a)])
            f = 1 + 0.18 * sum(math.sin(w @ d * 3 + i) for i, w in enumerate(wob)) / 3
            p = c + d * r * f * np.array([1.0, 0.55, 0.8])
            m.vert(p - np.array([0, 0.25 * r, 0]), d, (0, 0), ROCK, 0.0, 0.85)
    for k in range(rings):
        for s in range(sides):
            a0 = first + k * sides + s
            a1 = first + k * sides + (s + 1) % sides
            m.tri(a0, a1 + sides, a1)
            m.tri(a0, a0 + sides, a1 + sides)


def debris(lod, seed=0xDEB1):
    rng = np.random.default_rng(seed)
    m = Mesh()
    for k in range([7, 4, 2][lod]):
        c = np.array([rng.uniform(-0.6, 0.6), 0, rng.uniform(-0.6, 0.6)])
        pebble(m, rng, c, rng.uniform(0.03, 0.12), lod)
    if lod < 2:
        for k in range([6, 3][lod]):
            a = np.array([rng.uniform(-0.5, 0.5), 0.01, rng.uniform(-0.5, 0.5)])
            th = rng.uniform(0, 6.28)
            L = rng.uniform(0.25, 0.6)
            b = a + np.array([math.cos(th) * L, rng.uniform(0.0, 0.06), math.sin(th) * L])
            mid = (a + b) / 2 + np.array([0, 0.03, 0]) + rng.normal(0, 0.04, 3) * np.array([1, 0, 1])
            r = rng.uniform(0.008, 0.025)
            tube(m, [a, mid, b], [r, r * 0.8, r * 0.5], [5, 3][lod], BARK_BROAD, lambda q: 0.0, 0.7,
                 moss_of=lambda q, n: 0.25)
    # Опавшие листья плашмя: карточки атласа листьев у самой земли.
    for k in range([10, 5, 2][lod]):
        c = np.array([rng.uniform(-0.6, 0.6), 0.01 + 0.005 * k / 10, rng.uniform(-0.6, 0.6)])
        th = rng.uniform(0, 6.28)
        s = rng.uniform(0.08, 0.15)
        right = np.array([math.cos(th), 0, math.sin(th)]) * s
        up = np.array([-math.sin(th), 0.02, math.cos(th)]) * s
        card(m, c, right, up, np.array([0, 1.0, 0]), atlas_rect(1, rng.integers(4)), LEAVES, 0.15, 0.6)
    return m


def moss_cushion(lod, seed=0x3055):
    """2–4 сливающихся бугорка мха 0,3–0,7 м."""
    rng = np.random.default_rng(seed)
    m = Mesh()
    sides, rings = [14, 8, 5][lod], [5, 3, 2][lod]
    for b in range(3):
        c = np.array([rng.uniform(-0.4, 0.4), -0.04, rng.uniform(-0.4, 0.4)])
        R = rng.uniform(0.25, 0.55)
        A = R * rng.uniform(0.35, 0.6)
        ph = rng.uniform(0, 6.28)
        ids = []
        for k in range(rings + 1):
            u = k / rings
            row = []
            for s in range(sides + 1):
                th = 2 * math.pi * s / sides
                lump = 1 + 0.15 * math.sin(3 * th + ph) + 0.08 * math.sin(7 * th)
                r = R * u * lump
                h = A * (1 - u * u) - 0.05 * u
                d = np.array([math.cos(th), 0, math.sin(th)])
                n = d * (2 * A * u / R) + np.array([0, 1.0, 0])
                row.append(m.vert(c + d * r + np.array([0, h, 0]), n, (0, 0), SOIL, 0.0, 0.45 + 0.2 * (1 - u), 1.0))
            ids.append(row)
        for a, bb in zip(ids, ids[1:]):
            for s in range(sides):
                m.tri(a[s], a[s + 1], bb[s])
                m.tri(a[s + 1], bb[s + 1], bb[s])
    return m


def trail_roots(lod, seed=0x7207):
    """Корни поперёк тропы: 3–5 полузарытых изогнутых корней вдоль оси X
    (шейдер поворачивает их поперёк тропы) и плоские камни между ними."""
    rng = np.random.default_rng(seed)
    m = Mesh()
    for k in range([5, 3, 2][lod]):
        z = rng.uniform(-0.8, 0.8)
        r = rng.uniform(0.03, 0.07)
        steps = [10, 5, 3][lod]
        ph = rng.uniform(0, 6.28)
        pts, rr = [], []
        for j in range(steps + 1):
            t = j / steps
            x = -1.3 + 2.6 * t
            # Над землёй в середине, концы уходят в грунт.
            y = r * (0.6 * math.sin(math.pi * t) - 0.5) + 0.02 * math.sin(7 * t + ph)
            pts.append(np.array([x, y, z + 0.18 * math.sin(2.5 * t + ph)]))
            rr.append(r * (0.8 + 0.3 * math.sin(math.pi * t)))
        tube(m, pts, rr, [6, 4, 3][lod], BARK_BROAD, lambda q: 0.0, 0.75, u_tiles=1,
             moss_of=lambda q, n: 0.35 * max(n[1], 0))
    for k in range([4, 2, 1][lod]):
        c = np.array([rng.uniform(-0.9, 0.9), -0.02, rng.uniform(-0.7, 0.7)])
        pebble(m, rng, c, rng.uniform(0.12, 0.25), lod)
    return m


_RECTS = None


def grass_rects():
    """Рамки элементов фотоатласов травы (`grass_photo.py`)."""
    global _RECTS
    if _RECTS is None:
        path = Path(__file__).resolve().parents[2] / "assets/models/plants/grass_rects.json"
        r = json.loads(path.read_text())
        r["tufts"] = [t for t in r["tufts"] if t[2] - t[0] > 0.06]
        # Короткие обломки сухих стеблей — не травинки.
        r["strips"] = [t for t in r["strips"] if t[3] - t[1] > 0.45]
        _RECTS = r
    return _RECTS


def _photo_quad(m, base, yaw, h, rect, tilt, layer_shift):
    """Вертикальная карточка-фото: основание на земле, доля высоты — в канале
    затенения (0,55 → 1), ветер растёт к верху; u + 1 — атлас кустиков."""
    x0, y0, x1, y1 = rect
    w = h * (x1 - x0) / max(y1 - y0, 1e-3) * 0.5
    right = np.array([math.cos(yaw), 0.0, math.sin(yaw)]) * w
    fwd = np.array([-math.sin(yaw), 0.0, math.cos(yaw)])
    up = np.array([0.0, h, 0.0]) + fwd * h * tilt
    n = fwd * 0.3 + np.array([0, 1.0, 0])
    v = [m.vert(base - right, n, (x0 + layer_shift, y1), BLADE, 0.0, 0.55),
         m.vert(base + right, n, (x1 + layer_shift, y1), BLADE, 0.0, 0.55),
         m.vert(base + right + up, n, (x1 + layer_shift, y0), BLADE, 0.9, 1.0),
         m.vert(base - right + up, n, (x0 + layer_shift, y0), BLADE, 0.9, 1.0)]
    m.tri(v[0], v[1], v[2])
    m.tri(v[0], v[2], v[3])


def _photo_strip(m, rng, base, h, rect, segs, lean, az):
    """Изогнутая травинка с фото одиночной травинки (атлас `strips`)."""
    x0, y0, x1, y1 = rect
    w = h * (x1 - x0) / max(y1 - y0, 1e-3) * 0.5
    out = np.array([math.cos(az), 0.0, math.sin(az)])
    side = np.array([-out[2], 0.0, out[0]])
    prev = None
    for s_ in range(segs + 1):
        t = s_ / segs
        p = base + out * lean * h * t * t + np.array([0, h * t * (1 - 0.25 * lean * t), 0])
        n = np.cross(side, out * (2 * lean * h * t) + np.array([0, h, 0]))
        n = n / (np.linalg.norm(n) + 1e-9) + np.array([0, 0.5, 0])
        v = y1 + (y0 - y1) * t
        a = m.vert(p - side * w, n, (x0, v), BLADE, 0.9 * t, 0.55 + 0.45 * t)
        c = m.vert(p + side * w, n, (x1, v), BLADE, 0.9 * t, 0.55 + 0.45 * t)
        if prev:
            m.tri(prev[0], prev[1], c)
            m.tri(prev[0], c, a)
        prev = (a, c)


def grass_blades(lod, seed=0x6A55, kind="medium"):
    """Пучок травы из фототекстур Poly Haven (02.13): кустики-карточки
    (`grass_bermuda_01`) и изогнутые травинки (`grass_medium_02`) с альфой,
    материал 9. Ярусы: низкая густая — кустики, средняя — кустики и травинки,
    высокая — длинные травинки и стебли."""
    rng = np.random.default_rng(seed)
    R = grass_rects()
    m = Mesh()
    n_tuft, n_strip, hmin, hmax, radius = {"short": ([16, 9, 4], [0, 0, 0], 0.1, 0.22, 0.24),
                                           "medium": ([10, 6, 3], [10, 5, 2], 0.18, 0.4, 0.26),
                                           "tall": ([4, 2, 1], [10, 6, 3], 0.45, 0.85, 0.2)}[kind]
    segs = [4, 3, 2][lod]
    for _ in range(n_tuft[lod]):
        r = radius * math.sqrt(rng.uniform())
        th = rng.uniform(0, 6.28)
        base = np.array([r * math.cos(th), -0.01, r * math.sin(th)])
        pool = R["stems"] if kind == "tall" else R["tufts"]
        rect = pool[rng.integers(len(pool))]
        h = rng.uniform(hmin, hmax) * (1.4 if kind == "tall" else 1.0)
        _photo_quad(m, base, rng.uniform(0, math.pi), h, rect, rng.uniform(-0.15, 0.15), 1.0)
    for _ in range(n_strip[lod]):
        r = radius * math.sqrt(rng.uniform())
        th = rng.uniform(0, 6.28)
        base = np.array([r * math.cos(th), 0.0, r * math.sin(th)])
        rect = R["strips"][rng.integers(len(R["strips"]))]
        h = rng.uniform(hmin, hmax)
        az = math.atan2(base[2], base[0]) + rng.uniform(-0.8, 0.8) if r > 0.03 else rng.uniform(0, 6.28)
        _photo_strip(m, rng, base, h, rect, segs, rng.uniform(0.2, 0.6), az)
    return m


SPECIES = {"debris": debris, "moss_cushion": moss_cushion, "trail_roots": trail_roots,
           "grass_blades_a": lambda l: grass_blades(l, 0x6A55, "short"),
           "grass_blades_b": lambda l: grass_blades(l, 0x6A56, "medium"),
           "grass_blades_c": lambda l: grass_blades(l, 0x6A57, "tall")}
