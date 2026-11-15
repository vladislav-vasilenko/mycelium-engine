#include "common.h"
#include "terrain_patch.h"
#include "rt_ao.h"
#include "sky.h"
#include "probes.h"

// Сцена для лучей (11, сессия 1): нижние структуры растений (кора, камень,
// почва — без карточек листвы) строятся один раз, сетка рельефа вокруг
// камеры и экземпляры — каждый кадр, верхняя структура — косвенно на GPU.
// Координаты — мировые (f32 на 30 км — ~2 мм, лучам затенения достаточно).

constant uint RT_GRID = 161;              // узлов по стороне сетки рельефа
constant float RT_STEP_M = 0.6;           // 96 м вокруг камеры
constant uint RT_MAX_INSTANCES = 8192;
constant uint FOREST_CAP_RT = 8192;       // FOREST_CAP в forest.metal
constant uint RT_SOURCES = 15;           // MESHES в src/renderer/rt.rs

// Экземпляр косвенной верхней структуры: MTLIndirectAccelerationStructureInstanceDescriptor, 72 Б.
struct RtInstance {
    packed_float3 c0, c1, c2, c3;   // packed_float4x3 по столбцам
    uint options;
    uint mask;
    uint ift_offset;
    uint user_id;
    ulong as_id;                    // MTLResourceID нижней структуры
};

// Список леса → нижняя структура: номер списка и номер сетки (`RtSource`).
struct RtSource {
    uint list;
    uint mesh;
};

static float rt_far_height(float2 p, device const float* heights, constant FrameUniforms& frame) {
    uint nx = frame.grid_nx;
    int nz = int(frame.map_size_m / frame.grid_step_m + 0.5) + 1;
    float2 q = clamp(p / frame.grid_step_m, 0.0, float2(float(nx) - 1.001, float(nz) - 1.001));
    int2 i = int2(floor(q));
    float2 f = q - float2(i);
    auto h = [&](int a, int b) { return heights[uint(b) * nx + uint(a)]; };
    return mix(mix(h(i.x, i.y), h(i.x + 1, i.y), f.x), mix(h(i.x, i.y + 1), h(i.x + 1, i.y + 1), f.x), f.y);
}

// Угол сетки рельефа: камера, привязанная к шагу.
inline float2 rt_grid_origin(constant FrameUniforms& frame) {
    return floor(frame.camera_pos.xz / RT_STEP_M) * RT_STEP_M - RT_STEP_M * float(RT_GRID - 1) * 0.5;
}

kernel void rt_terrain_grid(device const float* heights [[buffer(0)]],
                            constant FrameUniforms& frame [[buffer(1)]],
                            device const float* zone_heights [[buffer(2)]],
                            constant TerrainZone& zone [[buffer(3)]],
                            device packed_float3* out [[buffer(4)]],
                            uint2 gid [[thread_position_in_grid]]) {
    if (any(gid >= uint2(RT_GRID))) {
        return;
    }
    float2 p = rt_grid_origin(frame) + float2(gid) * RT_STEP_M;
    float h;
    float2 q = (p - float2(zone.origin_x, zone.origin_z)) / zone.step_m;
    if (zone.far_count != 0xffffffffu && all(q >= 0.0) && all(q <= float(zone.n - 1))) {
        h = patch_tri_height(PatchZone{zone_heights, zone}, p);
    } else {
        h = rt_far_height(p, heights, frame);
    }
    out[gid.y * RT_GRID + gid.x] = packed_float3(p.x, h, p.y);
}

// Экземпляры: 0 — рельеф (единичная матрица), дальше — растения из списков
// леса этого кадра (деревья — списки теней, валежник и валуны — LOD 0).
kernel void rt_instances(device const uint* counts [[buffer(0)]],
                         device const PlantInstance* lists [[buffer(1)]],
                         constant RtSource* sources [[buffer(2)]],
                         constant ulong* blas [[buffer(3)]],   // 0 — рельеф, 1 + сетка — растения
                         device RtInstance* out [[buffer(4)]],
                         device atomic_uint* count [[buffer(5)]],
                         uint2 gid [[thread_position_in_grid]]) {
    uint s = gid.y;
    uint i = gid.x;
    if (s == 0 && i == 0) {
        RtInstance t;
        t.c0 = packed_float3(1, 0, 0);
        t.c1 = packed_float3(0, 1, 0);
        t.c2 = packed_float3(0, 0, 1);
        t.c3 = packed_float3(0, 0, 0);
        t.options = 0;
        t.mask = 0xff;
        t.ift_offset = 0;
        t.user_id = 0;
        t.as_id = blas[0];
        out[0] = t;
    }
    if (s >= RT_SOURCES) {
        return;
    }
    RtSource src = sources[s];
    uint n = min(counts[src.list], FOREST_CAP_RT);
    if (i >= n) {
        return;
    }
    PlantInstance p = lists[src.list * FOREST_CAP_RT + i];
    uint k = atomic_fetch_add_explicit(count, 1, memory_order_relaxed) + 1;
    if (k >= RT_MAX_INSTANCES) {
        return;
    }
    // Поворот вокруг Y и масштаб — как plant_world (без формы экземпляра и ветра).
    float c = cos(p.yaw) * p.scale, sn = sin(p.yaw) * p.scale;
    RtInstance t;
    t.c0 = packed_float3(c, 0, sn);
    t.c1 = packed_float3(0, p.scale, 0);
    t.c2 = packed_float3(-sn, 0, c);
    t.c3 = p.position;
    t.options = 0;
    t.mask = 0xff;
    t.ift_offset = 0;
    t.user_id = 1 + src.mesh;  // пробы (11): 0 — рельеф, 1 + сетка — растение
    t.as_id = blas[1 + src.mesh];
    out[k] = t;
}

// Число экземпляров для построения: 1 (рельеф) + растения, не больше предела.
kernel void rt_count_finish(device atomic_uint* count [[buffer(5)]], uint gid [[thread_position_in_grid]]) {
    if (gid == 0) {
        uint n = atomic_load_explicit(count, memory_order_relaxed);
        atomic_store_explicit(count, min(n + 1, RT_MAX_INSTANCES), memory_order_relaxed);
    }
}

// Затенение лучами в отдельном проходе (после `scene`): половина разрешения,
// позиция и нормаль — из глубины кадра (матрица без сдвига TAA), один луч на
// пиксель, накопление во времени в самой текстуре (кадр N+1 читает её при
// освещении: затенение медленное, отставание на кадр не видно).
kernel void rt_ao(depth2d<float> depth [[texture(0)]],
                  texture2d<float, access::read_write> ao [[texture(1)]],
                  constant FrameUniforms& frame [[buffer(1)]],
                  constant TaaUniforms& taa [[buffer(2)]],
                  raytracing::instance_acceleration_structure tlas [[buffer(3)]],
                  constant ShadowUniforms& shadows [[buffer(4)]],
                  uint2 gid [[thread_position_in_grid]]) {
    uint2 size = uint2(ao.get_width(), ao.get_height());
    if (any(gid >= size)) {
        return;
    }
    uint2 full = uint2(depth.get_width(), depth.get_height());
    uint2 p = min(gid * 2u, full - 2u);
    float d = depth.read(p);
    if (d <= 0.0) {
        ao.write(float4(1.0), gid);
        return;
    }
    auto pos = [&](uint2 q, float z) {
        float2 uv = (float2(q) + 0.5) / float2(full);
        float2 ndc = float2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0) - taa.jitter.xy;
        float4 r = taa.inv_view_proj_rel * float4(ndc, z, 1.0);
        return r.xyz / r.w;
    };
    float3 rel = pos(p, d);
    float3 rx = pos(p + uint2(1, 0), depth.read(p + uint2(1, 0)));
    float3 ry = pos(p + uint2(0, 1), depth.read(p + uint2(0, 1)));
    float3 n = normalize(cross(ry - rel, rx - rel));
    if (dot(n, -rel) < 0.0) {
        n = -n;
    }
    float3 world = rel + frame.camera_pos.xyz;
    float s = rt_ambient_occlusion(tlas, world, n, float2(gid), uint(shadows.params.w));
    // История годна — копим; после сброса (первый кадр, телепорт) — сразу луч.
    float prev = taa.cam_delta.w > 0.5 ? ao.read(gid).x : s;
    ao.write(float4(mix(prev, s, 0.2)), gid);
}

// --- Пробы освещённости (11, сессия 3) ---------------------------------------
// Группа из 64 потоков — одна проба, поток — луч. Луч по сцене: попадание —
// освещённость точки (альбедо по поясам рельефа или коры, солнце с теневым
// лучом, небо), промах — небо; вверх под лесом небо гасится пологом и
// добавляется свет листвы на просвет (карточек листвы в структурах нет —
// полог как среда по плотности леса). Затем 36 потоков собирают
// освещённость по направлениям октаэдра и смешивают с историей.

constant uint PROBE_RAYS = 64;
constant float PROBE_RAY_M = 60.0;

static float rt_ground(float2 p, device const float* heights, device const float* zone_heights,
                       constant TerrainZone& zone, constant FrameUniforms& frame) {
    float2 q = (p - float2(zone.origin_x, zone.origin_z)) / zone.step_m;
    if (zone.far_count != 0xffffffffu && all(q >= 0.0) && all(q <= float(zone.n - 1))) {
        return patch_tri_height(PatchZone{zone_heights, zone}, p);
    }
    return rt_far_height(p, heights, frame);
}

static float4 rt_cover(texture2d<float> cover, float2 p, constant FrameUniforms& frame) {
    constexpr sampler ls(filter::linear, address::clamp_to_edge);
    float cn = float(cover.get_width());
    return cover.sample(ls, (p / frame.map_size_m * (cn - 1.0) + 0.5) / cn, level(0));
}

kernel void probe_update(device float4* probes [[buffer(0)]],
                         device int4* cells [[buffer(1)]],
                         constant FrameUniforms& frame [[buffer(2)]],
                         constant ShadowUniforms& sh [[buffer(3)]],
                         device const SkyFrame& skyf [[buffer(4)]],
                         raytracing::instance_acceleration_structure tlas [[buffer(5)]],
                         device const float* heights [[buffer(6)]],
                         device const float* zone_heights [[buffer(7)]],
                         constant TerrainZone& zone [[buffer(8)]],
                         texture2d<float> cover [[texture(0)]],
                         uint pg [[threadgroup_position_in_grid]],
                         uint tid [[thread_index_in_threadgroup]]) {
    using namespace raytracing;
    // За кадр — половина проб, через одну по чётности кадра.
    uint p = pg * 2u + (uint(sh.params.w) & 1u);
    threadgroup float3 rad[PROBE_RAYS];
    threadgroup float3 dirs[PROBE_RAYS];
    int3 local = int3(int(p) % PROBE_DIM.x, int(p) / (PROBE_DIM.x * PROBE_DIM.z), (int(p) / PROBE_DIM.x) % PROBE_DIM.z);
    int3 cell = probe_base_cell(frame) + local;
    uint idx = probe_index(cell);
    // Проба сменила мировую клетку (камера ушла) — история сбрасывается.
    // Читается до барьера: поток 0 пишет новую клетку в конце.
    bool fresh = any(cells[idx].xyz != cell);
    float3 pos = float3(cell) * PROBE_STEP_M;
    float gh = rt_ground(pos.xz, heights, zone_heights, zone, frame);
    pos.y = max(pos.y, gh + 0.4);  // проба под землёй — светит из-под поверхности
    float4 cv = rt_cover(cover, pos.xz, frame);
    float forest = saturate(cv.x + cv.y);
    float3 L = -frame.light_dir.xyz;
    float3 sun_e = skyf.light_color.rgb;

    // Направление луча: сферическая спираль Фибоначчи, поворот — новый каждый кадр.
    float seed = sh.params.w * 0.618034;
    float i = float(tid) + fract(seed);
    float z = 1.0 - 2.0 * (i + 0.5) / float(PROBE_RAYS);
    float r = sqrt(max(1.0 - z * z, 0.0));
    float phi = 2.39996323 * i + seed * 6.2831853;
    float3 d = normalize(float3(r * cos(phi), z, r * sin(phi)));

    intersector<instancing, triangle_data> isect;
    isect.assume_geometry_type(geometry_type::triangle);
    isect.force_opacity(forced_opacity::opaque);
    ray rr(pos, d, 0.05, PROBE_RAY_M);
    auto hit = isect.intersect(rr, tlas);
    float3 L_out;
    if (hit.type != intersection_type::none) {
        float3 hp = pos + d * hit.distance;
        float3 n;
        float3 albedo;
        if (hit.instance_id == 0) {
            float hx = rt_ground(hp.xz + float2(0.5, 0), heights, zone_heights, zone, frame);
            float hz = rt_ground(hp.xz + float2(0, 0.5), heights, zone_heights, zone, frame);
            float h0 = rt_ground(hp.xz, heights, zone_heights, zone, frame);
            n = normalize(float3(h0 - hx, 0.5, h0 - hz));
            float4 hc = rt_cover(cover, hp.xz, frame);
            float wsum = hc.x + hc.y + hc.z + hc.w;
            float3 a = (hc.x + hc.y) * float3(0.09, 0.10, 0.055) + hc.z * float3(0.16, 0.18, 0.08)
                     + hc.w * float3(0.30, 0.29, 0.27) + max(1.0 - wsum, 0.0) * float3(0.18, 0.16, 0.12);
            albedo = a / max(wsum + max(1.0 - wsum, 0.0), 1e-3);
        } else {
            n = -d;
            albedo = float3(0.14, 0.11, 0.08);  // кора, камень, валежник
        }
        if (dot(n, d) > 0.0) {
            L_out = 0.0;  // изнанка: луч из-под поверхности
        } else {
            ray sr(hp + n * 0.05, L, 0.02, 200.0);
            intersector<instancing> shadow;
            shadow.accept_any_intersection(true);
            shadow.force_opacity(forced_opacity::opaque);
            float vis = shadow.intersect(sr, tlas).type == intersection_type::none ? 1.0 : 0.0;
            // Полог над точкой гасит и солнце: лист пропускает немного.
            float4 hc2 = rt_cover(cover, hp.xz, frame);
            float canopy = saturate(hc2.x + hc2.y);
            vis *= mix(1.0, 0.18, canopy) * step(0.0, L.y);
            float3 e = sun_e * saturate(dot(n, L)) * vis + sh_irradiance(skyf.sh, n) * mix(0.8, 0.35, canopy);
            L_out = albedo / SKY_PI * e;
        }
    } else {
        float3 sky = sh_irradiance(skyf.sh, d) / SKY_PI;
        if (d.y > 0.0) {
            // Полог: просвет неба гаснет, листья светятся зелёным на просвет.
            float T = exp(-2.0 * forest / max(d.y, 0.25));
            float3 leaf = sun_e / SKY_PI * 0.10 * float3(0.40, 0.60, 0.20) * step(0.0, L.y) + sky * 0.25 * float3(0.6, 0.85, 0.45);
            L_out = sky * T + leaf * (1.0 - T);
        } else {
            L_out = sky * 0.15;  // склон дальше 60 м
        }
    }
    rad[tid] = L_out;
    dirs[tid] = d;
    threadgroup_barrier(mem_flags::mem_threadgroup);
    if (tid >= PROBE_TEXELS) {
        return;
    }
    // Освещённость по направлению тексела (в единицах «яркость × π»: шейдер
    // умножает на альбедо как свет неба).
    uint2 tx = uint2(tid % PROBE_OCT, tid / PROBE_OCT);
    float3 dn = oct_decode((float2(tx) + 0.5) / float(PROBE_OCT));
    float3 acc = 0.0;
    float ws = 0.0;
    for (uint k = 0; k < PROBE_RAYS; ++k) {
        float w = saturate(dot(dn, dirs[k]));
        acc += rad[k] * w;
        ws += w;
    }
    float3 e = acc / max(ws, 1e-4);
    uint slot = idx * PROBE_TEXELS + tid;
    float3 prev = probes[slot].rgb;
    float3 v = fresh ? e : mix(prev, e, 0.1);
    probes[slot] = float4(v, 1.0);
    if (tid == 0 && fresh) {
        cells[idx] = int4(cell, 0);
    }
}
