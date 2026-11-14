"""Векторы OpenStreetMap для мира: реки, тропы, дороги, водопады, вершины,
приюты, криптомерии, болота — в мировых метрах (02.7, шаги 3 и 5).

Данные OSM — © участники OpenStreetMap, ODbL. Производный `features.json`
распространяется под ODbL; растровые слои — «произведённая работа» с указанием
авторства.
"""

import json

import numpy as np

TRAIL = {"path", "footway", "steps", "track", "bridleway"}
ROAD = {
    "motorway", "trunk", "primary", "secondary", "tertiary", "unclassified",
    "residential", "service", "living_street", "road",
}  # fmt: skip
WATERWAY = {"river", "stream"}


class Features:
    """Объекты карты в мировых координатах (x — восток, z — юг)."""

    def __init__(self):
        self.rivers = []  # (имя, вид, [(x, z)])
        self.trails = []  # (имя, вид highway, [(x, z)])
        self.roads = []
        self.wetlands = []  # (имя, [(x, z)]) — контур
        self.points = []  # (вид, имя, x, z)


def load(path, geo_to_world):
    data = json.loads(path.read_text())
    f = Features()
    for e in data["elements"]:
        t = e.get("tags", {})
        name = t.get("name", "")
        if e["type"] == "way" and "geometry" in e:
            pts = [geo_to_world(g["lat"], g["lon"]) for g in e["geometry"]]
            if t.get("waterway") in WATERWAY:
                f.rivers.append((name, t["waterway"], pts))
            elif t.get("highway") in TRAIL:
                f.trails.append((name, t["highway"], pts))
            elif t.get("highway") in ROAD:
                f.roads.append((name, t["highway"], pts))
            elif t.get("natural") == "wetland":
                f.wetlands.append((name, pts))
            elif t.get("tourism") in ("alpine_hut", "wilderness_hut", "camp_site"):
                xs, zs = zip(*pts)
                f.points.append((t["tourism"], name, sum(xs) / len(xs), sum(zs) / len(zs)))
        elif e["type"] == "node":
            x, z = geo_to_world(e["lat"], e["lon"])
            kind = (
                "waterfall"
                if t.get("waterway") == "waterfall" or t.get("natural") == "waterfall"
                else t.get("natural")
                if t.get("natural") in ("peak", "tree")
                else t.get("tourism") or t.get("amenity")
            )
            if kind:
                f.points.append((kind, name, x, z))
    return f


def segments(lines):
    """Отрезки всех ломаных: массив (N, 4) — x0, z0, x1, z1."""
    segs = []
    for pts in lines:
        p = np.asarray(pts, dtype=np.float64)
        if len(p) >= 2:
            segs.append(np.concatenate([p[:-1], p[1:]], axis=1))
    return np.concatenate(segs) if segs else np.zeros((0, 4))


def sample_lines(lines, step_m):
    """Точки вдоль ломаных через `step_m` — для сверки с расчётом стока."""
    out = []
    for pts in lines:
        p = np.asarray(pts, dtype=np.float64)
        for a, b in zip(p[:-1], p[1:]):
            n = max(1, int(np.ceil(np.linalg.norm(b - a) / step_m)))
            t = np.arange(n)[:, None] / n
            out.append(a + (b - a) * t)
    return np.concatenate(out) if out else np.zeros((0, 2))


def to_json(f, river_widths):
    """Компактный `features.json`: координаты с точностью 0,1 м."""

    def line(pts):
        return [[round(x, 1), round(z, 1)] for x, z in pts]

    return {
        "license": "© участники OpenStreetMap, ODbL 1.0; расчёт стока — по Copernicus DEM",
        "rivers": [
            {"name": n, "kind": k, "width_m": round(w, 1), "line": line(p)}
            for (n, k, p), w in zip(f.rivers, river_widths)
        ],
        "trails": [{"name": n, "kind": k, "line": line(p)} for n, k, p in f.trails],
        "roads": [{"name": n, "kind": k, "line": line(p)} for n, k, p in f.roads],
        "wetlands": [{"name": n, "outline": line(p)} for n, p in f.wetlands],
        "points": [
            {"kind": k, "name": n, "x": round(x, 1), "z": round(z, 1)} for k, n, x, z in f.points
        ],
    }
