#include "common.h"

// Ландшафт LOD 0: все тайлы одним instanced draw. Номер экземпляра — номер
// тайла, вершины тайла лежат подряд по TILE_SAMPLES² штук.

struct TerrainVaryings {
    float4 position [[position]];
    float3 world;
    float3 normal;
    float4 weights;   // лес, торф, берег, тропа
    float4 material;  // субстрат, русло, влажность
};

vertex TerrainVaryings terrain_vs(uint vid [[vertex_id]],
                                  uint iid [[instance_id]],
                                  const device TerrainVertex* vertices [[buffer(0)]],
                                  constant FrameUniforms& frame [[buffer(1)]]) {
    TerrainVertex v = vertices[iid * TILE_SAMPLES * TILE_SAMPLES + vid];
    TerrainVaryings out;
    out.world = float3(v.position);
    out.position = frame.view_proj * float4(out.world, 1.0);
    out.normal = float3(v.normal);
    out.weights = float4(v.weights) / 255.0;
    out.material = float4(float(v.material.x), float(v.material.y) / 255.0,
                          float(v.material.z) / 255.0, 0.0);
    return out;
}

// Субстраты: src/world/mod.rs, Substrate.
constant float SUBSTRATE_ROCK = 3.0;

fragment float4 terrain_fs(TerrainVaryings in [[stage_in]],
                           constant FrameUniforms& frame [[buffer(1)]],
                           texture2d<float> detail [[texture(0)]]) {
    constexpr sampler s(filter::linear, mip_filter::linear, address::repeat, max_anisotropy(8));
    float2 xz = in.world.xz;
    float4 fine = detail.sample(s, xz / 3.0);
    float4 coarse = detail.sample(s, xz / 23.0);
    float3 n = normalize(in.normal);
    float slope = 1.0 - n.y;

    // Базовые цвета биомов (линейные).
    const float3 forest_soil = float3(0.07, 0.10, 0.035);
    const float3 forest_moss = float3(0.10, 0.17, 0.04);
    const float3 peat = float3(0.11, 0.075, 0.045);
    const float3 sand = float3(0.50, 0.44, 0.31);
    const float3 rock = float3(0.24, 0.23, 0.22);
    const float3 gravel = float3(0.30, 0.27, 0.22);
    const float3 trail = float3(0.26, 0.19, 0.12);

    float3 forest = mix(forest_soil, forest_moss, coarse.g);
    bool rocky_shore = in.material.x > SUBSTRATE_ROCK - 0.5 && in.material.x < SUBSTRATE_ROCK + 0.5;
    float3 shore = rocky_shore ? mix(rock, gravel, fine.b) : sand;
    float3 albedo = forest * in.weights.x + peat * in.weights.y + shore * in.weights.z;
    albedo = mix(albedo, rock, smoothstep(0.22, 0.4, slope));       // склоны — камень
    albedo = mix(albedo, gravel, in.material.y);                    // русло — галька
    albedo = mix(albedo, trail, smoothstep(0.1, 0.6, in.weights.w)); // тропа
    albedo *= 0.7 + 0.6 * fine.r * (0.6 + 0.8 * coarse.g);
    // Влажный грунт темнее.
    albedo *= 1.0 - 0.35 * in.material.z * in.weights.y;
    // Под водой — тёмное дно (воду нарисует этап 03).
    if (in.world.y < 0.0) {
        albedo *= mix(float3(0.6, 0.75, 0.85), float3(0.25, 0.35, 0.5), saturate(-in.world.y / 6.0));
    }

    float ndl = max(dot(n, -frame.light_dir.xyz), 0.0);
    // Временный свет до этапа 04: солнце + небо, без теней и тумана.
    float3 sky = float3(0.20, 0.26, 0.36) * (0.5 + 0.5 * n.y);
    float3 color = albedo * (1.7 * ndl + sky);
    return float4(color, 1.0);
}
