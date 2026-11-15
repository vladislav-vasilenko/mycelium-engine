#include "common.h"

// Временное сглаживание (04.5, сессия 1). Кадр рисуется со сдвигом субпикселя
// (Халтон 2, 3); здесь пиксель перепроецируется в прошлый кадр по глубине и
// матрицам без сдвига, история ограничивается окрестностью 3 × 3 текущего
// кадра в YCoCg (обрезка по коробке среднее ± 1,25σ) и смешивается: 90 %
// истории, 10 % нового. Вес по яркости (1 / (1 + Y)) — яркие блики не
// «звенят». Движение листвы и воды не в векторах скорости: их удерживает
// обрезка по окрестности (векторы скорости — следующая итерация).

inline float3 rgb_to_ycocg(float3 c) {
    return float3(0.25 * c.r + 0.5 * c.g + 0.25 * c.b, 0.5 * c.r - 0.5 * c.b, -0.25 * c.r + 0.5 * c.g - 0.25 * c.b);
}

inline float3 ycocg_to_rgb(float3 c) {
    return float3(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// Обрезка точки истории к коробке (не просто clamp: сохраняет оттенок).
inline float3 clip_aabb(float3 lo, float3 hi, float3 p, float3 q) {
    float3 center = 0.5 * (hi + lo);
    float3 extent = 0.5 * (hi - lo) + 1e-5;
    float3 v = q - center;
    float3 a = abs(v / extent);
    float m = max(a.x, max(a.y, a.z));
    return m > 1.0 ? center + v / m : q;
}

// Фильтр Катмулла-Рома истории за 5 выборок: резче билинейной, меньше мыла.
inline float3 sample_history(texture2d<float> h, float2 uv, float2 size) {
    constexpr sampler ls(filter::linear, address::clamp_to_edge);
    float2 pos = uv * size;
    float2 c = floor(pos - 0.5) + 0.5;
    float2 f = pos - c;
    float2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    float2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    float2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    float2 w3 = f * f * (-0.5 + 0.5 * f);
    float2 w12 = w1 + w2;
    float2 t0 = (c - 1.0) / size, t3 = (c + 2.0) / size, t12 = (c + w2 / w12) / size;
    float3 r = h.sample(ls, float2(t12.x, t0.y)).rgb * (w12.x * w0.y)
             + h.sample(ls, float2(t0.x, t12.y)).rgb * (w0.x * w12.y)
             + h.sample(ls, t12).rgb * (w12.x * w12.y)
             + h.sample(ls, float2(t3.x, t12.y)).rgb * (w3.x * w12.y)
             + h.sample(ls, float2(t12.x, t3.y)).rgb * (w12.x * w3.y);
    float wsum = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(r / wsum, 0.0);
}

kernel void taa_resolve(texture2d<float> cur [[texture(0)]],
                        depth2d<float> depth [[texture(1)]],
                        texture2d<float> hist [[texture(2)]],
                        texture2d<float, access::write> out [[texture(3)]],
                        constant TaaUniforms& t [[buffer(0)]],
                        texture2d<float> vol [[texture(4)]],
                        uint2 gid [[thread_position_in_grid]]) {
    uint2 size = uint2(cur.get_width(), cur.get_height());
    if (any(gid >= size)) {
        return;
    }
    // Окрестность 3 × 3: среднее и разброс в YCoCg с весом по яркости,
    // ближайшая к камере глубина (reversed-Z: наибольшая) — для краёв.
    float3 m1 = 0.0, m2 = 0.0;
    float3 center = 0.0;
    float dmax = 0.0;
    int2 dpos = int2(gid);
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            int2 p = clamp(int2(gid) + int2(x, y), int2(0), int2(size) - 1);
            // Объёмный свет (11) — четверть разрешения, добавляется здесь:
            // TAA заодно усредняет его шум.
            constexpr sampler vs(filter::linear, address::clamp_to_edge);
            float3 c = cur.read(uint2(p)).rgb + vol.sample(vs, (float2(p) + 0.5) / float2(size), level(0)).rgb;
            c = c / (1.0 + dot(c, float3(0.2126, 0.7152, 0.0722)));
            float3 yc = rgb_to_ycocg(c);
            m1 += yc;
            m2 += yc * yc;
            if (x == 0 && y == 0) {
                center = c;
            }
            float d = depth.read(uint2(p));
            if (d > dmax) {
                dmax = d;
                dpos = p;
            }
        }
    }
    float3 mean = m1 / 9.0;
    float3 sigma = sqrt(max(m2 / 9.0 - mean * mean, 0.0));

    // Перепроекция: NDC пикселя без сдвига → относительно камеры (однородно:
    // небо с глубиной 0 даёт направление, w = 0) → прошлый кадр.
    float2 uv = (float2(dpos) + 0.5) / float2(size);
    float2 ndc = float2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0) - t.jitter.xy;
    float4 rel = t.inv_view_proj_rel * float4(ndc, dmax, 1.0);
    float4 prev = t.prev_view_proj_rel * float4(rel.xyz + t.cam_delta.xyz * rel.w, rel.w);
    float2 pndc = prev.xy / prev.w;
    float2 puv = float2(pndc.x * 0.5 + 0.5, 0.5 - pndc.y * 0.5);
    // Смещение пикселя (без дрожи центра) — к uv самого пикселя.
    float2 own = (float2(gid) + 0.5) / float2(size);
    puv += own - uv;

    float3 result = center;
    bool valid = t.cam_delta.w > 0.5 && prev.w > 0.0 && all(puv >= 0.0) && all(puv <= 1.0);
    if (valid) {
        float3 h = sample_history(hist, puv, float2(size));
        h = h / (1.0 + dot(h, float3(0.2126, 0.7152, 0.0722)));
        // Почти неподвижный пиксель — коробка шире (±2,5σ) и истории больше:
        // контрастные текстуры не «дрожат» от сдвига субпикселя (04.5). При
        // движении — строже, против шлейфов.
        float motion = length((puv - own) * float2(size));
        float still = 1.0 - saturate(motion / 2.0);
        float gamma = mix(1.25, 2.5, still);
        float3 lo = mean - gamma * sigma, hi = mean + gamma * sigma;
        h = ycocg_to_rgb(clip_aabb(lo, hi, rgb_to_ycocg(center), rgb_to_ycocg(h)));
        float a = mix(0.1, 0.35, saturate(motion / 24.0)) * mix(1.0, 0.6, still);
        result = mix(h, center, a);
    }
    // Обратно из веса по яркости.
    result = result / max(1.0 - dot(result, float3(0.2126, 0.7152, 0.0722)), 1e-4);
    out.write(float4(result, 1.0), gid);
}
