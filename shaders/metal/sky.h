// Атмосфера Земли по Hillaire 2020 («A Scalable and Production Ready Sky and
// Atmosphere Rendering Technique»): плотности Рэлея, Ми и озона, отображения
// LUT пропускания и вида неба. Расстояния — километры, коэффициенты — на км.
// Яркость — в долях освещённости солнца на верхней границе (E☉ = 1), затем
// умножается на SkyInputs.params.w.
#pragma once

constant float SKY_PI = 3.14159265;
constant float PLANET_R = 6360.0;
constant float ATMO_R = 6460.0;
constant float3 RAYLEIGH_SCAT = float3(5.802, 13.558, 33.1) * 1e-3;
constant float RAYLEIGH_H = 8.0;
constant float MIE_SCAT = 3.996e-3;
constant float MIE_ABS = 4.40e-3;
constant float MIE_H = 1.2;
constant float MIE_G = 0.8;
constant float3 OZONE_ABS = float3(0.650, 1.881, 0.085) * 1e-3;
constant float3 GROUND_ALBEDO = float3(0.1);
// Спектр солнца у верхней границы — слегка жёлтый.
constant float3 SUN_TINT = float3(1.0, 0.97, 0.92);
// Угловой радиус солнца и луны, рад (0,53° и 0,52° в диаметре).
constant float SUN_RADIUS = 0.004625;
constant float MOON_RADIUS = 0.004538;

// Освещённость от полной луны относительно солнца: настоящая ≈ 2,5·10⁻⁶,
// игровая в 10 раз ярче — лунной ночью маршрут читается.
constant float MOON_K = 2.5e-5;

// Размеры LUT (src/renderer/sky.rs).
constant uint2 TRANSMITTANCE_SIZE = uint2(256, 64);
constant uint2 MULTISCATTER_SIZE = uint2(32, 32);
constant uint2 SKYVIEW_SIZE = uint2(192, 108);

// Результат неба на GPU для освещения (пишет ядро sky_ambient).
struct SkyFrame {
    float4 light_dir;    // к главному светилу (солнце или луна)
    float4 light_color;  // освещённость от него у камеры, rgb
    float4 sh[9];        // освещённость от неба, сферические гармоники 2-го порядка
    float4 exposure;     // x — текущая, y — целевая, z — освещённость горизонтали
};

struct Medium {
    float3 scattering;
    float3 extinction;
    float3 rayleigh;
    float mie;
};

inline Medium medium_at(float h_km) {
    float r = exp(-h_km / RAYLEIGH_H);
    float m = exp(-h_km / MIE_H);
    float o = max(0.0, 1.0 - abs(h_km - 25.0) / 15.0);
    Medium s;
    s.rayleigh = RAYLEIGH_SCAT * r;
    s.mie = MIE_SCAT * m;
    s.scattering = s.rayleigh + s.mie;
    s.extinction = s.scattering + MIE_ABS * m + OZONE_ABS * o;
    return s;
}

inline float phase_rayleigh(float c) {
    return 3.0 / (16.0 * SKY_PI) * (1.0 + c * c);
}

// Корнетт-Шенкс: Ми с ярким ореолом вокруг солнца.
inline float phase_mie(float c) {
    float g = MIE_G;
    float k = 3.0 / (8.0 * SKY_PI) * (1.0 - g * g) / (2.0 + g * g);
    return k * (1.0 + c * c) / pow(1.0 + g * g - 2.0 * g * c, 1.5);
}

// Ближайшее пересечение луча со сферой радиуса R вокруг начала; < 0 — нет.
inline float ray_sphere(float3 ro, float3 rd, float R) {
    float b = dot(ro, rd);
    float c = dot(ro, ro) - R * R;
    float d = b * b - c;
    if (d < 0.0) {
        return -1.0;
    }
    float s = sqrt(d);
    float t0 = -b - s;
    float t1 = -b + s;
    if (t0 >= 0.0) {
        return t0;
    }
    return t1 >= 0.0 ? t1 : -1.0;
}

// LUT пропускания (Bruneton): (r, μ) ↔ uv.
inline float2 transmittance_uv(float r, float mu) {
    float H = sqrt(ATMO_R * ATMO_R - PLANET_R * PLANET_R);
    float rho = sqrt(max(r * r - PLANET_R * PLANET_R, 0.0));
    float disc = r * r * (mu * mu - 1.0) + ATMO_R * ATMO_R;
    float d = max(0.0, -r * mu + sqrt(max(disc, 0.0)));
    float d_min = ATMO_R - r;
    float d_max = rho + H;
    return float2((d - d_min) / (d_max - d_min), rho / H);
}

inline void transmittance_rmu(float2 uv, thread float& r, thread float& mu) {
    float H = sqrt(ATMO_R * ATMO_R - PLANET_R * PLANET_R);
    float rho = H * uv.y;
    r = sqrt(rho * rho + PLANET_R * PLANET_R);
    float d_min = ATMO_R - r;
    float d_max = rho + H;
    float d = d_min + uv.x * (d_max - d_min);
    mu = d == 0.0 ? 1.0 : (H * H - rho * rho - d * d) / (2.0 * r * d);
    mu = clamp(mu, -1.0, 1.0);
}

inline float3 sample_transmittance(texture2d<float> lut, float3 pos, float3 dir) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);
    float r = length(pos);
    float mu = dot(pos / r, dir);
    return lut.sample(s, transmittance_uv(r, mu), level(0)).rgb;
}

inline float3 sample_multiscatter(texture2d<float> lut, float3 pos, float3 sun) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);
    float r = length(pos);
    float mu_s = dot(pos / r, sun);
    float2 uv = float2(mu_s * 0.5 + 0.5, (r - PLANET_R) / (ATMO_R - PLANET_R));
    return lut.sample(s, uv, level(0)).rgb;
}

// Вид неба: нелинейная широта (сгущение к горизонту) и угол к солнцу по азимуту.
inline float2 skyview_uv(float view_height, float cos_view_zenith, float cos_light_view) {
    float vh = sqrt(max(view_height * view_height - PLANET_R * PLANET_R, 0.0));
    float beta = acos(clamp(vh / view_height, -1.0, 1.0));
    float zenith_horizon = SKY_PI - beta;
    float vza = acos(clamp(cos_view_zenith, -1.0, 1.0));
    float v;
    if (vza < zenith_horizon) {
        float c = 1.0 - sqrt(max(1.0 - vza / zenith_horizon, 0.0));
        v = c * 0.5;
    } else {
        float c = sqrt(max((vza - zenith_horizon) / beta, 0.0));
        v = 0.5 + c * 0.5;
    }
    float u = sqrt(clamp(-cos_light_view * 0.5 + 0.5, 0.0, 1.0));
    return float2(u, v);
}

inline void skyview_dirs(float2 uv, float view_height, thread float& cos_view_zenith,
                         thread float& cos_light_view) {
    float vh = sqrt(max(view_height * view_height - PLANET_R * PLANET_R, 0.0));
    float beta = acos(clamp(vh / view_height, -1.0, 1.0));
    float zenith_horizon = SKY_PI - beta;
    float vza;
    if (uv.y < 0.5) {
        float c = 1.0 - 2.0 * uv.y;
        c = 1.0 - c * c;
        vza = zenith_horizon * c;
    } else {
        float c = uv.y * 2.0 - 1.0;
        vza = zenith_horizon + beta * c * c;
    }
    cos_view_zenith = cos(vza);
    float c = uv.x * uv.x;
    cos_light_view = -(c * 2.0 - 1.0);
}

// Камера в осях атмосферы: центр планеты в начале, ось y — вверх.
inline float3 atmosphere_camera(float camera_y_m) {
    return float3(0.0, PLANET_R + max(camera_y_m * 0.001, 0.005), 0.0);
}

// Небо по направлению взгляда из LUT вида (без солнца и луны).
inline float3 sample_skyview(texture2d<float> lut, float3 cam, float3 view, float3 sun) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);
    float h = length(cam);
    float3 up = cam / h;
    float cos_vz = dot(view, up);
    float2 vxz = normalize(view.xz + float2(1e-6, 0.0));
    float2 sxz = normalize(sun.xz + float2(1e-6, 0.0));
    float cos_lv = dot(vxz, sxz);
    return lut.sample(s, skyview_uv(h, cos_vz, cos_lv), level(0)).rgb;
}

// Освещённость нормали n от неба по SH 2-го порядка (Ramamoorthi, Hanrahan).
inline float3 sh_irradiance(device const float4* sh, float3 n) {
    const float c1 = 0.429043, c2 = 0.511664, c3 = 0.743125, c4 = 0.886227, c5 = 0.247708;
    float3 e = c4 * sh[0].rgb - c5 * sh[6].rgb
             + 2.0 * c2 * (sh[3].rgb * n.x + sh[1].rgb * n.y + sh[2].rgb * n.z)
             + c3 * sh[6].rgb * n.z * n.z
             + c1 * sh[8].rgb * (n.x * n.x - n.y * n.y)
             + 2.0 * c1 * (sh[4].rgb * n.x * n.y + sh[7].rgb * n.x * n.z + sh[5].rgb * n.y * n.z);
    return max(e, 0.0);
}

// Свет сцены из SkyFrame в единицах материалов: «солнце» = E/π, «небо» =
// освещённость от неба по нормали / π; яркость Ламберта = альбедо·(солнце·nl + небо).
inline float3 sky_sun_term(device const SkyFrame& f) {
    return f.light_color.rgb / SKY_PI;
}

inline float3 sky_ambient_term(device const SkyFrame& f, float3 n) {
    return sh_irradiance(f.sh, n) / SKY_PI;
}

// Освещённость сверху относительно ясного полудня — масштаб цвета толщи под водой.
inline float underwater_light(device const SkyFrame& f) {
    float3 e = sky_sun_term(f) * max(f.light_dir.y, 0.0) + sky_ambient_term(f, float3(0, 1, 0));
    return dot(e, float3(0.2126, 0.7152, 0.0722)) / 0.42;
}
