#include "common.h"

// Интерфейс (05.1): треугольники в пикселях окна; u < 0 — сплошной цвет,
// иначе шрифт по полю расстояний. Цель — drawable sRGB: цвет переводится в
// линейный, смешивание — с предумноженной альфой.

struct UiOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

inline float3 srgb_to_linear(float3 c) {
    return select(pow((c + 0.055) / 1.055, 2.4), c / 12.92, c <= 0.04045);
}

vertex UiOut ui_vs(uint vid [[vertex_id]],
                   device const UiVertex* verts [[buffer(0)]],
                   constant float2& screen [[buffer(1)]]) {
    UiVertex v = verts[vid];
    UiOut o;
    float2 p = float2(v.pos) / screen;
    o.position = float4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    o.uv = float2(v.uv);
    float4 c = float4(v.color) / 255.0;
    o.color = float4(srgb_to_linear(c.rgb), c.a);
    return o;
}

fragment float4 ui_fs(UiOut in [[stage_in]], texture2d<float> font [[texture(0)]]) {
    float a = in.color.a;
    if (in.uv.x >= 0.0) {
        constexpr sampler s(filter::linear, mip_filter::none, address::clamp_to_edge);
        float d = font.sample(s, in.uv).r;
        float w = max(fwidth(d) * 0.75, 1e-3);
        a *= smoothstep(0.5 - w, 0.5 + w, d);
    }
    return float4(in.color.rgb * a, a);
}
