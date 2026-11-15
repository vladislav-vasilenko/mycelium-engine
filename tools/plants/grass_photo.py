"""Фототекстуры травы для пучков (02.13): атласы Poly Haven (CC0) —
`grass_medium_02` (одиночные травинки, зелёные и сухие) и `grass_bermuda_01`
(кустики и стебли).

Запуск: `.venv-geo/bin/python -I tools/plants/grass_photo.py` (после
`fetch_polyhaven.py`). Пишет в `assets/models/plants/`:
- `grass_strips.png`, `grass_tufts.png` — RGBA 1024², цвет с альфой, цвет
  прозрачных пикселей растянут от соседей (без каймы на мипах);
- `grass_rects.json` — прямоугольники элементов атласов в uv (x0, y0, x1, y1;
  y вниз): `strips` — травинки, `tufts` — кустики, `stems` — высокие стебли.
Элементы находятся как связные области альфы.
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_plants import bleed  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "assets/source/polyhaven"
OUT = ROOT / "assets/models/plants"


def components(alpha, min_px):
    """Связные области маски (4-связность) на уменьшенной копии → uv-рамки."""
    k = 4
    small = alpha.reshape(alpha.shape[0] // k, k, alpha.shape[1] // k, k).max(axis=(1, 3)) > 127
    h, w = small.shape
    seen = np.zeros_like(small)
    boxes = []
    for y in range(h):
        for x in range(w):
            if not small[y, x] or seen[y, x]:
                continue
            stack, pts = [(y, x)], []
            seen[y, x] = True
            while stack:
                cy, cx = stack.pop()
                pts.append((cy, cx))
                for ny, nx in ((cy + 1, cx), (cy - 1, cx), (cy, cx + 1), (cy, cx - 1)):
                    if 0 <= ny < h and 0 <= nx < w and small[ny, nx] and not seen[ny, nx]:
                        seen[ny, nx] = True
                        stack.append((ny, nx))
            if len(pts) < min_px:
                continue
            ys, xs = zip(*pts)
            boxes.append(((min(xs) - 1) / w, (min(ys) - 1) / h, (max(xs) + 2) / w, (max(ys) + 2) / h))
    return [tuple(float(np.clip(v, 0, 1)) for v in b) for b in boxes]


def atlas(name):
    tex = SRC / name / "textures"
    rgb = Image.open(tex / f"{name}_diff_1k.jpg").convert("RGB").resize((1024, 1024), Image.LANCZOS)
    a = Image.open(tex / f"{name}_alpha_1k.png").convert("L").resize((1024, 1024), Image.LANCZOS)
    img = rgb.copy()
    img.putalpha(a)
    return bleed(img), np.asarray(a)


def main():
    strips, sa = atlas("grass_medium_02")
    tufts, ta = atlas("grass_bermuda_01")
    strips.save(OUT / "grass_strips.png", optimize=True)
    tufts.save(OUT / "grass_tufts.png", optimize=True)
    # Травинки — вытянутые вверх (лежащая горизонтально внизу атласа — нет).
    s = [b for b in components(sa, 40) if (b[3] - b[1]) > 2.0 * (b[2] - b[0])]
    t_all = components(ta, 60)
    # Кустики — не слишком вытянутые; стебли — узкие высокие.
    tufts_r = [b for b in t_all if (b[3] - b[1]) < 1.6 * (b[2] - b[0]) or (b[3] - b[1]) < 0.3]
    stems = [b for b in t_all if b not in tufts_r]
    rects = {"strips": s, "tufts": tufts_r, "stems": stems}
    (OUT / "grass_rects.json").write_text(json.dumps(rects, indent=1) + "\n")
    print({k: len(v) for k, v in rects.items()})


if __name__ == "__main__":
    main()
