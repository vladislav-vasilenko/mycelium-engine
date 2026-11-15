#include "common.h"
#include "sky.h"

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
                           device const float4* exposure_state [[buffer(2)]],
                           texture2d<float> hdr [[texture(0)]]) {
    constexpr sampler s(filter::nearest, address::clamp_to_edge);
    // exposure > 0 — фиксированная; иначе — автоэкспозиция неба (04.1).
    // HDR уже умножен на предэкспозицию кадра: делим её обратно.
    float e = frame.exposure > 0.0 ? frame.exposure
                                   : exposure_state[0].x / max(frame.pre_exposure, 1e-9);
    float3 c = hdr.sample(s, in.uv).rgb * e;
    // Сумеречное зрение (эффект Пуркинье): при низкой яркости сцены работают
    // палочки — цвета блёкнут и уходят в синеву. Средняя яркость — из замера.
    if (frame.exposure <= 0.0) {
        float avg = exposure_state[0].z;
        float scotopic = 1.0 - smoothstep(log(2e-6), log(5e-4), log(max(avg, 1e-12)));
        float lum = dot(c, float3(0.2126, 0.7152, 0.0722));
        c = mix(c, lum * float3(0.62, 0.78, 1.05), scotopic * 0.75);
    }
    // ACES по каналам уводит яркие цвета в кислотно-жёлтый; кривая по
    // яркости сохраняет оттенок. Смесь — оттенок почти сохранён, а светлые
    // места всё же мягко выцветают к белому (02.10).
    float lum = max(dot(c, float3(0.2126, 0.7152, 0.0722)), 1e-6);
    float3 hue = c * (aces(float3(lum)).x / lum);
    float3 mapped = mix(aces(c), hue, 0.65);
    float over = max(max(mapped.r, mapped.g), mapped.b);
    mapped = over > 1.0 ? mix(mapped / over, float3(1.0), saturate(over - 1.0)) : mapped;
    // Дизеринг ±½ шага 8-битного вывода (02.10): плавный градиент неба без полос.
    uint2 px = uint2(in.position.xy);
    uint h = (px.x * 0x8da6b343u) ^ (px.y * 0xd8163841u) ^ uint(frame.time * 60.0) * 0x9e3779b9u;
    h ^= h >> 13;
    h *= 0x7feb352du;
    float noise = (float(h >> 8) / 16777216.0 - 0.5) / 255.0;
    // Дизеринг в гамме вывода: шаг 8 бит равномерен там, а не в линейном.
    float3 srgb = pow(saturate(mapped), 1.0 / 2.2) + noise;
    return float4(pow(saturate(srgb), 2.2), 1.0);
}
