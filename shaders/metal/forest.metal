#include "common.h"
#include "ground.h"
#include "features.h"
#include "sky.h"
#include "terrain_patch.h"
#include "probes.h"

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
constant uint GPU_SPECIES = 41;
constant uint TREE_MESHES = 6;
constant uint CAMERA_LISTS = GPU_SPECIES * 3;                 // вид × LOD
constant uint PROPS_LIST = CAMERA_LISTS + TREE_MESHES;        // 54
constant uint FOREST_LISTS = PROPS_LIST + 1;                  // 55
constant uint COUNTS_WORDS = 160;               // счётчики, затем indirect-аргументы
constant uint FOREST_CAP = 8192;             // экземпляров в списке
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
constant uint SPECIES_GRASS = 33;           // 33–37 — широколистные травы под пологом (сканы shrub_03/04)
constant uint SPECIES_BLADES = 38;          // 38–40 — пучки травинок (густая трава)
constant uint SPECIES_MUSHROOM = 41;
// Густая трава: шаг 0,35 м до GRASS_DIST.
constant float GRASS_DIST = 45.0;
constant int GRASS_SUB = 24;
// Высота вида при масштабе 1, м (сфера отсечения).
constant float SPECIES_H[GPU_SPECIES] = {27.0, 23.0, 24.0, 17.0, 17.0, 15.0, 2.2, 1.0, 1.4, 2.4, 1.0, 2.0, 15.0, 0.3, 0.4, 0.3,
                                         2.2, 2.2, 2.2, 1.0, 1.0, 1.0, 4.2, 4.6, 1.0, 2.0, 0.3, 0.3, 0.3,
                                         0.4, 0.4, 0.4, 0.4, 0.5, 0.5, 0.5, 0.5, 0.5, 0.7, 0.7, 0.7};
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

// Вариант сетки вида по хэшу (02.11, сессия 4): фотосканы дают по 2–5
// сеток на вид; списки — по сеткам.
static uint variant_of(uint species, uint h) {
    float r = rnd(h, 13);
    auto pick = [&](uint a, uint b, uint c, uint d, uint e, uint n) {
        uint i = min(uint(r * float(n)), n - 1);
        return i == 0 ? a : (i == 1 ? b : (i == 2 ? c : (i == 3 ? d : e)));
    };
    switch (species) {
        case 6: return pick(6, 16, 17, 18, 18, 4);       // валуны
        case 7: return pick(7, 19, 20, 21, 21, 4);       // папоротник
        case 9: return pick(9, 22, 23, 23, 23, 3);       // кустарник
        case 10: return pick(10, 24, 24, 24, 24, 2);     // ствол
        case 11: return pick(11, 25, 25, 25, 25, 2);     // пень
        case 15: return pick(15, 26, 27, 28, 28, 4);     // корни на тропе
        case 13: return pick(13, 29, 30, 31, 32, 5);     // сор
        case 33: return pick(33, 34, 35, 36, 37, 5);     // трава (сканы)
        case 38: return pick(38, 39, 40, 40, 40, 3);     // пучки травинок
        default: return species;
    }
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
            scale = 0.3 + 0.8 * rnd(h, 5) * rnd(h, 6);  // сканы камней 2–3 м
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
        // Метка дерева (02.13): `plant_world` не гнёт его как низкое растение.
        inst._pad = kind == 0 ? 20.0 : 0.0;
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
        uint list = variant_of(species, h) * 3 + lod;
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
    // 02.13, сессия 1: подклетки 1,5 м — подлесок сомкнутым ярусом.
    for (uint sub = 0; sub < 16; ++sub) {
        int2 sc = int2(sub % 4, sub / 4);
        uint h = hash3(cell.x * 4 + sc.x, cell.y * 4 + sc.y, 0x0DE2u);
        float2 p = (float2(cell) + (float2(sc) + 0.1 + 0.8 * float2(rnd(h, 1), rnd(h, 2))) / 4.0) * FOREST_CELL_M;
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
        float fern_p = forest * (0.4 + 0.5 * mc.b + 0.35 * hollow + 0.5 * brook) * (0.45 + 1.2 * fern_patch);
        float shrub_p = forest * (0.08 + 0.3 * edge) * (0.4 + 1.5 * shrub_patch);
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
        uint list = variant_of(species, h) * 3 + lod;
        uint k = atomic_fetch_add_explicit(&counts[list], 1, memory_order_relaxed);
        if (k < FOREST_CAP) {
            out[list * FOREST_CAP + k] = inst;
        }
    }
    // Густая трава (02.11, 02.13): фото-пучки на сетке 0,25 м в клетках у
    // камеры — сплошь на лугах, у троп и на открытом, умеренно под пологом,
    // редко на полотне тропы; пятнами по шуму.
    if (distance(cc, frame.camera_pos.xz) > GRASS_DIST + 6.0) {
        return;
    }
    FeatureDist fdc = feature_distances(cc, frame.map_size_m, feat_segs, feat_cells, feat_list);
    bool near_trail = fdc.trail < 9.0 || fdc.road < 9.0 || fdc.river < 9.0;
    for (int j = 0; j < GRASS_SUB; ++j) {
        for (int i = 0; i < GRASS_SUB; ++i) {
            uint h = hash3(cell.x * GRASS_SUB + i, cell.y * GRASS_SUB + j, 0x6B1Du);
            float2 p = (float2(cell) + (float2(i, j) + 0.1 + 0.8 * float2(rnd(h, 1), rnd(h, 2))) / float(GRASS_SUB))
                     * FOREST_CELL_M;
            float dxz = distance(p, frame.camera_pos.xz);
            if (dxz > GRASS_DIST) {
                continue;
            }
            float cn = float(cover.get_width());
            float4 cv = cover.sample(ls2, (p / frame.map_size_m * (cn - 1.0) + 0.5) / cn, level(0));
            float forest = cv.x + cv.y;
            // Гранит и скалы (cv.w) — трава лишь в трещинах.
            // Под пологом (02.13, сессия 1) — низкий ковёр пятнами: в пятнах
            // сомкнутый, между ними — опад и мох.
            float floor_patch = smoothstep(0.3, 0.6, patch_noise(p, 7.0, 0x6B3Au));
            float dens = (cv.z * 1.0 + max(1.0 - forest - cv.w, 0.0) * 0.95 + forest * (0.35 + 0.6 * floor_patch))
                       * (1.0 - 0.9 * cv.w);
            dens *= 0.65 + 0.7 * patch_noise(p, 8.0, 0x6B1Eu);
            float brook_g = 0.0;
            if (near_trail) {
                FeatureDist fd = feature_distances(p, frame.map_size_m, feat_segs, feat_cells, feat_list);
                brook_g = 1.0 - smoothstep(0.5, 6.0, fd.river);
                if (fd.road < 0.5 || fd.river < 0.3) {
                    continue;
                }
                // Каменистое полотно тропы — без травы, лишь узкая полоса у
                // кромки; за краем — полоса густой травы.
                dens = fd.trail < 0.0 ? 0.35 * smoothstep(-0.45, 0.0, fd.trail)
                                      : dens + 0.6 * (1.0 - smoothstep(0.0, 2.5, fd.trail));
            }
            // У ствола дерева этой клетки (кандидат 0, как в цикле деревьев)
            // трава редеет: тень и мшистый бугор корней.
            {
                uint ht = hash3(cell.x, cell.y, 0xF0E5u);
                float2 tp = (float2(cell) + 0.12 + 0.76 * float2(rnd(ht, 1), rnd(ht, 2))) * FOREST_CELL_M;
                dens *= mix(1.0, smoothstep(0.8, 2.5, distance(p, tp)), saturate(forest * 1.5));
            }
            if (rnd(h, 3) > dens) {
                continue;
            }
            float y = ground_height(p, heights, zone_heights, zone, frame);
            if (y < 2.0) {
                continue;
            }
            // К краю дальности пучки мельчают — нет резкой границы травы.
            float scale = (0.7 + 0.6 * rnd(h, 5)) * (1.0 - smoothstep(0.65 * GRASS_DIST, GRASS_DIST, dxz));
            if (scale < 0.05) {
                continue;
            }
            float3 base = float3(p.x, y - 0.02, p.y);
            float3 center = base + float3(0, 0.3 * scale, 0);
            bool visible = true;
            for (int k2 = 0; k2 < 6 && visible; ++k2) {
                visible = dot(view.planes[k2].xyz, center) + view.planes[k2].w >= -0.6 * scale;
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
            // Ярус: низкая густая (основа), средняя, высокая — редкими пятнами;
            // у воды гуще и выше, на гребнях — ниже.
            float tall_patch = patch_noise(p, 5.0, 0x6B2Fu);
            float rv = rnd(h, 13);
            // Под пологом — почти только низкая трава (света мало).
            uint tier = rv < 0.6 + 0.3 * saturate(forest) - 0.2 * brook_g ? 0u
                      : (rv < 0.93 - 0.1 * tall_patch ? 1u : (tall_patch > 0.55 ? 2u : 1u));
            uint lod = dxz < 10.0 ? 0 : (dxz < 24.0 ? 1 : 2);
            uint list = (SPECIES_BLADES + tier) * 3 + lod;
            uint k = atomic_fetch_add_explicit(&counts[list], 1, memory_order_relaxed);
            if (k < FOREST_CAP) {
                out[list * FOREST_CAP + k] = inst;
            }
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
        // Низкая трава (02.11): густо на лугах и открытом, умеренно под
        // пологом, пятнами; сор потеснился.
        // Под пологом — широколистные травы (как лесной покров), на открытом
        // их мало: там густая трава из травинок (цикл выше).
        float grass_p = (forest * 1.0 + cv.z * 0.15 + max(1.0 - forest - cv.w, 0.0) * 0.15)
                      * (0.45 + 0.8 * patch_noise(p, 9.0, 0x6A55u));
        float dp = debris_p * 0.45;
        // Тропа (02.11): корни поперёк; за краем — полоса трав.
        grass_p += 0.6 * (1.0 - smoothstep(0.0, 2.5, fd.trail)) * step(0.0, fd.trail);
        // На полотне тропы — только корни; травы — у кромки (цикл травы).
        uint species = on_trail ? (rnd(h, 10) < 0.3 ? SPECIES_TRAIL_ROOTS : 99u)
                                : (r < moss_p ? SPECIES_MOSS
                                   : (r < moss_p + dp ? SPECIES_DEBRIS
                                      : (r < moss_p + dp + grass_p ? SPECIES_GRASS : 99u)));
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
        // Травы-сканы мелкие (10–40 см) — крупнее.
        float scale = (0.6 + 0.6 * rnd(h, 5)) * (species == SPECIES_GRASS ? 2.2 : 1.0);
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
        uint list = variant_of(species, h) * 3 + lod;
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
    // Тени деревьев — LOD 1; у пихты-скана (виды 0–1) — LOD 2: её LOD 1 —
    // слои увеличенных карточек хвои, дорогие в каскадах (02.13).
    uint lod = i < CAMERA_LISTS ? i % 3 : (i == PROPS_LIST ? 0u : (species < 2u ? 2u : 1u));
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
    // Инвариантна: предпроход глубины и затенение дают одну глубину (02.13).
    float4 position [[position, invariant]];
    float3 world;
    float3 rel;
    float3 normal;
    float2 uv;
    float3 attr;      // материал, ветер, затенение
    float tint;
    float kind;       // вид гриба (05.1)
    float moss;       // мох на коре и почве (атрибут A)
    float4 prev_clip; // вершина при ветре прошлого кадра (04.6)
    float4 cur_clip;  // та же вершина сейчас (дрожание камеры в разности сокращается)
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
                          PlantGround g, thread float3& n, float t) {
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
    // Точка покоя (до ветра): по ней — драпировка по рельефу, иначе на
    // склоне качающееся растение ходит вверх-вниз.
    float3 q_rest = q;
    float2 dir = normalize(float2(0.8, 0.6));
    // Порывы (02.10): сила ветра медленно меняется во времени и по месту —
    // бывают паузы; соседние растения качаются не в такт.
    float2 bp = float2(inst.position.x, inst.position.z);
    float gust = mix(0.15, 1.0, saturate(0.5 + 0.6 * sin(t * 0.21 + bp.x * 0.013) * cos(t * 0.13 + bp.y * 0.011)));
    // Ветер погоды (04.3, сессия 4): в штиль слабее, в грозу — вдвое сильнее.
    gust *= 0.6 + 1.6 * frame.weather.w;
    float sway = (sin(t * 0.9 + inst.phase) * 0.6 + sin(t * 2.3 + inst.phase * 1.7) * 0.25) * w * w * gust;
    // Дрожание вдоль нормали — только листве: у трубок ветвей нормали
    // радиальные, и ветви «дышали» бы (02.10).
    float mat = float(v.attr.x);
    // Листва сканов (02.11): материал ≥ 16 с силой ветра.
    bool leafy = (mat > 1.5 && mat < 3.5) || (mat > 4.5 && mat < 6.5) || (mat > 8.5 && mat < 9.5)
                 || (mat > 15.5 && w > 0.02);
    float fph = dot(q, float3(1.3, 0.7, 1.1)) + inst.phase * 3.0 + dot(bp, float2(0.37, 0.23));
    // Травинки не дрожат (02.13): узкие полоски от дрожания «пульсировали».
    // Низкие растения (папоротник, саса, сканы трав и кустов) — без фазы по
    // вершине: иначе карточка коробится («сжимается и растягивается»).
    bool blade = mat > 8.5 && mat < 9.5;
    bool low = inst._pad < 15.5 && (blade || (mat > 4.5 && mat < 6.5) || mat > 15.5);
    if (low) {
        fph = inst.phase * 3.0 + dot(bp, float2(0.37, 0.23));
    }
    float flutter = leafy && !blade
                  ? sin(t * (5.0 + 3.0 * fract(inst.phase)) + fph) * 0.035 * w * gust * gust
                    * (low ? 0.4 * saturate(q.y / 0.8) : 1.0)
                  : 0.0;
    // Раскачка растёт с высотой точки: верх кроны качается, папоротник — едва.
    // Трава — мягкая волна по месту (у соседних пучков почти в фазе), а не
    // своя частота у каждого пучка.
    if (low) {
        // Изгиб, а не сдвиг: верх уходит вбок по квадрату высоты и
        // опускается так, что длина стебля сохраняется. Волна — по месту.
        float wave = sin(t * 1.3 - dot(bp, dir) * 0.35) * 0.6 + sin(t * 0.7 - dot(bp, dir) * 0.15) * 0.4;
        float lean = (blade ? 0.35 + 0.65 * wave : 0.6 * wave + 0.4 * sway) * w * w * gust;
        float hy = max(q.y, 0.0);
        float hn = saturate(hy / max(inst.scale * (blade ? 0.5 : 1.0), 0.05));
        float off = lean * (blade ? 0.12 : 0.08) * hn * hn;
        q.xz += dir * off;
        q.y -= 0.5 * off * off / max(hy, 0.05);
    } else {
        q.xz += dir * sway * inst.scale * 0.9 * saturate(q.y / 8.0);
    }
    q += n * flutter;
    float3 world = float3(inst.position) + q;
    // Драпировка (02.10): нижние ~0,6 м сетки — сор, листья, мох, бугор у
    // корней, основание ствола — повторяют рельеф под каждой вершиной, а не
    // плоскость основания; нормаль доворачивается по склону.
    float wd = 1.0 - smoothstep(0.1, 0.7, q.y);
    if (wd > 0.0) {
        float3 base = float3(inst.position);
        float sink = max(base.y - plant_ground_h(g, base.xz, frame), -0.08);
        float2 rest = float3(inst.position).xz + q_rest.xz;
        float h0 = plant_ground_h(g, rest, frame);
        float hx = plant_ground_h(g, rest + float2(0.5, 0), frame);
        float hz = plant_ground_h(g, rest + float2(0, 0.5), frame);
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
    PlantGround g{heights, zone_heights, zone};
    o.world = plant_world(v, inst, frame, g, n, frame.time);
    // Ветер прошлого кадра (04.6, сессия 2): та же вершина в момент
    // `time − cam_lo.w`, текущей матрицей — сдвиг на экране только от ветра
    // (движение камеры MetalFX получает по глубине).
    float3 np;
    float3 prev_world = plant_world(v, inst, frame, g, np, frame.time - frame.cam_lo.w);
    float4 pc = frame.view_proj_rel * float4(camera_relative(frame, prev_world), 1.0);
    o.rel = camera_relative(frame, o.world);
    o.position = frame.view_proj_rel * float4(o.rel, 1.0);
    o.prev_clip = pc;
    o.cur_clip = o.position;
    if (v.attr.x == 9) {
        // Травинки (02.13): нормаль пучка — полусфера от его центра, почти
        // вверх; пучок освещается мягкой массой, а не бликующими полосками.
        float2 r = o.world.xz - float2(inst.position.x, inst.position.z);
        float rl = length(r);
        float2 rd = rl > 1e-4 ? r / rl : float2(0.0);
        n = normalize(float3(rd.x * 0.45, 1.0, rd.y * 0.45) + n * 0.15);
    }
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

// Затенение растений. `alpha_test` = false — после предпрохода глубины
// (02.13): пиксели вне альфы отброшены сравнением глубины на равенство, а
// шейдер без `discard` не ломает удаление скрытых поверхностей GPU Apple —
// каждый пиксель листвы затеняется один раз, а не по слою на карточку.
static float4 plant_shade(PlantOut in, bool front, bool alpha_test, constant FrameUniforms& frame,
                          device const SkyFrame& skyf, constant ShadowUniforms& shadows,
                          texture2d_array<float> albedo_tex, depth2d_array<float> shadow_map,
                          texture3d<float> aerial, texture2d_array<float> mat_albedo,
                          texture2d_array<float> bark_normal, texture2d<float> rt_ao_tex,
                          texture2d_array<float> scan_albedo, texture2d_array<float> scan_normal,
                          device const float4* probes) {
    float mat = round(in.attr.x);
    float3 n = normalize(in.normal);
    constexpr sampler ms(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
    float3 albedo;
    // 2 хвоя, 3 листья, 5 папоротник, 6 якудзаса — листва; 4 — камень.
    bool leaf = (mat > 1.5 && mat < 3.5) || (mat > 4.5 && mat < 6.5) || (mat > 8.5 && mat < 9.5)
                || (mat > 15.5 && in.attr.y > 0.02);
    float3 glow = 0.0;
    // Доля высоты травы (0 — земля, 1 — верх): в канале затенения 0,55 → 1.
    float blade_t = saturate((in.attr.z - 0.55) / 0.45);
    if (mat > 8.5 && mat < 9.5) {
        // Трава (02.13): фото Poly Haven — травинки (слой 3) и кустики
        // (слой 4, u + 1), альфа заострена по производным. Цвет — пятнами по
        // месту (зелёные, оливковые, сухие куртины 3–12 м); у основания —
        // цвет травяного слоя рельефа: трава врастает в землю.
        float t = blade_t;
        bool tuft = in.uv.x >= 1.0;
        float2 guv = float2(in.uv.x - (tuft ? 1.0 : 0.0), in.uv.y);
        constexpr sampler gs(filter::linear, mip_filter::linear, address::clamp_to_edge, max_anisotropy(8));
        float4 ph = albedo_tex.sample(gs, guv, tuft ? 4u : 3u);
        float aw = max(fwidth(ph.a), 1e-4);
        if (saturate((ph.a - 0.5) / aw + 0.5) < 0.5) {
            if (alpha_test) {
                discard_fragment();
            }
        }
        float3 lp = in.rel + frame.cam_lo.xyz;
        float big = patch_noise(lp.xz, 12.0, 0x6C01u);
        float mid = patch_noise(lp.xz, 3.5, 0x6C02u);
        float dryk = saturate(smoothstep(0.6, 0.95, big) * 0.6 + (mid - 0.5) * 0.35 + (in.tint - 0.5) * 0.2);
        float olive = smoothstep(0.3, 0.7, mid);
        float3 photo = ph.rgb * mix(float3(1.0), float3(1.05, 0.95, 0.8), olive) * (0.85 + 0.25 * in.tint);
        float3 dry = dot(ph.rgb, float3(0.3, 0.55, 0.15)) * float3(1.05, 0.9, 0.55);
        // Фото и так содержит сухие травинки — их не осветляем.
        float3 tip = min(mix(photo, dry, dryk * 0.6), float3(0.45, 0.42, 0.3));
        float3 ground = mat_albedo.sample(ms, lp.xz / 2.0, 5).rgb;
        ground = mix(float3(dot(ground, float3(0.3, 0.5, 0.2))), ground, 0.55) * float3(0.78, 0.76, 0.66);
        albedo = mix(ground * 0.85, tip, smoothstep(0.0, 0.35, t));
    } else if (mat > 15.5) {
        // Фотоскан (02.11): свой цвет и нормаль (слой = материал − 16), базис
        // нормали — по производным, как у коры (AD-66).
        uint layer = uint(mat - 16.0);
        float4 sa = scan_albedo.sample(ms, in.uv, layer);
        // Альфа-маска листвы и травы (непрозрачные сканы — 1), заострённая
        // по производным, как у карточек.
        float aw = max(fwidth(sa.a), 1e-4);
        if (saturate((sa.a - 0.5) / aw + 0.5) < 0.5) {
            if (alpha_test) {
                discard_fragment();
            }
        }
        albedo = sa.rgb;
        float3 dp1 = dfdx(in.rel), dp2 = dfdy(in.rel);
        float2 du1 = dfdx(in.uv), du2 = dfdy(in.uv);
        float3 p2 = cross(dp2, n), p1 = cross(n, dp1);
        float3 T = p2 * du1.x + p1 * du2.x;
        float3 B = p2 * du1.y + p1 * du2.y;
        float inv = rsqrt(max(max(dot(T, T), dot(B, B)), 1e-20));
        float3 tn = scan_normal.sample(ms, in.uv, layer).xyz * 2.0 - 1.0;
        n = normalize(T * inv * tn.x + B * inv * tn.y + n * tn.z);
        // Немного разницы между экземплярами — как у процедурной листвы.
        albedo *= 0.85 + 0.3 * in.tint;
    } else if (mat > 7.5) {
        // Почва бугра у корней и корневой тарелки (02.4): подстилка леса и мох
        // в мировых координатах, как на рельефе, — бугор сливается с землёй.
        float3 lp = in.rel + frame.cam_lo.xyz;
        float3 litter = mat_albedo.sample(ms, lp.xz / 3.0, 4).rgb;
        float3 mossc = mat_albedo.sample(ms, lp.xz / 2.0, 3).rgb;
        float side = 1.0 - saturate(abs(n.x) > abs(n.z) ? abs(n.x) : abs(n.z));
        float3 soil = mat_albedo.sample(ms, (abs(n.x) > abs(n.z) ? lp.zy : lp.xy) / 2.5, 2).rgb;
        // Почва бугра темнее подстилки рельефа: влажная, в тени ствола (11).
        albedo = mix(soil, litter, saturate(side * 1.5)) * 0.6;
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
            if (alpha_test) {
                discard_fragment();
            }
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
            albedo *= mat < 0.5 ? float3(0.85, 0.80, 0.78)  // кора якусуги — скан японской криптомерии (02.11), свой цвет
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
    // Дождь (04.3): мокрая кора и камни темнее, листва — чуть.
    if (frame.weather.z > 0.0) {
        albedo *= 1.0 - (leaf ? 0.12 : 0.35) * saturate(frame.weather.z);
    }
    // У травинок нормаль пучка общая для обеих сторон (полусфера вверх).
    if (!front && !(mat > 8.5 && mat < 9.5)) {
        n = -n;
    }
    float3 L = -frame.light_dir.xyz;
    float3 V = normalize(-in.rel);
    float nl = leaf ? saturate(dot(n, L) * 0.6 + 0.4) : saturate(dot(n, L));
    float shadow = shadow_factor(shadows, shadow_map, in.rel, n, L) * cloud_shadow(frame, in.world, L);
    float3 sun = sky_sun_term(skyf) * shadow;
    float ao = in.attr.z;
    if (mat > 8.5 && mat < 9.5) {
        // Затенение в толще травы: низ пучка тёмный, кончики открыты небу.
        ao = mix(0.55, 1.0, pow(blade_t, 0.7));
    }
    float3 sky = sky_ambient_term(skyf, n) * ao;
    // Отражённый свет от освещённой земли и крон (как у рельефа, 02.10).
    float3 bounce = sky_sun_term(skyf) * max(L.y, 0.0) * 0.24 * float3(0.75, 0.85, 0.6);
    // Пробы освещённости (11, сессия 3) вместо неба и отражения.
    if (shadows.params.y > 0.5) {
        float3 pe = probe_irradiance(probes, frame, in.world, n);
        if (pe.x >= 0.0) {
            sky = pe * ao;
            bounce = 0.0;
        }
    }
    // Затенение лучами (11) — кора, камни, валежник (листва — карточки, её
    // свет уже приглушён затенением внутри кроны).
    if (shadows.params.z > 0.5 && !leaf) {
        constexpr sampler aos(filter::linear, address::clamp_to_edge);
        float rao = rt_ao_tex.sample(aos, in.position.xy / (2.0 * float2(rt_ao_tex.get_width(), rt_ao_tex.get_height()))).x;
        sky *= rao;
        bounce *= rao;
    }
    if (mat > 8.5 && mat < 9.5) {
        // Солнце в толще травы тоже гаснет к земле (без отдельной тени).
        sun *= mix(0.7, 1.0, blade_t);
    }
    float3 color = albedo * (sun * nl + sky + bounce * ao) + glow;
    if (leaf) {
        // Свет сквозь лист (11, сессия 2): тонкий лист пропускает ~40 %
        // света с обратной стороны (диффузно, по нормали — `sun` уже с тенью
        // от других листьев) и желтеет: свет проходит через мякоть листа.
        // Плюс узкий просвет против солнца, когда смотришь сквозь крону.
        float3 leaf_t = albedo * float3(1.05, 1.15, 0.55);
        float through = saturate(dot(-n, L));
        float back = pow(saturate(dot(-V, L)), 4.0);
        color += leaf_t * sun * (0.42 * through + 0.6 * back);
    }
    if (frame.underwater_m <= 0.0) {
        color = apply_aerial(aerial, frame, in.rel, color);
    }
    return float4(color * frame.pre_exposure, 1.0);
}

#define PLANT_FS_ARGS PlantOut in [[stage_in]], bool front [[front_facing]], \
    constant FrameUniforms& frame [[buffer(1)]], device const SkyFrame& skyf [[buffer(3)]], \
    constant ShadowUniforms& shadows [[buffer(4)]], texture2d_array<float> albedo_tex [[texture(0)]], \
    depth2d_array<float> shadow_map [[texture(1)]], texture3d<float> aerial [[texture(2)]], \
    texture2d_array<float> mat_albedo [[texture(3)]], texture2d_array<float> bark_normal [[texture(4)]], \
    texture2d<float> rt_ao_tex [[texture(5)]], texture2d_array<float> scan_albedo [[texture(6)]], \
    texture2d_array<float> scan_normal [[texture(7)]], device const float4* probes [[buffer(9)]]

// После предпрохода глубины: без `discard`, глубина — на равенство.
struct PlantColorOut {
    float4 color [[color(0)]];
    float2 wind [[color(1)]];  // сдвиг к прошлому кадру от ветра, uv (04.6)
};

fragment PlantColorOut plant_fs_prepassed(PLANT_FS_ARGS) {
    PlantColorOut o;
    o.color = plant_shade(in, front, false, frame, skyf, shadows, albedo_tex, shadow_map, aerial, mat_albedo,
                          bark_normal, rt_ao_tex, scan_albedo, scan_normal, probes);
    float2 d = in.prev_clip.xy / max(in.prev_clip.w, 1e-6) - in.cur_clip.xy / max(in.cur_clip.w, 1e-6);
    o.wind = d * float2(0.5, -0.5);
    return o;
}

// Предпроход глубины растений (02.13): та же альфа, что в `plant_shade`
// (заострение по производным), — глубина пишется ровно там, где лист.
fragment void plant_depth_fs(PlantOut in [[stage_in]],
                             texture2d_array<float> albedo_tex [[texture(0)]],
                             texture2d_array<float> scan_albedo [[texture(6)]]) {
    float mat = round(in.attr.x);
    float alpha = 1.0;
    if (mat > 8.5 && mat < 9.5) {
        bool tuft = in.uv.x >= 1.0;
        float2 guv = float2(in.uv.x - (tuft ? 1.0 : 0.0), in.uv.y);
        constexpr sampler gs(filter::linear, mip_filter::linear, address::clamp_to_edge, max_anisotropy(8));
        float4 ph = albedo_tex.sample(gs, guv, tuft ? 4u : 3u);
        alpha = saturate((ph.a - 0.5) / max(fwidth(ph.a), 1e-4) + 0.5);
    } else if (mat > 15.5) {
        constexpr sampler ms(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
        float4 sa = scan_albedo.sample(ms, in.uv, uint(mat - 16.0));
        alpha = saturate((sa.a - 0.5) / max(fwidth(sa.a), 1e-4) + 0.5);
    } else if (!(mat > 3.5 && mat < 4.5) && !(mat > 6.5 && mat < 8.5)) {
        alpha = plant_albedo(albedo_tex, mat, in.uv).a;
    }
    if (alpha < 0.5) {
        discard_fragment();
    }
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
    float3 world = plant_world(v, insts[iid], frame, PlantGround{heights, zone_heights, zone}, n, frame.time);
    PlantShadowOut o;
    o.position = cascade_vp * float4(camera_relative(frame, world), 1.0);
    o.uv = float2(v.uv);
    o.mat = float(v.attr.x);
    return o;
}

fragment void plant_shadow_fs(PlantShadowOut in [[stage_in]],
                              texture2d_array<float> albedo_tex [[texture(0)]],
                              texture2d_array<float> scan_albedo [[texture(2)]]) {
    float mat = round(in.mat);
    if (mat > 15.5) {
        // Сканы (02.11): отсечение по альфа-маске.
        constexpr sampler ss(filter::linear, mip_filter::linear, address::repeat);
        if (scan_albedo.sample(ss, in.uv, uint(mat - 16.0)).a < 0.5) {
            discard_fragment();
        }
        return;
    }
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

// --- Вода на земле (04.3, сессия 6) ------------------------------------------
// Сетка 256² клеток по 0,5 м вокруг камеры (128 м), кольцевая адресация, как у
// маски мокрого песка: тексель хранит клетку мира с номером ≡ gid (mod N).
// Дождь добавляет воду, почва впитывает; вода перетекает к соседям по
// разнице уровней «земля + вода» (поток антисимметричен — вода сохраняется,
// ограничен запасом источника) и собирается в ложбинах: лужи растут и
// сливаются, по склонам бегут ручейки. R — глубина, м; G — метка оборота
// окна; BA — поток (м/шаг по X и Z) для ряби течения.
constant int WATER_N = 256;
constant float WATER_CELL = 0.5;

static float water_micro(int2 cell) {
    // Микрорельеф ±2 см: лужи неровные, не по сетке поля высот.
    uint h = hash3(cell.x, cell.y, 0x57A7u);
    uint h2 = hash3(cell.x >> 2, cell.y >> 2, 0x57A8u);
    return (rnd(h, 1) - 0.5) * 0.012 + (rnd(h2, 2) - 0.5) * 0.03;
}

kernel void ground_water(texture2d<float, access::read> src [[texture(3)]],
                         texture2d<float, access::write> dst [[texture(4)]],
                         device const float* heights [[buffer(0)]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const float* zone_heights [[buffer(5)]],
                         constant TerrainZone& zone [[buffer(6)]],
                         uint2 gid [[thread_position_in_grid]]) {
    if (gid.x >= uint(WATER_N) || gid.y >= uint(WATER_N)) {
        return;
    }
    int n = WATER_N;
    int2 c0 = int2(floor(frame.camera_pos.xz / WATER_CELL)) - n / 2;
    auto cell_of = [&](int2 g) {
        int2 rel = (g - c0) % n;
        rel += select(int2(0), int2(n), rel < 0);
        return c0 + rel;
    };
    auto tag_of = [&](int2 cell, int2 g) {
        int2 lap = (cell - g) / n;
        return float((lap.x & 63) * 64 + (lap.y & 63) + 1);
    };
    int2 g = int2(gid);
    int2 cell = cell_of(g);
    float4 old = src.read(gid);
    float h = old.g == tag_of(cell, g) ? old.r : 0.0;
    float2 p = (float2(cell) + 0.5) * WATER_CELL;
    float Hi = ground_height(p, heights, zone_heights, zone, frame) + water_micro(cell) + h;
    // Шаг — реальное время (кадр делится на 4 подшага).
    float dt = frame.cam_lo.w * 0.25;
    float dh = 0.0;
    float2 flow = 0.0;
    const int2 offs[4] = {int2(1, 0), int2(-1, 0), int2(0, 1), int2(0, -1)};
    for (int k = 0; k < 4; ++k) {
        int2 nc = cell + offs[k];
        int2 rel = nc - c0;
        float hn = 0.0;
        if (all(rel >= 0) && all(rel < n)) {
            int2 ng = ((nc % n) + n) % n;
            float4 o = src.read(uint2(ng));
            hn = o.g == tag_of(nc, ng) ? o.r : 0.0;
        }
        float2 pn = (float2(nc) + 0.5) * WATER_CELL;
        float Hn = ground_height(pn, heights, zone_heights, zone, frame) + water_micro(nc) + hn;
        // Поток i → n: 0,2 разницы уровней, не больше четверти запаса источника.
        float f = 0.2 * (Hi - Hn);
        f = f > 0.0 ? min(f, h * 0.25) : max(f, -hn * 0.25);
        dh -= f;
        flow += float2(offs[k]) * f;
    }
    // Дождь (ливень ~1 мм за 3 с реального времени — с ускорением суток) и
    // впитывание почвой (лужа уходит за минуты после дождя).
    dh += frame.weather.y * 3.5e-4 * dt;
    h = max(h + dh - 4e-5 * dt, 0.0);
    dst.write(float4(h, tag_of(cell, g), flow), gid);
}
