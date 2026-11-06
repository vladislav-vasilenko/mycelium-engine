#include "common.h"

struct SceneVaryings {
    float4 position [[position]];
    float3 normal;
    float2 uv;
    float4 tint;
};

vertex SceneVaryings scene_vs(uint vid [[vertex_id]],
                              uint iid [[instance_id]],
                              const device Vertex* vertices [[buffer(0)]],
                              constant FrameUniforms& frame [[buffer(1)]],
                              const device InstanceData* instances [[buffer(2)]]) {
    Vertex v = vertices[vid];
    InstanceData inst = instances[iid];
    float4 world = inst.model * float4(float3(v.position), 1.0);
    SceneVaryings out;
    out.position = frame.view_proj * world;
    // Для равномерного масштаба хватает верхней 3x3 части.
    out.normal = (inst.model * float4(float3(v.normal), 0.0)).xyz;
    out.uv = v.uv;
    out.tint = inst.tint;
    return out;
}

fragment float4 scene_fs(SceneVaryings in [[stage_in]],
                         constant FrameUniforms& frame [[buffer(1)]],
                         texture2d<float> pattern [[texture(0)]]) {
    constexpr sampler s(filter::linear, address::repeat);
    float3 albedo = pattern.sample(s, in.uv).rgb * in.tint.rgb;
    float3 n = normalize(in.normal);
    float ndl = max(dot(n, -frame.light_dir.xyz), 0.0);
    // Временное освещение этапа 01: ambient + Ламберт в линейном HDR.
    float3 color = albedo * (0.12 + 1.6 * ndl);
    return float4(color, 1.0);
}
