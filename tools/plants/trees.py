"""Деревья, кустарник и валежник Якусимы (02.4, итерация 4).

Дерево растёт как скелет: ветвь — изогнутая ломаная (случайное блуждание
направления + тропизм вида), дочерние ветви — по золотому углу с углом
отхождения и длиной от положения на родителе; на концах ветвей старших
порядков — пучки листвы (3–4 карточки). Скелет строится один раз, сетки
трёх LOD — из него: LOD 1 и 2 отбрасывают тонкие ветви и укрупняют пучки,
поэтому силуэт при смене LOD сохраняется.

Виды (имена `.glb`):
- `sugi_a`, `sugi_b` — криптомерия: прямой ствол, ярусы почти горизонтальных
  ветвей с приподнятыми концами, коническая крона;
- `sugi_old` — древняя якусуги: толстый ствол с желобами и контрфорсами,
  вершина распалась на несколько лидеров, обломанные сучья, клочковатая крона;
- `broad_a`, `broad_b`, `broad_c` — вечнозелёные лиственные (кастанопсис,
  дистилиум): ствол делится на лидеров, раскидистый купол;
- `shrub` — кустарник подлеска, `log` — упавший ствол, `stump` — пень старой
  вырубки якусуги с мхом и папоротником сверху.
"""

import math

import numpy as np

from mesh import (BARK_BROAD, BARK_SUGI, FERN, LEAVES, NEEDLES, SOIL, Mesh, atlas_rect, bezier, card,
                  frame, tube)


def unit(v):
    return v / (np.linalg.norm(v) + 1e-9)


class Skeleton:
    def __init__(self):
        self.branches = []  # dict(pts, radii, order, stub)
        self.clumps = []    # (центр, направление ветви, размер)


def grow(sk, rng, P, p0, d0, length, r0, order, stub=False):
    """Ветвь порядка `order` из `p0` по `d0`; рекурсивно — дочерние и листва."""
    n = P["segs"][order]
    d = unit(np.asarray(d0, float))
    pts = [np.asarray(p0, float)]
    for k in range(n):
        d = unit(d + rng.normal(0, P["gnarl"][order], 3) + P["tropism"](order, d, k / n, pts[-1]))
        pts.append(pts[-1] + d * length / n)
    pts = np.array(pts)
    t = np.linspace(0, 1, n + 1)
    r1 = r0 * (0.55 if stub else P["taper"][order])
    radii = r0 + (r1 - r0) * t ** 0.8
    sk.branches.append(dict(pts=pts, radii=radii, order=order, stub=stub))
    if stub:
        return

    def at(tc):
        f = tc * n
        i = min(int(f), n - 1)
        return pts[i] + (pts[i + 1] - pts[i]) * (f - i), unit(pts[i + 1] - pts[i]), radii[i]

    if order < P["max_order"]:
        kids = P["kids"](order, length, rng)
        t0 = P["kid_t0"][order]
        az0 = rng.uniform(0, 6.28)
        for c in range(kids):
            tc = t0 + (1 - t0) * (c + rng.uniform(0.2, 0.8)) / kids
            pos, dpar, rpar = at(tc)
            u, w = frame(dpar)
            az = az0 + c * 2.39996 + rng.uniform(-0.4, 0.4)
            side = u * math.cos(az) + w * math.sin(az)
            ang = math.radians(P["angle"][order] + rng.normal(0, 8))
            dk = P["kid_dir"](order, unit(math.cos(ang) * dpar + math.sin(ang) * side), tc)
            lk = P["kid_len"](order, length, tc) * rng.uniform(0.75, 1.15)
            rk = max(rpar * P["kid_r"][order], 0.006)
            grow(sk, rng, P, pos - dk * rpar * 0.3, dk, lk, rk, order + 1)
    if order >= P["leaf_order"]:
        lt0 = P["leaf_t0"][order]
        nc = max(1, int(length * (1 - lt0) / P["clump_step"]))
        for k in range(nc):
            tc = lt0 + (1 - lt0) * (k + 1) / nc
            pos, dpar, _ = at(min(tc, 1.0))
            sk.clumps.append((pos, dpar, P["clump_size"] * rng.uniform(0.8, 1.2)))


def mesh_tree(sk, lod, rng, bark, leaf, leaf_side, H, moss_amount, flute=None, cards=3):
    """Сетка LOD из скелета: ветви трубками, листва пучками карточек."""
    m = Mesh()
    clumps = sk.clumps
    if clumps:
        cp = np.array([c[0] for c in clumps])
        axis_c = cp.mean(0)
        crown_r = max(np.percentile(np.hypot(cp[:, 0] - axis_c[0], cp[:, 2] - axis_c[2]), 90), 0.5)
        y0, y1 = cp[:, 1].min(), cp[:, 1].max() + 1e-3
    else:
        axis_c, crown_r, y0, y1 = np.zeros(3), 1.0, 0.0, 1.0

    def wind(q):
        return float(np.clip(np.hypot(q[0], q[2]) / (crown_r * 1.2), 0, 1) * 0.7 + np.clip(q[1] / H, 0, 1) * 0.3)

    def moss(q, n):
        base = math.exp(-max(q[1], 0) / (2.0 + 4.0 * moss_amount))
        up = max(n[1], 0.0)
        return moss_amount * min(1.0, 0.75 * base + 0.6 * up * up + 0.15)

    rmin = [0.0, 0.035, 0.10][lod]
    for bi, b in enumerate(sk.branches):
        if bi > 0 and b["radii"][0] < rmin:
            continue
        step = [1, 2, 3][lod] if b["order"] > 0 else [1, 1, 2][lod]
        idx = list(range(0, len(b["pts"]), step))
        if idx[-1] != len(b["pts"]) - 1:
            idx.append(len(b["pts"]) - 1)
        r = b["radii"][0]
        sides = int(np.clip(r * [70, 30, 12][lod], [4, 4, 3][lod], [28, 12, 6][lod]))
        circ = 2 * math.pi * r
        tube(m, b["pts"][idx], b["radii"][idx], sides, bark, wind, 1.0 if b["order"] == 0 else 0.85,
             flute if bi == 0 and lod < 2 else None, u_tiles=max(1, round(circ / 1.0)), moss_of=moss,
             v0=rng.uniform(0, 4))
    stride = [1, 2, 4][lod]
    grow_k = [1.0, 1.35, 1.9][lod]
    ncards = [cards, 2, 2][lod]
    for ci, (c, d, size) in enumerate(clumps):
        if ci % stride:
            continue
        out = np.array([c[0] - axis_c[0], 0.0, c[2] - axis_c[2]])
        radial = min(1.0, np.linalg.norm(out) / crown_r)
        out = unit(out + np.array([0, 0.35, 0]))
        hgt = (c[1] - y0) / (y1 - y0)
        ao = 0.35 + 0.4 * radial + 0.25 * hgt
        s = size * grow_k
        for j in range(ncards):
            rot = j * math.pi / ncards + rng.uniform(0, 0.6)
            u, w = frame(d)
            sd = u * math.cos(rot) + w * math.sin(rot)
            nrm = unit(np.cross(d, sd)) * 0.45 + out * 0.55
            if nrm[1] < 0:
                nrm = nrm * np.array([1, -1, 1])
            card(m, c + d * s * 0.25, d * s * 0.6, sd * s * 0.45, unit(nrm + np.array([0, 0.25, 0])),
                 atlas_rect(leaf_side, rng.integers(4)), leaf, wind(c), ao)
    return m


def roots(m, rng, R0, n, lod, mat, moss_of, reach=(2.0, 2.8)):
    """Корни-контрфорсы: изогнутые трубки от ствола под землю."""
    for k in range(n):
        th = 2 * math.pi * (k + rng.uniform(-0.25, 0.25)) / n
        d = np.array([math.cos(th), 0, math.sin(th)])
        start = np.array([0, rng.uniform(0.8, 1.6) * R0, 0]) + d * R0 * 0.55
        mid = d * R0 * rng.uniform(1.3, 1.8) + np.array([0, 0.1 * R0, 0])
        end = d * R0 * rng.uniform(*reach) + np.array([0, -0.8, 0])
        pts = bezier(start, mid, end, [10, 6, 3][lod])
        rr = np.linspace(R0 * 0.55, 0.1, len(pts))
        tube(m, pts, rr, [9, 6, 4][lod], mat, lambda q: 0.0, 0.8, moss_of=moss_of)


def append(m, other):
    off = len(m.p)
    m.p += other.p
    m.n += other.n
    m.uv += other.uv
    m.c += other.c
    m.i += [i + off for i in other.i]


def mound_h(r, R, A):
    """Профиль бугра почвы: A у ствола, к краю R уходит на 0,15 м под грунт."""
    u = min(r / R, 1.0)
    return A * (1 - u * u) ** 1.5 - 0.15 * u ** 4


def mound(m, rng, R, A, lod, moss=0.6):
    """Бугор почвы у основания: корни приподнимают грунт; край уходит под
    рельеф, поэтому шва с землёй не видно. Неровный по углу (над корнями выше)."""
    rings, sides = [7, 4, 0][lod], [28, 14, 0][lod]
    if rings == 0:
        return
    bumps = rng.uniform(0, 6.28, 3)
    ids = []
    for k in range(rings + 1):
        r = R * (k / rings) ** 0.8
        row = []
        for s in range(sides + 1):
            th = 2 * math.pi * s / sides
            lump = 1 + 0.35 * math.sin(5 * th + bumps[0]) * math.sin(3 * th + bumps[1])
            h = mound_h(r, R, A) * lump
            # Нормаль по разности высот вдоль радиуса.
            dr = 0.05 * R
            dh = (mound_h(min(r + dr, R), R, A) - mound_h(max(r - dr, 0), R, A)) * lump / (2 * dr)
            d = np.array([math.cos(th), 0, math.sin(th)])
            n = unit(np.array([0, 1.0, 0]) - d * dh)
            row.append(m.vert(d * r + np.array([0, h, 0]), n, (r * math.cos(th) / 2, r * math.sin(th) / 2),
                              SOIL, 0.0, 0.75 + 0.25 * k / rings, moss * (0.6 + 0.4 * n[1])))
        ids.append(row)
    for a, b in zip(ids, ids[1:]):
        for s in range(sides):
            m.tri(a[s], a[s + 1], b[s])
            m.tri(a[s + 1], b[s + 1], b[s])


def surface_roots(m, rng, R0, R, A, n, lod, mat, moss_of):
    """Корни, выходящие на поверхность: змеятся по бугру и уходят в грунт."""
    if lod == 2:
        return
    for k in range(n):
        th = 2 * math.pi * (k + rng.uniform(-0.3, 0.3)) / n
        length = R * rng.uniform(1.0, 1.6)
        steps = [12, 6][lod]
        pts, rr = [], []
        wob = rng.uniform(0, 6.28)
        for j in range(steps + 1):
            t = j / steps
            r = R0 * 0.6 + (length - R0 * 0.6) * t
            a = th + 0.25 * math.sin(wob + t * 5) * t
            rad = R0 * 0.28 * (1 - t) ** 1.3 + 0.04
            y = mound_h(r, R, A) + rad * (0.3 - 0.9 * t * t)
            pts.append(np.array([math.cos(a) * r, y, math.sin(a) * r]))
            rr.append(rad)
        tube(m, pts, rr, [7, 5][lod], mat, lambda q: 0.0, 0.8, moss_of=moss_of)


# --- Криптомерия ---------------------------------------------------------------

def sugi_params(H):
    crown0 = 0.35 * H

    def tropism(order, d, t, p):
        if order == 0:
            return np.array([0, 0.25, 0])
        if order == 1:
            return np.array([0, -0.05 + 0.16 * t, 0])  # провисает, конец приподнят
        return np.array([0, 0.02, 0])

    def kid_dir(order, d, tc):
        if order == 0:
            return unit(np.array([d[0], d[1] * 0.25 - 0.08, d[2]]))
        return d

    def kid_len(order, L, tc):
        if order == 0:
            u = np.clip((tc * H - crown0) / (H - crown0), 0, 1)
            return 0.6 + 4.4 * (1 - u) ** 0.85 * min(1.0, 0.45 + 2.5 * u)
        return 0.38 * L * (1 - 0.5 * tc)

    def kids(order, L, rng):
        return [58, max(3, int(L * 1.6)), 0][order]

    return dict(segs=[20, 6, 3], gnarl=[0.015, 0.10, 0.18], taper=[0.03, 0.15, 0.3],
                kid_t0=[crown0 / H, 0.25, 0], angle=[82, 55, 0], kid_r=[0.30, 0.45, 0],
                max_order=2, leaf_order=1, leaf_t0=[1, 0.45, 0.0], clump_step=0.75, clump_size=1.15,
                tropism=tropism, kid_dir=kid_dir, kid_len=kid_len, kids=kids)


def sugi(lod, seed, H=27.0, R0=0.85):
    rng = np.random.default_rng(seed)
    sk = Skeleton()
    P = sugi_params(H)
    lean = np.array([rng.normal(0, 0.06), 1.0, rng.normal(0, 0.06)])
    grow(sk, rng, P, np.zeros(3), lean, H, R0, 0)
    # Нижние ветви засохли: короткие обломки на стволе ниже кроны.
    for k in range(10):
        y = rng.uniform(3, 0.35 * H)
        th = rng.uniform(0, 6.28)
        d = unit(np.array([math.cos(th), rng.uniform(-0.3, 0.1), math.sin(th)]))
        grow(sk, rng, P, np.array([0, y, 0]), d, rng.uniform(0.3, 1.2), 0.05, 2, stub=True)
    phase = rng.uniform(0, 6.28)

    def flute(t, th):
        y = t * H
        return 1.0 + (0.09 * math.sin(7 * th + phase + 0.3 * y) + 0.05 * math.sin(13 * th - 0.5 * y)) * max(0.0, 1.2 - y / 6)

    m = mesh_tree(sk, lod, np.random.default_rng(seed + lod), BARK_SUGI, NEEDLES, 0, H, 0.55, flute)
    r = Mesh()
    rmoss = lambda q, n: 0.7 * (0.4 + 0.6 * max(n[1], 0))  # noqa: E731
    if lod < 2:
        roots(r, rng, R0, 6 if lod == 0 else 4, lod, BARK_SUGI, rmoss)
    mound(r, rng, R0 * 3.2, R0 * 0.45, lod)
    surface_roots(r, rng, R0, R0 * 3.2, R0 * 0.45, 5, lod, BARK_SUGI, rmoss)
    append(m, r)
    return m


def sugi_old(lod, seed=0x01D5):
    """Древняя якусуги: ствол 3,6 м в обхвате у основания (до масштаба экземпляра)."""
    rng = np.random.default_rng(seed)
    H, R0 = 21.0, 1.7
    sk = Skeleton()

    def tropism(order, d, t, p):
        if order == 0:
            return np.array([0, 0.2, 0])
        if order == 1:
            return np.array([0, 0.12, 0])
        if order == 2:
            return np.array([0, -0.02 + 0.10 * t, 0])
        return np.array([0, 0.03, 0])

    def kid_dir(order, d, tc):
        if order == 1:
            return unit(np.array([d[0], d[1] * 0.3, d[2]]))
        return d

    def kid_len(order, L, tc):
        return [8.5, 0.55 * L * (1.1 - 0.6 * tc), 0.35 * L][order]

    def kids(order, L, rng):
        return [3, 9, max(2, int(L * 1.2))][order]

    P = dict(segs=[16, 8, 6, 3], gnarl=[0.05, 0.12, 0.16, 0.2], taper=[0.32, 0.25, 0.2, 0.3],
             kid_t0=[0.98, 0.2, 0.3], angle=[28, 70, 50], kid_r=[0.55, 0.40, 0.45],
             max_order=3, leaf_order=2, leaf_t0=[1, 1, 0.5, 0.0], clump_step=0.7, clump_size=1.2,
             tropism=tropism, kid_dir=kid_dir, kid_len=kid_len, kids=kids)
    grow(sk, rng, P, np.zeros(3), np.array([0.05, 1.0, -0.03]), 0.6 * H, R0, 0)
    # Обломанные сучья — по стволу и вдоль лидеров.
    for k in range(12):
        y = rng.uniform(2.5, 0.6 * H)
        th = rng.uniform(0, 6.28)
        d = unit(np.array([math.cos(th), rng.uniform(-0.2, 0.4), math.sin(th)]))
        grow(sk, rng, P, np.array([0, y, 0]) + d * R0 * 0.3, d, rng.uniform(0.6, 2.4), rng.uniform(0.08, 0.2), 3, stub=True)
    phase = rng.uniform(0, 6.28)

    def flute(t, th):
        y = t * 0.6 * H
        deep = max(0.0, 1.3 - y / 10)
        return 1.0 + deep * (0.16 * math.sin(5 * th + phase + 0.25 * y) + 0.08 * math.sin(11 * th - 0.4 * y)
                             + 0.05 * math.sin(19 * th + 0.9 * y))

    m = mesh_tree(sk, lod, np.random.default_rng(seed + lod), BARK_SUGI, NEEDLES, 0, H, 1.0, flute)
    r = Mesh()
    rmoss = lambda q, n: 0.5 + 0.5 * max(n[1], 0)  # noqa: E731
    roots(r, rng, R0, [10, 7, 5][lod], lod, BARK_SUGI, rmoss, reach=(2.0, 3.2))
    mound(r, rng, R0 * 3.6, R0 * 0.55, lod, moss=0.9)
    surface_roots(r, rng, R0, R0 * 3.6, R0 * 0.55, 9, lod, BARK_SUGI, rmoss)
    append(m, r)
    return m


# --- Лиственные ----------------------------------------------------------------

def broad(lod, seed, trunk=5.0, R0=0.36, leaders=3, lead_angle=34, spread=1.0, stems=1):
    rng = np.random.default_rng(seed)
    sk = Skeleton()

    def tropism(order, d, t, p):
        out = unit(np.array([p[0], 0, p[2]])) * 0.06 * spread
        return [np.array([0, 0.18, 0]), np.array([0, 0.07, 0]) + out, np.array([0, 0.03, 0]) + out,
                np.array([0, 0.02, 0])][order]

    def kid_dir(order, d, tc):
        return d

    def kid_len(order, L, tc):
        return [7.0 * spread, 0.55 * L * (1.15 - 0.5 * tc), 0.5 * L * (1.1 - 0.4 * tc)][order]

    def kids(order, L, rng):
        return [leaders, 6, 4][order]

    P = dict(segs=[6, 7, 5, 3], gnarl=[0.07, 0.12, 0.16, 0.22], taper=[0.6, 0.22, 0.3, 0.35],
             kid_t0=[0.8, 0.3, 0.25], angle=[lead_angle, 48, 42], kid_r=[0.7, 0.45, 0.5],
             max_order=3, leaf_order=2, leaf_t0=[1, 1, 0.35, 0.0], clump_step=0.5, clump_size=1.15,
             tropism=tropism, kid_dir=kid_dir, kid_len=kid_len, kids=kids)
    for s in range(stems):
        th = rng.uniform(0, 6.28)
        tilt = 0.0 if stems == 1 else 0.25
        d = unit(np.array([math.sin(th) * tilt + rng.normal(0, 0.05), 1.0, math.cos(th) * tilt]))
        grow(sk, rng, P, np.array([0.15 * s, 0, -0.1 * s]), d, trunk * rng.uniform(0.9, 1.1), R0 / math.sqrt(stems), 0)
    H = trunk + 7.0 * spread + 3
    m = mesh_tree(sk, lod, np.random.default_rng(seed + lod), BARK_BROAD, LEAVES, 1, H, 0.45, None, cards=4)
    r = Mesh()
    rmoss = lambda q, n: 0.4 * max(n[1], 0)  # noqa: E731
    if lod < 2:
        roots(r, rng, R0, 4, lod, BARK_BROAD, rmoss, reach=(1.6, 2.2))
    mound(r, rng, R0 * 4.5, R0 * 0.5, lod, moss=0.4)
    surface_roots(r, rng, R0, R0 * 4.5, R0 * 0.5, 4, lod, BARK_BROAD, rmoss)
    append(m, r)
    return m


def shrub(lod, seed=0x5B4B):
    """Кустарник 1,5–2,5 м: несколько стеблей от основания, листва по всем."""
    rng = np.random.default_rng(seed)
    sk = Skeleton()
    P = dict(segs=[5, 3], gnarl=[0.12, 0.2], taper=[0.3, 0.3], kid_t0=[0.3], angle=[40], kid_r=[0.6],
             max_order=1, leaf_order=0, leaf_t0=[0.35, 0.0], clump_step=0.32, clump_size=0.55,
             tropism=lambda o, d, t, p: np.array([0, 0.06, 0]), kid_dir=lambda o, d, tc: d,
             kid_len=lambda o, L, tc: 0.45 * L, kids=lambda o, L, rng: 4)
    for s in range(7):
        th = s * 2.39996
        d = unit(np.array([math.cos(th) * 0.45, 1.0, math.sin(th) * 0.45]))
        grow(sk, rng, P, np.array([rng.normal(0, 0.1), 0, rng.normal(0, 0.1)]), d, rng.uniform(1.4, 2.3), 0.025, 0)
    return mesh_tree(sk, lod, np.random.default_rng(seed + lod), BARK_BROAD, LEAVES, 1, 2.2, 0.2, None, cards=3)


# --- Валежник ------------------------------------------------------------------

def jagged_cap(m, center, axis, ring_r, sides, mat, rng, moss):
    """Обломанный торец: веер от центра к рваному краю."""
    u, w = frame(axis)
    c = m.vert(center + axis * 0.05, axis, (0.5, 0.5), mat, 0.0, 0.6, moss)
    ring = []
    for s in range(sides + 1):
        th = 2 * math.pi * s / sides
        d = u * math.cos(th) + w * math.sin(th)
        p = center + d * ring_r + axis * rng.uniform(-0.15, 0.25)
        ring.append(m.vert(p, axis, (0.5 + 0.5 * math.cos(th), 0.5 + 0.5 * math.sin(th)), mat, 0.0, 0.6, moss))
    for s in range(sides):
        m.tri(c, ring[s], ring[s + 1])


def log(lod, seed=0x7071):
    """Упавший ствол 7 м, на треть в подстилке, сверху мох."""
    rng = np.random.default_rng(seed)
    m = Mesh()
    n = [12, 6, 3][lod]
    xs = np.linspace(-3.5, 3.5, n + 1)
    pts = np.stack([xs, 0.28 + 0.08 * np.sin(xs * 0.7), 0.15 * np.sin(xs * 0.45 + 1)], 1)
    radii = np.linspace(0.42, 0.32, n + 1) * (1 + 0.05 * rng.normal(size=n + 1))
    sides = [16, 9, 6][lod]
    moss = lambda q, nn: min(1.0, 0.25 + 0.9 * max(nn[1], 0))  # noqa: E731
    tube(m, pts, radii, sides, BARK_SUGI, lambda q: 0.0, 0.85, u_tiles=3, moss_of=moss)
    jagged_cap(m, pts[0], unit(pts[0] - pts[1]), radii[0], sides, BARK_SUGI, rng, 0.3)
    jagged_cap(m, pts[-1], unit(pts[-1] - pts[-2]), radii[-1], sides, BARK_SUGI, rng, 0.3)
    if lod < 2:
        for k in range(3):
            x = rng.uniform(-2.5, 2.5)
            base = np.array([x, 0.5, 0.1])
            d = unit(np.array([rng.normal(0, 0.3), 1.0, rng.normal(0, 0.5)]))
            tube(m, [base, base + d * rng.uniform(0.3, 0.8)], [0.06, 0.03], 5, BARK_SUGI, lambda q: 0.0, 0.8,
                 moss_of=moss)
    return m


def stump(lod, seed=0x5707):
    """Полый пень якусуги: стенки расщеплены вверху, сбоку — проём в дупло
    (как пни у Сиратани и пень Уилсона), сверху мох и папоротник."""
    rng = np.random.default_rng(seed)
    m = Mesh()
    R0, Hs = 1.2, 2.0
    sides = [28, 14, 8][lod]
    rings = [10, 5, 3][lod]
    wall = 0.22
    gap0, gap1 = 0.15 * 2 * math.pi, 0.32 * 2 * math.pi  # проём
    phase = rng.uniform(0, 6.28)
    spikes = rng.uniform(0.0, 1.0, sides + 1)
    moss = lambda q, n: min(1.0, 0.45 + 0.7 * max(n[1], 0))  # noqa: E731
    for inner in (False, True):
        grid = []
        for k in range(rings + 1):
            t = k / rings
            row = []
            for s in range(sides + 1):
                th = 2 * math.pi * s / sides
                top = Hs * (0.55 + 0.45 * spikes[s % sides]) if lod < 2 else Hs * 0.8
                y = top * t
                r = R0 * (1 + 0.5 * math.exp(-y / 0.5)) * (1 + (0.12 * math.sin(6 * th + phase) if lod < 2 else 0))
                if inner:
                    r -= wall
                d = np.array([math.cos(th), 0, math.sin(th)])
                n = -d if inner else d
                row.append(m.vert(d * r + np.array([0, y, 0]), n, (s / sides * 5, y / 1.5), BARK_SUGI, 0.0,
                                  0.35 if inner else 0.9, moss(d * r, n) * (0.4 if inner else 1.0)))
            grid.append(row)
        for k in range(rings):
            for s in range(sides):
                th = 2 * math.pi * (s + 0.5) / sides
                if gap0 < th < gap1 and k < rings * 0.6:
                    continue
                a, b2, c, dd = grid[k][s], grid[k][s + 1], grid[k + 1][s], grid[k + 1][s + 1]
                if inner:
                    m.tri(a, c, b2)
                    m.tri(b2, c, dd)
                else:
                    m.tri(a, b2, c)
                    m.tri(b2, dd, c)
    roots(m, rng, R0 * 0.9, [6, 4, 3][lod], lod, BARK_SUGI, moss, reach=(1.8, 2.4))
    mound(m, rng, R0 * 3.0, 0.35, lod, moss=0.9)
    if lod < 2:
        for k in range(5):
            th = k * 1.26 + rng.uniform(0, 0.4)
            if gap0 < th % 6.28 < gap1:
                continue
            d = np.array([math.cos(th), 0, math.sin(th)])
            c = d * (R0 - wall * 0.5) + np.array([0, Hs * 0.6, 0])
            card(m, c, d * 0.4 + np.array([0, 0.2, 0]), np.cross(np.array([0, 1.0, 0]), d) * 0.35,
                 np.array([0, 1.0, 0]), atlas_rect(2, k % 4), FERN, 0.5, 0.8)
    return m


def windfall(lod, seed=0x3F11):
    """Вывороченная бурей криптомерия: ствол лежит, у основания — корневая
    тарелка с землёй стоймя, крона обломана, всё обросло мхом."""
    rng = np.random.default_rng(seed)
    m = Mesh()
    L, R0 = 14.0, 0.55
    n = [16, 8, 4][lod]
    xs = np.linspace(0, L, n + 1)
    pts = np.stack([xs, R0 * 0.75 + 0.9 * np.exp(-xs / 2.0) + 0.05 * np.sin(xs * 0.6), 0.2 * np.sin(xs * 0.3)], 1)
    radii = R0 * (1 - xs / L * 0.6) * (1 + 0.35 * np.exp(-xs / 0.8))
    moss = lambda q, nn: min(1.0, 0.35 + 0.9 * max(nn[1], 0))  # noqa: E731
    tube(m, pts, radii, [16, 9, 6][lod], BARK_SUGI, lambda q: 0.0, 0.85, u_tiles=4, moss_of=moss)
    jagged_cap(m, pts[-1], unit(pts[-1] - pts[-2]), radii[-1], [16, 9, 6][lod], BARK_SUGI, rng, 0.5)
    # Корневая тарелка: диск почвы в плоскости YZ, корни торчат по краю.
    plate_c = np.array([-0.3, 1.4, 0.0])
    PR = 2.2
    sides = [24, 12, 8][lod]
    c0 = m.vert(plate_c + np.array([-0.35, 0, 0]), np.array([-1.0, 0, 0]), (0.5, 0.5), SOIL, 0.0, 0.6, 0.2)
    ring = []
    for s in range(sides + 1):
        th = 2 * math.pi * s / sides
        r = PR * (0.85 + 0.25 * math.sin(3 * th + 1) * math.sin(5 * th))
        p = plate_c + np.array([rng.uniform(-0.1, 0.2), math.sin(th) * r * 0.8, math.cos(th) * r])
        p[1] = max(p[1], -0.2)
        ring.append(m.vert(p, np.array([-1.0, 0.2, 0]), (0.5 + 0.5 * math.cos(th), 0.5 + 0.5 * math.sin(th)),
                           SOIL, 0.0, 0.6, 0.3 + 0.5 * max(math.sin(th), 0)))
    for s in range(sides):
        m.tri(c0, ring[s + 1], ring[s])
    c1 = m.vert(plate_c + np.array([0.35, 0, 0]), np.array([1.0, 0, 0]), (0.5, 0.5), SOIL, 0.0, 0.5, 0.6)
    for s in range(sides):
        m.tri(c1, ring[s], ring[s + 1])
    if lod < 2:
        for k in range([14, 7][lod]):
            th = 2 * math.pi * k / [14, 7][lod] + rng.uniform(-0.2, 0.2)
            d = np.array([rng.uniform(-0.6, -0.1), math.sin(th) * 0.8, math.cos(th)])
            start = plate_c + d * PR * 0.6
            tip = plate_c + d * PR * rng.uniform(1.05, 1.4) + np.array([-0.3, 0, 0])
            tube(m, bezier(start, (start + tip) / 2 + np.array([-0.2, 0.1, 0]), tip, 5), np.linspace(0.12, 0.02, 5),
                 5, BARK_SUGI, lambda q: 0.0, 0.7, moss_of=lambda q, nn: 0.3)
    # Обломки ветвей и остатки кроны у вершины: сухие сучья, кое-где мох.
    for k in range([12, 6, 2][lod]):
        x = rng.uniform(5.0, L)
        base = np.array([x, np.interp(x, xs, pts[:, 1]), np.interp(x, xs, pts[:, 2])])
        th = rng.uniform(0, 6.28)
        d = unit(np.array([rng.uniform(0.1, 0.6), math.cos(th), math.sin(th)]))
        tip = base + d * rng.uniform(0.8, 2.6)
        if tip[1] < 0.05:
            tip[1] = 0.05
        tube(m, [base, (base + tip) / 2 + np.array([0, 0.1, 0]), tip], [0.09, 0.06, 0.02], [6, 4, 3][lod], BARK_SUGI,
             lambda q: 0.0, 0.8, moss_of=moss)
    return m


SPECIES = {
    "sugi_a": lambda l: sugi(l, 0x5091, H=27.0, R0=0.85),
    "sugi_b": lambda l: sugi(l, 0x5C1B, H=23.0, R0=0.7),
    "sugi_old": sugi_old,
    "broad_a": lambda l: broad(l, 0xB20A),
    "broad_b": lambda l: broad(l, 0xB2B2, trunk=7.0, R0=0.32, leaders=2, lead_angle=22, spread=0.8),
    "broad_c": lambda l: broad(l, 0xB2C3, trunk=3.0, R0=0.42, leaders=3, lead_angle=40, spread=1.15, stems=2),
    "shrub": shrub,
    "log": log,
    "stump": stump,
    "windfall": windfall,
}
