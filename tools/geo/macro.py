"""Крупная детализация рельефа (02.9, шаг 1): маски `macro.png` по полю высот.

Каналы RGBA 8 бит на сетке `height_fine.png` (шаг 7,5 м, узел — тексель):

- R — видимость неба: доля косинусно-взвешенного неба над точкой по 16
  азимутам до 2 км, относительно касательной плоскости (ровный склон — 1,
  дно ущелья — меньше);
- G — кривизна: 0,5 — ровно, меньше — впадина, больше — гребень (лапласиан
  сглаженной высоты на масштабах 15 и 60 м);
- B — влажность: логарифм площади водосбора `flow.png`, сглаженный;
- A — осыпь: подножие обрывов круче 45°, до 60 м вниз, на склоне положе 35°.

Запуск: `.venv-geo/bin/python -I tools/geo/macro.py assets/world/yakushima/far`.
"""

import json
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

AZIMUTHS = 16
MAX_DIST_M = 2000.0
STEEP_DEG = 45.0
# Осыпь лежит там, где обломкам есть где остановиться.
SCREE_MAX_DEG = 35.0
SCREE_M = 60.0


def load_height(d):
    meta = json.loads((d / "world.json").read_text())
    lo, hi = meta["height_range_m"]
    fine = meta["height_fine"]
    raw = np.asarray(Image.open(d / fine["file"]), dtype=np.float32)
    return lo + raw / 65535.0 * (hi - lo), fine["step_m"]


def shifted(a, dz, dx):
    """a[z + dz, x + dx] с краевыми значениями за границей."""
    n0, n1 = a.shape
    zi = np.clip(np.arange(n0) + dz, 0, n0 - 1)
    xi = np.clip(np.arange(n1) + dx, 0, n1 - 1)
    return a[zi][:, xi]


def sky_visibility(h, step):
    """Косинусно-взвешенная видимость неба относительно касательной плоскости."""
    gz, gx = np.gradient(h, step)
    dists = []
    d = step
    while d <= MAX_DIST_M:
        dists.append(d)
        d = max(d * 1.22, d + step)
    vis = np.zeros_like(h)
    plane = np.zeros_like(h)
    for k in range(AZIMUTHS):
        a = 2.0 * np.pi * k / AZIMUTHS
        dx, dz = np.cos(a), np.sin(a)
        # Подъём касательной плоскости в этом направлении.
        t_plane = np.maximum(gx * dx + gz * dz, 0.0)
        best = t_plane.copy()
        for dist in dists:
            ox, oz = int(round(dx * dist / step)), int(round(dz * dist / step))
            real = np.hypot(ox, oz) * step
            np.maximum(best, (shifted(h, oz, ox) - h) / real, out=best)
        # cos² угла горизонта = 1 / (1 + tg²).
        vis += 1.0 / (1.0 + best * best)
        plane += 1.0 / (1.0 + t_plane * t_plane)
        print(f"  азимут {k + 1}/{AZIMUTHS}", end="\r", flush=True)
    print()
    return np.clip(vis / plane, 0.0, 1.0)


def curvature(h, land):
    """0,5 — ровно, < 0,5 — впадина, > 0,5 — гребень."""
    lap = 0.6 * ndimage.gaussian_laplace(h, 2.0) + 0.4 * ndimage.gaussian_laplace(h, 8.0) * 4.0
    scale = np.percentile(np.abs(lap[land]), 90) + 1e-6
    return 0.5 - 0.5 * np.tanh(lap / scale * 1.5)


def moisture(d, shape, land):
    flow = np.asarray(Image.open(d / "flow.png"), dtype=np.float32) / 2048.0  # log2 клеток 30 м
    f = ndimage.zoom(flow, (shape[0] / flow.shape[0], shape[1] / flow.shape[1]), order=1)
    f = f[: shape[0], : shape[1]]
    # Водосбор от 3 до 400 клеток 30 м: склон → ложбина → ручей.
    m = np.clip((f - 1.5) / 7.0, 0.0, 1.0)
    m = ndimage.gaussian_filter(m, 2.0)
    return np.where(land, m, 1.0)


def scree(h, step, land):
    gz, gx = np.gradient(h, step)
    slope = np.degrees(np.arctan(np.hypot(gx, gz)))
    steep = (slope > STEEP_DEG) & land
    cells = int(SCREE_M / step)
    dist = ndimage.distance_transform_edt(~steep) * step
    above = ndimage.grey_dilation(np.where(steep, h, -1e9), size=2 * cells + 1)
    s = (slope < SCREE_MAX_DEG) & land & (above > h + 5.0) & (dist < SCREE_M)
    w = np.where(s, 1.0 - dist / SCREE_M, 0.0)
    return ndimage.gaussian_filter(w, 1.0), slope


def main():
    d = Path(sys.argv[1] if len(sys.argv) > 1 else "assets/world/yakushima/far")
    t0 = time.time()
    h, step = load_height(d)
    land = h > 0.0
    print(f"поле {h.shape[0]}², шаг {step} м")
    ao = sky_visibility(h, step)
    cv = curvature(h, land)
    wet = moisture(d, h.shape, land)
    sc, slope = scree(h, step, land)
    rgba = np.stack([ao, cv, wet, sc], axis=-1)
    rgba = np.round(np.clip(rgba, 0.0, 1.0) * 255.0).astype(np.uint8)
    Image.fromarray(rgba, "RGBA").save(d / "macro.png", optimize=True)
    meta = json.loads((d / "world.json").read_text())
    meta["macro"] = {
        "file": "macro.png",
        "size": int(h.shape[0]),
        "channels": "R видимость неба, G кривизна (0,5 ровно), B влажность, A осыпь",
    }
    (d / "world.json").write_text(json.dumps(meta, ensure_ascii=False, indent=2) + "\n")
    L = land
    print(
        f"готово за {time.time() - t0:.0f} с: видимость неба на суше "
        f"p5 {np.percentile(ao[L], 5):.2f} медиана {np.median(ao[L]):.2f}; "
        f"впадин < 0,4 — {np.mean(cv[L] < 0.4):.0%}, гребней > 0,6 — {np.mean(cv[L] > 0.6):.0%}; "
        f"влажно > 0,5 — {np.mean(wet[L] > 0.5):.0%}; осыпи > 0,3 — {np.mean(sc[L] > 0.3):.1%}; "
        f"круче {STEEP_DEG:.0f}° — {np.mean(slope[L] > STEEP_DEG):.1%}"
    )


if __name__ == "__main__":
    main()
