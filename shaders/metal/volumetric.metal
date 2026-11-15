#include "common.h"
#include "sky.h"

// Объёмный свет (11, сессия 2): лучи солнца в дымке между стволами. Для
// пикселя четверти разрешения — шаги вдоль луча взгляда до поверхности (не
// дальше VOL_MAX_M), на каждом — видимость солнца по карте теней (один
// тексель) и рассеяние вперёд (Хеньи — Гринстейн, g = 0,6). Сдвиг шагов
// новый каждый кадр — шум усредняет TAA, который и добавляет результат к
// кадру. Плотность — влажная дымка леса Якусимы, гуще у земли.

constant uint VOL_STEPS = 24;
constant float VOL_MAX_M = 90.0;
constant float VOL_SIGMA = 0.0012;   // рассеяние, 1/м
constant float VOL_G = 0.6;

inline float vol_hash(uint2 p, uint k) {
    uint h = p.x * 0x8da6b343u ^ p.y * 0xd8163841u ^ k * 0x9e3779b9u;
    h ^= h >> 16;
    h *= 0x7feb352du;
    h ^= h >> 15;
    return float(h >> 8) / 16777216.0;
}

kernel void volumetric_light(depth2d<float> depth [[texture(0)]],
                             depth2d_array<float> shadow_map [[texture(1)]],
                             texture2d<float, access::write> out [[texture(2)]],
                             constant FrameUniforms& frame [[buffer(1)]],
                             constant TaaUniforms& taa [[buffer(2)]],
                             constant ShadowUniforms& sh [[buffer(3)]],
                             device const SkyFrame& skyf [[buffer(4)]],
                             uint2 gid [[thread_position_in_grid]]) {
    uint2 size = uint2(out.get_width(), out.get_height());
    if (any(gid >= size)) {
        return;
    }
    if (sh.params.x < 0.5 || frame.underwater_m > 0.0) {
        out.write(float4(0.0), gid);
        return;
    }
    uint2 full = uint2(depth.get_width(), depth.get_height());
    uint2 p = min(gid * 4u + 2u, full - 1u);
    float d = depth.read(p);
    float2 uv = (float2(p) + 0.5) / float2(full);
    float2 ndc = float2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0) - taa.jitter.xy;
    // Направление взгляда и расстояние до поверхности (небо — VOL_MAX_M).
    float4 far_h = taa.inv_view_proj_rel * float4(ndc, 1e-4, 1.0);
    float3 dir = normalize(far_h.xyz / far_h.w);
    float dist = VOL_MAX_M;
    if (d > 0.0) {
        float4 r = taa.inv_view_proj_rel * float4(ndc, d, 1.0);
        dist = min(length(r.xyz / r.w), VOL_MAX_M);
    }
    float3 L = -frame.light_dir.xyz;
    float mu = dot(dir, L);
    float phase = (1.0 - VOL_G * VOL_G) / (4.0 * M_PI_F * pow(1.0 + VOL_G * VOL_G - 2.0 * VOL_G * mu, 1.5));
    constexpr sampler cmp(filter::linear, address::clamp_to_edge, compare_func::less_equal);
    float jitter = vol_hash(gid, uint(sh.params.w));
    float ds = dist / float(VOL_STEPS);
    float vis = 0.0;
    float trans = 1.0;
    for (uint i = 0; i < VOL_STEPS; ++i) {
        float t = (float(i) + jitter) * ds;
        float3 rel = dir * t;
        uint c = t < sh.splits.x ? 0u : (t < sh.splits.y ? 1u : (t < sh.splits.z ? 2u : 3u));
        float4 clip = sh.vp_rel[c] * float4(rel + L * sh.texel_m[c] * 2.0, 1.0);
        float3 sn = clip.xyz / clip.w;
        float2 suv = float2(sn.x * 0.5 + 0.5, 0.5 - sn.y * 0.5);
        float lit = (any(suv < 0.0) || any(suv > 1.0)) ? 1.0 : shadow_map.sample_compare(cmp, suv, c, sn.z);
        // Гуще у земли камеры: дымка лежит в ущелье.
        float h = rel.y;
        float dens = VOL_SIGMA * (0.6 + 0.8 * exp(-max(h, 0.0) / 25.0));
        vis += lit * dens * trans * ds;
        trans *= exp(-dens * ds);
    }
    float3 L_in = sky_sun_term(skyf) * SKY_PI * phase * vis;
    out.write(float4(L_in * frame.pre_exposure, 1.0), gid);
}
