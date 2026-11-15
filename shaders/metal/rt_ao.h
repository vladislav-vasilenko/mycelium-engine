// Затенение лучами (11, сессия 1, проход `rt_ao`): 1 короткий луч на пиксель в косинусную полусферу
// от точки по верхней структуре сцены (рельеф у камеры + кора, камни,
// валежник). Попадание ближе — темнее. Направления новые каждый кадр — TAA
// (04.5) усредняет шум. Подключается после common.h.
#include <metal_raytracing>

constant float RT_AO_RANGE_M = 2.5;

inline float rt_hash(uint2 p, uint k) {
    uint h = p.x * 0x8da6b343u ^ p.y * 0xd8163841u ^ k * 0x9e3779b9u;
    h ^= h >> 16;
    h *= 0x7feb352du;
    h ^= h >> 15;
    h *= 0x846ca68bu;
    h ^= h >> 16;
    return float(h >> 8) / 16777216.0;
}

// Доля неба, видимая из точки (1 — открыто). `world` — мировая позиция,
// `n` — нормаль поверхности, `px` — пиксель, `seed` — номер кадра.
inline float rt_ambient_occlusion(raytracing::instance_acceleration_structure tlas, float3 world, float3 n,
                                  float2 px, uint seed) {
    using namespace raytracing;
    intersector<instancing, triangle_data> isect;
    isect.accept_any_intersection(false);
    isect.assume_geometry_type(geometry_type::triangle);
    isect.force_opacity(forced_opacity::opaque);
    float3 t = normalize(abs(n.y) < 0.95 ? cross(n, float3(0, 1, 0)) : cross(n, float3(1, 0, 0)));
    float3 b = cross(n, t);
    float occ = 0.0;
    for (uint k = 0; k < 1; ++k) {
        float u1 = rt_hash(uint2(px), seed * 2u + k);
        float u2 = rt_hash(uint2(px) + uint2(7, 13), seed * 2u + k + 101u);
        float r = sqrt(u1), phi = 6.2831853 * u2;
        float3 d = normalize(t * (r * cos(phi)) + b * (r * sin(phi)) + n * sqrt(max(1.0 - u1, 0.0)));
        // Начало над поверхностью: сетка лучей (0,6 м, треугольники) и видимая
        // заплатка (бикубика + кочки) расходятся на сантиметры.
        ray rr(world + n * 0.05, d, 0.15, RT_AO_RANGE_M);
        auto hit = isect.intersect(rr, tlas);
        if (hit.type != intersection_type::none) {
            float x = hit.distance / RT_AO_RANGE_M;
            occ += 1.0 - x * x;
        }
    }
    return 1.0 - occ;
}
