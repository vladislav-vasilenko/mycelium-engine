"""Примитивы сеток растений: вершины с атрибутами, трубки, карточки, атлас.

Атрибуты вершины COLOR_0 (u8): R — материал, G — сила ветра, B — затенение
в кроне, A — мох на коре (0…1, 02.4 итерация 4). Материал 8 — почва
бугра у корней и корневой тарелки (подстилка и мох из текстур рельефа).
"""

import math

import numpy as np

BARK_SUGI, BARK_BROAD, NEEDLES, LEAVES, ROCK, FERN, SASA, MUSHROOM, SOIL = 0, 1, 2, 3, 4, 5, 6, 7, 8


class Mesh:
    def __init__(self):
        self.p, self.n, self.uv, self.c, self.i = [], [], [], [], []

    def vert(self, p, n, uv, mat, wind, ao, moss=0.0):
        self.p.append(p)
        self.n.append(n / (np.linalg.norm(n) + 1e-9))
        self.uv.append(uv)
        self.c.append((mat, int(np.clip(wind, 0, 1) * 255), int(np.clip(ao, 0, 1) * 255),
                       int(np.clip(moss, 0, 1) * 255)))
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


def tube(m, pts, radii, sides, mat, wind_of, ao=1.0, flute=None, u_tiles=1.0, moss_of=None, v0=0.0):
    """Трубка по ломаной `pts` с радиусами `radii`; `flute(t, θ)` — множитель радиуса,
    `moss_of(q, n)` — мох на коре 0…1, `ao` — число или функция `ao(q)`."""
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
            a = ao(q) if callable(ao) else ao
            moss = moss_of(q, dirv) if moss_of else 0.0
            ring.append(m.vert(q, dirv, (s / sides * u_tiles, v0 + vlen / 1.5), mat, wind_of(q), a, moss))
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
    """Атлас 1024 × 1024, четверти по 512²: 0 — хвоя, 1 — листья (верх),
    2 — вайи папоротника, 3 — листья сасы (низ); в четверти 2 × 2 варианта."""
    x0 = (side % 2) * 0.5 + (variant % 2) * 0.25
    y0 = (side // 2) * 0.5 + (variant // 2) * 0.25
    e = 0.002
    return (x0 + e, y0 + e, x0 + 0.25 - e, y0 + 0.25 - e)


def bezier(a, b, c, n):
    t = np.linspace(0, 1, n)[:, None]
    return (1 - t) ** 2 * a + 2 * (1 - t) * t * b + t * t * c


