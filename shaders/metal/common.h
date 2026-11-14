// Общие типы для CPU и GPU. Раскладка повторяет src/layout.rs,
// проверка — ядро layout_probe и `cargo run -- --selftest`.
#include <metal_stdlib>
using namespace metal;

struct FrameUniforms {
    float4x4 view_proj;
    float4 camera_pos;
    float4 light_dir;      // от источника к сцене
    float time;
    float exposure;
    float pattern_time;
    uint frame_index;
    uint view_mode;        // TerrainView, 0 — с освещением
    float map_size_m;      // сторона карты: XZ → координаты маски влажности
    uint grid_nx;          // поле высот кластерного рельефа: узлов по x
    float grid_step_m;     // и шаг, м
};

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
    float2 _pad;
};

// Кластер рельефа (src/layout.rs, TerrainCluster), 16 байт.
struct TerrainCluster {
    uint vertex_offset;
    uint triangle_offset;
    uint triangle_count;
    uint depth;
};
