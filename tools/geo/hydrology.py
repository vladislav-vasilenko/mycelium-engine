"""Сток по рельефу (02.7, шаг 3): заполнение бессточных впадин (priority
flood), направление D8, накопление площади водосбора, русла по порогу.

Сетка — строки по z, столбцы по x, шаг `step` м. Море — выход стока.
"""

import heapq

import numpy as np

# Восемь соседей: (dz, dx).
NEIGHBORS = [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)]
# Подъём на шаг при заполнении: у заполненной впадины остаётся уклон к выходу.
FILL_EPS = 1e-3


def fill(height, land):
    """Priority flood: впадины суши заполняются до уровня перелива.

    Возвращает заполненный рельеф и порядок обработки клеток (от выхода вверх
    по течению): обратный порядок — топологический для накопления.
    """
    nz, nx = height.shape
    filled = np.where(land, height, -np.inf)
    done = ~land.copy()
    heap = []
    # Затравка: суша у моря и у края карты.
    sea = ~land
    border = np.zeros_like(land)
    border[0, :] = border[-1, :] = border[:, 0] = border[:, -1] = True
    near_sea = np.zeros_like(land)
    for dz, dx in NEIGHBORS:
        near_sea |= np.roll(np.roll(sea, dz, 0), dx, 1)
    seeds = land & (near_sea | border)
    for z, x in zip(*np.nonzero(seeds)):
        heapq.heappush(heap, (filled[z, x], int(z), int(x)))
        done[z, x] = True
    order = []
    h = filled
    while heap:
        hz, z, x = heapq.heappop(heap)
        order.append(z * nx + x)
        for dz, dx in NEIGHBORS:
            zz, xx = z + dz, x + dx
            if 0 <= zz < nz and 0 <= xx < nx and not done[zz, xx]:
                done[zz, xx] = True
                if h[zz, xx] <= hz:
                    h[zz, xx] = hz + FILL_EPS
                heapq.heappush(heap, (h[zz, xx], zz, xx))
    return h, np.array(order, dtype=np.int64)


def d8(filled, land, step):
    """Направление стока: индекс соседа с наибольшим уклоном вниз; −1 — море."""
    nz, nx = filled.shape
    pad = np.pad(np.where(land, filled, -1e9), 1, constant_values=-1e9)
    best = np.zeros((nz, nx))
    down = np.full((nz, nx), -1, dtype=np.int64)
    zs, xs = np.mgrid[0:nz, 0:nx]
    for dz, dx in NEIGHBORS:
        nb = pad[1 + dz : 1 + dz + nz, 1 + dx : 1 + dx + nx]
        drop = (filled - nb) / (step * np.hypot(dz, dx))
        zz = np.clip(zs + dz, 0, nz - 1)
        xx = np.clip(xs + dx, 0, nx - 1)
        inside = (zs + dz >= 0) & (zs + dz < nz) & (xs + dx >= 0) & (xs + dx < nx)
        target = np.where(inside & land[zz, xx], zz * nx + xx, -1)
        better = land & (drop > best)
        best = np.where(better, drop, best)
        down = np.where(better, target, down)
    return down


def accumulate(down, order, land):
    """Площадь водосбора в клетках (с самой клеткой)."""
    acc = np.where(land, 1.0, 0.0).ravel()
    flat = down.ravel()
    for i in order[::-1]:
        d = flat[i]
        if d >= 0:
            acc[d] += acc[i]
    return acc.reshape(down.shape)


def flow(height, land, step, burn=None, burn_m=10.0):
    """Сток целиком. `burn` — маска известных русел, врезаемых на `burn_m`."""
    h = height - (burn * burn_m if burn is not None else 0.0)
    filled, order = fill(h, land)
    down = d8(filled, land, step)
    acc = accumulate(down, order, land)
    return filled - h, down, acc


def channel_segments(down, acc, land, step, min_cells):
    """Отрезки русел от клетки к клетке вниз по течению: (x0, z0, x1, z1, площадь м²)."""
    nz, nx = down.shape
    zs, xs = np.nonzero(land & (acc >= min_cells))
    out = []
    for z, x in zip(zs, xs):
        d = down[z, x]
        if d >= 0:
            z1, x1 = divmod(int(d), nx)
            out.append((x * step, z * step, x1 * step, z1 * step, acc[z, x] * step * step))
    return np.array(out)


def river_width_m(area_m2):
    """Ширина русла по площади водосбора: 1,5 м у истока, ~30 м у устья Анбо."""
    return np.clip(1.5 + 3.5 * np.sqrt(area_m2 / 1e6), 1.5, 40.0)
