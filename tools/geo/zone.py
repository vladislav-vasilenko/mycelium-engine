"""Игровая зона с рельефом ~1 м (02.7, шаг 5): лазерная съёмка GSI DEM1A.

Зона — квадрат клеток острова [I0, I0 + Z) × [J0, J0 + Z) шага 7,5 м, внутри
каждая клетка делится на R × R (R = 8, шаг 0,9375 м). Каждый R-й узел зоны
совпадает с узлом острова — движок сшивает зону с островом без щелей
(внешнее кольцо клеток острова триангулируется веером к узлам границы зоны).
Высоты квантуются в 16 бит в собственном диапазоне зоны (шаг ~1 см).
"""

import numpy as np
from scipy import ndimage

import gsi

# Ущелье Сиратани: начало тропы и мшистый лес якусуги.
ZONE_I0, ZONE_J0, ZONE_CELLS, ZONE_RATIO = 2315, 1248, 384, 8


def dem1a_keep(meshes):
    """Отбор файлов DEM1A по коду квадрата 2-го уровня и номерам 3-го."""
    def keep(name):
        if name.endswith(".zip"):
            return any(f"-{m2}-" in name for m2, _ in meshes)
        return any(f"-{m2[:4]}-{m2[4:]}-{m3}-" in name for m2, m3 in meshes)
    return keep


def meshes_for(bbox):
    """Квадраты 3-го уровня (код 2-го уровня, «строка столбец»), покрывающие bbox."""
    s, w, n, e = bbox
    out = set()
    for lat in np.arange(s, n + 1e-9, 15 / 3600):
        for lon in np.arange(w, e + 1e-9, 20 / 3600):
            for la, lo in ((lat, lon), (min(lat, n), min(lon, e))):
                r2, c2 = int((la - 30) / (5 / 60)), int((lo - 130) / 0.125)
                r3 = int(((la - 30) - r2 * 5 / 60) / (30 / 3600))
                c3 = int(((lo - 130) - c2 * 0.125) / (45 / 3600))
                out.add((f"4530{r2}{c2}", f"{r3}{c3}"))
    return sorted(out)


def sample(m, bbox, step_arcsec, lat, lon):
    """Билинейная выборка мозаики (строка 0 — север); NaN — нет данных."""
    s, w, n, e = bbox
    d = step_arcsec / 3600.0
    r = (n - lat) / d - 0.5
    c = (lon - w) / d - 0.5
    r0 = np.clip(np.floor(r).astype(int), 0, m.shape[0] - 2)
    c0 = np.clip(np.floor(c).astype(int), 0, m.shape[1] - 2)
    fr, fc = np.clip(r - r0, 0, 1), np.clip(c - c0, 0, 1)
    return (m[r0, c0] * (1 - fc) + m[r0, c0 + 1] * fc) * (1 - fr) + (
        m[r0 + 1, c0] * (1 - fc) + m[r0 + 1, c0 + 1] * fc
    ) * fr


def build(archives, world_to_geo, far_step, fallback):
    """Высоты узлов зоны (n × n) и метаданные. `fallback(lat, lon)` — DEM5A там,
    где у DEM1A пропуски (вода, тени съёмки)."""
    n = ZONE_CELLS * ZONE_RATIO + 1
    step = far_step / ZONE_RATIO
    x0, z0 = ZONE_I0 * far_step, ZONE_J0 * far_step
    idx = np.arange(n) * step
    xs, zs = np.meshgrid(x0 + idx, z0 + idx)
    lat, lon = world_to_geo(xs, zs)
    pad = 0.0006
    bbox = (lat.min() - pad, lon.min() - pad, lat.max() + pad, lon.max() + pad)
    meshes = meshes_for(bbox)
    m, count = gsi.mosaic(archives, "DEM1A", bbox, 0.04, keep=dem1a_keep(meshes))
    h = sample(m, bbox, 0.04, lat, lon)
    holes = ~np.isfinite(h)
    if holes.any():
        h[holes] = fallback(lat[holes], lon[holes])
    # Остаточные дыры (за охватом обоих) — ближайшее известное значение.
    if (~np.isfinite(h)).any():
        idx_near = ndimage.distance_transform_edt(~np.isfinite(h), return_distances=False, return_indices=True)
        h = h[tuple(idx_near)]
    lo = float(np.floor(h.min()) - 1.0)
    hi = float(np.ceil(h.max()) + 1.0)
    meta = {
        "file": "height_zone.png",
        "origin_cell": [ZONE_I0, ZONE_J0],
        "cells": ZONE_CELLS,
        "ratio": ZONE_RATIO,
        "height_range_m": [lo, hi],
        "source": "GSI DEM1A",
    }
    print(
        f"зона Сиратани: DEM1A квадратов {count} ({len(meshes)} по охвату), узлов {n}², шаг {step:.4f} м, "
        f"высоты {h.min():.0f}…{h.max():.0f} м, пропусков DEM1A {100 * holes.mean():.1f} %"
    )
    return h, meta
