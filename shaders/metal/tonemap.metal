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

// AgX (02.13; аппроксимация Б. Врэнша кривой Т. Собатки, как в Blender 4):
// вход — линейный HDR, выход — линейный дисплейный. Насыщенные яркие цвета
// плавно выцветают, как у плёнки и сенсора камеры, — без «кислотной» зелени.
static float3 agx_curve(float3 x) {
    float3 x2 = x * x, x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

static float3 agx(float3 c) {
    const float3x3 inset = float3x3(float3(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
                                    float3(0.0784335999999992, 0.878468636469772, 0.0784336),
                                    float3(0.0792237451477643, 0.0791661274605434, 0.879142973793104));
    const float3x3 outset = float3x3(float3(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
                                     float3(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
                                     float3(-0.0990297440797205, -0.0989611768448433, 1.15107367264116));
    const float min_ev = -12.47393, max_ev = 4.026069;
    float3 v = inset * max(c, 1e-10);
    v = (clamp(log2(v), min_ev, max_ev) - min_ev) / (max_ev - min_ev);
    v = outset * agx_curve(v);
    return pow(saturate(v), 2.2);
}

fragment float4 tonemap_fs(FullscreenVaryings in [[stage_in]],
                           constant FrameUniforms& frame [[buffer(1)]],
                           device const float4* exposure_state [[buffer(2)]],
                           texture2d<float> hdr [[texture(0)]],
                           texture2d<float> bloom [[texture(1)]]) {
    constexpr sampler s(filter::nearest, address::clamp_to_edge);
    // exposure > 0 — фиксированная; иначе — автоэкспозиция неба (04.1).
    // HDR уже умножен на предэкспозицию кадра: делим её обратно.
    float e = frame.exposure > 0.0 ? frame.exposure
                                   : exposure_state[0].x / max(frame.pre_exposure, 1e-9);
    // Bloom (04.5): цепочка суммирует 6 уровней — делим на их число и
    // подмешиваем 4 % (рассеяние в оптике); без bloom здесь тот же кадр.
    constexpr sampler bs(filter::linear, address::clamp_to_edge);
    // Галлюциногенный переход (05.6): кадр «дышит» (масштаб к центру),
    // волны искажения бегут по экрану, каналы расходятся.
    float trip = frame.mycelium.y;
    float2 uv = in.uv;
    if (trip > 0.0) {
        float2 cc = uv - 0.5;
        float r = length(cc);
        float t = frame.time;
        float breathe = 1.0 - trip * 0.03 * (0.5 + 0.5 * sin(t * 1.7));
        float2 wave = float2(sin(cc.y * 18.0 + t * 2.3), cos(cc.x * 15.0 - t * 1.9)) * 0.006 * trip;
        uv = 0.5 + cc * breathe + wave * (0.3 + r);
    }
    float3 scene = hdr.sample(s, uv).rgb;
    if (trip > 0.0) {
        float2 ca = (uv - 0.5) * 0.02 * trip * (0.6 + 0.4 * sin(frame.time * 3.1));
        scene.r = hdr.sample(bs, uv + ca).r;
        scene.b = hdr.sample(bs, uv - ca).b;
    }
    // Хроматическая аберрация линзы (кинематографичная обработка): красный и
    // синий чуть расходятся к краям кадра (до ~4 пикселей в углу).
    float2 uvc = in.uv - 0.5;
    bool grade = frame.camera_pos.w > 0.5;
    if (grade) {
        float2 ca = uvc * dot(uvc, uvc) * 0.006;
        scene.r = hdr.sample(bs, uv + ca).r;
        scene.b = hdr.sample(bs, uv - ca).b;
    }
    float3 glow = bloom.sample(bs, in.uv).rgb;
    float3 c = mix(scene, glow * (bloom.get_width() == hdr.get_width() ? 1.0 : 1.0 / 6.0), 0.04) * e;
    // Сумеречное зрение (эффект Пуркинье): при низкой яркости сцены работают
    // палочки — цвета блёкнут и уходят в синеву. Средняя яркость — из замера.
    if (frame.exposure <= 0.0) {
        float avg = exposure_state[0].z;
        float scotopic = 1.0 - smoothstep(log(2e-6), log(5e-4), log(max(avg, 1e-12)));
        float lum = dot(c, float3(0.2126, 0.7152, 0.0722));
        // Связь с мицелием (05.6) меняет восприятие: цвета ночью не блёкнут.
        c = mix(c, lum * float3(0.62, 0.78, 1.05), scotopic * 0.75 * (1.0 - 0.85 * frame.mycelium.x));
    }
    // AgX (02.13) вместо смеси ACES и кривой по яркости (02.10): та
    // сохраняла оттенок, и трава с цветами выходили кислотно-яркими.
    // Сверху — мягкое общее снижение насыщенности (−10 %), как у плёнки.
    float3 mapped = agx(c * 1.35);  // 1,35: средний серый 0,18 → 0,267, как у прежней кривой
    float ml = dot(mapped, float3(0.2126, 0.7152, 0.0722));
    // Кинематографичная обработка в духе Hellblade: приглушённая палитра
    // (насыщенность 0,78), тени холоднее, света теплее, S-кривая контраста
    // в гамме вывода (средний серый на месте), виньетка, зерно плёнки.
    mapped = max(ml + (grade ? 0.78 : 0.9) * (mapped - ml), 0.0);
    if (grade) {
        float3 tint = mix(float3(0.93, 1.0, 1.07), float3(1.04, 1.0, 0.95), smoothstep(0.03, 0.5, ml));
        mapped *= tint;
        float3 g = pow(saturate(mapped), 1.0 / 2.2);
        g = mix(g, g * g * (3.0 - 2.0 * g), 0.4);
        mapped = pow(g, 2.2);
        mapped *= 1.0 - 0.3 * smoothstep(0.08, 0.5, dot(uvc, uvc));
    }
    // Переход: цвет пульсирует к бирюзе и фиолету, края кадра темнеют
    // в такт сердцу.
    if (trip > 0.0) {
        float t = frame.time;
        float beat = pow(0.5 + 0.5 * sin(t * 7.0), 8.0) * 0.6 + 0.4;
        float3 hue = mix(float3(0.7, 1.15, 1.05), float3(1.1, 0.8, 1.2), 0.5 + 0.5 * sin(t * 0.8 + length(uvc) * 6.0));
        mapped = mix(mapped, mapped * hue * 1.15, trip * 0.7);
        mapped *= 1.0 - trip * 0.45 * beat * smoothstep(0.05, 0.45, dot(uvc, uvc));
    }
    // Дизеринг ±½ шага 8-битного вывода (02.10): плавный градиент неба без полос.
    uint2 px = uint2(in.position.xy);
    uint h = (px.x * 0x8da6b343u) ^ (px.y * 0xd8163841u) ^ uint(frame.time * 60.0) * 0x9e3779b9u;
    h ^= h >> 13;
    h *= 0x7feb352du;
    float noise = (float(h >> 8) / 16777216.0 - 0.5) / 255.0;
    // Зерно плёнки: ±3 % яркости, сильнее в тенях, своё в каждом кадре.
    uint h2 = h * 0x846ca68bu ^ (h >> 16);
    float grain = grade ? (float(h2 >> 8) / 16777216.0 - 0.5) * 0.06 * (1.0 - 0.7 * saturate(ml * 2.0)) : 0.0;
    mapped = max(mapped * (1.0 + grain), 0.0);
    // Дизеринг в гамме вывода: шаг 8 бит равномерен там, а не в линейном.
    float3 srgb = pow(saturate(mapped), 1.0 / 2.2) + noise;
    return float4(pow(saturate(srgb), 2.2), 1.0);
}
