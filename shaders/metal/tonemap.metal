#include "common.h"

struct FullscreenVaryings {
    float4 position [[position]];
    float2 uv;
};

// Один треугольник на весь экран, без вершинного буфера.
vertex FullscreenVaryings tonemap_vs(uint vid [[vertex_id]]) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    FullscreenVaryings out;
    out.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = float2(p.x, 1.0 - p.y);
    return out;
}

// Аппроксимация ACES (Narkowicz 2015). Вход — линейный HDR,
// выход линейный, в sRGB переводит формат drawable.
inline float3 aces(float3 x) {
    const float a = 2.51, b = 0.03, c = 2.43, d = 0.59, e = 0.14;
    return saturate((x * (a * x + b)) / (x * (c * x + d) + e));
}

fragment float4 tonemap_fs(FullscreenVaryings in [[stage_in]],
                           constant FrameUniforms& frame [[buffer(1)]],
                           texture2d<float> hdr [[texture(0)]]) {
    constexpr sampler s(filter::nearest, address::clamp_to_edge);
    float3 c = hdr.sample(s, in.uv).rgb * frame.exposure;
    return float4(aces(c), 1.0);
}
