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
