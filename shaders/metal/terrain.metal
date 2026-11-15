#include "common.h"
#include "features.h"
#include "sky.h"
#include "terrain_patch.h"

// Ландшафт LOD 0: все тайлы одним instanced draw. Номер экземпляра — номер
// тайла, вершины тайла лежат подряд по TILE_SAMPLES² штук.
//
// Материалы (подэтап 02.2): массивы фотосканов ambientCG, смешивание по весам
// биомов, субстрату, уклону, типу берега и высоте рельефа материала; мокрый
// песок по маске влажности; отладочные виды по FrameUniforms.view_mode.

struct TerrainVaryings {
    float4 position [[position]];
    float3 world;
    // Позиция относительно камеры: малые числа, точные производные даже в 15 км
    // от начала координат (у мировой позиции шаг f32 там ~2 мм).
    float3 rel;
    float3 normal;
    float4 weights;   // лес, торф, берег, тропа
    float4 material;  // субстрат (индекс), русло, влажность почвы, тип берега
    float4 cover;     // пояса (02.7): низинный лес, кедровый лес, якудзаса, гранит
    uint cluster [[flat]];  // номер кластера (02.3), 0 — тайлы уровня 0
    uint depth [[flat]];    // уровень иерархии кластера
};

vertex TerrainVaryings terrain_vs(uint vid [[vertex_id]],
                                  uint iid [[instance_id]],
                                  const device TerrainVertex* vertices [[buffer(0)]],
                                  constant FrameUniforms& frame [[buffer(1)]]) {
    TerrainVertex v = vertices[iid * TILE_SAMPLES * TILE_SAMPLES + vid];
    TerrainVaryings out;
    out.world = float3(v.position);
    out.rel = camera_relative(frame, out.world);
    out.position = frame.view_proj_rel * float4(out.rel, 1.0);
    out.normal = float3(v.normal);
    out.weights = float4(v.weights) / 255.0;
    out.material = float4(float(v.material.x), float(v.material.y) / 255.0,
                          float(v.material.z) / 255.0, float(v.material.w) / 255.0);
    out.cover = 0.0;
    out.cluster = 0;
    out.depth = 0;
    return out;
}

// Позиция и нормаль узла кластера: узел поля острова или игровой зоны (02.7,
// шаг 5); нормаль — центральные разности по своей сетке.
static float3 node_position(uint node, device const float* heights, device const float* zone_heights,
                            constant TerrainZone& zone, constant FrameUniforms& frame,
                            thread float3& normal) {
    float3 world;
    float dx, dz;
    if (node >= zone.far_count) {
        uint k = node - zone.far_count;
        int zn = int(zone.n);
        int a = int(k % zone.n);
        int b = int(k / zone.n);
        auto zh = [&](int x, int y) {
            x = clamp(x, 0, zn - 1);
            y = clamp(y, 0, zn - 1);
            return zone_heights[uint(y) * zone.n + uint(x)];
        };
        float s = zone.step_m;
        world = float3(zone.origin_x + float(a) * s, zone_heights[k], zone.origin_z + float(b) * s);
        dx = (zh(a + 1, b) - zh(a - 1, b)) / (2.0 * s);
        dz = (zh(a, b + 1) - zh(a, b - 1)) / (2.0 * s);
    } else {
        uint nx = frame.grid_nx;
        int i = int(node % nx);
        int j = int(node / nx);
        int nz = int(frame.map_size_m / frame.grid_step_m + 0.5) + 1;
        auto h = [&](int a, int b) {
            a = clamp(a, 0, int(nx) - 1);
            b = clamp(b, 0, nz - 1);
            return heights[uint(b) * nx + uint(a)];
        };
        float s = frame.grid_step_m;
        world = float3(float(i) * s, heights[node], float(j) * s);
        dx = (h(i + 1, j) - h(i - 1, j)) / (2.0 * s);
        dz = (h(i, j + 1) - h(i, j - 1)) / (2.0 * s);
    }
    normal = normalize(float3(-dx, 1.0, -dz));
    return world;
}

// Тени (04.2): глубина кластеров в каскад карты теней. Экземпляр — кластер из
// списка каскада; матрица каскада — относительно камеры, как у кадра.
vertex float4 terrain_shadow_vs(uint vid [[vertex_id]],
                                uint iid [[instance_id]],
                                device const float* heights [[buffer(0)]],
                                constant FrameUniforms& frame [[buffer(1)]],
                                device const TerrainCluster* clusters [[buffer(5)]],
                                device const uint* cluster_vertices [[buffer(6)]],
                                device const uint* cluster_triangles [[buffer(7)]],
                                device const uint* visible [[buffer(8)]],
                                device const float* zone_heights [[buffer(10)]],
                                constant TerrainZone& zone [[buffer(11)]],
                                constant float4x4& cascade_vp [[buffer(12)]]) {
    TerrainCluster c = clusters[visible[iid]];
    uint tri = vid / 3;
    if (tri >= c.triangle_count) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    uint packed = cluster_triangles[c.triangle_offset + tri];
    uint node = cluster_vertices[c.vertex_offset + ((packed >> (8 * (vid % 3))) & 0xff)];
    float3 nrm;
    float3 world = node_position(node, heights, zone_heights, zone, frame, nrm);
    return cascade_vp * float4(camera_relative(frame, world), 1.0);
}

// Позиция, нормаль и слои мира в вершине рельефа (кластеры и заплатка).
static void terrain_layers(thread TerrainVaryings& out, float3 world, float3 nrm,
                           constant FrameUniforms& frame, texture2d<float> layer_weights,
                           texture2d<float> layer_material, texture2d<float> layer_cover) {
    out.world = world;
    out.rel = camera_relative(frame, world);
    out.position = frame.view_proj_rel * float4(out.rel, 1.0);
    out.normal = nrm;
    // Слои мира: тексель на узел сетки биомов, центр текселя — узел.
    constexpr sampler ls(filter::linear, address::clamp_to_edge);
    float2 uv = (world.xz / frame.map_size_m * float(layer_weights.get_width() - 1) + 0.5)
                / float(layer_weights.get_width());
    out.weights = layer_weights.sample(ls, uv, level(0));
    float4 m = layer_material.sample(ls, uv, level(0));
    // Субстрат — индекс: берём ближайший узел, как CPU.
    uint w = layer_material.get_width();
    uint2 near = uint2(clamp(world.xz / frame.map_size_m * float(w - 1) + 0.5, 0.0, float(w - 1)));
    out.material = float4(round(layer_material.read(near).x * 255.0), m.y, m.z, m.w);
    float cn = float(layer_cover.get_width());
    out.cover = layer_cover.sample(ls, (world.xz / frame.map_size_m * (cn - 1.0) + 0.5) / cn, level(0));
}

// Кластерный рельеф (02.3): экземпляр — видимый кластер, вершина — номер узла
// поля высот. Позиция и нормаль — из поля (нормаль центральными разностями,
// как HeightField::normal_at), веса и материал — из слоёв мира по XZ.
vertex TerrainVaryings terrain_cluster_vs(uint vid [[vertex_id]],
                                          uint iid [[instance_id]],
                                          device const float* heights [[buffer(0)]],
                                          constant FrameUniforms& frame [[buffer(1)]],
                                          device const TerrainCluster* clusters [[buffer(5)]],
                                          device const uint* cluster_vertices [[buffer(6)]],
                                          device const uint* cluster_triangles [[buffer(7)]],
                                          device const uint* visible [[buffer(8)]],
                                          texture2d<float> layer_weights [[texture(4)]],
                                          texture2d<float> layer_material [[texture(5)]],
                                          texture2d<float> layer_cover [[texture(6)]],
                                          device const float* zone_heights [[buffer(10)]],
                                          constant TerrainZone& zone [[buffer(11)]]) {
    uint cid = visible[iid];
    TerrainCluster c = clusters[cid];
    uint tri = vid / 3;
    TerrainVaryings out;
    out.cluster = cid;
    out.depth = c.depth;
    if (tri >= c.triangle_count) {
        // Лишние треугольники экземпляра схлопываются в точку и не растеризуются.
        out.position = float4(0.0, 0.0, 0.0, 1.0);
        out.world = 0.0;
        out.rel = 0.0;
        out.normal = float3(0, 1, 0);
        out.weights = 0.0;
        out.material = 0.0;
        out.cover = 0.0;
        return out;
    }
    uint packed = cluster_triangles[c.triangle_offset + tri];
    uint local = (packed >> (8 * (vid % 3))) & 0xff;
    uint node = cluster_vertices[c.vertex_offset + local];
    float3 nrm;
    float3 world = node_position(node, heights, zone_heights, zone, frame, nrm);
    // Под детальной заплаткой (02.10) кластеры уходят под землю.
    if (patch_active(frame, zone) && patch_edge_dist(world.xz, frame) > PATCH_HIDE_M) {
        world.y -= PATCH_SINK_M;
    }
    terrain_layers(out, world, nrm, frame, layer_weights, layer_material, layer_cover);
    return out;
}

// Детальная заплатка у камеры (02.10, сессия 2): сетка PATCH_CELLS² по
// PATCH_STEP_M, вершина — угол ячейки по номеру; вне игрового режима или
// вне зоны все вершины схлопываются.
vertex TerrainVaryings terrain_patch_vs(uint vid [[vertex_id]],
                                        constant FrameUniforms& frame [[buffer(1)]],
                                        texture2d<float> layer_weights [[texture(4)]],
                                        texture2d<float> layer_material [[texture(5)]],
                                        texture2d<float> layer_cover [[texture(6)]],
                                        device const float* zone_heights [[buffer(10)]],
                                        constant TerrainZone& zone [[buffer(11)]]) {
    TerrainVaryings out;
    out.cluster = PATCH_CLUSTER;
    out.depth = 0;
    if (!patch_active(frame, zone)) {
        out.position = float4(0.0, 0.0, 0.0, 1.0);
        out.world = 0.0;
        out.rel = 0.0;
        out.normal = float3(0, 1, 0);
        out.weights = 0.0;
        out.material = 0.0;
        out.cover = 0.0;
        return out;
    }
    uint cell = vid / 6;
    const uint2 corner[6] = {uint2(0, 0), uint2(1, 1), uint2(1, 0), uint2(0, 0), uint2(0, 1), uint2(1, 1)};
    uint2 ij = uint2(cell % uint(PATCH_CELLS), cell / uint(PATCH_CELLS)) + corner[vid % 6];
    float2 p = patch_origin(frame) + float2(ij) * PATCH_STEP_M;
    PatchZone g = {zone_heights, zone};
    float3 world = float3(p.x, patch_height(g, p, frame), p.y);
    terrain_layers(out, world, patch_normal(g, p, frame), frame, layer_weights, layer_material, layer_cover);
    return out;
}


// Слои массивов: src/terrain_materials.rs, LAYERS.
constant int MAT_ROCK = 0;
constant int MAT_SAND = 1;
constant int MAT_SOIL = 2;
constant int MAT_MOSS = 3;
constant int MAT_FOREST = 4;
constant int MAT_GRASS = 5;
// Переходные слои: тёмный опад, листовой опад, щебень, мшистый камень.
constant int MAT_LITTER = 6;
constant int MAT_LEAVES = 7;
constant int MAT_GRAVEL = 8;
constant int MAT_MOSSROCK = 9;
constant int MAT_COUNT = 10;
// Размер повтора слоя на земле, м.
constant float MAT_TILE_M[MAT_COUNT] = {4.0, 3.0, 2.5, 2.0, 3.0, 2.0, 2.5, 2.5, 2.0, 3.0};
// Насколько слой темнеет и блестит от влаги (песок впитывает и блестит сильнее камня).
constant float MAT_POROSITY[MAT_COUNT] = {0.25, 1.0, 0.7, 0.4, 0.5, 0.4, 0.5, 0.5, 0.6, 0.3};
// Цвета слоёв в отладочном виде «слои материалов».
constant float3 MAT_DEBUG[MAT_COUNT] = {
    float3(0.45, 0.45, 0.48), float3(0.95, 0.85, 0.45), float3(0.45, 0.28, 0.12),
    float3(0.15, 0.65, 0.15), float3(0.45, 0.55, 0.15), float3(0.6, 0.9, 0.3),
    float3(0.25, 0.18, 0.10), float3(0.75, 0.45, 0.15), float3(0.8, 0.8, 0.7),
    float3(0.3, 0.5, 0.35)};

// Виды: src/terrain_materials.rs, TerrainView.
constant uint VIEW_LIT = 0;
constant uint VIEW_HEIGHT = 1;
constant uint VIEW_SLOPE = 2;
constant uint VIEW_BIOME = 3;
constant uint VIEW_SUBSTRATE = 4;
constant uint VIEW_COAST = 5;
constant uint VIEW_WETNESS = 6;
constant uint VIEW_MATERIALS = 7;
constant uint VIEW_CLUSTERS = 11;
constant uint VIEW_LEVELS = 12;
constant uint VIEW_MACRO = 14;
constant uint VIEW_COVER = 13;
constant uint VIEW_RAW = 15;

struct MatSample {
    float3 albedo;
    float height;
    float2 nxy;   // нормаль в касательном пространстве, [-1, 1]
    float rough;
    float ao;     // затенение материала (щели между камешками)
};

// Глубина рельефа слоя для параллакса, м (02.2, детализация вблизи).
constant float MAT_DEPTH_M[MAT_COUNT] = {0.06, 0.015, 0.03, 0.04, 0.035, 0.05, 0.03, 0.03, 0.03, 0.06};

// Вторая выборка против повторов: повёрнутые и растянутые координаты.
inline float2 rot_uv(float2 uv) {
    return float2(0.8 * uv.x - 0.6 * uv.y, 0.6 * uv.x + 0.8 * uv.y) * 0.43 + 0.37;
}
inline float2 rot_grad(float2 g) {
    return float2(0.8 * g.x - 0.6 * g.y, 0.6 * g.x + 0.8 * g.y) * 0.43;
}

// Выборка слоя с подавлением повторов; градиенты — явно: смещение параллакса
// разрывно на краях выступов, и неявные производные дали бы искры мипов.
inline MatSample sample_layer_uv(texture2d_array<float> albedo, texture2d_array<float> normal,
                                 sampler s, int layer, float2 uv, float2 gx, float2 gy,
                                 float mixk) {
    float2 uv2 = rot_uv(uv);
    gradient2d g1(gx, gy), g2(rot_grad(gx), rot_grad(gy));
    float4 a = mix(albedo.sample(s, uv, layer, g1), albedo.sample(s, uv2, layer, g2), mixk);
    float4 n = mix(normal.sample(s, uv, layer, g1), normal.sample(s, uv2, layer, g2), mixk);
    MatSample m;
    m.albedo = a.rgb;
    m.height = a.a;
    m.nxy = n.rg * 2.0 - 1.0;
    m.rough = n.b;
    m.ao = n.a;
    return m;
}

// Высота слоя (0 — дно, 1 — верх) для шагов параллакса.
inline float layer_height(texture2d_array<float> albedo, sampler s, int layer, float2 uv,
                          float2 gx, float2 gy, float mixk) {
    return mix(albedo.sample(s, uv, layer, gradient2d(gx, gy)).a,
               albedo.sample(s, rot_uv(uv), layer, gradient2d(rot_grad(gx), rot_grad(gy))).a, mixk);
}

// Стены (02.9): три проекции по осям с весами |n|⁴ — текстура не тянется по
// склону. Нормаль карты берётся только из верхней проекции.
inline MatSample sample_layer_triplanar(texture2d_array<float> albedo, texture2d_array<float> normal,
                                        sampler s, int layer, constant FrameUniforms& frame,
                                        float3 local, float3 dlx, float3 dly, float3 n,
                                        float2 top_off, float mixk) {
    float p = MAT_TILE_M[layer];
    float3 o = fract(frame.cam_hi.xyz / p);
    float3 w = pow(abs(n), 4.0);
    w /= w.x + w.y + w.z;
    MatSample top = sample_layer_uv(albedo, normal, s, layer, o.xz + (local.xz + top_off) / p,
                                    dlx.xz / p, dly.xz / p, mixk);
    MatSample sx = sample_layer_uv(albedo, normal, s, layer, o.zy + local.zy / p, dlx.zy / p, dly.zy / p, mixk);
    MatSample sz = sample_layer_uv(albedo, normal, s, layer, o.xy + local.xy / p, dlx.xy / p, dly.xy / p, mixk);
    MatSample m;
    m.albedo = top.albedo * w.y + sx.albedo * w.x + sz.albedo * w.z;
    m.height = top.height * w.y + sx.height * w.x + sz.height * w.z;
    m.rough = top.rough * w.y + sx.rough * w.x + sz.rough * w.z;
    m.ao = top.ao * w.y + sx.ao * w.x + sz.ao * w.z;
    m.nxy = top.nxy * w.y;
    return m;
}

inline float3 ramp(float t, float3 a, float3 b, float3 c) {
    t = saturate(t);
    return t < 0.5 ? mix(a, b, t * 2.0) : mix(b, c, t * 2.0 - 1.0);
}

fragment float4 terrain_fs(TerrainVaryings in [[stage_in]],
                           constant FrameUniforms& frame [[buffer(1)]],
                           texture2d<float> detail [[texture(0)]],
                           texture2d_array<float> mat_albedo [[texture(1)]],
                           texture2d_array<float> mat_normal [[texture(2)]],
                           texture2d<float> wetness_mask [[texture(3)]],
                           device const FeatureSegment* feat_segs [[buffer(2)]],
                           device const uint2* feat_cells [[buffer(3)]],
                           device const uint* feat_list [[buffer(4)]],
                           device const SkyFrame& skyf [[buffer(9)]],
                           constant ShadowUniforms& shadows [[buffer(12)]],
                           depth2d_array<float> shadow_map [[texture(7)]],
                           texture3d<float> aerial [[texture(8)]],
                           texture2d<float> macro_tex [[texture(9)]]) {
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
    constexpr sampler clamp_s(filter::linear, address::clamp_to_edge);
    float2 xz = in.world.xz;
    // Узоры текстур — от начала cam_hi (AD-49): точные даже в 30 км от угла карты.
    float2 lxz = in.rel.xz + frame.cam_lo.xz;
    float3 n = normalize(in.normal);
    // Нормаль грани — как у треугольника LOD 0 в SampleGround.
    float3 face = normalize(cross(dfdy(in.rel), dfdx(in.rel)));
    face = face.y < 0.0 ? -face : face;
    float slope = 1.0 - n.y;
    float slope_deg = acos(clamp(face.y, -1.0, 1.0)) * (180.0 / M_PI_F);

    float rocky = in.material.w;
    // Тропы, дороги и реки уже шага сетки — попиксельно по точному расстоянию
    // до отрезков. `px` — метров на пиксель: край сглажен на пиксель при любой
    // дальности, а полоса уже пикселя гаснет по покрытию, а не мерцает.
    FeatureDist fd = feature_distances(xz, frame.map_size_m, feat_segs, feat_cells, feat_list);
    float px = max(length(dfdx(in.rel)), length(dfdy(in.rel)));
    float wobble = (detail.sample(s, pattern_uv(frame, lxz, 9.0)).g - 0.5) * 0.4;
    // Край тропы — не линия краски: полотно переходит в вытоптанную бровку
    // (почва пятнами по шуму) на ~1 м, край неровный.
    float edge_n = detail.sample(s, pattern_uv(frame, lxz, 3.7)).g;
    float trail_px = 1.0 - smoothstep(-px - 0.15, px + 0.55, fd.trail + wobble * 1.6);
    float trail_edge = (1.0 - smoothstep(0.2, 1.4, fd.trail + wobble)) * smoothstep(0.35, 0.75, edge_n);
    float road_px = 1.0 - smoothstep(-px - 0.1, px + 0.3, fd.road + wobble * 0.3);
    // Обочина дороги: щебень и опад пятнами на ~2 м за краем асфальта.
    float road_edge = (1.0 - smoothstep(0.2, 2.2, fd.road + wobble)) * (1.0 - road_px)
                    * smoothstep(0.3, 0.7, edge_n);
    // Камни берега русла и вода в русле (до 03.2 вода — только цвет).
    float bank = 1.0 - smoothstep(0.0, 1.5 + px, fd.river + wobble);
    float stream = 1.0 - smoothstep(-px, px, fd.river + 0.4);
    // Грубая маска тропы сетки биомов (30 м) — только у мира генератора; у
    // мира с поясами (Якусима) тропы — точные отрезки OSM, маска дала бы
    // полосу 30 м с прямыми краями.
    bool has_belts = (in.cover.x + in.cover.y + in.cover.z + in.cover.w) > 0.05;
    float trail = max(has_belts ? 0.0 : smoothstep(0.1, 0.6, in.weights.w), trail_px);
    float moist = in.material.z;
    // Маска непрерывная: smoothstep даёт гладкую границу мокрого песка.
    // Маска — окно 1 м на тексель вокруг камеры с кольцевой адресацией: мировые
    // XZ / сторону окна с повтором; к краю окна маска гаснет.
    constexpr sampler wrap_s(filter::linear, address::repeat);
    float wet_n = float(wetness_mask.get_width());
    float wet_edge = 1.0 - smoothstep(0.38 * wet_n, 0.48 * wet_n,
                                      max(abs(in.rel.x), abs(in.rel.z)));
    float wet_mask = smoothstep(0.1, 0.9, wetness_mask.sample(wrap_s, xz / wet_n).r) * wet_edge;
    // Крупная детализация (02.9): тексель — узел поля высот острова.
    float mn = float(macro_tex.get_width());
    float4 mc = macro_tex.sample(clamp_s, (xz / frame.map_size_m * (mn - 1.0) + 0.5) / mn);
    float sky_vis = mc.r;
    float curv = mc.g - 0.5;              // < 0 — впадина, > 0 — гребень
    float hollow = saturate(-curv * 2.5);
    float ridge = saturate(curv * 2.5);
    float damp = mc.b;
    float scree = mc.a;
    float north = saturate(-n.z * 1.5);   // склон к северу (ось z — на юг)

    if (frame.view_mode == VIEW_RAW) {
        float3 w = in.weights.xyz;
        float dominant = (w.x >= w.y && w.x >= w.z) ? 0.0 : (w.y >= w.z ? 1.0 : 2.0);
        // Высота относительно камеры: HDR — half, на высотах 1000–2000 м его шаг
        // 1 м, а вблизи камеры — сантиметры.
        return float4(in.world.y - frame.camera_pos.y, slope_deg, dominant, rocky);
    }

    // Доли слоёв до смешивания по высоте.
    float wl[MAT_COUNT];
    float forest_w = in.weights.x;
    float4 cv = in.cover;
    // Пояса из сетки 15 м интерполируются линейно — граница выходит прямой.
    // Шум на масштабах 20–30 м двигает её: край пятна неровный, как на местности.
    float4 bn = float4(detail.sample(s, pattern_uv(frame, lxz, 19.0)).r,
                       detail.sample(s, pattern_uv(frame, lxz, 27.0)).g,
                       detail.sample(s, pattern_uv(frame, lxz, 23.0)).r,
                       detail.sample(s, pattern_uv(frame, lxz, 31.0)).g);
    cv *= 0.45 + 1.1 * bn;
    float cv_sum = cv.x + cv.y + cv.z + cv.w;
    bool belts = cv_sum > 0.05;
    cv = belts ? cv / cv_sum : float4(0.0);
    if (belts) {
        // Пояса Якусимы (02.7): низинный лес — опад с пятнами мха, кедровый —
        // сплошной мох, якудзаса и луга — трава, голое — гранит в горах и
        // почва с гравием внизу.
        float high = smoothstep(800.0, 1300.0, in.world.y);
        wl[MAT_FOREST] = forest_w * (cv.x * 0.75 + cv.y * 0.45);
        wl[MAT_MOSS] = forest_w * (cv.x * 0.25 * (0.4 + moist) + cv.y * 0.55) + in.weights.y * 0.6;
        wl[MAT_GRASS] = forest_w * cv.z;
        wl[MAT_SOIL] = in.weights.y * 0.4 + forest_w * cv.w * (1.0 - high);
        wl[MAT_SAND] = in.weights.z * (1.0 - rocky);
        wl[MAT_ROCK] = in.weights.z * rocky + forest_w * cv.w * high;
        // 02.9: во впадинах, вдоль стока и на северных склонах мох гуще,
        // на гребнях суше — почва и трава.
        float mossk = saturate(0.5 * hollow + 0.6 * damp + 0.35 * north);
        wl[MAT_MOSS] += forest_w * (cv.x + cv.y) * mossk * 0.8;
        wl[MAT_FOREST] *= 1.0 - 0.5 * mossk;
        wl[MAT_SOIL] += forest_w * ridge * 0.25 * (cv.x + cv.z);
        wl[MAT_GRASS] += forest_w * ridge * 0.2 * cv.z;
        // Опад: в низинном лесу — листовой, в кедровом — тёмный с хвоей.
        wl[MAT_LEAVES] = forest_w * cv.x * 0.45 * (1.0 - 0.6 * mossk);
        wl[MAT_LITTER] = forest_w * cv.y * 0.35 * (1.0 - 0.5 * mossk);
        wl[MAT_GRAVEL] = forest_w * cv.w * 0.5;
        wl[MAT_MOSSROCK] = 0.0;
        // Переходные полосы: где встречаются мох и голая почва или трава и
        // почва, между ними ложится опад (как в природе), а не резкий шов.
        float ms_ = wl[MAT_MOSS] + wl[MAT_FOREST];
        wl[MAT_LITTER] += 1.4 * sqrt(ms_ * wl[MAT_SOIL]) + 0.8 * sqrt(wl[MAT_GRASS] * wl[MAT_SOIL]);
        wl[MAT_LEAVES] += 0.6 * sqrt(wl[MAT_FOREST] * wl[MAT_GRASS]);
    } else {
        wl[MAT_FOREST] = in.weights.x * (1.0 - 0.6 * moist);
        wl[MAT_MOSS] = in.weights.x * 0.6 * moist + in.weights.y * 0.6;
        wl[MAT_SOIL] = in.weights.y * 0.4;
        wl[MAT_SAND] = in.weights.z * (1.0 - rocky);
        wl[MAT_ROCK] = in.weights.z * rocky;
        wl[MAT_GRASS] = 0.0;
        wl[MAT_LITTER] = 0.0;
        wl[MAT_LEAVES] = 0.0;
        wl[MAT_GRAVEL] = 0.0;
        wl[MAT_MOSSROCK] = 0.0;
    }
    // Поверх: тропа — почва, русло — камень с песком, крутые склоны и дно у
    // каменистого берега — камень, остальное дно — песок.
    float river = max(in.material.y, bank);
    // Порог крутизны дрожит шумом: граница камня не повторяет рёбра сетки 7,5 м.
    float steep_n = (detail.sample(s, pattern_uv(frame, lxz, 17.0)).g - 0.5) * 0.16;
    float steep = smoothstep(0.18 + steep_n, 0.50 + steep_n, slope);
    float under = smoothstep(0.0, -0.6, in.world.y);
    float over[MAT_COUNT] = {0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0};
    // Полотно тропы — утоптанная почва с щебнем, бровка — опад с почвой.
    over[MAT_SOIL] += 0.6 * trail + 0.4 * road_px + 0.3 * trail_edge * (1.0 - trail);
    over[MAT_GRAVEL] += (belts ? 0.4 : 0.0) * trail;
    over[MAT_SOIL] += (belts ? 0.0 : 0.4) * trail;
    over[MAT_LITTER] += (belts ? 0.3 : 0.0) * trail_edge * (1.0 - trail);
    over[MAT_SOIL] += (belts ? 0.0 : 0.3) * trail_edge * (1.0 - trail);
    // В лесных поясах крутые склоны и выходы камня замшелые (Якусима — один из
    // самых влажных лесов Японии): часть «крутого камня» отдаётся мху.
    float mossy = belts ? 0.65 * (cv.x * 0.5 + cv.y) * forest_w : 0.0;
    over[MAT_ROCK] += 0.7 * river + steep * (1.0 - mossy) + under * rocky + 0.6 * road_px;
    over[MAT_GRAVEL] += (belts ? 0.6 : 0.0) * road_edge;
    over[MAT_LITTER] += (belts ? 0.4 : 0.0) * road_edge;
    // В лесу крутой склон — мшистый камень, а не голый мох.
    over[MAT_MOSSROCK] += steep * mossy * 0.7;
    over[MAT_MOSS] += steep * mossy * 0.3;
    over[MAT_SAND] += 0.3 * river + under * (1.0 - rocky);
    // Осыпи под обрывами (02.9): камень с почвой, под лесом — реже.
    float scree_k = scree * (1.0 - 0.7 * mossy) * (1.0 - under);
    over[MAT_ROCK] += 0.3 * scree_k;
    over[MAT_GRAVEL] += (belts ? 0.6 : 0.0) * scree_k;
    over[MAT_SOIL] += (belts ? 0.1 : 0.7) * scree_k;
    float cover_sum = trail + road_px + river + steep + under + scree_k + 0.6 * trail_edge * (1.0 - trail)
                    + (belts ? road_edge : 0.0);
    float cover = saturate(cover_sum);
    float total = 0.0;
    for (int i = 0; i < MAT_COUNT; ++i) {
        wl[i] = mix(wl[i], over[i] / max(cover_sum, 1e-4), cover);
        total += wl[i];
    }

    // Смешивание по высоте рельефа материала: на границе выше выступающий слой.
    float mixk = 0.3 + 0.4 * detail.sample(s, pattern_uv(frame, lxz, 41.0)).g;
    MatSample ms[MAT_COUNT];
    float best = -1.0;
    float wall = smoothstep(35.0, 55.0, slope_deg);
    float3 local = in.rel + frame.cam_lo.xyz;
    // Производные в равномерном потоке: выборки ниже идут в ветвях.
    float3 dlx = dfdx(local), dly = dfdy(local);
    float dist = length(in.rel);
    float3 V0 = normalize(-in.rel);
    float3 L0 = -frame.light_dir.xyz;
    // Главный слой — по нему считается параллакс.
    int dom = 0;
    for (int i = 1; i < MAT_COUNT; ++i) {
        dom = wl[i] > wl[dom] ? i : dom;
    }
    // Параллакс (02.2, детализация вблизи): луч взгляда опускается в рельеф
    // главного слоя; смещение в метрах по XZ получают все слои. Вблизи — до
    // 24 шагов, к 22 м эффект гаснет. Затем — самозатенение рельефа к солнцу.
    float2 pom_off = 0.0;
    float pom_shadow = 1.0;
    // При скользящем взгляде смещение растёт как 1/cos и тянет полосы — гасим.
    float vn0 = dot(V0, n);
    float depth_m = MAT_DEPTH_M[dom] * (1.0 - smoothstep(6.0, 22.0, dist)) * (1.0 - wall)
                  * smoothstep(0.2, 0.5, vn0);
    if (depth_m > 0.002 && frame.view_mode == VIEW_LIT) {
        float tile = MAT_TILE_M[dom];
        float2 uv0 = pattern_uv(frame, lxz, tile);
        float2 gx = dlx.xz / tile, gy = dly.xz / tile;
        float vn = max(vn0, 0.35);
        float2 dir = -(V0 - n * dot(V0, n)).xz / vn;  // м по XZ на метр вглубь
        int steps = int(mix(24.0, 8.0, vn));
        float dh = depth_m / float(steps);
        float prev_h = 0.0;
        float prev_d = (1.0 - layer_height(mat_albedo, s, dom, uv0, gx, gy, mixk)) * depth_m;
        float hit = depth_m;
        for (int k = 1; k <= steps; ++k) {
            float h = dh * float(k);
            float d = (1.0 - layer_height(mat_albedo, s, dom, uv0 + dir * h / tile, gx, gy, mixk)) * depth_m;
            if (d <= h) {
                float a0 = prev_d - prev_h, a1 = d - h;
                hit = mix(prev_h, h, a0 / max(a0 - a1, 1e-6));
                break;
            }
            prev_h = h;
            prev_d = d;
        }
        pom_off = dir * hit;
        float ln = dot(L0, n);
        if (ln > 0.05 && dist < 14.0) {
            float2 ldir = (L0 - n * ln).xz / ln;  // м по XZ на метр вверх
            float occl = 0.0;
            for (int k = 1; k <= 6; ++k) {
                float up = hit * float(k) / 6.0;
                float ray = hit - up;
                float d = (1.0 - layer_height(mat_albedo, s, dom, uv0 + (pom_off + ldir * up) / tile,
                                              gx, gy, mixk)) * depth_m;
                occl = max(occl, saturate((ray - d) / (0.25 * depth_m)));
            }
            pom_shadow = 1.0 - 0.8 * occl * (1.0 - smoothstep(8.0, 14.0, dist));
        }
    }
    for (int i = 0; i < MAT_COUNT; ++i) {
        wl[i] /= max(total, 1e-4);
        if (wl[i] > 0.01) {
            float tile = MAT_TILE_M[i];
            ms[i] = ((i == MAT_ROCK || i == MAT_MOSSROCK) && wall > 0.01)
                ? sample_layer_triplanar(mat_albedo, mat_normal, s, i, frame, local, dlx, dly, n,
                                         pom_off, mixk)
                : sample_layer_uv(mat_albedo, mat_normal, s, i, pattern_uv(frame, lxz + pom_off, tile),
                                  dlx.xz / tile, dly.xz / tile, mixk);
            best = max(best, wl[i] + 0.5 * ms[i].height);
        }
    }
    float3 albedo = 0.0;
    float2 nxy = 0.0;
    float rough = 0.0;
    float porosity = 0.0;
    float mat_ao = 0.0;
    float sum = 0.0;
    float final_w[MAT_COUNT];
    for (int i = 0; i < MAT_COUNT; ++i) {
        float f = wl[i] > 0.01 ? max(wl[i] + 0.5 * ms[i].height - (best - 0.3), 0.0) : 0.0;
        final_w[i] = f;
        sum += f;
    }
    for (int i = 0; i < MAT_COUNT; ++i) {
        float f = final_w[i] / max(sum, 1e-4);
        final_w[i] = f;
        if (f > 0.0) {
            float3 la = ms[i].albedo;
            if (i == MAT_GRASS) {
                // Скан Grass004 — сочный газон; луга Якусимы — приглушённые,
                // оливковые (02.10): меньше насыщенность и яркость.
                la = mix(float3(dot(la, float3(0.3, 0.5, 0.2))), la, 0.55) * float3(0.78, 0.76, 0.66);
            }
            albedo += la * f;
            nxy += ms[i].nxy * f;
            rough += ms[i].rough * f;
            mat_ao += ms[i].ao * f;
            porosity += MAT_POROSITY[i] * f;
        }
    }
    // Мелкий рельеф вблизи (02.2): нормаль главного слоя в 5,3 раза мельче,
    // повёрнутая, гаснет к 8 м — зерно камешков и мха под ногами.
    float fine_k = (1.0 - smoothstep(2.0, 8.0, dist)) * (frame.view_mode == VIEW_LIT ? 1.0 : 0.0);
    if (fine_k > 0.0) {
        float tile = MAT_TILE_M[dom] / 5.3;
        float2 uvf = rot_uv(pattern_uv(frame, lxz + pom_off, tile));
        gradient2d gf(rot_grad(dlx.xz / tile), rot_grad(dly.xz / tile));
        float4 nf = mat_normal.sample(s, uvf, dom, gf);
        nxy += (nf.rg * 2.0 - 1.0) * 0.5 * fine_k;
        mat_ao *= mix(1.0, nf.a, 0.5 * fine_k);
    }
    // Крупная вариация тона, чтобы поле не выглядело одной плиткой.
    float macro = detail.sample(s, pattern_uv(frame, lxz, 23.0)).g;
    albedo *= 0.85 + 0.3 * macro;
    // 02.9: впадины и мокрые ложбины темнее, гребни светлее; осыпи светлые.
    albedo *= (1.0 + 0.5 * min(curv, 0.0) + 0.2 * max(curv, 0.0)) * (1.0 - 0.3 * damp) * (1.0 + 0.35 * scree_k);
    // Влажная почва торфяника темнее.
    albedo *= 1.0 - 0.3 * moist * in.weights.y;

    // Тень полога вблизи (до деревьев 02.4): под лесом прямое солнце доходит
    // пятнами, рассеянный свет приглушён — пол леса не выглядит газоном.
    float canopy_shade = 0.0;
    float sun_flecks = 1.0;
    if (belts) {
        float near_k = 1.0 - smoothstep(80.0, 450.0, length(in.rel));
        // Над дорогой и широкой тропой полога нет — просвет.
        canopy_shade = (cv.x + cv.y) * forest_w * near_k * (1.0 - road_px) * (1.0 - 0.5 * trail);
        float fleck = detail.sample(s, pattern_uv(frame, lxz, 6.0)).r * 0.6
                    + detail.sample(s, pattern_uv(frame, lxz, 2.3)).g * 0.4;
        sun_flecks = mix(1.0, smoothstep(0.55, 0.75, fleck) * 0.6 + 0.08, canopy_shade);
        // С настоящим лесом (02.4) пятна дают тени деревьев, а не шум.
        sun_flecks = frame.forest > 0.5 ? 1.0 : sun_flecks;
    }

    // Полог леса вдали (до деревьев 02.4): с 80–450 м под лесом виден не опад,
    // а кроны — тёмная зелень с пятнами крон; у кедрового пояса темнее и синее.
    if (belts) {
        float tree = (cv.x + cv.y) * forest_w * (1.0 - steep * 0.7);
        // С лесом кроны вблизи — настоящие деревья (до ~480 м), полог-подкраска — дальше.
        float far_k = frame.forest > 0.5 ? smoothstep(400.0, 500.0, length(in.rel))
                                         : smoothstep(80.0, 450.0, length(in.rel));
        float3 crown = mix(float3(0.050, 0.090, 0.030), float3(0.026, 0.052, 0.030),
                           cv.y / max(cv.x + cv.y, 1e-4));
        crown *= 0.7 + 0.6 * detail.sample(s, pattern_uv(frame, lxz, 31.0)).g;
        // 02.9: кроны в ложбинах гуще и темнее, на гребнях светлее и желтее.
        crown *= (1.0 + 0.6 * min(curv, 0.0) * 2.0 + 0.4 * max(curv, 0.0)) * (1.0 - 0.3 * damp);
        crown = mix(crown, crown * float3(1.15, 1.05, 0.8), ridge * 0.4);
        // Якудзаса вдали — ровная светлая зелень.
        float3 bamboo = float3(0.10, 0.14, 0.05);
        float k_tree = tree * far_k * 0.92;
        float k_bamboo = cv.z * forest_w * far_k * 0.6;
        albedo = mix(albedo, crown, k_tree);
        albedo = mix(albedo, bamboo, k_bamboo * (1.0 - k_tree));
        rough = mix(rough, 0.85, k_tree);
        nxy *= 1.0 - k_tree;
    }

    // Асфальт дорог (мир с поясами — дороги OSM): тёмно-серый, без рисунка камня.
    if (belts && road_px > 0.0) {
        // Тёплый серый и шероховатый: в тени тёмный асфальт иначе отражал
        // небо синим (02.10).
        float3 asphalt = float3(0.13, 0.125, 0.115) * (0.85 + 0.3 * macro);
        albedo = mix(albedo, asphalt, road_px * 0.85);
        rough = mix(rough, 0.92, road_px);
        nxy *= 1.0 - 0.7 * road_px;
    }

    // Мокрый песок: темнее, глаже, с бликом.
    float wet = saturate(wet_mask * porosity);
    albedo *= mix(1.0, 0.4, wet);
    rough = mix(rough, 0.08, 0.9 * wet);


    // Нормаль из карты: касательная +X, бикасательная −Z (v растёт с z, а «вверх» на картинке — к меньшим v).
    float3 t = normalize(float3(1, 0, 0) - n * n.x);
    float3 b = normalize(float3(0, 0, -1) + n * n.z);
    float nz = sqrt(saturate(1.0 - dot(nxy, nxy)));
    float3 N = normalize(t * nxy.x + b * nxy.y + n * nz);

    if (frame.view_mode == VIEW_COVER) {
        // Пояса: салатовый — низинный лес, тёмно-зелёный — кедровый лес,
        // жёлтый — якудзаса и луга, серый — гранит и застройка; синий — нет слоя.
        float3 c = belts ? cv.x * float3(0.45, 0.80, 0.25) + cv.y * float3(0.03, 0.28, 0.10)
                           + cv.z * float3(0.95, 0.80, 0.20) + cv.w * float3(0.50, 0.50, 0.50)
                         : float3(0.1, 0.2, 0.5);
        float shade = 0.35 + 0.65 * saturate(dot(n, -frame.light_dir.xyz));
        return float4(c * shade, 1.0);
    }
    if (frame.view_mode == VIEW_MACRO) {
        // 02.9: синий — впадины, охра — гребни, зелёный — влажность, белый —
        // осыпь; яркость — видимость неба.
        float3 c = mix(float3(0.15, 0.25, 0.6), float3(0.9, 0.75, 0.4), saturate(0.5 + curv * 2.0));
        c = mix(c, float3(0.1, 0.6, 0.2), damp * 0.7);
        c = mix(c, float3(1.0), scree * 0.8);
        return float4(c * sky_vis * sky_vis, 1.0);
    }
    if (frame.view_mode == VIEW_CLUSTERS || frame.view_mode == VIEW_LEVELS) {
        // Кластеры — случайный цвет по номеру, уровни — от синего (0) к красному.
        uint hsh = in.cluster * 2654435761u;
        float3 c = frame.view_mode == VIEW_CLUSTERS
            ? float3((hsh >> 8) & 255, (hsh >> 16) & 255, (hsh >> 24) & 255) / 255.0 * 0.8 + 0.2
            : ramp(float(in.depth) / 8.0, float3(0.1, 0.3, 1.0), float3(0.2, 0.9, 0.3), float3(1.0, 0.2, 0.1));
        float shade = 0.35 + 0.65 * saturate(dot(n, -frame.light_dir.xyz));
        return float4(c * shade, 1.0);
    }

    switch (frame.view_mode) {
        case VIEW_HEIGHT: {
            float h = in.world.y;
            float3 c = h < 0.0 ? mix(float3(0.1, 0.3, 0.7), float3(0.02, 0.05, 0.25), saturate(-h / 24.0))
                               : ramp(h / 50.0, float3(0.1, 0.5, 0.1), float3(0.6, 0.45, 0.2), float3(1.0, 1.0, 1.0));
            float line = 1.0 - smoothstep(0.0, 0.08, abs(fract(h / 5.0 + 0.5) - 0.5));
            return float4(c * (1.0 - 0.6 * line), 1.0);
        }
        case VIEW_SLOPE: {
            float3 c = ramp(slope_deg / 40.0, float3(0.1, 0.6, 0.1), float3(0.9, 0.85, 0.1), float3(0.9, 0.15, 0.05));
            return float4(slope_deg > 40.0 ? float3(0.8, 0.1, 0.8) : c, 1.0);
        }
        case VIEW_BIOME:
            return float4(mix(in.weights.xyz, float3(1.0), 0.6 * trail), 1.0);
        case VIEW_SUBSTRATE: {
            // Индекс субстрата интерполируется между вершинами: берём ближайший.
            int sub = int(round(in.material.x));
            const float3 pal[5] = {float3(0.25, 0.45, 0.15), float3(0.35, 0.22, 0.1), float3(0.9, 0.8, 0.5),
                                   float3(0.5, 0.5, 0.52), float3(0.6, 0.5, 0.35)};
            return float4(pal[clamp(sub, 0, 4)], 1.0);
        }
        case VIEW_COAST: {
            float near = in.weights.z + under;
            float3 c = mix(float3(0.95, 0.85, 0.45), float3(0.4, 0.4, 0.45), rocky);
            return float4(mix(float3(0.08), c, saturate(near)), 1.0);
        }
        case VIEW_WETNESS:
            return float4(mix(float3(0.1, 0.1, 0.1), float3(0.1, 0.4, 1.0), wet_mask), 1.0);
        case VIEW_MATERIALS: {
            float3 c = 0.0;
            for (int i = 0; i < MAT_COUNT; ++i) {
                c += MAT_DEBUG[i] * final_w[i];
            }
            return float4(c, 1.0);
        }
        default:
            break;
    }

    // Свет неба 04.1: прямой свет светила с пропусканием атмосферы и рассеянный
    // свет неба (SH); тени и туман — 04.2.
    float3 L = -frame.light_dir.xyz;
    float3 V = normalize(-in.rel);
    float3 H = normalize(L + V);
    float nl = max(dot(N, L), 0.0);
    float nv = max(dot(N, V), 1e-3);
    float nh = max(dot(N, H), 0.0);
    float a = max(rough * rough, 0.002);
    float a2 = a * a;
    float dd = nh * nh * (a2 - 1.0) + 1.0;
    float D = a2 / (M_PI_F * dd * dd);
    float k = (rough + 1.0) * (rough + 1.0) / 8.0;
    float G = (nl / (nl * (1.0 - k) + k)) * (nv / (nv * (1.0 - k) + k));
    float F = 0.04 + 0.96 * pow(1.0 - saturate(dot(H, V)), 5.0);
    float spec = D * G * F / (4.0 * nv + 1e-4);
    // Тень от светила (04.2): каскады вокруг камеры и карта всего острова.
    float shadow = shadow_factor(shadows, shadow_map, in.rel, normalize(in.normal), L);
    // Мелкие складки, которые карта теней (21 м на тексель вдали) не ловит,
    // гасят и часть прямого света (02.9).
    float3 sun = sky_sun_term(skyf) * shadow * mix(1.0, sky_vis * (1.0 - 0.4 * hollow), 0.6);
    // Затенение рельефом (02.9): дно ущелий видит мало неба.
    // Степень 1,5, а не куб (02.10): в ущелье небо не гаснет до черноты.
    float occ = sky_vis * sqrt(sky_vis) * (1.0 - 0.4 * hollow);
    // Щели материала (AO фотоскана) гасят свет неба; самозатенение параллакса — солнце.
    float3 sky = sky_ambient_term(skyf, N) * occ * mix(1.0, mat_ao, 0.8);
    sun *= pom_shadow;
    // Под пологом: пятна солнца и приглушённое зелёное небо сквозь кроны.
    float3 sky_c = sky * mix(float3(1.0), float3(0.45, 0.55, 0.40), canopy_shade);
    // Блик — физический: BRDF × освещённость (π · «солнце»).
    // Отражённый свет (02.10, до GI этапа 11): освещённые склоны и кроны
    // подсвечивают тень — сильнее там, где неба видно мало.
    float3 bounce = sky_sun_term(skyf) * max(L.y, 0.0) * 0.16 * (1.0 - 0.5 * sky_vis) * mix(float3(1.0), float3(0.6, 0.75, 0.45), canopy_shade);
    float3 color = albedo * (sun * nl * sun_flecks + sky_c + bounce) + SKY_PI * sun * spec * sun_flecks;
    // Вода в русле до 03.2: тёмная толща, отражение неба по Френелю, блик солнца.
    if (stream > 0.0) {
        // Поверхность воды горизонтальна, а русло на сетке 30 м лежит на склоне:
        // отражение — по нормали воды (вверх) с рябью от детальной текстуры.
        float2 rip = (detail.sample(s, pattern_uv(frame, lxz, 3.0) + frame.time * 0.05).rg - 0.5) * 0.25;
        float3 Nw = normalize(float3(rip.x, 1.0, rip.y));
        float fres = 0.02 + 0.98 * pow(1.0 - saturate(dot(Nw, V)), 5.0);
        float3 sky_refl = sky_ambient_term(skyf, float3(0, 1, 0)) * 1.1;
        // Сквозь мелкую воду видно мокрое дно.
        float3 bed = albedo * 0.35 * (sun * nl + sky);
        float3 Hw = normalize(L + V);
        float glint = pow(saturate(dot(Nw, Hw)), 400.0) * 3.0;
        float3 water = mix(bed * float3(0.7, 0.9, 0.9), sky_refl, fres) + sun * glint;
        color = mix(color, water, stream * 0.9);
    }
    if (frame.underwater_m > 0.0) {
        // Под водой: свет у точки ниже моря ослаблен толщей над ней, путь от
        // глаз — дымкой толщи.
        color *= exp(-UW_ABSORB * max(-in.world.y, 0.0));
        color = underwater_fog(color, length(in.rel), frame.underwater_m, underwater_light(skyf));
    } else {
        // Воздушная перспектива и дымка (04.2): дальние склоны уходят в цвет неба.
        color = apply_aerial(aerial, frame, in.rel, color);
    }
    return float4(color * frame.pre_exposure, 1.0);
}
