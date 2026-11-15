"""Обрастание деревьев Якусимы (02.13, сессия 2): лианы, свисающий лишайник,
папоротники-эпифиты.

Влажный лес Якусимы — стволы в лианах (тайваньский плющ, гортензия
черешковая), ветви в бородах лишайника (サルオガセ, уснея) и папоротниках
в развилках. Всё строится по скелету дерева (`trees.Skeleton`), поэтому
обрастание следует его ветвям во всех LOD; LOD 2 его не несёт.
"""

import math

import numpy as np

from mesh import BARK_BROAD, FERN, LEAVES, SASA, atlas_rect, card, frame, tube

# Плитка лишайника — вариант 3 четверти сасы (`build_plants.draw_lichen`).
LICHEN_RECT = atlas_rect(3, 3)


def unit(v):
    return v / (np.linalg.norm(v) + 1e-9)


def _along(b, t):
    """Точка, радиус и направление ветви на доле длины `t`."""
    pts, radii = b["pts"], b["radii"]
    x = t * (len(pts) - 1)
    i = min(int(x), len(pts) - 2)
    f = x - i
    p = pts[i] * (1 - f) + pts[i + 1] * f
    r = radii[i] * (1 - f) + radii[i + 1] * f
    return p, r, unit(pts[i + 1] - pts[i])


def lianas(m, trunk, rng, lod, count, reach, bulge=1.25):
    """Лианы спиралью вверх по стволу с листьями вдоль; `reach` — доля длины ствола."""
    if lod >= 2:
        return
    for _ in range(count):
        th0 = rng.uniform(0, 2 * math.pi)
        turns = rng.uniform(0.6, 1.6) * rng.choice([-1, 1])
        top = rng.uniform(*reach)
        n = [40, 18][lod]
        pts, radii = [], []
        for k in range(n):
            t = top * k / (n - 1)
            p, r, d = _along(trunk, t)
            u, w = frame(d)
            th = th0 + turns * 2 * math.pi * t / max(top, 1e-3)
            radial = u * math.cos(th) + w * math.sin(th)
            pts.append(p + radial * (r * bulge + 0.03))
            radii.append(0.035 * (1 - 0.5 * k / n))
        pts, radii = np.array(pts), np.array(radii)
        tube(m, pts, radii, [5, 4][lod], BARK_BROAD, lambda q: 0.05, 0.75, u_tiles=1,
             moss_of=lambda q, nn: 0.5 + 0.4 * max(nn[1], 0))
        # Листья — пучками вдоль лианы, наружу от ствола.
        step = [3, 5][lod]
        for k in range(2, n, step):
            p = pts[k]
            c, _, _ = _along(trunk, top * k / (n - 1))
            out = unit(np.array([p[0] - c[0], 0.0, p[2] - c[2]]))
            s = rng.uniform(0.22, 0.38)
            right = unit(np.cross(out, np.array([0, 1.0, 0]))) * s
            up = unit(np.array([0, 1.0, 0]) + out * 0.6) * s
            card(m, p + out * s * 0.6, right, up, unit(out + np.array([0, 0.3, 0])),
                 atlas_rect(1, rng.integers(4)), LEAVES, 0.25, 0.6)


def lichen(m, sk, rng, lod, count, min_y=3.0):
    """Бороды лишайника с ветвей 1–2 порядка: крест из двух свисающих карточек."""
    if lod >= 2:
        return
    cand = [b for b in sk.branches if b["order"] in (1, 2) and not b.get("stub")]
    cand = [b for b in cand if b["pts"][:, 1].max() > min_y]
    if not cand:
        return
    n = count if lod == 0 else count // 2
    for _ in range(n):
        b = cand[rng.integers(len(cand))]
        p, r, d = _along(b, rng.uniform(0.15, 0.85))
        if p[1] < min_y:
            continue
        L = rng.uniform(0.5, 1.4)
        w = L * rng.uniform(0.28, 0.4)
        top = p - np.array([0, r, 0])
        center = top - np.array([0, L * 0.5, 0])
        up = np.array([0, L * 0.5, 0])
        a = rng.uniform(0, math.pi)
        for j in range(2 if lod == 0 else 1):
            ang = a + j * math.pi / 2
            right = np.array([math.cos(ang), 0, math.sin(ang)]) * w * 0.5
            nrm = unit(np.cross(up, right))
            card(m, center, right, up, unit(nrm + np.array([0, 0.4, 0])), LICHEN_RECT, SASA, 0.95, 0.7)


def fork_ferns(m, sk, rng, lod, count, min_r=0.08):
    """Папоротники-эпифиты в развилках: розетка вай у начала крупной ветви."""
    if lod >= 2:
        return
    forks = [b for b in sk.branches if b["order"] == 1 and not b.get("stub") and b["radii"][0] > min_r]
    rng.shuffle(forks)
    for b in forks[:count]:
        p0, r, d = _along(b, 0.04)
        base = p0 + np.array([0, r * 0.8, 0])
        k = [6, 4][lod]
        for j in range(k):
            th = 2 * math.pi * j / k + rng.uniform(-0.3, 0.3)
            out = np.array([math.cos(th), 0, math.sin(th)])
            s = rng.uniform(0.3, 0.5)
            up = unit(out + np.array([0, rng.uniform(0.5, 1.0), 0])) * s
            right = unit(np.cross(np.array([0, 1.0, 0]), out)) * s * 0.35
            card(m, base + up, right, up, unit(np.cross(right, up) + np.array([0, 0.5, 0])),
                 atlas_rect(2, rng.integers(4)), FERN, 0.4, 0.8)


def dress(m, sk, rng, lod, lianas_n, lichen_n, ferns_n, reach=(0.25, 0.55), bulge=1.25):
    """Всё обрастание дерева; ствол — первая ветвь скелета."""
    trunk = sk.branches[0]
    lianas(m, trunk, rng, lod, lianas_n, reach, bulge)
    lichen(m, sk, rng, lod, lichen_n)
    fork_ferns(m, sk, rng, lod, ferns_n)
