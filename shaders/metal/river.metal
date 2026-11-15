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
    float channel;    // 1 — русло (ложбина), 0 — склон: вода стекла бы вбок
};

static RiverOut river_vertex(uint vid, uint iid, constant FrameUniforms& frame,
                             device const FeatureSegment* segs, device const float* heights,
                             device const float* zone_heights, constant TerrainZone& zone,
                             device const float4* levels) {
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
    // Вода выше — разлив шире (откос берега ~0,6): +1,5 м на метр глубины.
    // Лента шире на подъём воды над дном (откос ~0,4): край воды задаёт
    // пересечение с берегом (растворение по глубине под пикселем), а не
    // прямой срез ленты.
    // Глубина демо — с CPU (`Hydrology::demo_depth`: на крутых ручьях мала).
    float depth = max(levels[iid].z, 0.1);
    float hw = s.half_width + s._pad.x + depth * 2.5;
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
    // Поверхность — над дном оси: запас 12 см, рельеф на GPU (сглаживание,
    // шум участка детализации, упрощение вдали) выше поля высот на сантиметры.
    // Рельеф между концами может выпирать над прямой (ложе врезано
    // ступенями): уровень — не ниже середины отрезка.
    // Уровень воды — профиль с CPU (`Hydrology::levels`): плёсы ровные и
    // горизонтальные, перепады — водопады и сливы.
    float4 lv = levels[iid];
    // Не ниже рельефа GPU + глубина: дно профиля — поле высот CPU, а рельеф
    // на GPU (детализация зоны, шум) у русла выше на десятки сантиметров —
    // вода выходила тонкой плёнкой и уходила под кочки. Над ямами профиль
    // выше — плёс остаётся ровным.
    // Только высота самой вершины — она общая у соседей (без ступенек).
    float y = max(v < 0.5 ? lv.x : lv.y, (v < 0.5 ? ha : hb) + depth);
    o.world = float3(xz.x, y, xz.y);
    o.position = frame.view_proj_rel * float4(camera_relative(frame, o.world), 1.0);
    // Течение у вершины — среднее с соседом на стыке: плавно вдоль ленты
    // (иначе узор ряби поворачивался скачком на каждом стыке).
    float2 vflow = flow;
    if (v < 0.5 && has_prev) {
        float2 pd = normalize(a - segs[iid - 1].a + 1e-6);
        vflow = normalize(flow + (dot(pd, dir) > 0.0 ? pd : -pd) * sign(dot(flow, dir)));
    } else if (v > 0.5 && has_next) {
        float2 nd = normalize(segs[iid + 1].b - b + 1e-6);
        vflow = normalize(flow + (dot(nd, dir) > 0.0 ? nd : -nd) * sign(dot(flow, dir)));
    }
    o.flow = vflow;
    o.along = v * len;
    o.across = u;
    // Уклон — по поверхности воды: на плёсе ~0, на перепаде — скат и
    // водопад. У вершины — среднее с соседом на стыке: пена без полос по
    // границам отрезков.
    float sl = abs(lv.x - lv.y) / max(len, 0.5);
    if (v < 0.5 && has_prev) {
        float4 pl = levels[iid - 1];
        sl = 0.5 * (sl + abs(pl.x - pl.y) / max(length(a - segs[iid - 1].a), 0.5));
    } else if (v > 0.5 && has_next) {
        float4 nl = levels[iid + 1];
        sl = 0.5 * (sl + abs(nl.x - nl.y) / max(length(segs[iid + 1].b - b), 0.5));
    }
    o.slope = sl;
    o.depth = depth;
    // Русло или склон: поперёк ленты (±0,6 полуширины, не ближе 3 м) рельеф с одной из
    // сторон много ниже воды — это склон (линия ручья легла мимо ложбины),
    // а не русло; вода здесь не держится — лента растворяется.
    {
        float2 c = end_p;
        // Не ближе 3 м: у узкого ручья ±0,6 полуширины — меньше метра, и
        // склон не отличался от русла.
        float side = max(hw * 0.6, 3.0);
        float hl = ground_height(c + end_n * side, heights, zone_heights, zone, frame);
        float hr = ground_height(c - end_n * side, heights, zone_heights, zone, frame);
        float drop = y - min(hl, hr);
        o.channel = 1.0 - smoothstep(depth + 1.0, depth + 2.5, drop);
    }
    return o;
}

vertex RiverOut river_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const FeatureSegment* segs [[buffer(2)]],
                         device const float* heights [[buffer(6)]],
                         device const float* zone_heights [[buffer(7)]],
                         constant TerrainZone& zone [[buffer(8)]],
                         device const float4* levels [[buffer(5)]]) {
    return river_vertex(vid, iid, frame, segs, heights, zone_heights, zone, levels);
}

// Поверхность реки в карте укрытия от дождя (04.3): капли бьются о воду
// (брызги на реке), а дно под ней — «под крышей» (без луж под рекой).
vertex float4 river_rain_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                            constant FrameUniforms& frame [[buffer(1)]],
                            device const FeatureSegment* segs [[buffer(2)]],
                            device const float* heights [[buffer(6)]],
                            device const float* zone_heights [[buffer(7)]],
                            constant TerrainZone& zone [[buffer(8)]],
                            device const float4* levels [[buffer(5)]],
                            constant float4x4& rain_vp [[buffer(12)]]) {
    RiverOut o = river_vertex(vid, iid, frame, segs, heights, zone_heights, zone, levels);
    if (o.position.z < -1.0) {
        return o.position;
    }
    return rain_vp * float4(camera_relative(frame, o.world), 1.0);
}

fragment float4 river_fs(RiverOut in [[stage_in]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         device const SkyFrame& skyf [[buffer(3)]],
                         texture2d<float> detail [[texture(0)]],
                         texture3d<float> aerial [[texture(1)]],
                         device const float* heights [[buffer(6)]],
                         device const float* zone_heights [[buffer(7)]],
                         constant TerrainZone& zone [[buffer(8)]]) {
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat);
    float t = frame.time;
    if (frame.underwater_m > 0.0 && frame.camera_pos.y < in.world.y) {
        // Поверхность снизу (как у моря, `water.metal`): в окне Снелла
        // (~48,6° от вертикали) — преломлённое небо, дальше — полное
        // внутреннее отражение толщи.
        float3 I = normalize(in.world - frame.camera_pos.xyz);
        float2 rp = (detail.sample(s, (in.world.xz - floor(frame.camera_pos.xz / 256.0) * 256.0) / 2.0
                                   + float2(0.03, 0.02) * t).rg - 0.5) * 0.25;
        float3 Nd = -normalize(float3(rp.x, 1.0, rp.y));
        float3 Rf = refract(I, Nd, 1.33);
        float light = underwater_light(skyf);
        float3 c;
        if (dot(Rf, Rf) < 1e-6) {
            c = UW_COLOR * 0.6 * light;
        } else {
            float F = 0.02 + 0.98 * pow(1.0 - saturate(dot(-I, Nd)), 5.0);
            c = sky_ambient_term(skyf, Rf) * (1.0 - F) + UW_COLOR * light * F;
        }
        c = underwater_fog(c, length(in.world - frame.camera_pos.xyz), frame.underwater_m, light);
        return float4(c * frame.pre_exposure, 1.0);
    }
    float slope = saturate(in.slope);
    // Скорость: перекат и скат — быстро, плёс — медленно.
    float speed = clamp(0.4 + sqrt(slope) * 6.0, 0.4, 5.0);
    float2 fd = normalize(in.flow);
    float2 fn = float2(-fd.y, fd.x);
    // Мировые координаты со сдвигом по (плавному) течению, две фазы — узор
    // непрерывен на стыках отрезков и не растягивается со временем.
    // Начало координат узора — у камеры, шаг 256 м (кратен всем масштабам
    // ниже — смена начала без скачка): мировые координаты ~20 км в float
    // теряют точность, а поворот в систему течения умножал малое изменение
    // направления на 20 км — узор пены сжимался в поперечные полосы.
    float2 xz = in.world.xz - floor(frame.camera_pos.xz / 256.0) * 256.0;
    float ph0 = fract(t * 0.25), ph1 = fract(t * 0.25 + 0.5);
    float w0 = 1.0 - abs(ph0 * 2.0 - 1.0);
    auto adv = [&](float ph, float k) { return fd * ph * speed * 4.0 * k; };
    auto rip = [&](float ph, float sc) {
        return detail.sample(s, (xz - adv(ph, 1.0)) / sc).rg - 0.5;
    };
    float2 r = (rip(ph0, 2.0) * w0 + rip(ph1, 2.0) * (1.0 - w0))
             + 0.5 * (rip(ph0, 0.8) * w0 + rip(ph1, 0.8) * (1.0 - w0));
    float turb = smoothstep(0.02, 0.12, slope);
    // Бурление: на скатах волны выше и круче.
    // Рябь всегда заметна (вода живая), на скатах — сильнее.
    float3 n = normalize(float3(r.x, 0.0, r.y) * (0.7 + 1.4 * turb) + float3(0.0, 1.0, 0.0));
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
    // Без SSR: низкие лучи отражают тёмные лесистые берега, высокие — небо;
    // переход резкий — на ряби пятна неба среди тёмного отражения леса.
    float3 refl = mix(sky_up * 0.05 * float3(0.6, 0.8, 0.6), sky_up * 0.6, smoothstep(0.18, 0.3, R.y));
    // Блики солнца на ряби.
    float3 L = -frame.light_dir.xyz;
    float3 Hh = normalize(L + V);
    refl += sun * pow(saturate(dot(n, Hh)), 400.0) * 2.0 * saturate(L.y * 3.0);
    // Пена (белая вода горных ручьёв): струи и клочья поверх воды, доля
    // покрытия растёт с уклоном: плёс — редкие линии, перекат — струи,
    // слив — почти сплошь. Узор в мировых координатах со сдвигом по течению;
    // вытянутость вдоль течения — среднее четырёх выборок по течению (без
    // поворота координат — без швов и полос). Порог по шуму с мягким краем —
    // форма клочьев, а не пятна «камуфляжа».
    auto foam_n = [&](float ph, float sc, float k) {
        return detail.sample(s, (xz - adv(ph, k)) / sc);
    };
    auto streak_n = [&](float ph) {
        float2 o = xz - adv(ph, 1.0);
        float v = 0.0;
        for (int k = 0; k < 4; ++k) {
            v += detail.sample(s, (o + fd * (float(k) - 1.5) * 0.3) / 0.4).b;
        }
        return v * 0.25;
    };
    float lines = streak_n(ph0) * w0 + streak_n(ph1) * (1.0 - w0);
    // Среднее сжимает разброс шума — растянуть обратно к 0…1.
    lines = saturate((lines - 0.5) * 1.8 + 0.5);
    float clumps = foam_n(ph0, 0.8, 1.2).r * w0 + foam_n(ph1, 0.8, 1.2).r * (1.0 - w0);
    float big = foam_n(ph0, 4.0, 0.8).g * w0 + foam_n(ph1, 4.0, 0.8).g * (1.0 - w0);
    float edge = smoothstep(0.55, 1.0, abs(in.across));
    float cover = saturate(smoothstep(0.04, 0.45, slope) * 0.75 + smoothstep(0.45, 0.9, slope) * 0.3 + 0.05 * edge);
    // Покрытие неравномерно: крупный шум делит русло на струи и окна воды.
    cover *= 0.55 + 0.9 * big;
    float fv = lines * 0.55 + clumps * 0.45;
    float thr = mix(0.8, 0.25, saturate(cover));
    float foam = smoothstep(thr, thr + 0.12, fv) * smoothstep(0.0, 0.08, cover);
    // Тонкая плёнка пены вокруг клочьев.
    foam = max(foam, 0.2 * smoothstep(thr - 0.12, thr, fv) * smoothstep(0.15, 0.45, cover));
    // Пена объёмная: в провалах ряби темнее.
    float3 foam_c = float3(0.8, 0.83, 0.84) * (sky_up + sun * max(-frame.light_dir.y, 0.0) * 0.5) * (0.75 + 0.25 * n.y * n.y + 0.3 * (fv - 0.5));
    // Толща: мелкая вода — прозрачная (видно дно), глубокая — тёмная бирюза.
    float path = in.depth / max(V.y, 0.15);
    // Толща под пикселем: уровень воды − дно (рельеф GPU). У берега мелко —
    // прозрачно, видно дно; на глубине — голубая, затем тёмно-бирюзовая
    // мгла. Путь света — по преломлённому лучу (закон Снеллиуса, n = 1,33):
    // под водой луч круче, чем над ней.
    float bed = ground_height(in.world.xz, heights, zone_heights, zone, frame);
    float dz = max(in.world.y - bed, 0.0);
    float3 Tr = refract(-V, n, 1.0 / 1.33);
    float path_w = dz / max(-Tr.y, 0.2);
    // Та же модель воды, что у моря (`water.metal`): поглощение и рассеяние
    // одной воды — у устья цвета совпадают. Реки Якусимы прозрачные, с
    // бирюзой на глубине; толща ×1,5 — немного взвеси от течения.
    // Плюс мутность горного потока (пузырьки, взвесь) — серое ослабление
    // 1,2/м: за метр толщи дно почти не видно, вода голубеет, а не
    // просвечивает до камней.
    const float3 absorb = float3(0.42, 0.085, 0.055) * 1.5 + 0.5;
    float3 T = exp(-absorb * path_w);
    // Рассеяние неба, преломлённого в толщу, ярче под рябью, повёрнутой к небу.
    float sky_in = 0.7 + 0.5 * saturate(n.y);
    float3 scatter = float3(0.010, 0.085, 0.095) * (sun * 0.6 + sky_up * sky_in) * 2.6;
    // Взвесь не только гасит, но и рассеивает свет: мутная вода светлая,
    // голубовато-бирюзовая (доля мутности в ослаблении ~0,7).
    scatter = mix(scatter, float3(0.035, 0.085, 0.095) * (sun * 0.4 * saturate(L.y) + sky_up * sky_in) * 2.6, 0.45);
    // Толща в ущелье затенена склонами и кронами (видимость неба ниже, чем
    // у моря): рассеяние ×0,6 — вода не светлее леса вокруг.
    float3 water = refl * fres + scatter * 0.6 * (1.0 - T) * (1.0 - fres);
    float opacity = saturate(1.0 - dot(T, float3(0.33)) * (1.0 - fres));
    // Край ленты растворяется — без резкой границы на берегу.
    // Край: вода растворяется к береговой линии (глубина под пикселем → 0)
    // и в крайней трети ленты — на склоне вбок прямой срез не виден.
    float soft = smoothstep(0.0, 0.35, dz) * (1.0 - smoothstep(0.55, 1.0, abs(in.across)));
    // Под пикселем много глубже расчётной глубины — лента висит над откосом
    // (сечение плоское, а склон уходит вниз; линия ручья на склоне): такой
    // воды нет — растворяется.
    soft *= (1.0 - smoothstep(in.depth + 0.8, in.depth + 2.0, dz)) * in.channel;
    // Устье: где дно ниже моря, лента растворяется в морской поверхности
    // (море рисуется своим проходом, с волнами и прибоем).
    soft *= smoothstep(-1.5, 0.0, bed);
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
    float period = 1.5 + 3.0 * fish_hash(iid * 13u + 7u);
    float ph = fract(frame.time / period + fish_hash(iid * 31u + 1u));
    float jump_t = 0.75;                 // прыжок, с
    float t = ph * period;
    // Против течения: угол течения + π.
    // Против течения с разбросом ±70° — прыжки видны сбоку, а не в лоб.
    float ang = f.w + 3.14159 + (fish_hash(iid * 5u + 3u) - 0.5) * 2.4;
    float2 dir = float2(cos(ang), sin(ang));
    const float v0 = 3.6, vh = 1.4, g = 9.8;
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
        // Плоскость тела — к камере (силуэт виден с любой стороны).
        float3 to_cam = normalize(frame.camera_pos.xyz - pos);
        float3 up = normalize(cross(vel, to_cam) + 1e-4);
        float3 side = normalize(cross(up, vel));
        // Ромб: длина 0,28 м, высота 0,07 м; хвост — вторая пара вершин.
        // Тело 0,35 м × 0,09 м (крупная аю — видно с плота); изгиб хвоста.
        float along = q.x * 0.175;
        float h = q.y * 0.045 * (q.x > 0.0 ? 1.0 : 0.6);
        float wag = sin(frame.time * 25.0) * 0.03 * step(q.x, 0.0);
        p = pos + vel * along + up * h + side * wag;
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
        c = float3(0.9, 0.93, 0.95) * (sky + sun * 0.5) * 0.4 * in.fade;
    } else {
        // Аю: оливковая спина, серебристые бока с бликом.
        float3 n = normalize(front ? in.normal : -in.normal);
        float3 body = float3(0.45, 0.48, 0.42);
        c = body * 0.35 * (sky + sun * saturate(dot(n, -frame.light_dir.xyz))) + float3(0.7, 0.72, 0.75) * sky * 0.12;
    }
    return float4(c * frame.pre_exposure, 1.0);
}
