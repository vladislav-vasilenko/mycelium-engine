#include "common.h"
#include "water.h"
#include "sky.h"

// Море и прибой (подэтап 03.1). Кольцевая сетка вокруг камеры: экземпляр —
// уровень, вершина — узел сетки (N + 1)². Цвет воды по толщине до дна из поля
// берега, отражение неба по Френелю, блик солнца, пена. Смешивание
// dual-source: dst × пропускание воды (по каналам) + свой цвет.

constant uint WATER_GRID_N = 64;
// Приподнятие сетки в полосе прибоя, м: треугольники гарантированно накрывают
// кромку, а точную поверхность и глубину считает фрагментный шейдер.
constant float SURF_LIFT_M = 0.15;

// Виды: src/terrain_materials.rs, TerrainView.
constant uint VIEW_LIT = 0;
constant uint VIEW_WETNESS = 6;
constant uint VIEW_WATER_DEPTH = 8;
constant uint VIEW_WATER_DIR = 9;
constant uint VIEW_SURF = 10;
constant uint VIEW_RAW = 15;

struct WaterVaryings {
    float4 position [[position]];
    float3 world;
    float2 base;
    float3 normal;
    float foam;
    float swash;
    float shore;
    float2 inner_min [[flat]];
    float2 inner_max [[flat]];
};

// Узел уровня с учётом сшивки: нечётные узлы внешнего края лежат посередине
// между соседями, их точка — среднее соседних точек, как у грубого уровня.
inline WaveOut level_point(texture2d<float> field, constant SeaParams& sp, WaterLevel lv,
                           int i, int j, float t) {
    int n = int(WATER_GRID_N);
    float2 base = lv.origin + float2(i, j) * lv.cell;
    bool edge_x = (i == 0 || i == n) && (j & 1) == 1;
    bool edge_z = (j == 0 || j == n) && (i & 1) == 1;
    if (lv.morph_edge != 0 && (edge_x || edge_z)) {
        float2 off = edge_x ? float2(0, lv.cell) : float2(lv.cell, 0);
        WaveOut a = wave_at(field, sp, base - off, t, 2.0 * lv.cell);
        WaveOut b = wave_at(field, sp, base + off, t, 2.0 * lv.cell);
        WaveOut o;
        o.pos = 0.5 * (a.pos + b.pos);
        o.foam = 0.5 * (a.foam + b.foam);
        o.swash = 0.5 * (a.swash + b.swash);
        o.shore = 0.5 * (a.shore + b.shore);
        return o;
    }
    // На сшиваемом крае и чётные узлы фильтруют волны как грубый уровень,
    // иначе высоты на общей границе расходятся и появляется щель.
    bool border = i == 0 || i == n || j == 0 || j == n;
    float filt = (lv.morph_edge != 0 && border) ? 2.0 * lv.cell : lv.cell;
    return wave_at(field, sp, base, t, filt);
}

vertex WaterVaryings water_vs(uint vid [[vertex_id]],
                              uint iid [[instance_id]],
                              constant FrameUniforms& frame [[buffer(1)]],
                              constant SeaParams& sp [[buffer(2)]],
                              constant WaterLevel* levels [[buffer(3)]],
                              texture2d<float> field [[texture(0)]]) {
    WaterLevel lv = levels[iid];
    int n = int(WATER_GRID_N);
    int i = int(vid) % (n + 1);
    int j = int(vid) / (n + 1);
    float t = frame.time;
    WaveOut w = level_point(field, sp, lv, i, j, t);
    // Нормаль по соседним узлам того же уровня.
    WaveOut wx = level_point(field, sp, lv, min(i + 1, n), j, t);
    WaveOut wz = level_point(field, sp, lv, i, min(j + 1, n), t);
    WaveOut wx0 = level_point(field, sp, lv, max(i - 1, 0), j, t);
    WaveOut wz0 = level_point(field, sp, lv, i, max(j - 1, 0), t);
    float3 n3 = normalize(cross(wz.pos - wz0.pos, wx.pos - wx0.pos));
    WaterVaryings out;
    float lift = SURF_LIFT_M * saturate(max(w.shore, w.swash) * 4.0);
    out.world = w.pos + float3(0.0, lift, 0.0);
    out.base = lv.origin + float2(i, j) * lv.cell;
    out.position = clip_from_world(frame, out.world);
    out.normal = n3.y < 0.0 ? -n3 : n3;
    out.foam = w.foam;
    out.swash = w.swash;
    out.shore = w.shore;
    out.inner_min = lv.inner_min;
    out.inner_max = lv.inner_max;
    return out;
}

struct WaterOut {
    float4 color [[color(0), index(0)]];
    float4 transmit [[color(0), index(1)]];
    float depth [[depth(any)]];
};




fragment WaterOut water_fs(WaterVaryings in [[stage_in]],
                           constant FrameUniforms& frame [[buffer(1)]],
                           constant SeaParams& sp [[buffer(2)]],
                           texture2d<float> field [[texture(0)]],
                           texture2d<float> detail [[texture(1)]],
                           device const SkyFrame& skyf [[buffer(4)]],
                           constant SkyInputs& skyin [[buffer(5)]],
                           texture2d<float> skyview [[texture(2)]]) {
    // Небо 04.1: отражение — из LUT вида неба, свет — от светила с пропусканием.
    float3 sky_cam = atmosphere_camera(frame.camera_pos.y);
    float3 sky_sun = normalize(skyin.sun_dir.xyz);
    float sky_e = skyin.params.w;
    auto sky_color = [&](float3 dir) {
        dir.y = max(dir.y, 0.01);
        return sample_skyview(skyview, sky_cam, normalize(dir), sky_sun) * sky_e;
    };
    // Внутреннюю часть уровня рисует более мелкий уровень.
    if (all(in.base > in.inner_min) && all(in.base < in.inner_max)) {
        discard_fragment();
    }
    WaterOut o;
    o.depth = in.position.z;
    float3 world = in.world;
    float4 f = field_bilinear(field, sp, world.xz);
    float bed = f.r;
    if (in.shore > 0.001 || in.swash > 0.001) {
        // Полоса прибоя: высота береговой волны и заплеска нелинейна по глубине,
        // а линейная интерполяция между вершинами и тест глубины по треугольникам
        // дают ступени с шагом сетки. Ищем пересечение луча взгляда с точной
        // поверхностью (два уточнения) и выдаём его глубину.
        float3 ro = frame.camera_pos.xyz;
        float3 rd = normalize(world - ro);
        if (abs(rd.y) > 1e-3) {
            for (int it = 0; it < 2; ++it) {
                float y = wave_at(field, sp, world.xz, frame.time, 0.0).pos.y;
                world = ro + rd * ((y - ro.y) / rd.y);
            }
            float4 clip = clip_from_world(frame, world);
            o.depth = clip.z / clip.w;
            f = field_bilinear(field, sp, world.xz);
            bed = f.r;
        }
    }
    float depth = world.y - bed;          // толщина воды по вертикали
    float still = sp.sea_level - bed;     // глубина спокойной воды

    if (frame.view_mode == VIEW_RAW || (frame.view_mode != VIEW_LIT && frame.view_mode < VIEW_WATER_DEPTH)) {
        // Самопроверка и отладочные виды ландшафта: вода прозрачна.
        o.color = float4(0.0);
        o.transmit = float4(1.0);
        return o;
    }
    if (depth <= 0.0) {
        discard_fragment();
    }
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));

    if (frame.view_mode >= VIEW_WATER_DEPTH) {
        float3 c;
        if (frame.view_mode == VIEW_WATER_DEPTH) {
            c = mix(float3(0.6, 1.0, 0.9), float3(0.0, 0.05, 0.35), saturate(still / 20.0));
        } else if (frame.view_mode == VIEW_WATER_DIR) {
            ShoreSample ss = shore_at(field, sp, world.xz);
            c = float3(0.5 + 0.5 * ss.dir.x, 0.5 * in.shore, 0.5 + 0.5 * ss.dir.y);
        } else {
            c = float3(in.foam, in.swash, in.shore * 0.5);
        }
        o.color = float4(c, 1.0);
        o.transmit = float4(0.0);
        return o;
    }

    float t = frame.time;
    if (frame.underwater_m > 0.0) {
        // Поверхность снизу: в конусе ~48,6° от вертикали (окно Снелла) небо
        // преломляется внутрь, дальше — полное внутреннее отражение толщи.
        float3 I = normalize(world - frame.camera_pos.xyz);
        float2 rip = (detail.sample(s, world.xz / 5.0 + float2(0.03, 0.02) * t).rg - 0.5) * 0.2;
        float3 Nd = -normalize(in.normal + float3(rip.x, 0.0, rip.y));
        float3 Rf = refract(I, Nd, 1.33);
        float3 c;
        if (dot(Rf, Rf) < 1e-6) {
            c = UW_COLOR * 0.6 * underwater_light(skyf);
        } else {
            float F = 0.02 + 0.98 * pow(1.0 - saturate(dot(-I, Nd)), 5.0);
            c = sky_color(Rf) * (1.0 - F) + UW_COLOR * underwater_light(skyf) * F;
        }
        c = underwater_fog(c, length(world - frame.camera_pos.xyz), frame.underwater_m,
                           underwater_light(skyf));
        o.color = float4(c * frame.pre_exposure, 1.0);
        o.transmit = float4(0.0);
        return o;
    }
    float3 V = normalize(frame.camera_pos.xyz - world);
    // Мелкая рябь поверх нормали волн: две бегущие выборки шума.
    float2 r1 = detail.sample(s, world.xz / 7.0 + float2(0.03, 0.02) * t).rg - 0.5;
    float2 r2 = detail.sample(s, world.xz / 3.1 - float2(0.02, 0.035) * t).rg - 0.5;
    float2 ripple = (r1 + 0.6 * r2) * 0.18;
    float3 N = normalize(in.normal + float3(ripple.x, 0.0, ripple.y));

    float nv = saturate(dot(N, V));
    float F = 0.02 + 0.98 * pow(1.0 - nv, 5.0);
    float3 R = reflect(-V, N);
    float3 refl = sky_color(R);

    // Блик солнца (GGX, гладкая вода).
    float3 L = skyf.light_dir.xyz;
    float3 H = normalize(L + V);
    float nl = saturate(dot(N, L));
    float nh = saturate(dot(N, H));
    float a2 = 0.0016;
    float dd = nh * nh * (a2 - 1.0) + 1.0;
    float spec = a2 / (WATER_PI * dd * dd) * F * nl / max(4.0 * nv, 0.05);
    // «Солнце» — освещённость светила / π в единицах материалов (sky.h).
    float3 sun = sky_sun_term(skyf);
    float3 sky_up = sky_ambient_term(skyf, float3(0, 1, 0));

    // Пропускание по пути взгляда через толщу: красный гаснет быстрее —
    // бирюза на мелководье, синева на глубине.
    float path = min(depth / max(V.y, 0.08), 60.0);
    const float3 absorb = float3(0.42, 0.085, 0.055);
    float3 T = exp(-absorb * path);
    // За краем карты под водой нет геометрии дна: вода там непрозрачна,
    // иначе сквозь неё просвечивает цвет очистки (небо).
    float2 edge_d = min(world.xz, float2(frame.map_size_m) - world.xz);
    T *= smoothstep(0.0, 30.0, min(edge_d.x, edge_d.y));
    float3 scatter = float3(0.010, 0.085, 0.095) * (sun * 0.6 + sky_up) * 2.6;
    float3 inscatter = scatter * (1.0 - T);

    // Пена: обрушение и камни из функции волн, кромка заплеска, шум для рваного края.
    float2 fdir = shore_at(field, sp, world.xz).dir;
    float noise = detail.sample(s, world.xz / 2.3 - fdir * t * 0.35).r;
    float noise2 = detail.sample(s, world.xz / 1.7 + fdir * t * 0.2).b;
    float edge = (1.0 - smoothstep(0.0, 0.14, depth)) * in.swash;
    float foam = saturate((in.foam * 1.3 + edge) * smoothstep(0.4, 0.8, noise * 0.75 + noise2 * 0.35));
    float3 foam_col = float3(0.85, 0.88, 0.9) * (sun * saturate(dot(in.normal, L)) * 0.8 + sky_up);

    // Мягкая кромка: слой тоньше ~15 см постепенно растворяется. Видимая граница
    // определяется прозрачностью, а не пересечением треугольников воды и рельефа.
    // Растворение начинается на 3 см выше дна: рельеф нарисован треугольниками
    // 1 м и отличается от гладкого (билинейного) дна поля на 1–2 см. К месту,
    // где вода уходит под треугольники, она уже прозрачна — видна гладкая линия
    // равной глубины, а не диагонали сетки рельефа.
    float thin = smoothstep(0.03, 0.2, depth);
    thin *= thin;
    float3 water = refl * F + SKY_PI * sun * spec + inscatter * (1.0 - F);
    float3 col = mix(water, foam_col, foam) * thin;
    float3 tr = mix(float3(1.0), T * (1.0 - F) * (1.0 - foam), thin);
    o.color = float4(col * frame.pre_exposure, 1.0);
    o.transmit = float4(tr, 1.0);
    return o;
}

// Маска мокрого песка: 1 м на тексель, R16Float. Где вода выше грунта — 1,
// иначе высыхает за dry_time_s. Под водой — 1, далеко на суше не меняется.
kernel void wet_sand_update(texture2d<float, access::read_write> wet [[texture(0)]],
                            texture2d<float> field [[texture(1)]],
                            constant FrameUniforms& frame [[buffer(1)]],
                            constant SeaParams& sp [[buffer(2)]],
                            uint2 gid [[thread_position_in_grid]]) {
    if (gid.x >= wet.get_width() || gid.y >= wet.get_height()) {
        return;
    }
    float2 p = (float2(gid) + 0.5) * (frame.map_size_m / float(wet.get_width()));
    float4 f = field_bilinear(field, sp, p);
    float coast_d = f.g;
    float v = wet.read(gid).r;
    // Все переходы растянуты на несколько текселей: двоичная маска при
    // билинейной выборке рисует ступени 1 м. Под водой мокро всегда.
    float under = smoothstep(-1.0, -6.0, coast_d);
    if (coast_d < 45.0) {
        WaveOut w = wave_at(field, sp, p, frame.time, 0.0);
        float covered = smoothstep(-0.12, 0.25, w.pos.y - f.r);
        v = max(covered, v - sp.dt / sp.dry_time_s);
    }
    v = max(v, under);
    wet.write(float4(v), gid);
}

// Проверка функции волн: точки (x, z, t) из SeaParams.probe → смещённая точка и пена.
kernel void water_probe(device float4* out [[buffer(0)]],
                        constant SeaParams& sp [[buffer(2)]],
                        texture2d<float> field [[texture(0)]],
                        uint gid [[thread_position_in_grid]]) {
    if (gid >= WATER_PROBES) {
        return;
    }
    float4 p = sp.probe[gid];
    WaveOut w = wave_at(field, sp, p.xy, p.z, 0.0);
    out[gid] = float4(w.pos.x, w.pos.y, w.pos.z, w.foam);
}
