#include "common.h"
#include "features.h"

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
                                          texture2d<float> layer_cover [[texture(6)]]) {
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
    float3 world = float3(float(i) * s, heights[node], float(j) * s);
    float dx = (h(i + 1, j) - h(i - 1, j)) / (2.0 * s);
    float dz = (h(i, j + 1) - h(i, j - 1)) / (2.0 * s);
    out.world = world;
    out.rel = camera_relative(frame, world);
    out.position = frame.view_proj_rel * float4(out.rel, 1.0);
    out.normal = normalize(float3(-dx, 1.0, -dz));
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
    return out;
}

// Слои массивов: src/terrain_materials.rs, LAYERS.
constant int MAT_ROCK = 0;
constant int MAT_SAND = 1;
constant int MAT_SOIL = 2;
constant int MAT_MOSS = 3;
constant int MAT_FOREST = 4;
constant int MAT_GRASS = 5;
constant int MAT_COUNT = 6;
// Размер повтора слоя на земле, м.
constant float MAT_TILE_M[MAT_COUNT] = {4.0, 3.0, 2.5, 2.0, 3.0, 2.0};
// Насколько слой темнеет и блестит от влаги (песок впитывает и блестит сильнее камня).
constant float MAT_POROSITY[MAT_COUNT] = {0.25, 1.0, 0.7, 0.4, 0.5, 0.4};
// Цвета слоёв в отладочном виде «слои материалов».
constant float3 MAT_DEBUG[MAT_COUNT] = {
    float3(0.45, 0.45, 0.48), float3(0.95, 0.85, 0.45), float3(0.45, 0.28, 0.12),
    float3(0.15, 0.65, 0.15), float3(0.45, 0.55, 0.15), float3(0.6, 0.9, 0.3)};

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
constant uint VIEW_COVER = 13;
constant uint VIEW_RAW = 15;

struct MatSample {
    float3 albedo;
    float height;
    float2 nxy;   // нормаль в касательном пространстве, [-1, 1]
    float rough;
};

// Выборка слоя по мировым XZ с подавлением повторов: вторая выборка в
// повёрнутых и растянутых координатах, доля — из крупного шума.
inline MatSample sample_layer(texture2d_array<float> albedo, texture2d_array<float> normal,
                              sampler s, int layer, constant FrameUniforms& frame,
                              float2 local_xz, float mixk) {
    float2 uv = pattern_uv(frame, local_xz, MAT_TILE_M[layer]);
    float2 uv2 = float2(0.8 * uv.x - 0.6 * uv.y, 0.6 * uv.x + 0.8 * uv.y) * 0.43 + 0.37;
    float4 a = mix(albedo.sample(s, uv, layer), albedo.sample(s, uv2, layer), mixk);
    float4 n = mix(normal.sample(s, uv, layer), normal.sample(s, uv2, layer), mixk);
    MatSample m;
    m.albedo = a.rgb;
    m.height = a.a;
    m.nxy = n.rg * 2.0 - 1.0;
    m.rough = n.b;
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
                           device const uint* feat_list [[buffer(4)]]) {
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
    float trail_px = 1.0 - smoothstep(-px, px + 0.2, fd.trail + wobble);
    float road_px = 1.0 - smoothstep(-px, px + 0.1, fd.road);
    // Камни берега русла и вода в русле (до 03.2 вода — только цвет).
    float bank = 1.0 - smoothstep(0.0, 1.5 + px, fd.river + wobble);
    float stream = 1.0 - smoothstep(-px, px, fd.river + 0.4);
    float trail = max(smoothstep(0.1, 0.6, in.weights.w), trail_px);
    float moist = in.material.z;
    // Маска непрерывная: smoothstep даёт гладкую границу мокрого песка.
    float wet_mask = smoothstep(0.1, 0.9, wetness_mask.sample(clamp_s, xz / frame.map_size_m).r);

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
    } else {
        wl[MAT_FOREST] = in.weights.x * (1.0 - 0.6 * moist);
        wl[MAT_MOSS] = in.weights.x * 0.6 * moist + in.weights.y * 0.6;
        wl[MAT_SOIL] = in.weights.y * 0.4;
        wl[MAT_SAND] = in.weights.z * (1.0 - rocky);
        wl[MAT_ROCK] = in.weights.z * rocky;
        wl[MAT_GRASS] = 0.0;
    }
    // Поверх: тропа — почва, русло — камень с песком, крутые склоны и дно у
    // каменистого берега — камень, остальное дно — песок.
    float river = max(in.material.y, bank);
    float steep = smoothstep(0.22, 0.42, slope);
    float under = smoothstep(0.0, -0.6, in.world.y);
    float over[MAT_COUNT] = {0.0, 0.0, 0.0, 0.0, 0.0, 0.0};
    over[MAT_SOIL] += trail + 0.4 * road_px;
    // В лесных поясах крутые склоны и выходы камня замшелые (Якусима — один из
    // самых влажных лесов Японии): часть «крутого камня» отдаётся мху.
    float mossy = belts ? 0.65 * (cv.x * 0.5 + cv.y) * forest_w : 0.0;
    over[MAT_ROCK] += 0.7 * river + steep * (1.0 - mossy) + under * rocky + 0.6 * road_px;
    over[MAT_MOSS] += steep * mossy;
    over[MAT_SAND] += 0.3 * river + under * (1.0 - rocky);
    float cover_sum = trail + road_px + river + steep + under;
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
    for (int i = 0; i < MAT_COUNT; ++i) {
        wl[i] /= max(total, 1e-4);
        if (wl[i] > 0.01) {
            ms[i] = sample_layer(mat_albedo, mat_normal, s, i, frame, lxz, mixk);
            best = max(best, wl[i] + 0.5 * ms[i].height);
        }
    }
    float3 albedo = 0.0;
    float2 nxy = 0.0;
    float rough = 0.0;
    float porosity = 0.0;
    float sum = 0.0;
    float final_w[MAT_COUNT];
    for (int i = 0; i < MAT_COUNT; ++i) {
        float f = wl[i] > 0.01 ? max(wl[i] + 0.5 * ms[i].height - (best - 0.18), 0.0) : 0.0;
        final_w[i] = f;
        sum += f;
    }
    for (int i = 0; i < MAT_COUNT; ++i) {
        float f = final_w[i] / max(sum, 1e-4);
        final_w[i] = f;
        if (f > 0.0) {
            albedo += ms[i].albedo * f;
            nxy += ms[i].nxy * f;
            rough += ms[i].rough * f;
            porosity += MAT_POROSITY[i] * f;
        }
    }
    // Крупная вариация тона, чтобы поле не выглядело одной плиткой.
    float macro = detail.sample(s, pattern_uv(frame, lxz, 23.0)).g;
    albedo *= 0.85 + 0.3 * macro;
    // Влажная почва торфяника темнее.
    albedo *= 1.0 - 0.3 * moist * in.weights.y;

    // Тень полога вблизи (до деревьев 02.4): под лесом прямое солнце доходит
    // пятнами, рассеянный свет приглушён — пол леса не выглядит газоном.
    float canopy_shade = 0.0;
    float sun_flecks = 1.0;
    if (belts) {
        float near_k = 1.0 - smoothstep(80.0, 450.0, length(in.rel));
        canopy_shade = (cv.x + cv.y) * forest_w * near_k;
        float fleck = detail.sample(s, pattern_uv(frame, lxz, 6.0)).r * 0.6
                    + detail.sample(s, pattern_uv(frame, lxz, 2.3)).g * 0.4;
        sun_flecks = mix(1.0, smoothstep(0.55, 0.75, fleck) * 0.6 + 0.08, canopy_shade);
    }

    // Полог леса вдали (до деревьев 02.4): с 80–450 м под лесом виден не опад,
    // а кроны — тёмная зелень с пятнами крон; у кедрового пояса темнее и синее.
    if (belts) {
        float tree = (cv.x + cv.y) * forest_w * (1.0 - steep * 0.7);
        float far_k = smoothstep(80.0, 450.0, length(in.rel));
        float3 crown = mix(float3(0.050, 0.090, 0.030), float3(0.026, 0.052, 0.030),
                           cv.y / max(cv.x + cv.y, 1e-4));
        crown *= 0.7 + 0.6 * detail.sample(s, pattern_uv(frame, lxz, 31.0)).g;
        // Якудзаса вдали — ровная светлая зелень.
        float3 bamboo = float3(0.10, 0.14, 0.05);
        float k_tree = tree * far_k * 0.92;
        float k_bamboo = cv.z * forest_w * far_k * 0.6;
        albedo = mix(albedo, crown, k_tree);
        albedo = mix(albedo, bamboo, k_bamboo * (1.0 - k_tree));
        rough = mix(rough, 0.85, k_tree);
        nxy *= 1.0 - k_tree;
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

    // Временный свет до этапа 04: солнце (Ламберт + GGX) и небо, без теней и тумана.
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
    const float3 sun = float3(1.15, 1.1, 1.0);
    float3 sky = float3(0.20, 0.26, 0.36) * (0.5 + 0.5 * N.y);
    // Под пологом: пятна солнца и приглушённое зелёное небо сквозь кроны.
    float3 sky_c = sky * mix(float3(1.0), float3(0.45, 0.55, 0.40), canopy_shade);
    float3 color = albedo * (sun * nl * sun_flecks + sky_c) + sun * spec * sun_flecks;
    // Вода в русле до 03.2: тёмная толща, отражение неба по Френелю, блик солнца.
    if (stream > 0.0) {
        // Поверхность воды горизонтальна, а русло на сетке 30 м лежит на склоне:
        // отражение — по нормали воды (вверх) с рябью от детальной текстуры.
        float2 rip = (detail.sample(s, pattern_uv(frame, lxz, 3.0) + frame.time * 0.05).rg - 0.5) * 0.25;
        float3 Nw = normalize(float3(rip.x, 1.0, rip.y));
        float fres = 0.02 + 0.98 * pow(1.0 - saturate(dot(Nw, V)), 5.0);
        float3 sky_refl = float3(0.30, 0.42, 0.62) * 0.7;
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
        color = underwater_fog(color, length(in.rel), frame.underwater_m);
    }
    return float4(color, 1.0);
}
