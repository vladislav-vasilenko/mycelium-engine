#include "common.h"

// Ландшафт LOD 0: все тайлы одним instanced draw. Номер экземпляра — номер
// тайла, вершины тайла лежат подряд по TILE_SAMPLES² штук.
//
// Материалы (подэтап 02.2): массивы фотосканов ambientCG, смешивание по весам
// биомов, субстрату, уклону, типу берега и высоте рельефа материала; мокрый
// песок по маске влажности; отладочные виды по FrameUniforms.view_mode.

struct TerrainVaryings {
    float4 position [[position]];
    float3 world;
    float3 normal;
    float4 weights;   // лес, торф, берег, тропа
    float4 material;  // субстрат (индекс), русло, влажность почвы, тип берега
};

vertex TerrainVaryings terrain_vs(uint vid [[vertex_id]],
                                  uint iid [[instance_id]],
                                  const device TerrainVertex* vertices [[buffer(0)]],
                                  constant FrameUniforms& frame [[buffer(1)]]) {
    TerrainVertex v = vertices[iid * TILE_SAMPLES * TILE_SAMPLES + vid];
    TerrainVaryings out;
    out.world = float3(v.position);
    out.position = frame.view_proj * float4(out.world, 1.0);
    out.normal = float3(v.normal);
    out.weights = float4(v.weights) / 255.0;
    out.material = float4(float(v.material.x), float(v.material.y) / 255.0,
                          float(v.material.z) / 255.0, float(v.material.w) / 255.0);
    return out;
}

// Слои массивов: src/terrain_materials.rs, LAYERS.
constant int MAT_ROCK = 0;
constant int MAT_SAND = 1;
constant int MAT_SOIL = 2;
constant int MAT_MOSS = 3;
constant int MAT_FOREST = 4;
constant int MAT_COUNT = 5;
// Размер повтора слоя на земле, м.
constant float MAT_TILE_M[MAT_COUNT] = {4.0, 3.0, 2.5, 2.0, 3.0};
// Насколько слой темнеет и блестит от влаги (песок впитывает и блестит сильнее камня).
constant float MAT_POROSITY[MAT_COUNT] = {0.25, 1.0, 0.7, 0.4, 0.5};
// Цвета слоёв в отладочном виде «слои материалов».
constant float3 MAT_DEBUG[MAT_COUNT] = {
    float3(0.45, 0.45, 0.48), float3(0.95, 0.85, 0.45), float3(0.45, 0.28, 0.12),
    float3(0.15, 0.65, 0.15), float3(0.45, 0.55, 0.15)};

// Виды: src/terrain_materials.rs, TerrainView.
constant uint VIEW_LIT = 0;
constant uint VIEW_HEIGHT = 1;
constant uint VIEW_SLOPE = 2;
constant uint VIEW_BIOME = 3;
constant uint VIEW_SUBSTRATE = 4;
constant uint VIEW_COAST = 5;
constant uint VIEW_WETNESS = 6;
constant uint VIEW_MATERIALS = 7;
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
                              sampler s, int layer, float2 xz, float mixk) {
    float2 uv = xz / MAT_TILE_M[layer];
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
                           texture2d<float> wetness_mask [[texture(3)]]) {
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
    constexpr sampler clamp_s(filter::linear, address::clamp_to_edge);
    float2 xz = in.world.xz;
    float3 n = normalize(in.normal);
    // Нормаль грани — как у треугольника LOD 0 в SampleGround.
    float3 face = normalize(cross(dfdy(in.world), dfdx(in.world)));
    face = face.y < 0.0 ? -face : face;
    float slope = 1.0 - n.y;
    float slope_deg = acos(clamp(face.y, -1.0, 1.0)) * (180.0 / M_PI_F);

    float rocky = in.material.w;
    float trail = smoothstep(0.1, 0.6, in.weights.w);
    float moist = in.material.z;
    // Маска непрерывная: smoothstep даёт гладкую границу мокрого песка.
    float wet_mask = smoothstep(0.1, 0.9, wetness_mask.sample(clamp_s, xz / frame.map_size_m).r);

    if (frame.view_mode == VIEW_RAW) {
        float3 w = in.weights.xyz;
        float dominant = (w.x >= w.y && w.x >= w.z) ? 0.0 : (w.y >= w.z ? 1.0 : 2.0);
        return float4(in.world.y, slope_deg, dominant, rocky);
    }

    // Доли слоёв до смешивания по высоте.
    float wl[MAT_COUNT];
    wl[MAT_FOREST] = in.weights.x * (1.0 - 0.6 * moist);
    wl[MAT_MOSS] = in.weights.x * 0.6 * moist + in.weights.y * 0.6;
    wl[MAT_SOIL] = in.weights.y * 0.4;
    wl[MAT_SAND] = in.weights.z * (1.0 - rocky);
    wl[MAT_ROCK] = in.weights.z * rocky;
    // Поверх: тропа — почва, русло — камень с песком, крутые склоны и дно у
    // каменистого берега — камень, остальное дно — песок.
    float river = in.material.y;
    float steep = smoothstep(0.22, 0.42, slope);
    float under = smoothstep(0.0, -0.6, in.world.y);
    float over[MAT_COUNT] = {0.0, 0.0, 0.0, 0.0, 0.0};
    over[MAT_SOIL] += trail;
    over[MAT_ROCK] += 0.5 * river + steep + under * rocky;
    over[MAT_SAND] += 0.5 * river + under * (1.0 - rocky);
    float cover = saturate(trail + river + steep + under);
    float total = 0.0;
    for (int i = 0; i < MAT_COUNT; ++i) {
        wl[i] = mix(wl[i], over[i] / max(trail + river + steep + under, 1e-4), cover);
        total += wl[i];
    }

    // Смешивание по высоте рельефа материала: на границе выше выступающий слой.
    float mixk = 0.3 + 0.4 * detail.sample(s, xz / 41.0).g;
    MatSample ms[MAT_COUNT];
    float best = -1.0;
    for (int i = 0; i < MAT_COUNT; ++i) {
        wl[i] /= max(total, 1e-4);
        if (wl[i] > 0.01) {
            ms[i] = sample_layer(mat_albedo, mat_normal, s, i, xz, mixk);
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
    float macro = detail.sample(s, xz / 23.0).g;
    albedo *= 0.85 + 0.3 * macro;
    // Влажная почва торфяника темнее.
    albedo *= 1.0 - 0.3 * moist * in.weights.y;

    // Мокрый песок: темнее, глаже, с бликом.
    float wet = saturate(wet_mask * porosity);
    albedo *= mix(1.0, 0.4, wet);
    rough = mix(rough, 0.08, 0.9 * wet);


    // Нормаль из карты: касательная +X, бикасательная −Z (v растёт с z, а «вверх» на картинке — к меньшим v).
    float3 t = normalize(float3(1, 0, 0) - n * n.x);
    float3 b = normalize(float3(0, 0, -1) + n * n.z);
    float nz = sqrt(saturate(1.0 - dot(nxy, nxy)));
    float3 N = normalize(t * nxy.x + b * nxy.y + n * nz);

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
    float3 V = normalize(frame.camera_pos.xyz - in.world);
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
    float3 color = albedo * (sun * nl + sky) + sun * spec;
    return float4(color, 1.0);
}
