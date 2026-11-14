"""Импорт реального рельефа Якусимы в слои мира движка (подэтап 02.7).

Запуск из корня проекта:

    python3 -m venv .venv-geo
    .venv-geo/bin/pip install -r tools/geo/requirements.txt
    .venv-geo/bin/python tools/geo/yakushima.py fetch
    .venv-geo/bin/python tools/geo/yakushima.py far

`fetch` скачивает исходные данные в `assets/source/geo/cache/` (вне git) и
сверяет SHA-256 из `assets/source/geo/sources.json`. `far` строит дальний
рельеф всего острова: сетка 1025 × 1025 с шагом 30 м (30,72 × 30,72 км) в
`assets/world/yakushima/far/`. Повторный запуск даёт побайтно те же файлы.

Проекция — касательная плоскость в центре острова: x — восток, z — юг,
метры по эллипсоиду WGS84; ошибка на 15 км от центра меньше 0,1 %.
"""

import hashlib
import json
import math
import sys
import urllib.request
from pathlib import Path

import numpy as np
import tifffile
from PIL import Image
from scipy import ndimage

ROOT = Path(__file__).resolve().parents[2]
SOURCES = ROOT / "assets/source/geo/sources.json"
CACHE = ROOT / "assets/source/geo/cache"
OUT_FAR = ROOT / "assets/world/yakushima/far"

# Центр локальной проекции: середина острова.
CENTER_LAT = 30.335
CENTER_LON = 130.525
# Сетка мира: 16 × 16 тайлов по 64 шага (формат слоёв движка, TILE_SAMPLES = 65).
TILES = 16
NODES = TILES * 64 + 1
STEP_M = 30.0
HALF_M = TILES * 64 * STEP_M / 2.0
SEA_LEVEL = 0.0
COAST_RANGE_M = (-4000.0, 4000.0)
TEMPERATURE_RANGE_C = (-10.0, 40.0)
WORLD_SEED = 0x5A4B  # для процедурных частей движка (детали, шум материалов)
GENERATOR_VERSION = 4
FORMAT_VERSION = 3

# Места маршрута — приблизительные координаты (уточняются по OSM на шаге 5).
MARKERS = [
    ("Сиратани Унсуикё, начало тропы", "TrailStart", 30.3790, 130.5500),
    ("Дзёмон-суги", "Viewpoint", 30.3586, 130.5317),
    ("гора Миянoура, 1936 м", "Viewpoint", 30.3361, 130.5047),
    ("устье реки Анбо", "RiverMouth", 30.3086, 130.6575),
    ("пляж Нагата Инакахама", "ShoreEnd", 30.4156, 130.4353),
]


def meters_per_degree(lat):
    """Длина градуса широты и долготы на эллипсоиде WGS84, м."""
    p = math.radians(lat)
    m_lat = 111132.954 - 559.822 * math.cos(2 * p) + 1.175 * math.cos(4 * p)
    m_lon = 111412.84 * math.cos(p) - 93.5 * math.cos(3 * p)
    return m_lat, m_lon


M_LAT, M_LON = meters_per_degree(CENTER_LAT)


def world_to_geo(x, z):
    """Мировые метры (угол карты — (0, 0)) → широта, долгота."""
    east = x - HALF_M
    north = HALF_M - z
    return CENTER_LAT + north / M_LAT, CENTER_LON + east / M_LON


def geo_to_world(lat, lon):
    return (lon - CENTER_LON) * M_LON + HALF_M, HALF_M - (lat - CENTER_LAT) * M_LAT


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch():
    sources = json.loads(SOURCES.read_text())
    CACHE.mkdir(parents=True, exist_ok=True)
    for s in sources["files"]:
        path = CACHE / s["file"]
        if not path.exists():
            print(f"скачиваю {s['url']}")
            urllib.request.urlretrieve(s["url"], path)
        digest = sha256(path)
        if digest != s["sha256"]:
            sys.exit(f"{path.name}: SHA-256 {digest}, ожидалось {s['sha256']}")
        print(f"{path.name}: ok")


def load_copernicus():
    """Copernicus GLO-30 N30E130: 3600 × 3600, верх 31° с. ш., лево 130° в. д., шаг 1″."""
    with tifffile.TiffFile(CACHE / "Copernicus_DSM_COG_10_N30_00_E130_00_DEM.tif") as t:
        return t.pages[0].asarray().astype(np.float64)


def sample_dem(dem, lat, lon):
    """Билинейная выборка в центрах пикселей."""
    r = (31.0 - lat) * 3600.0 - 0.5
    c = (lon - 130.0) * 3600.0 - 0.5
    r0 = np.clip(np.floor(r).astype(int), 0, dem.shape[0] - 2)
    c0 = np.clip(np.floor(c).astype(int), 0, dem.shape[1] - 2)
    fr = np.clip(r - r0, 0, 1)
    fc = np.clip(c - c0, 0, 1)
    a = dem[r0, c0]
    b = dem[r0, c0 + 1]
    cc = dem[r0 + 1, c0]
    d = dem[r0 + 1, c0 + 1]
    return (a * (1 - fc) + b * fc) * (1 - fr) + (cc * (1 - fc) + d * fc) * fr


def smoothstep(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3 - 2 * t)


def quantize(v, lo, hi, maxv):
    return np.clip(np.round((v - lo) / (hi - lo) * maxv), 0, maxv)


def write_png16(path, a):
    Image.fromarray(a.astype(np.uint16)).save(path, optimize=False)


def write_png8(path, a):
    Image.fromarray(a.astype(np.uint8)).save(path, optimize=False)


def build_far():
    dem = load_copernicus()
    idx = np.arange(NODES) * STEP_M
    xs, zs = np.meshgrid(idx, idx)  # строка — z, столбец — x
    lat, lon = world_to_geo(xs, zs)
    h = sample_dem(dem, lat, lon)

    # Суша — связная область, содержащая центр (отсекаем шум и соседние острова).
    land = h > 0.5
    labels, _ = ndimage.label(land)
    center = labels[NODES // 2, NODES // 2]
    land = labels == center
    land = ndimage.binary_fill_holes(land)

    # Знаковое расстояние до кромки, м: > 0 на суше.
    d_in = ndimage.distance_transform_edt(land) * STEP_M
    d_out = ndimage.distance_transform_edt(~land) * STEP_M
    coast_d = np.where(land, d_in - STEP_M / 2, -(d_out - STEP_M / 2))

    # Дно моря: шельф и склон до −250 м (временно, до батиметрии GEBCO).
    sea = np.clip(-coast_d, 0.0, None)
    seabed = -(2.0 + 0.01 * sea + 160.0 * smoothstep(0.0, 2500.0, sea))
    seabed = np.maximum(seabed, -250.0)
    height = np.where(land, np.maximum(h, 0.3), seabed)

    # Уклон, тип берега: крутой берег — камень. Сглаживаем на ~150 м.
    gz, gx = np.gradient(height, STEP_M)
    slope = np.hypot(gx, gz)
    rocky = smoothstep(0.18, 0.45, ndimage.uniform_filter(slope, size=5))
    rocky = np.where(land, rocky, ndimage.grey_dilation(rocky, size=7))

    # Климат по высоте: осадки растут в горах, температура −0,6 °C на 100 м.
    hp = np.maximum(height, 0.0)
    moisture = np.clip(0.55 + 0.4 * smoothstep(0.0, 1500.0, hp), 0, 1)
    temperature = 20.0 - 0.006 * hp
    coast = 1.0 - smoothstep(0.0, 45.0, np.maximum(coast_d, 0.0))

    lo = math.floor(height.min() / 10.0) * 10.0 - 10.0
    hi = math.ceil(height.max() / 10.0) * 10.0 + 10.0
    OUT_FAR.mkdir(parents=True, exist_ok=True)
    write_png16(OUT_FAR / "height.png", quantize(height, lo, hi, 65535))
    write_png16(OUT_FAR / "coast_distance.png", quantize(coast_d, *COAST_RANGE_M, 65535))
    zero = np.zeros_like(height)
    masks = np.stack([zero, zero, zero], -1)  # тропа, русло, торфяник — шаги 3 и 5
    write_png8(OUT_FAR / "masks.png", masks)
    climate = np.stack(
        [
            quantize(moisture, 0, 1, 255),
            quantize(temperature, *TEMPERATURE_RANGE_C, 255),
            quantize(coast, 0, 1, 255),
        ],
        -1,
    )
    write_png8(OUT_FAR / "climate.png", climate)
    write_png8(OUT_FAR / "coast_type.png", quantize(rocky, 0, 1, 255))

    markers = []
    for name, kind, mlat, mlon in MARKERS:
        x, z = geo_to_world(mlat, mlon)
        markers.append({"name": name, "kind": kind, "x": round(x, 2), "z": round(z, 2)})
    sources = json.loads(SOURCES.read_text())
    manifest = {
        "format": FORMAT_VERSION,
        "seed": WORLD_SEED,
        "generator_version": GENERATOR_VERSION,
        "tiles_x": TILES,
        "tiles_z": TILES,
        "sample_step_m": STEP_M,
        "size_samples": [NODES, NODES],
        "height_range_m": [lo, hi],
        "temperature_range_c": list(TEMPERATURE_RANGE_C),
        "substrate_codes": ["Soil", "Peat", "Sand", "Rock", "Gravel"],
        "overrides": [],
        "exported_checksum": "",
        "markers": markers,
        "coast_distance_range_m": list(COAST_RANGE_M),
        "geo": {
            "center_lat": CENTER_LAT,
            "center_lon": CENTER_LON,
            "center_world_m": [HALF_M, HALF_M],
            "projection": "local tangent plane WGS84: x — восток, z — юг, метры",
            "sources": [s["attribution"] for s in sources["files"]],
        },
    }
    (OUT_FAR / "world.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")

    # Отчёт о данных.
    cell = STEP_M * STEP_M
    ys, xs_ = np.where(land)
    print(f"суша {land.sum() * cell / 1e6:.1f} км², высшая точка {height.max():.0f} м")
    print(
        f"размер суши {(xs_.max() - xs_.min()) * STEP_M / 1000:.1f} × "
        f"{(ys.max() - ys.min()) * STEP_M / 1000:.1f} км"
    )
    deg = np.degrees(np.arctan(slope[land]))
    print(
        f"уклон суши: медиана {np.median(deg):.0f}°, 90 % {np.percentile(deg, 90):.0f}°, "
        f"максимум {deg.max():.0f}°; круче 40° — {100 * (deg > 40).mean():.1f} %"
    )
    print(f"высоты слоя {lo:.0f}…{hi:.0f} м, дно до {height.min():.0f} м")
    for m in markers:
        print(f"маркер {m['name']}: ({m['x']:.0f}, {m['z']:.0f}) м")


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "fetch":
        fetch()
    elif cmd == "far":
        build_far()
    else:
        sys.exit("использование: yakushima.py fetch | far")
