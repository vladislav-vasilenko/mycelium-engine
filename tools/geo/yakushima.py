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
import urllib.parse
import urllib.request
from pathlib import Path

import numpy as np
import tifffile
from PIL import Image
from scipy import ndimage

import cover
import zone
import gsi
import hydrology
import osm

ROOT = Path(__file__).resolve().parents[2]
SOURCES = ROOT / "assets/source/geo/sources.json"
CACHE = ROOT / "assets/source/geo/cache"
OUT_FAR = ROOT / "assets/world/yakushima/far"
# Архив GSI скачивается вручную в браузере (сервер отвечает 403 скрипту).
GSI_DIR = ROOT / "assets/source/geo/gsi/raw"
GSI_BBOX = (30.20, 130.36, 30.48, 130.69)

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

# Поле высот полного разрешения для кластерного рельефа (02.3): 4097², 7,5 м.
FINE_NODES = 4097
FINE_STEP_M = (NODES - 1) * STEP_M / (FINE_NODES - 1)
# Пляж Нагата Инакахама: в OSM нет точки — приблизительные координаты.
NAGATA_BEACH = (30.4156, 130.4353)
# Русло начинается с площади водосбора 0,25 км².
CHANNEL_MIN_KM2 = 0.25
OSM_FILE = "osm_yakushima.json"


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
        path = (ROOT / s["dir"] if "dir" in s else CACHE) / s["file"]
        if s.get("manual") and not path.exists():
            print(f"{s['file']}: скачайте вручную ({s['url']}) в {path.parent}")
            continue
        if not path.exists():
            print(f"скачиваю {s['url']}")
            if "query" in s:
                # Overpass: запрос из файла, общий User-Agent без личных данных.
                body = urllib.parse.urlencode({"data": (ROOT / s["query"]).read_text()})
                req = urllib.request.Request(
                    s["url"], body.encode(), {"User-Agent": "mycelium-engine-geo/0.1"}
                )
                with urllib.request.urlopen(req, timeout=300) as r:
                    path.write_bytes(r.read())
            else:
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


def load_gsi():
    """Мозаика GSI DEM5A (шаг 0,2″ ≈ 5 м) по архивам в `GSI_DIR`; кэш — .npy."""
    archives = sorted(GSI_DIR.glob("*.zip"))
    if not archives:
        return None
    key = hashlib.sha256("".join(f"{a.name}:{a.stat().st_size}" for a in archives).encode())
    cache = CACHE / f"gsi_DEM5A_{key.hexdigest()[:12]}.npy"
    if cache.exists():
        return np.load(cache)
    m, n = gsi.mosaic(archives, "DEM5A", GSI_BBOX, 0.2)
    print(f"GSI DEM5A: тайлов {n}, мозаика {m.shape[1]} × {m.shape[0]}")
    CACHE.mkdir(parents=True, exist_ok=True)
    np.save(cache, m)
    return m


def sample_gsi(m, lat, lon, step_m):
    """Высота GSI в узлах мира: среднее по окну ≈ шагу мира (без наложения
    частот), затем билинейно; NaN там, где данных нет (море)."""
    d = 0.2 / 3600.0
    k = max(1, int(round(step_m / 5.0)))
    valid = np.isfinite(m)
    s = ndimage.uniform_filter(np.where(valid, m, 0.0).astype(np.float64), k)
    c = ndimage.uniform_filter(valid.astype(np.float64), k)
    avg = np.where(c > 0.5, s / np.maximum(c, 1e-9), np.nan)
    south, west, north, _ = GSI_BBOX
    r = (north - lat) / d - 0.5
    col = (lon - west) / d - 0.5
    inside = (r >= 0) & (col >= 0) & (r < m.shape[0] - 1) & (col < m.shape[1] - 1)
    r0 = np.clip(np.floor(r).astype(int), 0, m.shape[0] - 2)
    c0 = np.clip(np.floor(col).astype(int), 0, m.shape[1] - 2)
    fr, fc = r - r0, col - c0
    v = (avg[r0, c0] * (1 - fc) + avg[r0, c0 + 1] * fc) * (1 - fr) + (
        avg[r0 + 1, c0] * (1 - fc) + avg[r0 + 1, c0 + 1] * fc
    ) * fr
    return np.where(inside, v, np.nan)


def compare_dem(cop, g, ok):
    """Отчёт: Copernicus (поверхность с кронами) минус GSI (грунт) по поясам."""
    diff = cop - g
    for lo, hi, name in [(5, 600, "0–600 м"), (600, 1600, "600–1600 м"), (1600, 2000, "выше 1600 м")]:
        sel = ok & (g >= lo) & (g < hi)
        if sel.any():
            print(
                f"Copernicus − GSI, {name}: медиана {np.median(diff[sel]):+.1f} м, "
                f"90 % {np.percentile(diff[sel], 90):+.1f} м"
            )


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
    mosaic = load_gsi()
    if mosaic is not None:
        # Грунт GSI вместо поверхности Copernicus; за охватом GSI — Copernicus.
        g = sample_gsi(mosaic, lat, lon, STEP_M)
        ok = np.isfinite(g)
        compare_dem(h, g, ok)
        land = ok & (g > 0.0)
        h = np.where(ok, g, h)
        print("рельеф: GSI DEM5A (грунт), за охватом — Copernicus")
    else:
        land = h > 0.5
        print("рельеф: Copernicus (поверхность с кронами); архива GSI нет")

    # Суша — связная область, содержащая центр (отсекаем шум и соседние острова).
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

    feats = osm.load(CACHE / OSM_FILE, geo_to_world)
    acc, _channels, river_widths = build_hydrology(height, land, feats)
    OUT_FAR.mkdir(parents=True, exist_ok=True)
    write_features(feats, river_widths)
    flow = np.clip(np.round(np.log2(np.maximum(acc, 1.0)) * 2048.0), 0, 65535)
    write_png16(OUT_FAR / "flow.png", flow)

    lo = math.floor(height.min() / 10.0) * 10.0 - 10.0
    hi = math.ceil(height.max() / 10.0) * 10.0 + 10.0
    write_png16(OUT_FAR / "height.png", quantize(height, lo, hi, 65535))
    zone_meta = None
    if mosaic is not None:
        write_fine(mosaic, height, land, lo, hi)
        zone_meta = write_zone(mosaic)
    write_png16(OUT_FAR / "coast_distance.png", quantize(coast_d, *COAST_RANGE_M, 65535))
    zero = np.zeros_like(height)
    # Тропы и реки уже текселя сетки рисуются попиксельно по features.png;
    # здесь — только русла по стоку (для 03.2) и болота.
    peat = wetland_mask(feats, NODES)
    # Русло в masks.png — признак «мокро» для ходьбы на сетке 30 м: его не
    # ставим, реки идут через features.png; русла по стоку — в flow.png.
    masks = np.stack([zero, zero, peat * 255.0], -1)
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

    markers = osm_markers(feats, height, coast_d)
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
        "features": {"file": "features.json"},
        "height_fine": {"file": "height_fine.png", "size": FINE_NODES, "step_m": FINE_STEP_M},
        **({"height_zone": zone_meta} if zone_meta else {}),
        "cover": {
            "file": "cover.png",
            "size": COVER_NODES,
            "channels": "R низинный лес, G кедровый лес якусуги, B якудзаса и луга, A гранит и застройка",
        },
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
        h = bilinear(height, m["x"], m["z"])
        print(f"маркер {m['name']}: ({m['x']:.0f}, {m['z']:.0f}) м, высота {h:.0f} м")


def bilinear(grid, x, z):
    fx, fz = x / STEP_M, z / STEP_M
    i = int(np.clip(np.floor(fx), 0, NODES - 2))
    j = int(np.clip(np.floor(fz), 0, NODES - 2))
    tx, tz = fx - i, fz - j
    a = grid[j, i] * (1 - tx) + grid[j, i + 1] * tx
    b = grid[j + 1, i] * (1 - tx) + grid[j + 1, i + 1] * tx
    return a * (1 - tz) + b * tz


def build_hydrology(height, land, feats):
    """Сток по рельефу и сверка с реками OSM. Русла — по рельефу с врезанными
    руслами OSM (stream burning); сверка — без врезки, честно по рельефу."""
    min_cells = CHANNEL_MIN_KM2 * 1e6 / (STEP_M * STEP_M)
    named = [p for _, k, p in feats.rivers if k == "river"]
    pts = osm.sample_lines(named, 10.0)
    ii = np.clip(np.round(pts[:, 0] / STEP_M).astype(int), 0, NODES - 1)
    jj = np.clip(np.round(pts[:, 1] / STEP_M).astype(int), 0, NODES - 1)
    on_land = land[jj, ii]

    def match(acc):
        ch = land & (acc >= min_cells)
        d = ndimage.distance_transform_edt(~ch) * STEP_M
        return (d[jj, ii][on_land] <= 60.0).mean(), ch

    depth0, _, acc0 = hydrology.flow(height, land, STEP_M)
    m0, _ = match(acc0)
    burn = np.zeros_like(land)
    wpts = osm.sample_lines([p for _, _, p in feats.rivers], STEP_M / 3)
    bi = np.clip(np.round(wpts[:, 0] / STEP_M).astype(int), 0, NODES - 1)
    bj = np.clip(np.round(wpts[:, 1] / STEP_M).astype(int), 0, NODES - 1)
    burn[bj, bi] = True
    _, _, acc = hydrology.flow(height, land & True, STEP_M, burn=burn & land)
    m1, ch = match(acc)
    pits = land & (depth0 > 1.0)
    lab, n_pits = ndimage.label(pits)
    print(
        f"сток: русел {ch.sum()} клеток (водосбор ≥ {CHANNEL_MIN_KM2} км²); реки OSM в 60 м "
        f"от русел: {100 * m0:.0f} % по рельефу, {100 * m1:.0f} % с врезкой OSM; "
        f"впадин глубже 1 м: {n_pits}"
    )
    # Ширина каждой реки OSM — по наибольшему водосбору у её узлов.
    acc_max = ndimage.maximum_filter(acc, size=3)
    widths = []
    for _, kind, p in feats.rivers:
        q = np.asarray(p)
        i = np.clip(np.round(q[:, 0] / STEP_M).astype(int), 0, NODES - 1)
        j = np.clip(np.round(q[:, 1] / STEP_M).astype(int), 0, NODES - 1)
        area = acc_max[j, i].max() * STEP_M * STEP_M
        w = float(hydrology.river_width_m(area))
        widths.append(w if kind == "river" else min(w, 4.0))
    return acc, ch.astype(np.float64), widths


def write_features(feats, river_widths):
    """features.json — векторы троп, рек и дорог; движок рисует их по точному
    расстоянию до отрезков (поле расстояний в текстуре ломается на самой линии)."""
    (OUT_FAR / "features.json").write_text(
        json.dumps(osm.to_json(feats, river_widths), ensure_ascii=False, separators=(",", ":"))
        + "\n"
    )

    def km(lines):
        return sum(np.linalg.norm(np.diff(np.asarray(p), axis=0), axis=1).sum() for p in lines) / 1000

    print(
        f"OSM: рек {len(feats.rivers)} ({km([p for _, _, p in feats.rivers]):.0f} км), "
        f"троп {len(feats.trails)} ({km([p for _, _, p in feats.trails]):.0f} км), "
        f"дорог {len(feats.roads)} ({km([p for _, _, p in feats.roads]):.0f} км), "
        f"точек {len(feats.points)}"
    )


def wetland_mask(feats, n):
    """Болота OSM (Ханоэго) → маска торфяника на сетке мира."""
    from PIL import ImageDraw

    img = Image.new("L", (n, n), 0)
    draw = ImageDraw.Draw(img)
    for _, outline in feats.wetlands:
        draw.polygon([(x / STEP_M, z / STEP_M) for x, z in outline], fill=255)
    return np.asarray(img, dtype=np.float64) / 255.0


def write_fine(mosaic, coarse, coarse_land, lo, hi):
    """height_fine.png: грунт GSI с шагом 7,5 м; море и всё вне суши острова —
    дно грубой сетки (билинейно), не выше −0,3 м, чтобы у кромки не было
    ложных островков из сглаженной суши."""
    idx = np.arange(FINE_NODES) * FINE_STEP_M
    xs, zs = np.meshgrid(idx, idx)
    lat, lon = world_to_geo(xs, zs)
    g = sample_gsi(mosaic, lat, lon, FINE_STEP_M)
    gx, gz = xs / STEP_M, zs / STEP_M
    base = ndimage.map_coordinates(coarse, [gz, gx], order=1, mode="nearest")
    island = ndimage.map_coordinates(coarse_land.astype(np.float32), [gz, gx], order=0) > 0.5
    land = np.isfinite(g) & (g > 0.0) & ndimage.binary_dilation(island, iterations=4)
    fine = np.where(land, g, np.minimum(base, -0.3))
    write_png16(OUT_FAR / "height_fine.png", quantize(fine, lo, hi, 65535))
    write_cover(fine, land)
    print(
        f"поле 7,5 м: {FINE_NODES}², суша {land.sum() * FINE_STEP_M**2 / 1e6:.1f} км², "
        f"высшая точка {fine.max():.0f} м"
    )


# Пояса растительности (02.7, шаг 4): 2049² × 15 м — каждый второй узел поля 7,5 м.
COVER_NODES = 2049
WORLDCOVER = "ESA_WorldCover_10m_2021_v200_N30E129_Map.tif"


def write_zone(mosaic):
    """height_zone.png: игровая зона ~0,94 м по DEM1A (ущелье Сиратани)."""
    archives = sorted(GSI_DIR.glob("*.zip"))

    def fallback(lat, lon):
        return sample_gsi(mosaic, lat, lon, 5.0)

    h, meta = zone.build(archives, world_to_geo, FINE_STEP_M, fallback)
    lo, hi = meta["height_range_m"]
    write_png16(OUT_FAR / meta["file"], quantize(h, lo, hi, 65535))
    return meta


def write_cover(fine, fine_land):
    stride = (FINE_NODES - 1) // (COVER_NODES - 1)
    h = fine[::stride, ::stride]
    land = fine_land[::stride, ::stride]
    step = FINE_STEP_M * stride
    idx = np.arange(COVER_NODES) * step
    xs, zs = np.meshgrid(idx, idx)
    lat, lon = world_to_geo(xs, zs)
    frac, origin = cover.load_fractions(CACHE / WORLDCOVER, GSI_BBOX)
    w = cover.belts(frac, origin, lat, lon, h, land)
    cover.write(OUT_FAR / "cover.png", w)
    cover.report(w, land, step * step)


def snap_to_shore(x, z, coast_d, lo=8.0, hi=40.0):
    """Ближайший узел суши в полосе `lo…hi` м от кромки."""
    js, is_ = np.nonzero((coast_d >= lo) & (coast_d <= hi))
    k = np.argmin((is_ * STEP_M - x) ** 2 + (js * STEP_M - z) ** 2)
    return is_[k] * STEP_M, js[k] * STEP_M


def osm_markers(feats, height, coast_d):
    """Места маршрута по OSM; пляж Нагата — приблизительно (нет точки в OSM).
    Устье и пляж переносятся на ближайший берег: точки OSM лежат на кромке."""

    def point(kind, name):
        return next((x, z) for k, n, x, z in feats.points if k == kind and n == name)

    def line_end(lines, name, highest):
        ends = [pt for n, _, p in lines if n == name for pt in (p[0], p[-1])]
        key = lambda e: bilinear(height, *e)  # noqa: E731
        return max(ends, key=key) if highest else min(ends, key=key)

    hanaego = next(p for n, p in feats.wetlands if n == "花之江河")
    places = [
        ("Сиратани Унсуикё, начало тропы", "TrailStart",
         line_end(feats.roads, "白谷雲水峡宮之浦線", True)),
        ("Дзёмон-суги", "Viewpoint", point("tree", "縄文杉")),
        ("гора Миянoура", "Viewpoint", point("peak", "宮之浦岳")),
        ("болото Ханоэго", "Viewpoint", tuple(np.mean(np.asarray(hanaego), axis=0))),
        ("устье реки Анбо", "RiverMouth",
         snap_to_shore(*line_end(feats.rivers, "安房川", False), coast_d)),
        ("пляж Нагата Инакахама (приблизительно)", "ShoreEnd",
         snap_to_shore(*geo_to_world(*NAGATA_BEACH), coast_d)),
    ]  # fmt: skip
    return [
        {"name": n, "kind": k, "x": round(float(x), 2), "z": round(float(z), 2)}
        for n, k, (x, z) in places
    ]


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "fetch":
        fetch()
    elif cmd == "far":
        build_far()
    else:
        sys.exit("использование: yakushima.py fetch | far")
