#include "common.h"

// Bloom (04.5, сессия 2) без порога, как в физических камерах: рассеяние в
// оптике касается всего света, но яркое заметно сильнее. Цепочка вниз —
// 13 выборок (Jimenez 2014) с весом 1 / (1 + Y) против «светлячков»; вверх —
// тент 3 × 3 плюс уровень цепочки вниз. Тонмаппинг подмешивает итог (≈4 %).

inline float bloom_luma(float3 c) {
    return dot(c, float3(0.2126, 0.7152, 0.0722));
}

kernel void bloom_down(texture2d<float> src [[texture(0)]],
                       texture2d<float, access::write> dst [[texture(1)]],
                       uint2 gid [[thread_position_in_grid]]) {
    if (any(gid >= uint2(dst.get_width(), dst.get_height()))) {
        return;
    }
    constexpr sampler ls(filter::linear, address::clamp_to_edge);
    float2 texel = 1.0 / float2(src.get_width(), src.get_height());
    float2 uv = (float2(gid) + 0.5) / float2(dst.get_width(), dst.get_height());
    auto s = [&](float2 o) {
        float3 c = src.sample(ls, uv + o * texel, level(0)).rgb;
        return float4(c, 1.0) / (1.0 + bloom_luma(c));
    };
    float4 a = s(float2(-2, -2)), b = s(float2(0, -2)), c = s(float2(2, -2));
    float4 d = s(float2(-1, -1)), e = s(float2(1, -1));
    float4 f = s(float2(-2, 0)), g = s(float2(0, 0)), h = s(float2(2, 0));
    float4 i = s(float2(-1, 1)), j = s(float2(1, 1));
    float4 k = s(float2(-2, 2)), l = s(float2(0, 2)), m = s(float2(2, 2));
    float4 sum = (d + e + i + j) * 0.5 + (a + b + f + g) * 0.125 + (b + c + g + h) * 0.125
               + (f + g + k + l) * 0.125 + (g + h + l + m) * 0.125;
    // Взвешенное среднее: rgb / w возвращает яркость без светлячков.
    dst.write(float4(sum.rgb / max(sum.a, 1e-6), 1.0), gid);
}

kernel void bloom_up(texture2d<float> low [[texture(0)]],
                     texture2d<float> cur [[texture(1)]],
                     texture2d<float, access::write> dst [[texture(2)]],
                     uint2 gid [[thread_position_in_grid]]) {
    if (any(gid >= uint2(dst.get_width(), dst.get_height()))) {
        return;
    }
    constexpr sampler ls(filter::linear, address::clamp_to_edge);
    float2 texel = 1.0 / float2(low.get_width(), low.get_height());
    float2 uv = (float2(gid) + 0.5) / float2(dst.get_width(), dst.get_height());
    float3 t = 0.0;
    const float w[3] = {1.0, 2.0, 1.0};
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            t += low.sample(ls, uv + float2(x, y) * texel, level(0)).rgb * w[x + 1] * w[y + 1];
        }
    }
    dst.write(float4(cur.read(gid).rgb + t / 16.0, 1.0), gid);
}
