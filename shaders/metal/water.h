// Море и прибой (подэтап 03.1): общие функции волн для отрисовки воды,
// маски мокрого песка и проверки CPU ↔ GPU. Повторяет src/water.rs —
// при изменении формул правятся оба файла, расхождение ловит самопроверка.
// Подключается после common.h.

constant float WATER_G = 9.81;
constant float WATER_PI = 3.14159265;

// Поле берега: RGBA32Float (field_size)², точка (i, j) — мировые XZ (i, j) · шаг.
// R — высота дна или грунта, G — знаковое расстояние до кромки (> 0 — суша),
// B — тип берега (0 песок, 1 камень).
struct ShoreSample {
    float bed;
    float coast_d;
    float rocky;
    float2 dir;   // направление к берегу (вдоль роста coast_d), единичное
};

inline float4 field_bilinear(texture2d<float> field, constant SeaParams& sp, float2 p) {
    float n = float(sp.field_size - 1);
    float2 g = clamp(p / sp.field_step_m, float2(0.0), float2(n));
    float2 g0 = min(floor(g), float2(n - 1.0));
    float2 f = g - g0;
    uint2 i = uint2(g0);
    float4 a = field.read(i);
    float4 b = field.read(i + uint2(1, 0));
    float4 c = field.read(i + uint2(0, 1));
    float4 d = field.read(i + uint2(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

inline ShoreSample shore_at(texture2d<float> field, constant SeaParams& sp, float2 p) {
    float4 v = field_bilinear(field, sp, p);
    float e = 2.0 * sp.field_step_m;
    float gx = field_bilinear(field, sp, p + float2(e, 0)).g - field_bilinear(field, sp, p - float2(e, 0)).g;
    float gz = field_bilinear(field, sp, p + float2(0, e)).g - field_bilinear(field, sp, p - float2(0, e)).g;
    float2 grad = float2(gx, gz);
    float len = length(grad);
    ShoreSample s;
    s.bed = v.r;
    s.coast_d = v.g;
    s.rocky = v.b;
    s.dir = len > 1e-6 ? grad / len : float2(0.0, 1.0);
    return s;
}

// Медленный сдвиг фазы вдоль берега: накат приходит не везде одновременно.
inline float alongshore_phase(float2 p) {
    return 1.7 * sin(p.x * 0.013) + 1.9 * sin(p.y * 0.011 + 1.3);
}

struct WaveOut {
    float3 pos;     // смещённая точка поверхности, мир
    float foam;     // пена: обрушение и брызги у камней, 0..1
    float swash;    // вес слоя заплеска на пляже, 0..1
    float shore;    // вес береговой волны, 0..1
};

// Точка поверхности для узла сетки `base` в момент `t`. `filter_cell` —
// шаг сетки, волны короче ~4 шагов гасятся (0 — без фильтра).
inline WaveOut wave_at(texture2d<float> field, constant SeaParams& sp, float2 base, float t, float filter_cell) {
    ShoreSample s = shore_at(field, sp, base);
    float depth0 = sp.sea_level - s.bed;
    float3 disp = float3(0.0);

    // Открытое море: волны Герстнера по ветру, гаснут на мелководье.
    float w_deep = smoothstep(4.0, 14.0, depth0);
    // У берега волны открытого моря погашены (w_deep = 0): цикл пропускается,
    // результат тот же — каждое слагаемое было бы нулём.
    for (uint i = 0; i < (w_deep > 0.0 ? sp.wave_count : 0u); ++i) {
        GerstnerWave w = sp.waves[i];
        float filt = filter_cell > 0.0 ? smoothstep(4.0 * filter_cell, 8.0 * filter_cell, w.wavelength) : 1.0;
        float k = 2.0 * WATER_PI / w.wavelength;
        float th = k * dot(w.dir, base) - w.omega * t + w.phase;
        float c = cos(th);
        float a = w.amplitude * w_deep * filt;
        disp.x += w.dir.x * w.steepness * a * c;
        disp.z += w.dir.y * w.steepness * a * c;
        disp.y += a * sin(th);
    }

    // Береговая волна: гребни — линии равного расстояния до кромки, поэтому
    // фронт параллелен берегу. К берегу волна укорачивается и растёт (shoaling),
    // при высоте ~0,8 глубины обрушивается.
    float u = max(-s.coast_d, 0.0);
    float ks = 2.0 * WATER_PI / sp.shore_wavelength;
    float th_s = ks * (u + 8.0 * log(1.0 + u / 8.0)) + sp.shore_omega * t + alongshore_phase(base);
    float d0 = max(depth0, 0.0);
    float amp = sp.shore_amplitude * (1.0 + 0.8 * (1.0 - smoothstep(1.0, 10.0, depth0)));
    amp = min(amp, 0.39 * d0 + 0.02);
    float w_shore = (1.0 - w_deep) * smoothstep(-0.2, 0.6, depth0);
    float sn = sin(th_s);
    disp.x += s.dir.x * 0.6 * amp * cos(th_s) * w_shore;
    disp.z += s.dir.y * 0.6 * amp * cos(th_s) * w_shore;
    disp.y += amp * sn * w_shore;
    float breaking = smoothstep(0.5, 0.78, 2.0 * amp / max(d0, 0.05)) * smoothstep(0.3, 0.9, sn) * w_shore;
    float splash = s.rocky * (1.0 - smoothstep(0.0, 6.0, depth0)) * smoothstep(0.2, 1.0, sn) * smoothstep(-0.2, 0.3, depth0);

    // Заплеск: после обрушения язык воды забегает на пляж и откатывается.
    // Фаза совпадает с приходом гребня к кромке, подъём быстрый, откат медленный.
    float phase = fract((sp.shore_omega * t + alongshore_phase(base) - 0.5 * WATER_PI) / (2.0 * WATER_PI));
    float profile = phase < 0.2 ? smoothstep(0.0, 0.2, phase) : 1.0 - smoothstep(0.2, 1.0, phase);
    float run_up = mix(sp.swash_sand_m, sp.swash_rock_m, s.rocky) * profile;
    float w_swash = (1.0 - smoothstep(0.3, 1.5, depth0)) * (1.0 - smoothstep(10.0, 40.0, s.coast_d));

    WaveOut o;
    float y = sp.sea_level + disp.y;
    y = mix(y, max(y, sp.sea_level + run_up), w_swash);
    o.pos = float3(base.x + disp.x, y, base.y + disp.z);
    o.foam = saturate(breaking + splash);
    o.swash = w_swash;
    o.shore = w_shore;
    return o;
}
