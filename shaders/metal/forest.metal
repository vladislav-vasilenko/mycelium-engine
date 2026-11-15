#include "common.h"
#include "features.h"
#include "sky.h"

// Лес (02.4 + 02.8). Расстановка и отсечение — на GPU каждый кадр: сетка
// клеток FOREST_CELL_M вокруг камеры, в клетке — кандидат на детерминированном
// смещении (хэш номера клетки мира), отбор по поясам, уклону, тропам, рекам,
// морю. Видимые экземпляры идут в списки (вид × LOD), рядом с камерой — ещё и в
// списки теней; рисование — по indirect-аргументам.

constant float FOREST_CELL_M = 6.0;
constant int FOREST_GRID = 160;               // клеток по стороне: радиус ~480 м
constant uint FOREST_LISTS = 18;              // 5 видов × 3 LOD + тени двух деревьев + грибы с CPU
constant uint CAMERA_LISTS = 15;
constant uint COUNTS_WORDS = 32;               // счётчики, затем indirect-аргументы
constant uint FOREST_CAP = 16384;             // экземпляров в списке
constant uint SPECIES_YAKUSUGI = 0;
constant uint SPECIES_BROADLEAF = 1;
constant uint SPECIES_BOULDER = 2;
constant uint SPECIES_FERN = 3;
constant uint SPECIES_SASA = 4;
// Высота вида при масштабе 1, м (сфера отсечения).
constant float SPECIES_H[5] = {29.0, 17.0, 2.2, 1.0, 1.4};
// Подлесок: подклетки 3 м до UNDER_DIST, LOD на 15 и 35 м.
constant float UNDER_DIST = 70.0;
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
    // Два кандидата в клетке: дерево и валун.
    for (uint kind = 0; kind < 2; ++kind) {
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
            species = rnd(h, 4) < cedar ? SPECIES_YAKUSUGI : SPECIES_BROADLEAF;
            // Редкие великаны в естественном кедровом лесу (масштаб от 0,7).
            bool giant = species == SPECIES_YAKUSUGI && srange.x >= 0.7 && rnd(h, 6) < 0.04;
            scale = giant ? 1.6 : mix(srange.x, srange.y, rnd(h, 5));
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
        float clear = kind == 0 ? 2.5 * scale : 1.0;
        if (fd.trail < clear || fd.road < clear + 1.5 || (kind == 0 && fd.river < 2.0)) {
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
        float lim = species == SPECIES_BOULDER ? 180.0 : 480.0;
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
    // Подлесок (02.4): 4 подклетки по 3 м в клетках у камеры — папоротники во
    // влажных ложбинах леса, якудзаса в высокогорье и на лугах.
    float2 cc = (float2(cell) + 0.5) * FOREST_CELL_M;
    if (distance(cc, frame.camera_pos.xz) > UNDER_DIST + 6.0) {
        return;
    }
    constexpr sampler ls2(filter::linear, address::clamp_to_edge);
    for (uint sub = 0; sub < 4; ++sub) {
        uint h = hash3(cell.x * 2 + int(sub & 1), cell.y * 2 + int(sub >> 1), 0x0DE2u);
        float2 p = (float2(cell) + 0.5 * float2(sub & 1, sub >> 1) + 0.05 + 0.4 * float2(rnd(h, 1), rnd(h, 2)))
                 * FOREST_CELL_M;
        float dxz = distance(p, frame.camera_pos.xz);
        if (dxz > UNDER_DIST || any(p < 0.0) || any(p > frame.map_size_m)) {
            continue;
        }
        float cn = float(cover.get_width());
        float4 cv = cover.sample(ls2, (p / frame.map_size_m * (cn - 1.0) + 0.5) / cn, level(0));
        float mn = float(macro.get_width());
        float4 mc = macro.sample(ls2, (p / frame.map_size_m * (mn - 1.0) + 0.5) / mn, level(0));
        float hollow = saturate((0.5 - mc.g) * 2.5);
        float fern_p = (cv.x + cv.y) * (0.25 + 0.5 * mc.b + 0.35 * hollow);
        float sasa_p = cv.z * 0.75 + (cv.x + cv.y) * 0.04;
        float r = rnd(h, 3);
        uint species;
        if (r < fern_p) {
            species = SPECIES_FERN;
        } else if (r < fern_p + sasa_p) {
            species = SPECIES_SASA;
        } else {
            continue;
        }
        float y = ground_height(p, heights, zone_heights, zone, frame);
        if (y < 2.0) {
            continue;
        }
        FeatureDist fd = feature_distances(p, frame.map_size_m, feat_segs, feat_cells, feat_list);
        if (fd.trail < 1.0 || fd.road < 2.0 || fd.river < 0.5) {
            continue;
        }
        float scale = 0.7 + 0.6 * rnd(h, 5);
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
        uint lod = dxz < 15.0 ? 0 : (dxz < 35.0 ? 1 : 2);
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
    uint species = i < CAMERA_LISTS ? i / 3 : (i == 17 ? 5u : i - CAMERA_LISTS);
    uint lod = i < CAMERA_LISTS ? i % 3 : (i == 17 ? 0u : 1u);
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
};

// Мир точки сетки растения: поворот, масштаб, ветер (по силе из атрибута).
static float3 plant_world(PlantVertex v, PlantInstance inst, constant FrameUniforms& frame,
                          thread float3& n) {
    float c = cos(inst.yaw), s = sin(inst.yaw);
    float3 p = float3(v.position);
    float3 q = float3(c * p.x - s * p.z, p.y, s * p.x + c * p.z) * inst.scale;
    float3 nn = float3(v.normal);
    n = float3(c * nn.x - s * nn.z, nn.y, s * nn.x + c * nn.z);
    float w = float(v.attr.y) / 255.0;
    float t = frame.time;
    float2 dir = normalize(float2(0.8, 0.6));
    float sway = (sin(t * 0.9 + inst.phase) * 0.6 + sin(t * 2.3 + inst.phase * 1.7) * 0.25) * w * w;
    float flutter = sin(t * 7.0 + dot(q, float3(1.3, 0.7, 1.1))) * 0.05 * w;
    // Раскачка растёт с высотой точки: верх кроны качается, папоротник — едва.
    q.xz += dir * sway * inst.scale * 0.9 * saturate(q.y / 8.0);
    q += n * flutter;
    return float3(inst.position) + q;
}

vertex PlantOut plant_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                         device const PlantVertex* verts [[buffer(0)]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const PlantInstance* insts [[buffer(2)]]) {
    PlantVertex v = verts[vid];
    PlantInstance inst = insts[iid];
    PlantOut o;
    float3 n;
    o.world = plant_world(v, inst, frame, n);
    o.rel = camera_relative(frame, o.world);
    o.position = frame.view_proj_rel * float4(o.rel, 1.0);
    o.normal = n;
    o.uv = float2(v.uv);
    o.attr = float3(float(v.attr.x), float(v.attr.y) / 255.0, float(v.attr.z) / 255.0);
    o.tint = inst.tint;
    o.kind = inst._pad;
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
                         texture2d_array<float> mat_albedo [[texture(3)]]) {
    float mat = round(in.attr.x);
    float3 n = normalize(in.normal);
    float3 albedo;
    // 2 хвоя, 3 листья, 5 папоротник, 6 якудзаса — листва; 4 — камень.
    bool leaf = (mat > 1.5 && mat < 3.5) || (mat > 4.5 && mat < 6.5);
    float3 glow = 0.0;
    if (mat > 6.5) {
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
        if (mat < 0.5) {
            albedo *= float3(0.62, 0.40, 0.30);  // кора якусуги — красно-бурая
        } else if (mat < 1.5) {
            albedo *= 0.35;                      // серая кора лиственных, не белая
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
    float3 color = albedo * (sun * nl + sky) + glow;
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
                                      constant float4x4& cascade_vp [[buffer(5)]]) {
    PlantVertex v = verts[vid];
    float3 n;
    float3 world = plant_world(v, insts[iid], frame, n);
    PlantShadowOut o;
    o.position = cascade_vp * float4(camera_relative(frame, world), 1.0);
    o.uv = float2(v.uv);
    o.mat = float(v.attr.x);
    return o;
}

fragment void plant_shadow_fs(PlantShadowOut in [[stage_in]],
                              texture2d_array<float> albedo_tex [[texture(0)]]) {
    float mat = round(in.mat);
    if (((mat > 1.5 && mat < 3.5) || (mat > 4.5 && mat < 6.5)) && plant_albedo(albedo_tex, mat, in.uv).a < 0.5) {
        discard_fragment();
    }
}
