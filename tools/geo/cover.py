"""Пояса растительности Якусимы (02.7, шаг 4) по покрову ESA WorldCover и высоте.

WorldCover 2021 (10 м) даёт классы: 10 лес, 20 кустарник, 30 трава,
40 пашня, 50 застройка, 60 голый грунт, 80 вода, 90 водно-болотные,
95 мангры, 100 мох и лишайник. Вместе с высотой и уклоном получаются четыре
пояса — каналы `cover.png` (RGBA 8 бит, сумма на суше = 255):

- R — субтропический и вечнозелёный широколиственный лес (низы до ~700 м);
- G — мшистый лес криптомерии «якусуги» (~600–1600 м);
- B — бамбуковая трава «якудзаса», луга, кустарник и пашня;
- A — голый гранит, осыпи и застройка.

Граница лесных поясов размыта шумом ±80 м, выше ~1550 м лес редеет в
якудзасу. © ESA WorldCover project 2021, CC BY 4.0.
"""

import numpy as np
import tifffile
from PIL import Image
from scipy import ndimage

# Тайл N30E129: 3° × 3°, 36 000², верх 33° с. ш., лево 129° в. д.
TILE_TOP, TILE_LEFT, PX_PER_DEG = 33.0, 129.0, 12000
CLASSES = {
    "tree": (10, 95),
    "low": (20, 30, 40, 90, 100),
    "bare": (60,),
    "built": (50,),
}


def load_fractions(path, bbox):
    """Доли групп классов в окне 3 × 3 пикселя (30 м) внутри `bbox` (ю, з, с, в)."""
    s, w, n, e = bbox
    r0, r1 = int((TILE_TOP - n) * PX_PER_DEG), int((TILE_TOP - s) * PX_PER_DEG)
    c0, c1 = int((w - TILE_LEFT) * PX_PER_DEG), int((e - TILE_LEFT) * PX_PER_DEG)
    with tifffile.TiffFile(path) as t:
        m = t.pages[0].asarray()[r0:r1, c0:c1]
    frac = {}
    for name, codes in CLASSES.items():
        frac[name] = ndimage.uniform_filter(np.isin(m, codes).astype(np.float32), 3)
    return frac, (n, w)


def sample(grid, origin, lat, lon):
    n, w = origin
    r = np.clip(((n - lat) * PX_PER_DEG).astype(int), 0, grid.shape[0] - 1)
    c = np.clip(((lon - w) * PX_PER_DEG).astype(int), 0, grid.shape[1] - 1)
    return grid[r, c]


def smoothstep(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3 - 2 * t)


def belts(frac, origin, lat, lon, height, land, seed=0x5A4B):
    """Веса четырёх поясов (H × W × 4), на суше сумма 1, в море 0."""
    f = {k: sample(v, origin, lat, lon) for k, v in frac.items()}
    rng = np.random.default_rng(seed)
    # Плавный шум ±80 м по высоте: граница поясов не горизонталь.
    jitter = ndimage.gaussian_filter(rng.standard_normal(height.shape), 6) * 80 / 0.07
    h = height + np.clip(jitter, -120, 120)
    cedar = smoothstep(500.0, 900.0, h)
    treeline = smoothstep(1500.0, 1750.0, h)
    tree = f["tree"]
    lowland = tree * (1 - cedar) * (1 - treeline)
    yakusugi = tree * cedar * (1 - treeline)
    grass = f["low"] + tree * treeline
    bare = f["bare"] + f["built"]
    w = np.stack([lowland, yakusugi, grass, bare], -1)
    s = w.sum(-1, keepdims=True)
    # Где WorldCover не дал суши (кромка, вода) — пояс по высоте.
    fallback = np.stack([1 - cedar, cedar * (1 - treeline), treeline, np.zeros_like(h)], -1)
    w = np.where(s > 0.05, w / np.maximum(s, 1e-6), fallback)
    return np.where(land[..., None], w, 0.0)


def encode(w):
    """Веса → RGBA 8 бит с суммой ровно 255 (наибольшие остатки)."""
    scaled = w * 255.0
    q = np.floor(scaled).astype(np.int32)
    total = np.where(w.sum(-1) > 0, 255, 0)
    rest = total - q.sum(-1)
    order = np.argsort(-(scaled - q), axis=-1)
    for k in range(4):
        idx = order[..., k]
        add = (rest > k).astype(np.int32)
        np.put_along_axis(q, idx[..., None], np.take_along_axis(q, idx[..., None], -1) + add[..., None], -1)
    return q.astype(np.uint8)


def write(path, w):
    Image.fromarray(encode(w), "RGBA").save(path, optimize=False)


def report(w, land, cell_m2):
    names = ["низинный лес", "кедровый лес якусуги", "якудзаса и луга", "гранит и застройка"]
    area = land.sum() * cell_m2 / 1e6
    parts = ", ".join(
        f"{n} {100 * (w[..., k] * land).sum() / max(land.sum(), 1):.0f} %" for k, n in enumerate(names)
    )
    print(f"пояса ({area:.0f} км² суши): {parts}")
