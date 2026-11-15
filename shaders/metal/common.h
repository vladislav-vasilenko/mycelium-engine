// Общие типы для CPU и GPU. Раскладка повторяет src/layout.rs,
// проверка — ядро layout_probe и `cargo run -- --selftest`.
#include <metal_stdlib>
using namespace metal;

struct FrameUniforms {
    float4x4 view_proj;
    float4 camera_pos;     // w — кинематографичная обработка в тонмаппинге (04.5): 1 — вкл.
    float4 light_dir;      // от источника к сцене
    float time;
    float exposure;
    float pattern_time;
    uint frame_index;
    uint view_mode;        // TerrainView, 0 — с освещением
    float map_size_m;      // сторона карты: XZ → координаты маски влажности
    uint grid_nx;          // поле высот кластерного рельефа: узлов по x
    float grid_step_m;     // и шаг, м
    float underwater_m;    // глубина глаз под водой, 0 — над водой
    float canopy_eye;      // доля леса у глаз 0…1: тень полога на руках
    float pre_exposure;    // освещённые шейдеры пишут яркость × это (04.1)
    float forest;          // лес нарисован (02.4): без ложных пятен полога
    float4x4 view_proj_rel; // проекция × поворот вида без сдвига (AD-49)
    float4 cam_hi;         // камера = cam_hi + cam_lo; cam_hi кратна 256 м; w — масштаб рендера (04.6, смещение мипов)
    float4 cam_lo;         // w — шаг времени мира с прошлого кадра, с (04.6: ветер в векторах движения)
    float4 weather;        // 04.3: x — облачность, y — дождь, z — влажность земли + уровень луж (0…2), w — ветер
    float4 spot_pos;       // 05.6: прожектор дрона — xyz относительно cam_hi, w — cos половины угла (0 — выкл.)
    float4 spot_dir;       // xyz — направление луча, w — сила света
    float4 mycelium;       // 05.6: x — связь с мицелием, y — галлюциногенный переход, zw — цель пульса (xz отн. cam_hi)
};

// Шум значений для узоров мицелия (05.6): гладкий, без текстур.
inline float myc_hash(float2 p) {
    p = fract(p * float2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}
inline float myc_noise(float2 p) {
    float2 i = floor(p), f = fract(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(myc_hash(i), myc_hash(i + float2(1, 0)), u.x),
               mix(myc_hash(i + float2(0, 1)), myc_hash(i + float2(1, 1)), u.x), u.y);
}

// Нити мицелия под почвой (05.6, сессия 2): только вдоль маршрута уровня —
// полоса ~3 м вокруг оси тропы (`route` — расстояние до оси, `left` — путь
// до цели по тропе). Сеть — гребни шума двух масштабов с изгибом; пульс бежит
// по пути к цели (по тропе, не напрямую через скалы), ярче у игрока.
// Излучение — в единицах «после предэкспозиции» (не зависит от экспозиции).
inline float3 mycelium_glow(constant FrameUniforms& f, float3 rel, float route, float left) {
    if (f.mycelium.x <= 0.0 || route > 4.0) {
        return float3(0.0);
    }
    float2 q = rel.xz + f.cam_lo.xz + f.cam_hi.xz;  // мировые: узор не «плывёт»
    float2 w = q + 1.5 * float2(myc_noise(q * 0.09), myc_noise(q * 0.09 + 17.0));
    float r1 = 1.0 - abs(myc_noise(w * 0.7) * 2.0 - 1.0);
    float r2 = 1.0 - abs(myc_noise(w * 1.9 + 5.0) * 2.0 - 1.0);
    // Вдоль оси — главная жила (тонкая линия по центру), вокруг — ветви.
    float spine = exp(-route * route / 0.08) * (0.6 + 0.4 * myc_noise(q * 0.5));
    float vein = max(pow(r1, 12.0) + 0.6 * pow(r2, 16.0), spine);
    float band = 1.0 - smoothstep(1.2, 3.5, route);
    float t = f.time;
    float pulse = pow(0.5 + 0.5 * sin(left * 0.35 + t * 2.4), 6.0);
    float signal = pow(0.5 + 0.5 * sin(left * 0.06 + t * 1.05), 24.0);
    float near = exp(-length(rel.xz) / 35.0);
    float breathe = 0.55 + 0.45 * sin(t * 0.9);
    float k = f.mycelium.x * vein * band * (0.2 * breathe + 1.2 * pulse + 2.5 * signal) * (0.3 + 0.7 * near);
    return float3(0.25, 1.0, 0.75) * k * 0.12;
}

// Прожектор дрона (05.6): освещённость точки (`rel` — относительно камеры) с нормалью `n` (уже × на
// альбедо снаружи). Мягкий край конуса, спад 1/d², без теней (сессия 2 — тени
// крон в луче).
inline float3 spot_light(constant FrameUniforms& f, float3 rel, float3 n) {
    if (f.spot_pos.w <= 0.0) {
        return float3(0.0);
    }
    float3 to = rel + f.cam_lo.xyz - f.spot_pos.xyz;
    float d = length(to);
    float3 l = to / max(d, 1e-3);
    float c = dot(l, f.spot_dir.xyz);
    float cone = smoothstep(f.spot_pos.w, mix(f.spot_pos.w, 1.0, 0.35), c);
    float nl = saturate(dot(n, -l));
    return float3(1.0, 0.97, 0.9) * f.spot_dir.w * cone * nl / max(d * d, 1.0);
}

// Позиция относительно камеры (AD-49): мир − cam_hi точна (cam_hi кратна 256 м,
// узлы сетки — кратны шагу), минус малая cam_lo. Без огромного сдвига в матрице
// точность f32 не теряется даже в 30 км от начала карты.
inline float3 camera_relative(constant FrameUniforms& f, float3 world) {
    return (world - f.cam_hi.xyz) - f.cam_lo.xyz;
}

inline float4 clip_from_world(constant FrameUniforms& f, float3 world) {
    return f.view_proj_rel * float4(camera_relative(f, world), 1.0);
}

// Координаты узора с периодом `period` м: дробь крупной части (постоянна, пока
// камера в той же клетке 256 м) плюс точная малая часть `local = мир − cam_hi`.
inline float2 pattern_uv(constant FrameUniforms& f, float2 local_xz, float period) {
    return fract(f.cam_hi.xz / period) + local_xz / period;
}

// Подводная толща (src/app/frame.rs, underwater_m): ослабление по пути, м⁻¹. Красный
// гаснет первым; к поглощению добавлено рассеяние — видимость ~25 м.
constant float3 UW_EXTINCTION = float3(0.46, 0.13, 0.10);
// Цвет толщи у поверхности при солнце, линейный; с глубиной гаснет по поглощению.
constant float3 UW_COLOR = float3(0.020, 0.115, 0.135);
constant float3 UW_ABSORB = float3(0.42, 0.085, 0.055);

// Дымка толщи между глазом и точкой на расстоянии `dist` (только под водой);
// `light` — освещённость сверху относительно полудня (sky.h, underwater_light).
inline float3 underwater_fog(float3 color, float dist, float eye_depth, float light) {
    float3 t = exp(-UW_EXTINCTION * dist);
    float3 fogc = UW_COLOR * light * exp(-UW_ABSORB * eye_depth);
    return color * t + fogc * (1.0 - t);
}

struct InstanceData {
    float4x4 model;
    float4 tint;
};

struct Vertex {
    packed_float3 position;
    packed_float3 normal;
    float2 uv;
};

// Вершина ландшафта (src/layout.rs, TerrainVertex).
struct TerrainVertex {
    packed_float3 position;
    packed_float3 normal;
    uchar4 weights;   // лес, торф, берег, тропа
    uchar4 material;  // субстрат, русло, влажность, тип берега (0 песок, 255 камень)
};

// Волна Герстнера (src/layout.rs, GerstnerWave), 32 байта.
struct GerstnerWave {
    float2 dir;
    float wavelength;
    float amplitude;
    float steepness;
    float phase;
    float omega;
    float _pad;
};

constant uint SEA_WAVES = 8;
constant uint WATER_PROBES = 8;

// Параметры моря и прибоя (src/layout.rs, SeaParams), 432 байта.
struct SeaParams {
    float sea_level;
    uint wave_count;
    float shore_wavelength;
    float shore_amplitude;
    float shore_omega;
    float swash_sand_m;
    float swash_rock_m;
    float dry_time_s;
    uint field_size;
    float field_step_m;
    float dt;
    float _pad;
    GerstnerWave waves[SEA_WAVES];
    float4 probe[WATER_PROBES];
};

// Уровень кольцевой сетки воды (src/layout.rs, WaterLevel), 32 байта.
struct WaterLevel {
    float2 origin;
    float cell;
    uint morph_edge;
    float2 inner_min;
    float2 inner_max;
};

// Точек на сторону тайла LOD 0 (src/world/mod.rs, TILE_SAMPLES).
constant uint TILE_SAMPLES = 65;

// Эталон на CPU: src/reference.rs, pattern_color.
inline float3 pattern_color(uint2 p, uint size, float t) {
    float u = float(p.x) / float(size - 1);
    float v = float(p.y) / float(size - 1);
    uint cell = ((p.x / 32) + (p.y / 32)) % 2;
    float k = cell == 0 ? 1.0 : 0.6;
    float stripe = 0.5 + 0.5 * precise::sin(t * 2.0 + float(p.x + p.y) * 0.05);
    return float3(u * k, v * k, stripe * k);
}

// Вершина рук со скином (src/layout.rs, SkinnedVertex), 52 байта, всё упаковано.
struct SkinnedVertex {
    packed_float3 position;
    packed_float3 normal;
    packed_float2 uv;
    uchar4 joints;
    packed_float4 weights;
};

// Параметры прохода viewmodel (src/layout.rs, HandsUniforms), 144 байта.
struct HandsUniforms {
    float4x4 proj;           // проекция рук, reversed-Z
    float4x4 view_to_world;  // поворот глаз → мир для нормалей
    uint vertex_count;
    uint joint_count;
    uint2 _pad;
};

constant uint SKIN_PROBES = 8;

// Отрезок объекта карты (src/layout.rs, FeatureSegment), 32 байта.
struct FeatureSegment {
    float2 a;
    float2 b;
    float half_width;
    uint kind;          // 0 тропа, 1 река, 2 дорога
    float2 _pad;        // x — разлив реки от подъёма воды, м (03.2)
};

// Кластер рельефа (src/layout.rs, TerrainCluster), 16 байт.
struct TerrainCluster {
    uint vertex_offset;
    uint triangle_offset;
    uint triangle_count;
    uint depth;
};

// Входы неба от CPU (src/layout.rs, SkyInputs), 176 байт.
// Интерфейс (05.1): src/layout.rs, UiVertex.
// Временное сглаживание (04.5): src/layout.rs, TaaUniforms, 160 байт.
struct TaaUniforms {
    float4x4 inv_view_proj_rel;   // NDC + глубина → относительно камеры (без сдвига)
    float4x4 prev_view_proj_rel;  // прошлый кадр, без сдвига
    float4 cam_delta;             // xyz — камера сейчас − прошлая, м; w — история годна
    float4 jitter;                // xy — сдвиг кадра в NDC; zw — размер, пикселей
};

struct UiVertex {
    packed_float2 pos;
    packed_float2 uv;
    uchar4 color;
};

// Растения (02.8, 02.4): src/layout.rs, PlantVertex / PlantInstance.
struct PlantVertex {
    packed_float3 position;
    packed_float3 normal;
    packed_float2 uv;
    uchar4 attr;
};

struct PlantInstance {
    packed_float3 position;
    float scale;
    float yaw;
    float tint;
    float phase;
    float _pad;
};

// Выбор кластеров на GPU (02.5): src/layout.rs, CullView / CullNode / CullBounds / CullCluster.
struct CullView {
    float4 planes[6];  // xyz — нормаль внутрь, w — смещение
    float4 camera;     // xyz — камера, w — пикселей на единицу
    float4 params;     // x — znear, y — порог, z — орто, w — вид включён
};

struct CullNode {
    float4 sphere;
    float error;
    int group;
    uint child_offset;
    uint child_count;
};

struct CullBounds {
    float4 sphere;
    float error;
    float _pad[3];
};

struct CullCluster {
    float4 sphere;
    uint group;
    int refined;
    uint _pad[2];
};

struct SkyInputs {
    float4 sun_dir;      // к солнцу в мировых осях (x восток, y вверх, z юг)
    float4 moon_dir;     // к луне; w — освещённая доля
    float4 star_rot[3];  // строки поворота экваториальных осей в мировые
    float4 params;       // x: фикс. экспозиция (0 — авто), y: dt, с, z: время, с, w: E☉
    float4x4 inv_view_proj_rel;  // обратная к view_proj_rel: направление пикселя
    float4 view;         // x — телесный угол пикселя, ср; y — звёзд
};

// Игровая зона рельефа (src/layout.rs, TerrainZone), 32 байта.
struct TerrainZone {
    uint far_count;    // узлов поля острова; ~0u — зоны нет
    uint n;            // узлов зоны по стороне
    float step_m;
    float origin_x;    // не float2: выравнивание по 8 сдвинуло бы поле
    float origin_z;
    float _pad[3];
};

// Каскадные тени (src/layout.rs, ShadowUniforms), 304 байта.
struct ShadowUniforms {
    float4x4 vp_rel[4];  // свет: проекция × вид относительно камеры, глубина [0, 1]
    float4 splits;       // дальние границы каскадов 0–2, м (каскад 3 — весь остров)
    float4 texel_m;      // размер текселя каскада, м
    float4 params;       // x — тени включены
    float4x4 rain_vp_rel; // 04.3: карта укрытия от дождя — сверху вниз, отн. камеры
    float4 rain;          // x — полуширина, м; y — верх над камерой, м; z — диапазон высот, м; w — включена
};
