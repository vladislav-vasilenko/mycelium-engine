#include "common.h"
#include "features.h"
#include "sky.h"
#include "terrain_patch.h"

// Лес (02.4 + 02.8). Расстановка и отсечение — на GPU каждый кадр: сетка
// клеток FOREST_CELL_M вокруг камеры, в клетке — кандидат на детерминированном
// смещении (хэш номера клетки мира), отбор по поясам, уклону, тропам, рекам,
// морю. Видимые экземпляры идут в списки (вид × LOD), рядом с камерой — ещё и в
// списки теней; рисование — по indirect-аргументам.

constant float FOREST_CELL_M = 6.0;
constant int FOREST_GRID = 160;               // клеток по стороне: радиус ~480 м
// Виды на GPU (`plants::SPECIES`): 0–2 криптомерии (две обычные, древняя),
// 3–5 лиственные, 6 валун, 7 папоротник, 8 якудзаса, 9 кустарник,
// 10 упавший ствол, 11 полый пень, 12 вывороченное дерево, 13 лесной сор,
// 14 подушка мха, 15 корни и камни на тропе; 16 — гриб, только из списка
// предметов с CPU.
constant uint GPU_SPECIES = 16;
constant uint TREE_MESHES = 6;
constant uint CAMERA_LISTS = GPU_SPECIES * 3;                 // вид × LOD
constant uint PROPS_LIST = CAMERA_LISTS + TREE_MESHES;        // 54
constant uint FOREST_LISTS = PROPS_LIST + 1;                  // 55
constant uint COUNTS_WORDS = 64;               // счётчики, затем indirect-аргументы
constant uint FOREST_CAP = 16384;             // экземпляров в списке
constant uint SPECIES_SUGI = 0;
constant uint SPECIES_SUGI_OLD = 2;
constant uint SPECIES_BROAD = 3;
constant uint SPECIES_BOULDER = 6;
constant uint SPECIES_FERN = 7;
constant uint SPECIES_SASA = 8;
constant uint SPECIES_SHRUB = 9;
constant uint SPECIES_LOG = 10;
constant uint SPECIES_STUMP = 11;
constant uint SPECIES_WINDFALL = 12;
constant uint SPECIES_DEBRIS = 13;
constant uint SPECIES_MOSS = 14;
constant uint SPECIES_TRAIL_ROOTS = 15;
constant uint SPECIES_MUSHROOM = 16;
// Высота вида при масштабе 1, м (сфера отсечения).
constant float SPECIES_H[GPU_SPECIES] = {29.0, 25.0, 24.0, 17.0, 17.0, 15.0, 2.2, 1.0, 1.4, 2.4, 1.0, 2.0, 15.0, 0.3, 0.4, 0.3};
// Подлесок: подклетки 2 м до UNDER_DIST (кусты — до SHRUB_DIST), LOD на 15 и 35 м.
constant float UNDER_DIST = 70.0;
constant float SHRUB_DIST = 100.0;
// Мелочь под ногами: подклетки 1,5 м до CLUTTER_DIST, LOD на 8 и 16 м.
constant float CLUTTER_DIST = 26.0;
constant float LOD_DIST[2] = {45.0, 150.0};
constant float SHADOW_DIST = 160.0;

inline uint hash3(int x, int z, uint salt) {
    uint h = uint(x) * 0x8da6b343u ^ uint(z) * 0xd8163841u ^ salt * 0xcb1ab31fu;
    h ^= h >> 16;
    h *= 0x7feb352du;
    h ^= h >> 15;
    h *= 0x846ca68bu;
    h ^= h >> 16;
    return h;
}
inline float rnd(uint h, uint k) {
    return float(hash3(int(h), int(k), 0x51u) & 0xffffffu) / 16777216.0;
}

// Гладкий шум пятен 0…1 с ячейкой `cell` м: куртины папоротника и кустов.
static float patch_noise(float2 p, float cell, uint salt) {
    float2 q = p / cell;
    int2 i = int2(floor(q));
    float2 f = q - float2(i);
    f = f * f * (3.0 - 2.0 * f);
    auto v = [&](int a, int b) { return rnd(hash3(i.x + a, i.y + b, salt), 1); };
    return mix(mix(v(0, 0), v(1, 0), f.x), mix(v(0, 1), v(1, 1), f.x), f.y);
}

// Высота рельефа под точкой: билинейно по полю острова или зоны 1 м.
static float ground_height(float2 p, device const float* heights, device const float* zone_heights,
                           constant TerrainZone& zone, constant FrameUniforms& frame) {
    if (zone.far_count != 0xffffffffu) {
        float2 q = (p - float2(zone.origin_x, zone.origin_z)) / zone.step_m;
        float nz1 = float(zone.n - 1);
        if (all(q >= 0.0) && all(q <= nz1)) {
            int2 i = min(int2(floor(q)), int2(int(zone.n) - 2));
            float2 f = q - float2(i);
            auto h = [&](int a, int b) { return zone_heights[uint(b) * zone.n + uint(a)]; };
            return mix(mix(h(i.x, i.y), h(i.x + 1, i.y), f.x), mix(h(i.x, i.y + 1), h(i.x + 1, i.y + 1), f.x), f.y);
        }
    }
    uint nx = frame.grid_nx;
    int nz = int(frame.map_size_m / frame.grid_step_m + 0.5) + 1;
    float2 q = clamp(p / frame.grid_step_m, 0.0, float2(float(nx) - 1.001, float(nz) - 1.001));
    int2 i = int2(floor(q));
    float2 f = q - float2(i);
    auto h = [&](int a, int b) { return heights[uint(b) * nx + uint(a)]; };
    return mix(mix(h(i.x, i.y), h(i.x + 1, i.y), f.x), mix(h(i.x, i.y + 1), h(i.x + 1, i.y + 1), f.x), f.y);
}

kernel void forest_place(device const float* heights [[buffer(0)]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const FeatureSegment* feat_segs [[buffer(2)]],
                         device const uint2* feat_cells [[buffer(3)]],
                         device const uint* feat_list [[buffer(4)]],
                         device const float* zone_heights [[buffer(5)]],
                         constant TerrainZone& zone [[buffer(6)]],
                         constant CullView& view [[buffer(7)]],
                         device PlantInstance* out [[buffer(8)]],
                         device atomic_uint* counts [[buffer(9)]],
                         device const float4* veg_table [[buffer(11)]],
                         texture2d<float> cover [[texture(0)]],
                         texture2d<float> veg [[texture(1)]],
                         texture2d<float> macro [[texture(2)]],
                         uint2 gid [[thread_position_in_grid]]) {
    if (any(gid >= uint2(FOREST_GRID))) {
        return;
    }
    int2 cell = int2(floor(frame.camera_pos.xz / FOREST_CELL_M)) - FOREST_GRID / 2 + int2(gid);
    // Три кандидата в клетке: дерево, валун, валежник (ствол или пень).
    for (uint kind = 0; kind < 3; ++kind) {
        uint h = hash3(cell.x, cell.y, 0xF0E5u + kind);
        float2 p = (float2(cell) + 0.12 + 0.76 * float2(rnd(h, 1), rnd(h, 2))) * FOREST_CELL_M;
        if (any(p < 0.0) || any(p > frame.map_size_m)) {
            continue;
        }
        constexpr sampler ls(filter::linear, address::clamp_to_edge);
        float cn = float(cover.get_width());
        float4 cv = cover.sample(ls, (p / frame.map_size_m * (cn - 1.0) + 0.5) / cn, level(0));
        float forest = cv.x + cv.y;
        // Сообщество карты растительности (02.7): густота, доля криптомерии,
        // масштаб, валуны. Точка выборки дрожит на ±10 м: граница сообществ
        // не ступенька сетки 15 м. Без карты — по поясам.
        float density = forest * 0.6;
        float cedar = cv.y / max(forest, 1e-4);
        float2 srange = float2(0.55, 1.1);
        float boulders = 0.06 * forest + 0.25 * cv.w;
        uint vn = veg.get_width();
        if (vn > 1) {
            float2 pj = p + (float2(rnd(h, 11), rnd(h, 12)) - 0.5) * 20.0;
            uint2 t = uint2(clamp(pj / frame.map_size_m * float(vn - 1) + 0.5, 0.0, float(vn - 1)));
            uint id = uint(veg.read(t).r * 255.0 + 0.5);
            float4 a = veg_table[id * 2];
            float4 b = veg_table[id * 2 + 1];
            density = a.x;
            cedar = a.y;
            srange = a.zw;
            boulders = b.x;
        }
        float y = ground_height(p, heights, zone_heights, zone, frame);
        // На склоне основание — по нижней точке у ствола: корни уходят в грунт.
        float y_low = y;
        for (int k = 0; k < 4; ++k) {
            float2 o = float2(k < 2 ? (k == 0 ? 1.5 : -1.5) : 0.0, k >= 2 ? (k == 2 ? 1.5 : -1.5) : 0.0);
            y_low = min(y_low, ground_height(p + o, heights, zone_heights, zone, frame));
        }
        if (y < 2.0) {
            continue;  // море и пляж
        }
        // Уклон по разностям в 3 м.
        float gx = ground_height(p + float2(3, 0), heights, zone_heights, zone, frame) - y;
        float gz = ground_height(p + float2(0, 3), heights, zone_heights, zone, frame) - y;
        float slope = length(float2(gx, gz)) / 3.0;
        uint species;
        float scale;
        if (kind == 0) {
            if (rnd(h, 3) > density || slope > 0.9) {
                continue;
            }
            // Вариант сетки — по хэшу; древние якусуги — в естественном
            // кедровом лесу (масштаб сообщества от 0,7).
            bool sugi = rnd(h, 4) < cedar;
            bool giant = sugi && srange.x >= 0.7 && rnd(h, 6) < 0.06;
            uint variant = min(uint(rnd(h, 10) * 3.0), 2u);
            species = sugi ? (giant ? SPECIES_SUGI_OLD : SPECIES_SUGI + min(variant, 1u)) : SPECIES_BROAD + variant;
            scale = giant ? mix(0.9, 1.25, rnd(h, 5)) : mix(srange.x, srange.y, rnd(h, 5));
        } else if (kind == 2) {
            // Валежник: в лесу, пни — чаще в кедровом (вырубки эпохи Эдо).
            if (rnd(h, 3) > forest * (0.05 + 0.12 * cedar) || slope > 0.8) {
                continue;
            }
            float r4 = rnd(h, 4);
            species = r4 < 0.3 + 0.3 * cedar ? SPECIES_STUMP : (r4 < 0.75 ? SPECIES_LOG : SPECIES_WINDFALL);
            scale = 0.7 + 0.5 * rnd(h, 5);
        } else {
            // Валуны: редко в лесу, чаще на голом и в руслах.
            if (rnd(h, 3) > boulders || slope > 1.2) {
                continue;
            }
            species = SPECIES_BOULDER;
            scale = 0.4 + 1.2 * rnd(h, 5) * rnd(h, 6);
        }
        // Не на тропах, дорогах и в руслах.
        FeatureDist fd = feature_distances(p, frame.map_size_m, feat_segs, feat_cells, feat_list);
        float clear = kind == 0 ? 2.5 * scale : (kind == 2 ? (species == SPECIES_WINDFALL ? 15.0 : 4.0) * scale : 1.0);
        if (fd.trail < clear || fd.road < clear + 1.5 || (kind != 1 && fd.river < 2.0)) {
            continue;
        }
        float H = SPECIES_H[species] * scale;
        float3 base = float3(p.x, y_low - 0.2 * scale, p.y);
        float3 center = base + float3(0, H * 0.5, 0);
        float dist = length(center - frame.camera_pos.xyz);
        PlantInstance inst;
        inst.position = packed_float3(base);
        inst.scale = scale;
        inst.yaw = rnd(h, 7) * 6.2831853;
        inst.tint = rnd(h, 8);
        inst.phase = rnd(h, 9) * 6.2831853;
        inst._pad = 0.0;
        if (kind == 0 && dist < SHADOW_DIST) {
            uint k = atomic_fetch_add_explicit(&counts[CAMERA_LISTS + species], 1, memory_order_relaxed);
            if (k < FOREST_CAP) {
                out[(CAMERA_LISTS + species) * FOREST_CAP + k] = inst;
            }
        }
        float lim = kind == 0 ? 480.0 : (kind == 1 ? 180.0 : 120.0);
        bool visible = dist - H * 0.6 < lim;
        for (int i = 0; i < 6 && visible; ++i) {
            visible = dot(view.planes[i].xyz, center) + view.planes[i].w >= -H * 0.6;
        }
        if (!visible) {
            continue;
        }
        uint lod = dist < LOD_DIST[0] ? 0 : (dist < LOD_DIST[1] ? 1 : 2);
        uint list = species * 3 + lod;
        uint k = atomic_fetch_add_explicit(&counts[list], 1, memory_order_relaxed);
        if (k < FOREST_CAP) {
            out[list * FOREST_CAP + k] = inst;
        }
    }
    // Подлесок (02.4): подклетки 2 м в клетках у камеры. Папоротники —
    // куртинами во влажных ложбинах и у ручьёв, кусты — пятнами, гуще на
    // опушках у троп; якудзаса — в высокогорье и на лугах.
    float2 cc = (float2(cell) + 0.5) * FOREST_CELL_M;
    if (distance(cc, frame.camera_pos.xz) > SHRUB_DIST + 6.0) {
        return;
    }
    constexpr sampler ls2(filter::linear, address::clamp_to_edge);
    for (uint sub = 0; sub < 9; ++sub) {
        int2 sc = int2(sub % 3, sub / 3);
        uint h = hash3(cell.x * 3 + sc.x, cell.y * 3 + sc.y, 0x0DE2u);
        float2 p = (float2(cell) + (float2(sc) + 0.1 + 0.8 * float2(rnd(h, 1), rnd(h, 2))) / 3.0) * FOREST_CELL_M;
        float dxz = distance(p, frame.camera_pos.xz);
        if (dxz > SHRUB_DIST || any(p < 0.0) || any(p > frame.map_size_m)) {
            continue;
        }
        float cn = float(cover.get_width());
        float4 cv = cover.sample(ls2, (p / frame.map_size_m * (cn - 1.0) + 0.5) / cn, level(0));
        float mn = float(macro.get_width());
        float4 mc = macro.sample(ls2, (p / frame.map_size_m * (mn - 1.0) + 0.5) / mn, level(0));
        FeatureDist fd = feature_distances(p, frame.map_size_m, feat_segs, feat_cells, feat_list);
        float forest = cv.x + cv.y;
        float hollow = saturate((0.5 - mc.g) * 2.5);
        float brook = 1.0 - smoothstep(1.0, 14.0, fd.river);
        float edge = smoothstep(1.5, 3.0, fd.trail) * (1.0 - smoothstep(4.0, 12.0, fd.trail));
        float fern_patch = smoothstep(0.35, 0.75, patch_noise(p, 14.0, 0xFE11u));
        float shrub_patch = smoothstep(0.45, 0.8, patch_noise(p, 22.0, 0x5B11u));
        float fern_p = forest * (0.15 + 0.5 * mc.b + 0.35 * hollow + 0.5 * brook) * (0.25 + 1.2 * fern_patch);
        float shrub_p = forest * (0.04 + 0.25 * edge) * (0.3 + 1.4 * shrub_patch);
        float sasa_p = cv.z * 0.6 + forest * 0.03;
        float r = rnd(h, 3);
        uint species;
        if (r < shrub_p) {
            species = SPECIES_SHRUB;
        } else if (dxz > UNDER_DIST) {
            continue;
        } else if (r < shrub_p + fern_p) {
            species = SPECIES_FERN;
        } else if (r < shrub_p + fern_p + sasa_p) {
            species = SPECIES_SASA;
        } else {
            continue;
        }
        float y = ground_height(p, heights, zone_heights, zone, frame);
        if (y < 2.0) {
            continue;
        }
        if (fd.trail < 1.0 || fd.road < 2.0 || fd.river < 0.5) {
            continue;
        }
        float scale = species == SPECIES_SHRUB ? 0.6 + 0.6 * rnd(h, 5) : 0.7 + 0.6 * rnd(h, 5);
        float H = SPECIES_H[species] * scale;
        float3 base = float3(p.x, y - 0.05, p.y);
        float3 center = base + float3(0, H * 0.5, 0);
        bool visible = true;
        for (int i = 0; i < 6 && visible; ++i) {
            visible = dot(view.planes[i].xyz, center) + view.planes[i].w >= -H;
        }
        if (!visible) {
            continue;
        }
        PlantInstance inst;
        inst.position = packed_float3(base);
        inst.scale = scale;
        inst.yaw = rnd(h, 7) * 6.2831853;
        inst.tint = rnd(h, 8);
        inst.phase = rnd(h, 9) * 6.2831853;
        inst._pad = 0.0;
        float l0 = species == SPECIES_SHRUB ? 25.0 : 15.0;
        uint lod = dxz < l0 ? 0 : (dxz < l0 * 2.4 ? 1 : 2);
        uint list = species * 3 + lod;
        uint k = atomic_fetch_add_explicit(&counts[list], 1, memory_order_relaxed);
        if (k < FOREST_CAP) {
            out[list * FOREST_CAP + k] = inst;
        }
    }
    // Мелочь под ногами (02.4, итерация 4): сор и подушки мха в подклетках
    // 1,5 м у камеры — в лесу и на тропах по краю; мох — во влажном.
    if (distance(cc, frame.camera_pos.xz) > CLUTTER_DIST + 6.0) {
        return;
    }
    for (uint sub = 0; sub < 16; ++sub) {
        int2 sc = int2(sub % 4, sub / 4);
        uint h = hash3(cell.x * 4 + sc.x, cell.y * 4 + sc.y, 0xC1A7u);
        float2 p = (float2(cell) + (float2(sc) + 0.1 + 0.8 * float2(rnd(h, 1), rnd(h, 2))) / 4.0) * FOREST_CELL_M;
        float dxz = distance(p, frame.camera_pos.xz);
        if (dxz > CLUTTER_DIST || any(p < 0.0) || any(p > frame.map_size_m)) {
            continue;
        }
        float cn = float(cover.get_width());
        float4 cv = cover.sample(ls2, (p / frame.map_size_m * (cn - 1.0) + 0.5) / cn, level(0));
        float mn = float(macro.get_width());
        float4 mc = macro.sample(ls2, (p / frame.map_size_m * (mn - 1.0) + 0.5) / mn, level(0));
        float forest = cv.x + cv.y;
        float r = rnd(h, 3);
        float moss_p = forest * (0.08 + 0.35 * mc.b) * smoothstep(0.3, 0.7, patch_noise(p, 6.0, 0x3055u));
        float debris_p = 0.12 + 0.35 * forest + 0.3 * cv.w;
        FeatureDist fd = feature_distances(p, frame.map_size_m, feat_segs, feat_cells, feat_list);
        // На полотне лесной тропы (расстояние до края < 0) — корни и камни
        // поперёк неё: ось сетки X — по градиенту расстояния до края.
        bool on_trail = fd.trail < -0.2 && forest > 0.2;
        uint species = on_trail ? (rnd(h, 10) < 0.35 ? SPECIES_TRAIL_ROOTS : 99u)
                                : (r < moss_p ? SPECIES_MOSS : (r < moss_p + debris_p ? SPECIES_DEBRIS : 99u));
        if (species == 99u) {
            continue;
        }
        float y = ground_height(p, heights, zone_heights, zone, frame);
        if (y < 2.0) {
            continue;
        }
        if (fd.road < 1.5 || fd.river < 0.3 || (species == SPECIES_MOSS && fd.trail < 0.8)) {
            continue;
        }
        float yaw = rnd(h, 7) * 6.2831853;
        if (species == SPECIES_TRAIL_ROOTS) {
            float2 g = float2(feature_distances(p + float2(0.5, 0), frame.map_size_m, feat_segs, feat_cells, feat_list).trail,
                              feature_distances(p + float2(0, 0.5), frame.map_size_m, feat_segs, feat_cells, feat_list).trail)
                     - fd.trail;
            yaw = atan2(g.y, g.x) + (rnd(h, 11) - 0.5) * 0.5;
        }
        // Сор и мох ложатся по рельефу (драпировка в `plant_world`); на
        // осыпях круче ~35° их нет.
        float gx = ground_height(p + float2(0.7, 0), heights, zone_heights, zone, frame) - y;
        float gz = ground_height(p + float2(0, 0.7), heights, zone_heights, zone, frame) - y;
        if (length(float2(gx, gz)) / 0.7 > 0.7) {
            continue;
        }
        float scale = 0.6 + 0.6 * rnd(h, 5);
        float3 base = float3(p.x, y - 0.03, p.y);
        bool visible = true;
        for (int i = 0; i < 6 && visible; ++i) {
            visible = dot(view.planes[i].xyz, base) + view.planes[i].w >= -1.5 * scale;
        }
        if (!visible) {
            continue;
        }
        PlantInstance inst;
        inst.position = packed_float3(base);
        inst.scale = scale;
        inst.yaw = yaw;
        inst.tint = rnd(h, 8);
        inst.phase = rnd(h, 9) * 6.2831853;
        inst._pad = 0.0;
        uint lod = dxz < 8.0 ? 0 : (dxz < 16.0 ? 1 : 2);
        uint list = species * 3 + lod;
        uint k = atomic_fetch_add_explicit(&counts[list], 1, memory_order_relaxed);
        if (k < FOREST_CAP) {
            out[list * FOREST_CAP + k] = inst;
        }
    }
}

// Indirect-аргументы drawIndexedPrimitives по спискам; `lods` — (первый
// индекс, число индексов, базовая вершина) вида × LOD.
kernel void forest_finish(device uint* counts [[buffer(9)]],
                          device const uint4* lods [[buffer(10)]],
                          uint i [[thread_position_in_grid]]) {
    if (i >= FOREST_LISTS) {
        return;
    }
    uint species = i < CAMERA_LISTS ? i / 3 : (i == PROPS_LIST ? SPECIES_MUSHROOM : i - CAMERA_LISTS);
    uint lod = i < CAMERA_LISTS ? i % 3 : (i == PROPS_LIST ? 0u : 1u);
    uint4 r = lods[species * 3 + lod];
    device uint* a = counts + COUNTS_WORDS + i * 5;
    a[0] = r.y;
    a[1] = min(counts[i], FOREST_CAP);
    a[2] = r.x;
    a[3] = r.z;
    a[4] = 0;
}

// --- Отрисовка ---------------------------------------------------------------

struct PlantOut {
    float4 position [[position]];
    float3 world;
    float3 rel;
    float3 normal;
    float2 uv;
    float3 attr;      // материал, ветер, затенение
    float tint;
    float kind;       // вид гриба (05.1)
    float moss;       // мох на коре и почве (атрибут A)
};

// Мир точки сетки растения: поворот, масштаб, ветер (по силе из атрибута).
// Поле высот для «драпировки» растений по рельефу (02.10).
struct PlantGround {
    device const float* heights;
    device const float* zone_heights;
    constant TerrainZone& zone;
};

// Земля под точкой для драпировки: в детальной заплатке у камеры — её
// поверхность (бикубика + кочки), иначе — билинейно по полю.
static float plant_ground_h(PlantGround g, float2 p, constant FrameUniforms& frame) {
    if (patch_active(frame, g.zone) && patch_edge_dist(p, frame) > 0.0) {
        return patch_height(PatchZone{g.zone_heights, g.zone}, p, frame);
    }
    return ground_height(p, g.heights, g.zone_heights, g.zone, frame);
}

static float3 plant_world(PlantVertex v, PlantInstance inst, constant FrameUniforms& frame,
                          PlantGround g, thread float3& n) {
    float c = cos(inst.yaw), s = sin(inst.yaw);
    float3 p = float3(v.position);
    // Своя форма у каждого экземпляра (02.4, итерация 4): вытянутость,
    // ширина, сплюснутость по одной оси и наклон растут с высотой точки —
    // основание с корнями и бугром почвы остаётся на земле.
    uint hs = as_type<uint>(inst.phase);
    float4 rv = float4(hs & 0xffu, (hs >> 8) & 0xffu, (hs >> 16) & 0xffu, hs >> 24) / 255.0;
    float up = saturate(p.y / 3.0);
    p.y *= mix(1.0, 0.85 + 0.35 * rv.x, up);
    p.xz *= mix(float2(1.0), float2(0.8 + 0.4 * rv.y, 0.8 + 0.4 * rv.z), up);
    p.xz += (rv.wy - 0.5) * 0.12 * max(p.y - 1.0, 0.0);
    float3 q = float3(c * p.x - s * p.z, p.y, s * p.x + c * p.z) * inst.scale;
    float3 nn = float3(v.normal);
    n = float3(c * nn.x - s * nn.z, nn.y, s * nn.x + c * nn.z);
    float w = float(v.attr.y) / 255.0;
    float t = frame.time;
    float2 dir = normalize(float2(0.8, 0.6));
    // Порывы (02.10): сила ветра медленно меняется во времени и по месту —
    // бывают паузы; соседние растения качаются не в такт.
    float2 bp = float2(inst.position.x, inst.position.z);
    float gust = mix(0.15, 1.0, saturate(0.5 + 0.6 * sin(t * 0.21 + bp.x * 0.013) * cos(t * 0.13 + bp.y * 0.011)));
    float sway = (sin(t * 0.9 + inst.phase) * 0.6 + sin(t * 2.3 + inst.phase * 1.7) * 0.25) * w * w * gust;
    // Дрожание вдоль нормали — только листве: у трубок ветвей нормали
    // радиальные, и ветви «дышали» бы (02.10).
    float mat = float(v.attr.x);
    bool leafy = (mat > 1.5 && mat < 3.5) || (mat > 4.5 && mat < 6.5);
    float fph = dot(q, float3(1.3, 0.7, 1.1)) + inst.phase * 3.0 + dot(bp, float2(0.37, 0.23));
    float flutter = leafy ? sin(t * (5.0 + 3.0 * fract(inst.phase)) + fph) * 0.035 * w * gust * gust : 0.0;
    // Раскачка растёт с высотой точки: верх кроны качается, папоротник — едва.
    q.xz += dir * sway * inst.scale * 0.9 * saturate(q.y / 8.0);
    q += n * flutter;
    float3 world = float3(inst.position) + q;
    // Драпировка (02.10): нижние ~0,6 м сетки — сор, листья, мох, бугор у
    // корней, основание ствола — повторяют рельеф под каждой вершиной, а не
    // плоскость основания; нормаль доворачивается по склону.
    float wd = 1.0 - smoothstep(0.1, 0.7, q.y);
    if (wd > 0.0) {
        float3 base = float3(inst.position);
        float sink = max(base.y - plant_ground_h(g, base.xz, frame), -0.08);
        float h0 = plant_ground_h(g, world.xz, frame);
        float hx = plant_ground_h(g, world.xz + float2(0.5, 0), frame);
        float hz = plant_ground_h(g, world.xz + float2(0, 0.5), frame);
        world.y = mix(world.y, h0 + sink + q.y, wd);
        float2 grad = float2(hx - h0, hz - h0) / 0.5;
        n = normalize(n + float3(-grad.x, 0.0, -grad.y) * max(n.y, 0.0) * wd);
    }
    return world;
}

vertex PlantOut plant_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                         device const PlantVertex* verts [[buffer(0)]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const PlantInstance* insts [[buffer(2)]],
                         device const float* heights [[buffer(6)]],
                         device const float* zone_heights [[buffer(7)]],
                         constant TerrainZone& zone [[buffer(8)]]) {
    PlantVertex v = verts[vid];
    PlantInstance inst = insts[iid];
    PlantOut o;
    float3 n;
    o.world = plant_world(v, inst, frame, PlantGround{heights, zone_heights, zone}, n);
    o.rel = camera_relative(frame, o.world);
    o.position = frame.view_proj_rel * float4(o.rel, 1.0);
    o.normal = n;
    o.uv = float2(v.uv);
    o.attr = float3(float(v.attr.x), float(v.attr.y) / 255.0, float(v.attr.z) / 255.0);
    o.tint = inst.tint;
    o.kind = inst._pad;
    o.moss = float(v.attr.w) / 255.0;
    return o;
}

// Альбедо и покрытие: кора — слои 0/1, листва — атлас в верхней половине слоя 2.
// Альфа «заострена» по производным: отсечение устойчиво на всех мипах.
static float4 plant_albedo(texture2d_array<float> albedo, float mat, float2 uv) {
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
    if (mat < 1.5) {
        return float4(albedo.sample(s, uv, uint(mat)).rgb, 1.0);
    }
    float4 a = albedo.sample(s, uv, 2);
    float w = max(fwidth(a.a), 1e-4);
    a.a = saturate((a.a - 0.5) / w + 0.5);
    return a;
}

fragment float4 plant_fs(PlantOut in [[stage_in]], bool front [[front_facing]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const SkyFrame& skyf [[buffer(3)]],
                         constant ShadowUniforms& shadows [[buffer(4)]],
                         texture2d_array<float> albedo_tex [[texture(0)]],
                         depth2d_array<float> shadow_map [[texture(1)]],
                         texture3d<float> aerial [[texture(2)]],
                         texture2d_array<float> mat_albedo [[texture(3)]],
                         texture2d_array<float> bark_normal [[texture(4)]],
                         texture2d<float> rt_ao_tex [[texture(5)]]) {
    float mat = round(in.attr.x);
    float3 n = normalize(in.normal);
    constexpr sampler ms(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
    float3 albedo;
    // 2 хвоя, 3 листья, 5 папоротник, 6 якудзаса — листва; 4 — камень.
    bool leaf = (mat > 1.5 && mat < 3.5) || (mat > 4.5 && mat < 6.5);
    float3 glow = 0.0;
    if (mat > 7.5) {
        // Почва бугра у корней и корневой тарелки (02.4): подстилка леса и мох
        // в мировых координатах, как на рельефе, — бугор сливается с землёй.
        float3 lp = in.rel + frame.cam_lo.xyz;
        float3 litter = mat_albedo.sample(ms, lp.xz / 3.0, 4).rgb;
        float3 mossc = mat_albedo.sample(ms, lp.xz / 2.0, 3).rgb;
        float side = 1.0 - saturate(abs(n.x) > abs(n.z) ? abs(n.x) : abs(n.z));
        float3 soil = mat_albedo.sample(ms, (abs(n.x) > abs(n.z) ? lp.zy : lp.xy) / 2.5, 2).rgb;
        albedo = mix(soil, litter, saturate(side * 1.5));
        float mm = smoothstep(0.35, 0.65, in.moss + (dot(mossc, float3(0.33)) - 0.25) * 0.8);
        albedo = mix(albedo, mossc, mm);
    } else if (mat > 6.5) {
        // Гриб (05.1): цвет шляпки и ножки по виду; светящийся мицен — свечение
        // шляпки и пластинок (видно только ночью: экспозиция дня его гасит).
        const float3 caps[5] = {float3(0.62, 0.66, 0.50), float3(0.30, 0.20, 0.12),
                                float3(0.42, 0.24, 0.14), float3(0.35, 0.10, 0.05),
                                float3(0.45, 0.36, 0.22)};
        uint k = min(uint(in.kind + 0.5), 4u);
        albedo = in.uv.y > 0.5 ? caps[k] : float3(0.75, 0.72, 0.62);
        if (in.kind > 7.5) {
            // Бусина чувства мицелия (05.2): 8 — к грибам, 9 — к следам,
            // 10 — узел. Яркость на экране не зависит от экспозиции.
            const float3 beads[3] = {float3(0.45, 1.0, 0.55), float3(1.0, 0.72, 0.30),
                                     float3(0.60, 0.80, 1.0)};
            uint b = min(uint(in.kind - 7.5), 2u);
            albedo = float3(0.04);
            glow = beads[b] * in.tint * 0.8 / max(frame.pre_exposure, 1e-12);
        } else if (k == 0) {
            glow = float3(0.35, 1.0, 0.45) * 3e-6 * (in.uv.y > 0.5 ? 1.0 : 0.4);
        }
    } else if (mat > 3.5 && mat < 4.5) {
        // Валун: камень сбоку, мшистый камень сверху (слои рельефа 0 и 9).
        constexpr sampler s(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
        float3 lp = in.rel + frame.cam_lo.xyz;
        float3 w = pow(abs(n), 4.0);
        w /= w.x + w.y + w.z;
        auto tri = [&](uint layer) {
            return mat_albedo.sample(s, lp.zy / 3.0, layer).rgb * w.x + mat_albedo.sample(s, lp.xz / 3.0, layer).rgb * w.y
                 + mat_albedo.sample(s, lp.xy / 3.0, layer).rgb * w.z;
        };
        float moss = smoothstep(0.2, 0.7, n.y);
        albedo = mix(tri(0), tri(9), moss);
    } else {
        float4 a = plant_albedo(albedo_tex, mat, in.uv);
        if (a.a < 0.5) {
            discard_fragment();
        }
        albedo = a.rgb;
        if (mat < 1.5) {
            // Кора: карта нормалей (Bark012/Bark007, OpenGL) в базисе по
            // производным позиции и uv — касательных в вершинах нет.
            float3 dp1 = dfdx(in.rel), dp2 = dfdy(in.rel);
            float2 du1 = dfdx(in.uv), du2 = dfdy(in.uv);
            float3 p2 = cross(dp2, n), p1 = cross(n, dp1);
            float3 T = p2 * du1.x + p1 * du2.x;
            float3 B = p2 * du1.y + p1 * du2.y;
            float inv = rsqrt(max(max(dot(T, T), dot(B, B)), 1e-20));
            float3 tn = bark_normal.sample(ms, in.uv, uint(mat)).xyz * 2.0 - 1.0;
            n = normalize(T * inv * tn.x + B * inv * tn.y + n * tn.z);
            albedo *= mat < 0.5 ? float3(0.52, 0.38, 0.30)  // кора якусуги — красно-бурая, не оранжевая
                                : float3(0.24, 0.22, 0.19); // серо-бурая кора лиственных
            // Мох: у основания, на верхних сторонах ветвей и на валежнике.
            float3 mossc = mat_albedo.sample(ms, in.uv * 1.3, 3).rgb;
            float mm = smoothstep(0.4, 0.7, in.moss + (dot(mossc, float3(0.33)) - 0.25) * 0.9 + n.y * 0.15);
            albedo = mix(albedo, mossc, mm);
        }
        if (leaf) {
            albedo *= 0.55 + 0.3 * in.tint;
            albedo = mix(albedo, albedo * float3(1.1, 1.05, 0.75), in.tint * 0.4);
        }
    }
    if (!front) {
        n = -n;
    }
    float3 L = -frame.light_dir.xyz;
    float3 V = normalize(-in.rel);
    float nl = leaf ? saturate(dot(n, L) * 0.6 + 0.4) : saturate(dot(n, L));
    float shadow = shadow_factor(shadows, shadow_map, in.rel, n, L);
    float3 sun = sky_sun_term(skyf) * shadow;
    float ao = in.attr.z;
    float3 sky = sky_ambient_term(skyf, n) * ao;
    // Отражённый свет от освещённой земли и крон (как у рельефа, 02.10).
    float3 bounce = sky_sun_term(skyf) * max(L.y, 0.0) * 0.24 * float3(0.75, 0.85, 0.6);
    // Затенение лучами (11) — кора, камни, валежник (листва — карточки, её
    // свет уже приглушён затенением внутри кроны).
    if (shadows.params.z > 0.5 && !leaf) {
        constexpr sampler aos(filter::linear, address::clamp_to_edge);
        float rao = rt_ao_tex.sample(aos, in.position.xy / (2.0 * float2(rt_ao_tex.get_width(), rt_ao_tex.get_height()))).x;
        sky *= rao;
        bounce *= rao;
    }
    float3 color = albedo * (sun * nl + sky + bounce * ao) + glow;
    if (leaf) {
        // Просвет листвы против солнца.
        float back = pow(saturate(dot(-V, L)), 3.0);
        color += albedo * sun * back * 0.8 * float3(0.9, 1.1, 0.6);
    }
    if (frame.underwater_m <= 0.0) {
        color = apply_aerial(aerial, frame, in.rel, color);
    }
    return float4(color * frame.pre_exposure, 1.0);
}

// Тени: глубина растений в каскад; листва — с отсечением по альфе.
struct PlantShadowOut {
    float4 position [[position]];
    float2 uv;
    float mat;
};

vertex PlantShadowOut plant_shadow_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                                      device const PlantVertex* verts [[buffer(0)]],
                                      constant FrameUniforms& frame [[buffer(1)]],
                                      device const PlantInstance* insts [[buffer(2)]],
                                      constant float4x4& cascade_vp [[buffer(5)]],
                                      device const float* heights [[buffer(6)]],
                                      device const float* zone_heights [[buffer(7)]],
                                      constant TerrainZone& zone [[buffer(8)]]) {
    PlantVertex v = verts[vid];
    float3 n;
    float3 world = plant_world(v, insts[iid], frame, PlantGround{heights, zone_heights, zone}, n);
    PlantShadowOut o;
    o.position = cascade_vp * float4(camera_relative(frame, world), 1.0);
    o.uv = float2(v.uv);
    o.mat = float(v.attr.x);
    return o;
}

fragment void plant_shadow_fs(PlantShadowOut in [[stage_in]],
                              texture2d_array<float> albedo_tex [[texture(0)]]) {
    float mat = round(in.mat);
    bool leafy = (mat > 1.5 && mat < 3.5) || (mat > 4.5 && mat < 6.5);
    if (leafy && plant_albedo(albedo_tex, mat, in.uv).a < 0.5) {
        discard_fragment();
    }
    // Листва пропускает часть света (02.10): 55 % текселей карточки не
    // затеняют — после PCF тень кроны пятнистая и неполная, а не чёткий
    // силуэт веточки.
    if (leafy) {
        uint2 px = uint2(in.position.xy);
        uint h = (px.x * 0x8da6b343u) ^ (px.y * 0xd8163841u);
        h ^= h >> 13;
        h *= 0x7feb352du;
        if (float(h >> 8) / 16777216.0 < 0.55) {
            discard_fragment();
        }
    }
}
