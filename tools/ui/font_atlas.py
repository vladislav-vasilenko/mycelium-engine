"""Атлас шрифта интерфейса (05.1): PT Sans (OFL) → `assets/ui/font.png` + `font.json`.

Поле расстояний (SDF) в одном канале: текст остаётся чётким при любом
масштабе. Набор знаков — ASCII, кириллица, «»—…№°·×.

Запуск: `.venv-geo/bin/python -I tools/ui/font_atlas.py`.
"""

import json
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
from scipy import ndimage

ROOT = Path(__file__).resolve().parents[2]
TTF = ROOT / "assets/source/fonts/pt-sans/PTSans-Regular.ttf"
OUT = ROOT / "assets/ui"
SIZE = 48          # пиксели глифа при рендере
SPREAD = 6         # радиус поля расстояний, пиксели
ATLAS = 1024
CHARS = "".join(chr(c) for c in range(32, 127)) + "".join(chr(c) for c in range(0x410, 0x450)) + "ЁёЇїІіЄє«»—–…№°·×✓"


def sdf(mask):
    inside = ndimage.distance_transform_edt(mask)
    outside = ndimage.distance_transform_edt(~mask)
    d = np.where(mask, inside - 0.5, -(outside - 0.5))
    return np.clip(d / SPREAD * 0.5 + 0.5, 0, 1)


def main():
    font = ImageFont.truetype(str(TTF), SIZE)
    ascent, descent = font.getmetrics()
    line = ascent + descent
    atlas = np.zeros((ATLAS, ATLAS), np.float32)
    x = y = 0
    row_h = line + 2 * SPREAD
    glyphs = {}
    for ch in CHARS:
        adv = font.getlength(ch)
        l, t, r, b = font.getbbox(ch)
        w = max(1, r - l) + 2 * SPREAD
        h = row_h
        if x + w > ATLAS:
            x, y = 0, y + row_h
        img = Image.new("L", (w, h), 0)
        ImageDraw.Draw(img).text((SPREAD - l, SPREAD), ch, font=font, fill=255)
        mask = np.asarray(img) > 127
        atlas[y:y + h, x:x + w] = sdf(mask) if mask.any() else 0.0
        glyphs[ch] = {
            "x": x, "y": y, "w": w, "h": h,
            "ox": l - SPREAD, "oy": -SPREAD, "adv": adv,
        }
        x += w + 1
    OUT.mkdir(parents=True, exist_ok=True)
    Image.fromarray(np.uint8(atlas * 255), "L").save(OUT / "font.png", optimize=True)
    (OUT / "font.json").write_text(json.dumps({
        "size": SIZE, "line": line, "ascent": ascent, "spread": SPREAD, "atlas": ATLAS,
        "source": "PT Sans Regular © ParaType, SIL Open Font License 1.1",
        "glyphs": glyphs,
    }, ensure_ascii=False) + "\n")
    print(f"знаков {len(glyphs)}, строк атласа до {y + row_h}")


if __name__ == "__main__":
    main()
