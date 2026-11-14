"""Цифровая модель рельефа GSI (基盤地図情報 数値標高モデル) в формате JPGIS GML.

Архив с сайта GSI содержит вложенные zip: `FG-GML-<квадрат>-DEM10B-*.zip`
(один XML на квадрат 2-го уровня, шаг 0,4″ ≈ 10 м), `DEM5A` и `DEM1A` (по
XML на квадрат 3-го уровня, шаг 0,2″ ≈ 5 м и 0,04″ ≈ 1 м). Высоты — грунт
(地表面), без крон; −9999 — нет данных (море, пропуски съёмки).

Разбор: охват `gml:Envelope` (широта, долгота угла), размер `gml:high`, начало
`gml:startPoint`, порядок `+x-y` (строки с севера на юг, внутри — с запада на
восток); значения после запятой в `gml:tupleList`.

出典：国土地理院 基盤地図情報（数値標高モデル）を加工して作成.
"""

import io
import re
import zipfile

import numpy as np

NODATA = -9999.0
_ENV = re.compile(
    rb"<gml:lowerCorner>([\d.]+) ([\d.]+)</gml:lowerCorner>\s*"
    rb"<gml:upperCorner>([\d.]+) ([\d.]+)</gml:upperCorner>"
)
_HIGH = re.compile(rb"<gml:high>(\d+) (\d+)</gml:high>")
_START = re.compile(rb"<gml:startPoint>(\d+) (\d+)</gml:startPoint>")
_TUPLES = re.compile(rb"<gml:tupleList>(.*?)</gml:tupleList>", re.S)
_VALUE = re.compile(rb",(-?\d+(?:\.\d+)?)")


def parse_gml(data):
    """Один XML → (lat_south, lon_west, lat_north, lon_east, массив высот [строка с севера])."""
    lat0, lon0, lat1, lon1 = (float(v) for v in _ENV.search(data).groups())
    cols, rows = (int(v) + 1 for v in _HIGH.search(data).groups())
    sx, sy = (int(v) for v in _START.search(data).groups())
    values = np.array(_VALUE.findall(_TUPLES.search(data).group(1)), dtype=np.float32)
    grid = np.full(rows * cols, NODATA, dtype=np.float32)
    start = sy * cols + sx
    grid[start : start + len(values)] = values[: rows * cols - start]
    return lat0, lon0, lat1, lon1, grid.reshape(rows, cols)


def iter_tiles(archive, kind):
    """Все XML вида `kind` (`DEM10B`, `DEM5A`, `DEM1A`) из архива GSI."""
    with zipfile.ZipFile(archive) as outer:
        for name in sorted(outer.namelist()):
            if f"-{kind}-" not in name or not name.endswith(".zip"):
                continue
            with zipfile.ZipFile(io.BytesIO(outer.read(name))) as inner:
                for xml in sorted(inner.namelist()):
                    if xml.endswith(".xml"):
                        yield parse_gml(inner.read(xml))


def mosaic(archives, kind, bbox, step_arcsec):
    """Мозаика вида `kind` на сетке широта × долгота с шагом `step_arcsec`.

    `bbox` = (lat_south, lon_west, lat_north, lon_east). Строка 0 — север.
    Тайл кладётся по своему охвату; клетки без данных — NaN.
    """
    s, w, n, e = bbox
    d = step_arcsec / 3600.0
    rows = int(round((n - s) / d))
    cols = int(round((e - w) / d))
    out = np.full((rows, cols), np.nan, dtype=np.float32)
    count = 0
    for archive in archives:
        for lat0, lon0, lat1, lon1, g in iter_tiles(archive, kind):
            r0 = int(round((n - lat1) / d))
            c0 = int(round((lon0 - w) / d))
            r1, c1 = r0 + g.shape[0], c0 + g.shape[1]
            # Обрезка по рамке мозаики.
            rr0, cc0 = max(r0, 0), max(c0, 0)
            rr1, cc1 = min(r1, rows), min(c1, cols)
            if rr0 >= rr1 or cc0 >= cc1:
                continue
            part = g[rr0 - r0 : rr1 - r0, cc0 - c0 : cc1 - c0]
            dst = out[rr0:rr1, cc0:cc1]
            valid = part > NODATA + 1
            dst[valid] = part[valid]
            count += 1
    return out, count
