// Небо (04.1): LUT атмосферы по Hillaire 2020, свет неба (SH) и экспозиция,
// проход неба за геометрией с солнцем, луной и звёздами.
#include "common.h"
#include "sky.h"

// --- LUT пропускания: один раз (зависит только от атмосферы). ---------------
kernel void sky_transmittance_lut(texture2d<float, access::write> out [[texture(0)]],
                                  uint2 gid [[thread_position_in_grid]]) {
    if (any(gid >= TRANSMITTANCE_SIZE)) {
        return;
    }
    float2 uv = (float2(gid) + 0.5) / float2(TRANSMITTANCE_SIZE);
    float r, mu;
    transmittance_rmu(uv, r, mu);
    float3 pos = float3(0.0, r, 0.0);
    float3 dir = float3(sqrt(max(1.0 - mu * mu, 0.0)), mu, 0.0);
    float t_max = ray_sphere(pos, dir, ATMO_R);
    const int STEPS = 40;
    float dt = t_max / STEPS;
    float3 depth = 0.0;
    for (int i = 0; i < STEPS; ++i) {
        float3 p = pos + dir * (float(i) + 0.5) * dt;
        depth += medium_at(length(p) - PLANET_R).extinction * dt;
    }
    out.write(float4(exp(-depth), 1.0), gid);
}

// --- LUT многократного рассеяния (Hillaire, раздел 5.5): один раз. ----------
kernel void sky_multiscatter_lut(texture2d<float, access::write> out [[texture(0)]],
                                 texture2d<float> transmittance [[texture(1)]],
                                 uint2 gid [[thread_position_in_grid]]) {
    if (any(gid >= MULTISCATTER_SIZE)) {
        return;
    }
    float2 uv = (float2(gid) + 0.5) / float2(MULTISCATTER_SIZE);
    float mu_s = uv.x * 2.0 - 1.0;
    float h = mix(0.0, ATMO_R - PLANET_R, uv.y);
    float3 pos = float3(0.0, PLANET_R + max(h, 0.01), 0.0);
    float3 sun = float3(sqrt(max(1.0 - mu_s * mu_s, 0.0)), mu_s, 0.0);
    const int DIRS = 8;
    const int STEPS = 20;
    float3 l2 = 0.0;
    float3 fms = 0.0;
    for (int a = 0; a < DIRS; ++a) {
        for (int b = 0; b < DIRS; ++b) {
            float cos_t = 1.0 - 2.0 * (float(a) + 0.5) / DIRS;
            float phi = 2.0 * SKY_PI * (float(b) + 0.5) / DIRS;
            float sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
            float3 dir = float3(sin_t * cos(phi), cos_t, sin_t * sin(phi));
            float t_ground = ray_sphere(pos, dir, PLANET_R);
            float t_top = ray_sphere(pos, dir, ATMO_R);
            float t_max = t_ground > 0.0 ? t_ground : t_top;
            float dt = t_max / STEPS;
            float3 T = 1.0;
            float3 L = 0.0;
            float3 F = 0.0;
            for (int i = 0; i < STEPS; ++i) {
                float3 p = pos + dir * (float(i) + 0.5) * dt;
                Medium m = medium_at(length(p) - PLANET_R);
                float3 step_t = exp(-m.extinction * dt);
                float3 t_sun = sample_transmittance(transmittance, p, sun);
                float shadow = ray_sphere(p, sun, PLANET_R) > 0.0 ? 0.0 : 1.0;
                float3 s = m.scattering * (1.0 / (4.0 * SKY_PI)) * t_sun * shadow;
                float3 integ = (s - s * step_t) / max(m.extinction, 1e-6);
                L += T * integ;
                float3 fi = (m.scattering - m.scattering * step_t) / max(m.extinction, 1e-6);
                F += T * fi;
                T *= step_t;
            }
            if (t_ground > 0.0) {
                float3 p = pos + dir * t_ground;
                float3 n = normalize(p);
                L += T * sample_transmittance(transmittance, p, sun) * saturate(dot(n, sun))
                     * GROUND_ALBEDO / SKY_PI;
            }
            float w = 1.0 / float(DIRS * DIRS);
            l2 += L * w;
            fms += F * w;
        }
    }
    out.write(float4(l2 / max(1.0 - fms, 1e-3), 1.0), gid);
}

// --- LUT вида неба: каждый кадр (солнце, луна и высота камеры меняются). ----
// Одно ядро для солнца и луны: светило — `light`, его яркость учитывается позже.
static void sky_view(texture2d<float, access::write> out, texture2d<float> transmittance,
                     texture2d<float> multiscatter, float camera_y, float3 light, uint2 gid) {
    if (any(gid >= SKYVIEW_SIZE)) {
        return;
    }
    float2 uv = (float2(gid) + 0.5) / float2(SKYVIEW_SIZE);
    float3 cam = atmosphere_camera(camera_y);
    float h = length(cam);
    float cos_vz, cos_lv;
    skyview_dirs(uv, h, cos_vz, cos_lv);
    // Локальная система LUT: вверх — y, светило в плоскости x-y.
    float3 sun_l = normalize(float3(sqrt(max(1.0 - light.y * light.y, 0.0)), light.y, 0.0));
    float sin_vz = sqrt(max(1.0 - cos_vz * cos_vz, 0.0));
    float sin_lv = sqrt(max(1.0 - cos_lv * cos_lv, 0.0));
    float3 dir = float3(sin_vz * cos_lv, cos_vz, sin_vz * sin_lv);
    float t_ground = ray_sphere(cam, dir, PLANET_R);
    float t_top = ray_sphere(cam, dir, ATMO_R);
    float t_max = t_ground > 0.0 ? t_ground : t_top;
    const int STEPS = 30;
    float3 T = 1.0;
    float3 L = 0.0;
    float c = dot(dir, sun_l);
    float pr = phase_rayleigh(c);
    float pm = phase_mie(c);
    float t_prev = 0.0;
    for (int i = 0; i < STEPS; ++i) {
        // Шаги гуще у камеры.
        float t = t_max * pow((float(i) + 0.5) / STEPS, 2.0);
        float dt = t - t_prev;
        t_prev = t;
        float3 p = cam + dir * t;
        Medium m = medium_at(length(p) - PLANET_R);
        float3 step_t = exp(-m.extinction * dt);
        float3 t_sun = sample_transmittance(transmittance, p, sun_l);
        float shadow = ray_sphere(p, sun_l, PLANET_R) > 0.0 ? 0.0 : 1.0;
        float3 ms = sample_multiscatter(multiscatter, p, sun_l);
        float3 s = (m.rayleigh * pr + m.mie * pm) * t_sun * shadow + m.scattering * ms;
        L += T * (s - s * step_t) / max(m.extinction, 1e-6);
        T *= step_t;
    }
    out.write(float4(L * SUN_TINT, 1.0), gid);
}

kernel void sky_view_lut(texture2d<float, access::write> out [[texture(0)]],
                         texture2d<float> transmittance [[texture(1)]],
                         texture2d<float> multiscatter [[texture(2)]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         constant SkyInputs& sky [[buffer(2)]],
                         uint2 gid [[thread_position_in_grid]]) {
    sky_view(out, transmittance, multiscatter, frame.camera_pos.y, normalize(sky.sun_dir.xyz), gid);
}

// Небо, освещённое луной: та же атмосфера, яркость умножается на MOON_K · фазу.
kernel void sky_view_lut_moon(texture2d<float, access::write> out [[texture(0)]],
                              texture2d<float> transmittance [[texture(1)]],
                              texture2d<float> multiscatter [[texture(2)]],
                              constant FrameUniforms& frame [[buffer(1)]],
                              constant SkyInputs& sky [[buffer(2)]],
                              uint2 gid [[thread_position_in_grid]]) {
    sky_view(out, transmittance, multiscatter, frame.camera_pos.y, normalize(sky.moon_dir.xyz), gid);
}

// --- Свет неба (SH), главное светило и экспозиция: одна группа потоков. -----
constant uint AMBIENT_THREADS = 64;
constant uint AMBIENT_SAMPLES = 64 * 32;

kernel void sky_ambient(texture2d<float> skyview [[texture(0)]],
                        texture2d<float> transmittance [[texture(1)]],
                        texture2d<float> skyview_moon [[texture(2)]],
                        constant FrameUniforms& frame [[buffer(1)]],
                        constant SkyInputs& sky [[buffer(2)]],
                        device SkyFrame& outf [[buffer(3)]],
                        uint tid [[thread_index_in_threadgroup]]) {
    threadgroup float3 acc[AMBIENT_THREADS][10];
    float3 cam = atmosphere_camera(frame.camera_pos.y);
    float3 sun = normalize(sky.sun_dir.xyz);
    float3 moon = normalize(sky.moon_dir.xyz);
    float E = sky.params.w;
    // Игровая луна в 10 раз ярче настоящей — лунной ночью маршрут читается.
    float moon_k = MOON_K * sky.moon_dir.w;
    float3 local[10];
    for (int k = 0; k < 10; ++k) {
        local[k] = 0.0;
    }
    // Верхняя полусфера: яркость неба из LUT; нижняя — отражённый от земли свет
    // (после первого прохода известна освещённость горизонтали).
    for (uint i = tid; i < AMBIENT_SAMPLES; i += AMBIENT_THREADS) {
        float u = (float(i % 64) + 0.5) / 64.0;
        float v = (float(i / 64) + 0.5) / 32.0;
        float y = v;  // только верхняя полусфера, равномерно по высоте
        float phi = 2.0 * SKY_PI * u;
        float s = sqrt(max(1.0 - y * y, 0.0));
        float3 d = float3(s * cos(phi), y, s * sin(phi));
        float3 L = sample_skyview(skyview, cam, d, sun) * E
                 + sample_skyview(skyview_moon, cam, d, moon) * E * moon_k;
        float w = 2.0 * SKY_PI / float(AMBIENT_SAMPLES);
        float sh[9] = {0.282095, 0.488603 * d.y, 0.488603 * d.z, 0.488603 * d.x,
                       1.092548 * d.x * d.y, 1.092548 * d.y * d.z,
                       0.315392 * (3.0 * d.z * d.z - 1.0), 1.092548 * d.x * d.z,
                       0.546274 * (d.x * d.x - d.y * d.y)};
        for (int k = 0; k < 9; ++k) {
            local[k] += L * sh[k] * w;
        }
        local[9] += L * d.y * w;  // освещённость горизонтали от неба
    }
    for (int k = 0; k < 10; ++k) {
        acc[tid][k] = local[k];
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);
    if (tid != 0) {
        return;
    }
    float3 total[10];
    for (int k = 0; k < 10; ++k) {
        total[k] = 0.0;
        for (uint t = 0; t < AMBIENT_THREADS; ++t) {
            total[k] += acc[t][k];
        }
    }
    // Главное светило: солнце, пока оно выше −1°, иначе луна (по CPU).
    float3 ldir = normalize(frame.light_dir.xyz) * -1.0;
    bool is_sun = dot(ldir, sun) > 0.999;
    float3 t_light = sample_transmittance(transmittance, cam, ldir);
    float above = smoothstep(-0.02, 0.01, ldir.y);
    // Луна: освещённость полной луны ≈ 2,5·10⁻⁶ солнечной, пропорционально фазе.
    float3 light = (is_sun ? E * SUN_TINT : E * moon_k * float3(0.9, 0.95, 1.0)) * t_light * above;
    float3 e_sky = total[9];
    // Земля под горизонтом: альбедо 0,1 от неба и светила.
    float3 ground_l = 0.1 / SKY_PI * (e_sky + light * max(ldir.y, 0.0));
    // Нижняя полусфера с постоянной яркостью: вклад в SH аналитически.
    total[0] += ground_l * 0.282095 * 2.0 * SKY_PI;
    total[1] += ground_l * 0.488603 * (-SKY_PI);
    // Ночной минимум: маршрут читается и без луны (звёзды и рассеянный свет).
    float3 night_floor = float3(0.35, 0.45, 0.7) * 3e-6 * E;
    total[0] += night_floor * 0.282095 * 4.0 * SKY_PI;
    for (int k = 0; k < 9; ++k) {
        outf.sh[k] = float4(total[k], 0.0);
    }
    outf.light_dir = float4(ldir, 0.0);
    outf.light_color = float4(light, 0.0);
    // Освещённость горизонтали — для отладки; экспозицию меряет кадр (exposure_meter).
    float3 e_h = light * max(ldir.y, 0.0) + e_sky + night_floor * SKY_PI;
    float lum = dot(e_h, float3(0.2126, 0.7152, 0.0722));
    outf.exposure = float4(0.0, 0.0, lum, 0.0);
}

// --- Проход неба: полноэкранный треугольник на глубине 0 (reversed-Z). ------
struct SkyOut {
    float4 position [[position]];
    float2 ndc;
};

vertex SkyOut sky_vs(uint vid [[vertex_id]]) {
    float2 p = float2((vid << 1) & 2, vid & 2) * 2.0 - 1.0;
    SkyOut o;
    o.position = float4(p, 0.0, 1.0);
    o.ndc = p;
    return o;
}

fragment float4 sky_fs(SkyOut in [[stage_in]],
                       constant FrameUniforms& frame [[buffer(1)]],
                       constant SkyInputs& sky [[buffer(2)]],
                       device const SkyFrame& skyf [[buffer(3)]],
                       texture2d<float> skyview [[texture(0)]],
                       texture2d<float> transmittance [[texture(1)]],
                       texture2d<float> skyview_moon [[texture(2)]]) {
    // Направление взгляда: обратная к view_proj_rel проекция точки ближней плоскости.
    float4 p = sky.inv_view_proj_rel * float4(in.ndc, 1.0, 1.0);
    float3 view = normalize(p.xyz / p.w);
    float3 cam = atmosphere_camera(frame.camera_pos.y);
    float3 sun = normalize(sky.sun_dir.xyz);
    float E = sky.params.w;
    if (frame.underwater_m > 0.0) {
        // Под водой фон — толща.
        return float4(UW_COLOR * underwater_light(skyf) * exp(-UW_ABSORB * frame.underwater_m)
                      * frame.pre_exposure, 1.0);
    }
    float3 moon_dir = normalize(sky.moon_dir.xyz);
    float3 col = sample_skyview(skyview, cam, view, sun) * E
               + sample_skyview(skyview_moon, cam, view, moon_dir) * E * MOON_K * sky.moon_dir.w;
    // Ночное небо не чёрное: свечение атмосферы и рассеянный свет звёзд.
    col += float3(0.25, 0.35, 0.65) * 4e-7 * E;
    float3 t_view = sample_transmittance(transmittance, cam, view);
    float hits_ground = ray_sphere(cam, view, PLANET_R) > 0.0 ? 0.0 : 1.0;
    // Солнце: диск 0,53° с потемнением к краю, яркость — E / телесный угол.
    float cs = dot(view, sun);
    float sun_r = acos(clamp(cs, -1.0, 1.0)) / SUN_RADIUS;
    if (sun_r < 1.0) {
        float mu = sqrt(max(1.0 - sun_r * sun_r, 0.0));
        float limb = 1.0 - 0.6 * (1.0 - mu);
        col += E * SUN_TINT * t_view * hits_ground * limb / (SKY_PI * SUN_RADIUS * SUN_RADIUS);
    }
    // Луна: диск 0,52°, освещённая сторона — по направлению на солнце.
    float3 moon = normalize(sky.moon_dir.xyz);
    float cm = dot(view, moon);
    float moon_r = acos(clamp(cm, -1.0, 1.0)) / MOON_RADIUS;
    if (moon_r < 1.0) {
        // Нормаль точки диска на сфере луны (к наблюдателю − направление).
        float3 tx = normalize(cross(moon, abs(moon.y) < 0.99 ? float3(0, 1, 0) : float3(1, 0, 0)));
        float3 ty = cross(tx, moon);
        float2 q = float2(dot(view - moon * cm, tx), dot(view - moon * cm, ty)) / MOON_RADIUS;
        float z = sqrt(max(1.0 - dot(q, q), 0.0));
        float3 nrm = normalize(-moon * z + tx * q.x + ty * q.y);
        float lit = saturate(dot(nrm, sun));
        // Альбедо 0,12 с пятнами «морей» по шуму направлений.
        float maria = 0.75 + 0.25 * sin(q.x * 9.0 + 1.3) * sin(q.y * 7.0 - 0.4);
        float3 moon_l = E * 0.12 / SKY_PI * lit * maria;
        // Пепельный свет: тёмная часть едва видна.
        moon_l += E * 2e-8;
        col += moon_l * t_view * hits_ground * smoothstep(1.0, 0.97, moon_r);
    }
    return float4(col * frame.pre_exposure, 1.0);
}

// --- Экспозиция по кадру: логарифмическое среднее яркости HDR с плавной ------
// адаптацией (глаз и камера меряют то, что видят, а не освещённость земли).
constant uint METER_THREADS = 256;
constant uint2 METER_GRID = uint2(64, 36);

kernel void exposure_meter(texture2d<float> hdr [[texture(0)]],
                           constant FrameUniforms& frame [[buffer(1)]],
                           constant SkyInputs& sky [[buffer(2)]],
                           device float4* state [[buffer(4)]],
                           uint tid [[thread_index_in_threadgroup]]) {
    threadgroup float2 acc[METER_THREADS];
    constexpr sampler s(filter::linear, address::clamp_to_edge);
    float2 sum = 0.0;
    uint n = METER_GRID.x * METER_GRID.y;
    for (uint i = tid; i < n; i += METER_THREADS) {
        float2 uv = (float2(i % METER_GRID.x, i / METER_GRID.x) + 0.5) / float2(METER_GRID);
        float3 c = hdr.sample(s, uv, level(0)).rgb;
        float l = dot(c, float3(0.2126, 0.7152, 0.0722)) / max(frame.pre_exposure, 1e-9);
        // Центр кадра весит больше (центрально-взвешенный замер).
        float2 d = uv - 0.5;
        float w = 1.0 + 2.0 * exp(-dot(d, d) * 12.0);
        sum += float2(log(max(l, 1e-9)) * w, w);
    }
    acc[tid] = sum;
    threadgroup_barrier(mem_flags::mem_threadgroup);
    if (tid != 0) {
        return;
    }
    float2 total = 0.0;
    for (uint t = 0; t < METER_THREADS; ++t) {
        total += acc[t];
    }
    float avg = exp(total.x / total.y);
    // Ключ 0,22 днём — средний кадр в полутон; ночью ключ падает до 0,03:
    // глаз адаптируется к темноте не полностью, ночь остаётся ночью.
    float day = smoothstep(log(1e-6), log(3e-3), log(avg));
    float key = mix(0.03, 0.22, day);
    float target = clamp(key / avg, 0.5, 1e5);
    float cur = state[0].x > 0.0 ? state[0].x : target;
    float k = 1.0 - exp(-sky.params.y / 0.8);
    cur = exp(mix(log(cur), log(target), k));
    state[0] = float4(cur, target, avg, 0.0);
}

// --- Звёзды: каталог BSC5 точками на бесконечности. --------------------------
// Освещённость от звезды 0m ≈ 2,5·10⁻¹¹ солнечной; свет делится на телесный
// угол пикселя. Днём при экспозиции ~3 звёзды тонут в небе, ночью проступают.
struct StarVertex {
    packed_float3 dir;   // экваториальные оси J2000
    float vmag;
    float bv;
};

struct StarOut {
    float4 position [[position]];
    float point_size [[point_size]];
    float3 color;
};

inline float3 star_color(float bv) {
    float t = clamp((bv + 0.3) / 2.0, 0.0, 1.0);
    return mix(float3(0.62, 0.72, 1.0), float3(1.0, 0.72, 0.42), t);
}

vertex StarOut stars_vs(uint vid [[vertex_id]],
                        device const StarVertex* stars [[buffer(0)]],
                        constant FrameUniforms& frame [[buffer(1)]],
                        constant SkyInputs& sky [[buffer(2)]],
                        texture2d<float> transmittance [[texture(1)]]) {
    StarVertex s = stars[vid];
    float3 e = float3(s.dir);
    float3 w = float3(dot(sky.star_rot[0].xyz, e), dot(sky.star_rot[1].xyz, e),
                      dot(sky.star_rot[2].xyz, e));
    StarOut o;
    float4 clip = frame.view_proj_rel * float4(w * 1000.0, 1.0);
    // На бесконечности (reversed-Z: 0): проходит тест только по пустому небу.
    clip.z = 0.0;
    o.position = clip;
    float3 cam = atmosphere_camera(frame.camera_pos.y);
    float3 t = sample_transmittance(transmittance, cam, w) * smoothstep(-0.01, 0.02, w.y);
    // Художественное усиление ×6: глаз ночью адаптируется к темноте (палочки),
    // а экран нет — без него видны только звёзды ярче 3m.
    float flux = 6.0 * 2.5e-11 * pow(10.0, -0.4 * s.vmag) * sky.params.w;
    // Лёгкое мерцание у горизонта сильнее.
    float tw = 1.0 + 0.25 * (1.0 - saturate(w.y * 4.0)) * sin(sky.params.z * 13.0 + float(vid) * 7.31);
    float size = s.vmag < 2.0 ? 2.0 : 1.0;
    o.point_size = size;
    o.color = star_color(s.bv) * flux * t * tw / (sky.view.x * size * size) * frame.pre_exposure;
    return o;
}

fragment float4 stars_fs(StarOut in [[stage_in]]) {
    return float4(in.color, 1.0);
}
