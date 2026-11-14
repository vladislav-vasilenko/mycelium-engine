// Руки от первого лица (этап 10): скиннинг по 4 весам в вершинном шейдере,
// освещение кожи и ядро сверки скиннинга с CPU.
//
// Пространство рук — пространство глаза: начало в глазу, взгляд −Z, вверх +Y.
// Палитра `palette[j]` = смещение рук × глобальная матрица сустава × обратная
// матрица привязки (src/hands/skeleton.rs, Skeleton::palette).
#include "common.h"
#include "sky.h"

struct Skinned {
    float3 position;
    float3 normal;
};

// Порядок операций совпадает с CPU-эталоном `skin_position`.
static Skinned skin(SkinnedVertex v, device const float4x4* palette) {
    float4 p = float4(float3(v.position), 1.0);
    float4 n = float4(float3(v.normal), 0.0);
    float4 sp = float4(0.0);
    float4 sn = float4(0.0);
    for (uint k = 0; k < 4; k++) {
        float4x4 m = palette[v.joints[k]];
        sp += (m * p) * v.weights[k];
        sn += (m * n) * v.weights[k];
    }
    Skinned s;
    s.position = sp.xyz;
    s.normal = normalize(sn.xyz);
    return s;
}

struct HandsOut {
    float4 position [[position]];
    float3 normal;   // в пространстве глаза
    float2 uv;
};

vertex HandsOut hands_vs(uint vid [[vertex_id]],
                         device const SkinnedVertex* verts [[buffer(0)]],
                         constant HandsUniforms& h [[buffer(1)]],
                         device const float4x4* palette [[buffer(2)]]) {
    SkinnedVertex v = verts[vid];
    Skinned s = skin(v, palette);
    HandsOut out;
    out.position = h.proj * float4(s.position, 1.0);
    out.normal = s.normal;
    out.uv = float2(v.uv);
    return out;
}

// Временный свет до этапа 04 (как у ландшафта): солнце и небо. Для кожи —
// «обёрнутый» Ламберт: свет заходит за терминатор, тени мягче, как от
// подповерхностного рассеивания (шаг 5 заменит это шейдером кожи).
fragment float4 hands_fs(HandsOut in [[stage_in]],
                         bool front [[front_facing]],
                         constant HandsUniforms& h [[buffer(1)]],
                         constant FrameUniforms& frame [[buffer(3)]],
                         device const SkyFrame& skyf [[buffer(4)]],
                         texture2d<float> albedo_tex [[texture(0)]]) {
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(4));
    float3 albedo = albedo_tex.sample(s, in.uv).rgb;
    float3 n_eye = normalize(in.normal) * (front ? 1.0 : -1.0);
    float3 N = normalize((h.view_to_world * float4(n_eye, 0.0)).xyz);
    float3 L = skyf.light_dir.xyz;
    float wrap = saturate((dot(N, L) + 0.35) / 1.35);
    // Красноватый край терминатора: свет, прошедший под кожей.
    float3 scatter = float3(0.25, 0.06, 0.03) * saturate(1.0 - abs(dot(N, L) * 2.0 - 0.2));
    // Мягкий блик кожи: широкий, слабый.
    float3 V = -normalize((h.view_to_world * float4(0.0, 0.0, -1.0, 0.0)).xyz);
    float3 H = normalize(L + V);
    float spec = 0.08 * pow(saturate(dot(N, H)), 24.0) * saturate(dot(N, L));
    float3 sun = sky_sun_term(skyf);
    float3 sky = sky_ambient_term(skyf, N);
    // Под пологом леса (02.7): прямого солнца мало, небо зеленее и тусклее.
    float shade = frame.canopy_eye;
    float3 sun_k = sun * mix(1.0, 0.22, shade);
    sky *= mix(float3(1.0), float3(0.45, 0.55, 0.40), shade);
    float3 color = albedo * (sun_k * (wrap + scatter) + sky) + sun_k * spec;
    if (frame.underwater_m > 0.0) {
        color *= exp(-UW_ABSORB * frame.underwater_m);
        color = underwater_fog(color, 0.4, frame.underwater_m, underwater_light(skyf));
    }
    return float4(color * frame.pre_exposure, 1.0);
}

// Сверка с CPU: позиции после скиннинга в вершинах i × (n / SKIN_PROBES).
kernel void skin_probe(device const SkinnedVertex* verts [[buffer(0)]],
                       constant HandsUniforms& h [[buffer(1)]],
                       device const float4x4* palette [[buffer(2)]],
                       device float4* out [[buffer(3)]],
                       uint gid [[thread_position_in_grid]]) {
    if (gid >= SKIN_PROBES) {
        return;
    }
    uint idx = gid * (h.vertex_count / SKIN_PROBES);
    out[gid] = float4(skin(verts[idx], palette).position, float(idx));
}
