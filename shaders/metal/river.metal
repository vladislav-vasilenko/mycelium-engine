#include "common.h"
#include "ground.h"
#include "sky.h"

// Поверхность рек (03.2, сессия 3): лента воды над каждым отрезком реки из
// буфера объектов карты. Уровень — дно (поле высот на оси русла) плюс глубина
// из гидрологии (`_pad.y`), ширина — полуширина плюс разлив (`_pad.x`).
// Течение — рябь бежит вдоль отрезка со скоростью по уклону; на крутых
// участках (скаты, перекаты) — белая пена полосами вдоль потока. Мелкая вода
// прозрачна (видно дно), глубокая темнее; отражение — небо и тёмные берега.

// Уровень воды демо: глубина гидрологии × 3 (тот же множитель — в
// `app::raft::LEVEL_SCALE`).
constant float LEVEL_SCALE = 3.0;

struct RiverOut {
    float4 position [[position]];
    float3 world;
    float2 flow;      // направление течения (XZ)
    float along;      // м вдоль течения от начала отрезка
    float across;     // −1…1 поперёк
    float slope;      // уклон отрезка
    float depth;      // глубина, м
};

vertex RiverOut river_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const FeatureSegment* segs [[buffer(2)]],
                         device const float* heights [[buffer(6)]],
                         device const float* zone_heights [[buffer(7)]],
                         constant TerrainZone& zone [[buffer(8)]]) {
    RiverOut o;
    FeatureSegment s = segs[iid];
    float dcam = distance(0.5 * (s.a + s.b), frame.camera_pos.xz);
    if (s.kind != 1u || dcam > 900.0) {
        o.position = float4(0.0, 0.0, -2.0, 1.0);
        return o;
    }
    // Геометрия — в порядке отрезков (соседи по буферу — та же линия):
    // общий угол на стыках (без щелей), нахлёст на концах без соседа.
    float2 a = s.a, b = s.b;
    float2 dir = normalize(b - a + 1e-6);
    float2 nrm = float2(-dir.y, dir.x);
    float len = length(b - a);
    float hw = s.half_width + s._pad.x;
    bool has_prev = iid > 0 && segs[iid - 1].kind == 1u && all(abs(segs[iid - 1].b - a) < 0.01);
    bool has_next = segs[iid + 1].kind == 1u && all(abs(segs[iid + 1].a - b) < 0.01);
    uint corner = vid % 6u;
    float u = (corner == 1u || corner == 2u || corner == 4u) ? 1.0 : -1.0;
    float v = (corner == 2u || corner == 4u || corner == 5u) ? 1.0 : 0.0;
    float2 end_n = nrm;
    float2 end_p = mix(a, b, v);
    if (v < 0.5) {
        if (has_prev) {
            float2 pd = normalize(a - segs[iid - 1].a + 1e-6);
            float2 m = normalize(float2(-pd.y, pd.x) + nrm);
            end_n = m / max(dot(m, nrm), 0.5);
        } else {
            end_p -= dir * hw * 0.5;
        }
    } else {
        if (has_next) {
            float2 nd = normalize(segs[iid + 1].b - b + 1e-6);
            float2 m = normalize(float2(-nd.y, nd.x) + nrm);
            end_n = m / max(dot(m, nrm), 0.5);
        } else {
            end_p += dir * hw * 0.5;
        }
    }
    float2 xz = end_p + end_n * u * hw;
    float ha = ground_height(a, heights, zone_heights, zone, frame);
    float hb = ground_height(b, heights, zone_heights, zone, frame);
    // Течение — от высокого конца к низкому.
    float2 flow = hb > ha ? -dir : dir;
    float depth = max(s._pad.y * LEVEL_SCALE, 0.3);
    // Поверхность — над дном оси: запас 12 см, рельеф на GPU (сглаживание,
    // шум участка детализации, упрощение вдали) выше поля высот на сантиметры.
    // Рельеф между концами может выпирать над прямой (ложе врезано
    // ступенями): уровень — не ниже середины отрезка.
    // В общей вершине уровень у обоих соседей один: максимум из высоты
    // вершины и середин обоих примыкающих отрезков (иначе — ступени и щели).
    float hm = ground_height(0.5 * (a + b), heights, zone_heights, zone, frame);
    float h_end;
    if (v < 0.5) {
        float hp = has_prev ? ground_height(0.5 * (segs[iid - 1].a + a), heights, zone_heights, zone, frame) : hm;
        h_end = max(ha, max(hm, hp));
    } else {
        float hn = has_next ? ground_height(0.5 * (b + segs[iid + 1].b), heights, zone_heights, zone, frame) : hm;
        h_end = max(hb, max(hm, hn));
    }
    float y = h_end + depth + 0.12;
    o.world = float3(xz.x, y, xz.y);
    o.position = frame.view_proj_rel * float4(camera_relative(frame, o.world), 1.0);
    o.flow = flow;
    o.along = v * len;
    o.across = u;
    // Уклон — по 10 м до и после отрезка: высоты по 4-метровым отрезкам
    // шумные, и почти везде выходил «скат» с пеной.
    float2 up_p = hb > ha ? b : a, dn_p = hb > ha ? a : b;
    float hu = ground_height(up_p - flow * 10.0, heights, zone_heights, zone, frame);
    float hd = ground_height(dn_p + flow * 10.0, heights, zone_heights, zone, frame);
    o.slope = max(hu - hd, 0.0) / (len + 20.0);
    o.depth = depth;
    return o;
}

fragment float4 river_fs(RiverOut in [[stage_in]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const SkyFrame& skyf [[buffer(3)]],
                         texture2d<float> detail [[texture(0)]],
                         texture3d<float> aerial [[texture(1)]]) {
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat);
    float t = frame.time;
    float slope = saturate(in.slope);
    // Скорость: перекат и скат — быстро, плёс — медленно.
    float speed = clamp(0.4 + sqrt(slope) * 6.0, 0.4, 5.0);
    float2 fd = normalize(in.flow);
    float2 fn = float2(-fd.y, fd.x);
    // Координаты в системе течения (м): вдоль — со сдвигом по времени.
    float2 q = float2(dot(in.world.xz, fn), dot(in.world.xz, fd));
    // Две фазы сдвига: текстура не растягивается бесконечно.
    float ph0 = fract(t * 0.25), ph1 = fract(t * 0.25 + 0.5);
    float w0 = 1.0 - abs(ph0 * 2.0 - 1.0);
    auto rip = [&](float ph, float2 sc) {
        float2 uv = (q - float2(0.0, ph * speed * 4.0)) / sc;
        return detail.sample(s, uv).rg - 0.5;
    };
    float2 r = (rip(ph0, float2(1.6, 2.4)) * w0 + rip(ph1, float2(1.6, 2.4)) * (1.0 - w0))
             + 0.5 * (rip(ph0, float2(0.5, 0.9)) * w0 + rip(ph1, float2(0.5, 0.9)) * (1.0 - w0));
    float turb = smoothstep(0.02, 0.12, slope);
    // Бурление: на скатах волны выше и круче.
    float3 n = normalize(float3(fn * r.x + fd * r.y, 0.0).xzy * (0.3 + 1.6 * turb) + float3(0.0, 1.0, 0.0));
    float3 V = normalize(frame.camera_pos.xyz - in.world);
    // Френель — по нормали с рябью (пятна неба на волнах), не меньше, чем
    // у ровной воды.
    float fres = max(0.02 + 0.98 * pow(1.0 - saturate(dot(n, V)), 5.0), 0.02 + 0.98 * pow(1.0 - saturate(V.y), 5.0));
    float3 R = reflect(-V, n);
    float3 sky_up = sky_ambient_term(skyf, float3(0, 1, 0));
    float3 sun = sky_sun_term(skyf);
    // В отражении: у горизонта — тёмные берега и лес, выше — небо.
    // Отражение: небо — как в лужах, но ярче (открытая вода), у горизонта —
    // тёмные берега и лес.
    float3 refl = mix(sky_up * 0.08 * float3(0.7, 0.85, 0.7), sky_up * 0.75, smoothstep(0.05, 0.35, R.y));
    // Пена: на скатах сплошная полосами вдоль течения, на перекатах и у
    // берегов — клочьями; бежит с водой.
    float fnoise = detail.sample(s, (q - float2(0.0, t * speed)) / float2(0.35, 1.4)).b;
    float fnoise2 = detail.sample(s, (q - float2(0.0, t * speed * 1.3)) / float2(0.9, 0.6)).r;
    float edge = smoothstep(0.6, 1.0, abs(in.across));
    // Мягкая пена: широкий порог, крупные пятна (fnoise2) несут мелкие
    // полосы (fnoise) — без резких белых штрихов.
    float streak = smoothstep(0.35, 0.95, fnoise);
    float patch = smoothstep(0.35, 0.8, fnoise2);
    float foam = saturate(streak * patch * (turb * 1.2 + 0.2 * edge) + smoothstep(0.06, 0.2, slope) * patch * 0.6) * 0.75;
    float3 foam_c = float3(0.82, 0.85, 0.86) * (sky_up + sun * max(-frame.light_dir.y, 0.0) * 0.5);
    // Толща: мелкая вода — прозрачная (видно дно), глубокая — тёмная бирюза.
    float path = in.depth / max(V.y, 0.15);
    float3 T = exp(-float3(0.35, 0.09, 0.06) * path * 3.0);
    float3 scatter = float3(0.02, 0.06, 0.055) * (sky_up + sun * 0.3);
    float3 water = refl * fres + scatter * (1.0 - T);
    float opacity = saturate(1.0 - dot(T, float3(0.33)) * (1.0 - fres));
    // Край ленты растворяется — без резкой границы на берегу.
    float soft = 1.0 - smoothstep(0.75, 1.0, abs(in.across));
    float3 col = mix(water, foam_c, foam);
    float a = mix(opacity, 0.85, foam) * soft;
    col = apply_aerial(aerial, frame, in.world - frame.camera_pos.xyz, col);
    return float4(col * a * frame.pre_exposure, a);
}

// --- Рыба аю (03.2): выпрыгивает вверх по течению по параболе с брызгами.
// Точки — с CPU (`Renderer::set_fish`): xz, уровень воды, угол течения.
struct FishOut {
    float4 position [[position]];
    float3 normal;
    float kind;   // 0 — тело, 1 — брызги
    float fade;
};

static float fish_hash(uint x) {
    x ^= x >> 16; x *= 0x7feb352du; x ^= x >> 15; x *= 0x846ca68bu; x ^= x >> 16;
    return float(x) / 4294967295.0;
}

vertex FishOut fish_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                       constant FrameUniforms& frame [[buffer(1)]],
                       device const float4* fish [[buffer(4)]]) {
    FishOut o;
    float4 f = fish[iid];
    float period = 3.0 + 5.0 * fish_hash(iid * 13u + 7u);
    float ph = fract(frame.time / period + fish_hash(iid * 31u + 1u));
    float jump_t = 0.75;                 // прыжок, с
    float t = ph * period;
    // Против течения: угол течения + π.
    float ang = f.w + 3.14159;
    float2 dir = float2(cos(ang), sin(ang));
    const float v0 = 3.0, vh = 1.1, g = 9.8;
    o.kind = 0.0;
    o.fade = 1.0;
    o.normal = float3(0, 1, 0);
    uint part = vid / 6u, corner = vid % 6u;
    float2 q = float2(corner == 1u || corner == 2u || corner == 4u ? 1.0 : -1.0,
                      corner == 2u || corner == 4u || corner == 5u ? 1.0 : -1.0);
    float3 base = float3(f.x, f.y, f.z);
    float3 p;
    if (part == 0u) {
        // Тело: в воздухе только во время прыжка; ромб вдоль скорости.
        if (t > jump_t) {
            o.position = float4(0, 0, -2, 1);
            return o;
        }
        float3 pos = base + float3(dir.x * vh * t, v0 * t - 0.5 * g * t * t, dir.y * vh * t);
        float3 vel = normalize(float3(dir.x * vh, v0 - g * t, dir.y * vh));
        float3 side = normalize(cross(vel, float3(0, 1, 0)) + 1e-4);
        float3 up = normalize(cross(side, vel));
        // Ромб: длина 0,28 м, высота 0,07 м; хвост — вторая пара вершин.
        float along = q.x * 0.14;
        float h = (1.0 - abs(q.x)) * 0.0 + (q.y > 0.0 ? 0.035 : -0.035) * (1.0 - abs(along) / 0.16);
        p = pos + vel * along + up * h;
        o.normal = side;
    } else {
        // Брызги: у взлёта (part 1–2) и у входа в воду (part 3–4).
        bool at_entry = part >= 3u;
        float te = at_entry ? t - jump_t : t;
        if (te < 0.0 || te > 0.45) {
            o.position = float4(0, 0, -2, 1);
            return o;
        }
        float3 at = base + (at_entry ? float3(dir.x * vh * jump_t, 0.0, dir.y * vh * jump_t) : float3(0.0));
        float a2 = float(part) * 1.7 + fish_hash(iid * 7u + part) * 6.28;
        float3 v = float3(cos(a2) * 0.7, 1.8, sin(a2) * 0.7);
        float3 pos = at + v * te - float3(0, 0.5 * g * te * te, 0);
        float3 to_cam = normalize(frame.camera_pos.xyz - pos);
        float3 side = normalize(cross(normalize(v), to_cam) + 1e-4);
        p = pos + side * q.x * 0.008 + normalize(v) * q.y * 0.02;
        o.kind = 1.0;
        o.fade = 1.0 - te / 0.45;
    }
    o.position = frame.view_proj_rel * float4(camera_relative(frame, p), 1.0);
    return o;
}

fragment float4 fish_fs(FishOut in [[stage_in]], bool front [[front_facing]],
                        constant FrameUniforms& frame [[buffer(1)]],
                        device const SkyFrame& skyf [[buffer(3)]]) {
    float3 sky = sky_ambient_term(skyf, float3(0, 1, 0));
    float3 sun = sky_sun_term(skyf) * saturate(-frame.light_dir.y);
    float3 c;
    if (in.kind > 0.5) {
        c = float3(0.9, 0.93, 0.95) * (sky + sun * 0.5) * in.fade;
    } else {
        // Аю: оливковая спина, серебристые бока с бликом.
        float3 n = normalize(front ? in.normal : -in.normal);
        float3 body = float3(0.35, 0.38, 0.3);
        c = body * (sky + sun * saturate(dot(n, -frame.light_dir.xyz)) ) + float3(0.6) * sky * 0.3;
    }
    return float4(c * frame.pre_exposure, 1.0);
}
