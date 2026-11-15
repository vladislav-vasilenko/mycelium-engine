"""Пояса растительности по карте растительности 1 : 25 000 Минприроды Японии.

Источник — 1/25,000植生図 (環境省生物多様性センター), сборка Pacific Spatial
Solutions (GeoParquet, CC BY 4.0); выборка Якусимы — `assets/source/geo/
vegetation/yakushima.parquet`. Карта различает сообщества (леса якусуги,
кастанопсисовые и дубовые леса, заросли якудзасы, посадки криптомерии,
болота, застройку), а не 11 классов спутникового покрова ESA WorldCover.

Скрипт растеризует полигоны на сетку `cover.png` (узел — 15 м) и
переписывает пояса: где есть полигон — по сообществу, где нет — остаётся
WorldCover. Пишет и `vegetation.png` (u8, номер сообщества) с легендой
`vegetation.json` — для расстановки леса (02.4).

Запуск: `.venv-geo/bin/python -I tools/geo/vegetation.py assets/world/yakushima/far`.

出典：1/25,000植生図（環境省生物多様性センター）を Pacific Spatial Solutions
株式会社が加工したデータを使用。CC BY 4.0.
"""

import json
import math
import sys
from pathlib import Path

import duckdb
import numpy as np
import shapely
from PIL import Image, ImageDraw
from scipy import ndimage

SRC = Path(__file__).resolve().parents[2] / "assets/source/geo/vegetation/yakushima.parquet"

# Пояса cover.png: R низинный лес, G кедровый лес якусуги, B якудзаса и луга,
# A гранит и застройка. Правила по японскому названию сообщества (統一凡例 и
# 植生区分): первое совпавшее слово задаёт веса.
RULES = [
    (("岩壁", "断崖", "珊瑚礁"), (0.0, 0.0, 0.4, 0.6)),
    (("竹林", "マツ"), (1.0, 0.0, 0.0, 0.0)),
    (("ヤクスギ", "スギ", "モミ", "ツガ", "ヒノキ", "針葉樹"), (0.15, 0.85, 0.0, 0.0)),
    (("ヤクシマダケ", "ヤクザサ", "ササ", "ススキ", "メダケ", "クズ", "ダンチク", "シバ", "草", "シャクナゲ",
      "低木", "高山", "風衝"), (0.0, 0.05, 0.95, 0.0)),
    (("湿原", "湿地", "ヨシ", "スゲ"), (0.0, 0.0, 0.9, 0.1)),
    (("自然裸地", "裸地", "崩壊", "岩", "砂丘", "礫"), (0.0, 0.0, 0.2, 0.8)),
    (("市街地", "住宅", "造成", "工場", "公園", "墓地", "人工", "道路", "ゴルフ", "緑の多い"), (0.0, 0.0, 0.3, 0.7)),
    (("水田", "畑", "牧草", "耕作", "果樹", "茶"), (0.1, 0.0, 0.8, 0.1)),
    (("ジイ", "シイ", "ガシ", "カシ", "タブ", "イスノキ", "アカメガシワ", "エノキ", "ムクノキ", "ミズキ",
      "ヤナギ", "コナラ", "クス", "アコウ", "ガジュマル", "広葉", "照葉", "竹林", "マツ"), (1.0, 0.0, 0.0, 0.0)),
]
# Нет правила по названию — по зоне (植生区分): зона бука (на Якусиме — горные
# хвойные леса) или зона камелии (вечнозелёный широколиственный лес).
ZONES = [("ブナクラス", (0.2, 0.8, 0.0, 0.0)), ("ヤブツバキクラス", (0.9, 0.0, 0.1, 0.0))]
# Лес по сообществу (02.4): густота (доля клеток 6 м с деревом), доля
# криптомерии, масштаб дерева от и до, частота валунов. Первое совпадение.
FOREST = [
    (("植林",), (0.9, 1.0, 0.45, 0.7, 0.02)),            # посадки криптомерии: густо, молодо
    (("ヤクスギ", "スギ群集"), (0.55, 0.7, 0.7, 1.3, 0.15)),  # естественный лес якусуги, мшистые валуны
    (("ツガ", "モミ"), (0.55, 0.8, 0.6, 1.1, 0.1)),
    (("シャクナゲ", "ビャクシン", "ヤクシマダケ", "ササ"), (0.03, 1.0, 0.3, 0.5, 0.25)),  # высокогорье
    (("岩壁", "裸地", "崩壊", "断崖", "珊瑚礁"), (0.0, 0.0, 0.5, 0.5, 0.4)),
    (("二次林", "アカメガシワ", "伐採", "ハドノキ", "カナクギノキ", "ミズキ"), (0.45, 0.0, 0.5, 0.85, 0.03)),
    (("ジイ", "シイ", "カシ", "ガシ", "タブ", "イスノキ", "アコウ", "ハマビワ", "サルスベリ"), (0.7, 0.03, 0.8, 1.2, 0.05)),
    (("マツ",), (0.4, 0.6, 0.5, 0.8, 0.02)),
    (("竹林",), (0.1, 0.0, 0.5, 0.8, 0.0)),
]
FOREST_DEFAULT = (0.01, 0.0, 0.5, 0.8, 0.01)


def forest_params(hanrei, shoku):
    for name in (hanrei, shoku):
        for words, f in FOREST:
            if any(word in name for word in words):
                return f
    return FOREST_DEFAULT


WATER = ("開放水域", "水域")


def classify(hanrei, shoku):
    """Сначала название сообщества, потом общая категория, потом зона."""
    for name in (hanrei, shoku):
        for words, w in RULES:
            if any(word in name for word in words):
                return np.array(w, np.float32)
    name = shoku
    for zone, w in ZONES:
        if zone in name:
            return np.array(w, np.float32)
    return None


def load(meta):
    con = duckdb.connect()
    con.execute("INSTALL spatial; LOAD spatial;")
    cols = [r[0] for r in con.execute(f"DESCRIBE SELECT * FROM '{SRC}'").fetchall()]
    geom = next(c for c in cols if c.lower() in ("geometry", "geom", "wkb_geometry"))
    # Колонка GEOMETRY (GeoParquet, WGS 84, долгота–широта) → WKB.
    return con.execute(f"SELECT hanrei_n, shoku_n, ST_AsWKB({geom}) FROM '{SRC}'").fetchall()


def main():
    d = Path(sys.argv[1] if len(sys.argv) > 1 else "assets/world/yakushima/far")
    meta = json.loads((d / "world.json").read_text())
    geo = meta["geo"]
    lat0, lon0 = geo["center_lat"], geo["center_lon"]
    cx, cz = geo["center_world_m"]
    m_lat = 111132.954 - 559.822 * math.cos(2 * math.radians(lat0)) + 1.175 * math.cos(4 * math.radians(lat0))
    m_lon = 111412.84 * math.cos(math.radians(lat0)) - 93.5 * math.cos(3 * math.radians(lat0))
    size = meta["cover"]["size"]
    map_m = meta["size_samples"][0] - 1
    map_m *= meta["sample_step_m"]
    step = map_m / (size - 1)

    def to_px(lon, lat):
        x = (lon - lon0) * m_lon + cx
        z = cz - (lat - lat0) * m_lat
        return x / step, z / step

    rows = load(meta)
    legend, index = [], {}
    cls = Image.new("L", (size, size), 0)
    dc = ImageDraw.Draw(cls)
    unknown = {}
    for hanrei, shoku, wkb in rows:
        name = f"{hanrei or ''} {shoku or ''}"
        if any(w in name for w in WATER):
            continue
        w = classify(hanrei or '', shoku or '')
        if w is None:
            unknown[name] = unknown.get(name, 0) + 1
            continue
        key = hanrei or shoku
        if key not in index:
            index[key] = len(legend) + 1
            legend.append({"id": index[key], "name": key, "belts": w.tolist(),
                           "forest": list(forest_params(hanrei or "", shoku or ""))})
        g = shapely.from_wkb(bytes(wkb))
        for poly in getattr(g, "geoms", [g]):
            if poly.geom_type != "Polygon":
                continue
            ext = [to_px(x, y) for x, y in poly.exterior.coords]
            dc.polygon(ext, fill=index[key])
            for hole in poly.interiors:
                dc.polygon([to_px(x, y) for x, y in hole.coords], fill=0)
    ids = np.asarray(cls)
    table = np.zeros((256, 4), np.float32)
    for e in legend:
        table[e["id"]] = e["belts"]
    veg = table[ids]
    has = ids > 0
    # Смягчение на 1–2 узла: граница полигона — не ступенька сетки.
    for c in range(4):
        veg[..., c] = ndimage.gaussian_filter(veg[..., c], 1.0)
    # Исходные пояса по WorldCover — отдельный файл: повторный запуск не
    # подмешивает уже пересчитанные пояса.
    cover = np.asarray(Image.open(SRC.parent / "cover_worldcover.png")).astype(np.float32) / 255.0
    land = cover.sum(-1) > 0.05
    near = ndimage.gaussian_filter(has.astype(np.float32), 1.0)
    w = veg / np.maximum(near[..., None], 1e-3)
    w = np.where(near[..., None] > 0.05, w * 0.85 + cover * 0.15, cover)
    s = w.sum(-1, keepdims=True)
    w = np.where(land[..., None], w / np.maximum(s, 1e-6), 0.0)
    sys.path.insert(0, str(Path(__file__).parent))
    import cover as cover_mod  # noqa: E402

    cover_mod.write(d / "cover.png", w)
    Image.fromarray(ids.astype(np.uint8), "L").save(d / "vegetation.png", optimize=True)
    (d / "vegetation.json").write_text(json.dumps({
        "source": "1/25,000植生図（環境省生物多様性センター）, processed by Pacific Spatial Solutions inc., CC BY 4.0",
        "size": size,
        "legend": legend,
    }, ensure_ascii=False, indent=1) + "\n")
    cover_mod.report(w, land, step * step)
    covered = (has & land).sum() / max(land.sum(), 1)
    print(f"сообществ {len(legend)}, полигонов {len(rows)}, карта покрывает {covered:.0%} суши")
    for name, n in sorted(unknown.items(), key=lambda t: -t[1])[:15]:
        print(f"  без правила: {name} ({n})")


if __name__ == "__main__":
    main()
