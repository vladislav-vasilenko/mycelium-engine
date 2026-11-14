#include "common.h"

// Выбор кластеров рельефа на GPU (02.5): то же правило, что
// terrain_clusters::select на CPU. Обход дерева групп по глубинам: dispatch
// на глубину d, поток — узел фронта глубины d и вид (основная камера и
// каскады теней). Узел с ошибкой ≤ порога или вне пирамиды пропускается;
// лист проверяет кластеры своей группы и пишет видимые в список вида;
// внутренний узел дописывает детей во фронт глубины d + 1.
//
// Буфер счётчиков: на вид CULL_STRIDE u32 — [0, MAX_DEPTH) фронт по глубинам,
// CULL_OUT — видимых, CULL_OVERFLOW — не поместилось; после всех видов —
// indirect-аргументы отрисовки (по 4 u32 на вид).

constant uint CULL_STRIDE = 64;
constant uint CULL_OUT = 48;
constant uint CULL_OVERFLOW = 49;

struct CullLevel {
    uint depth;
    uint offset;       // начало фронта этой глубины в списке узлов вида
    uint next_offset;  // начало фронта следующей глубины
    uint total;        // узлов в дереве: длина списка фронта одного вида
};

// Списки видимых по видам: основной и каскады (src/renderer/cull.rs).
struct CullRegions {
    uint4 base;    // начало списка вида 0–3
    uint4 cap;     // ёмкость вида 0–3
    uint4 extra;   // x — начало вида 4, y — его ёмкость, z — видов
};

inline float cull_error(constant CullView& v, float4 sphere, float error) {
    if (v.params.z > 0.5) {
        return error * v.camera.w;
    }
    float d = max(length(sphere.xyz - v.camera.xyz) - sphere.w, v.params.x);
    return error / d * v.camera.w;
}

inline bool cull_visible(constant CullView& v, float4 sphere) {
    for (int i = 0; i < 6; ++i) {
        if (dot(v.planes[i].xyz, sphere.xyz) + v.planes[i].w < -sphere.w) {
            return false;
        }
    }
    return true;
}

kernel void cull_clusters(device const CullNode* nodes [[buffer(0)]],
                          device const CullBounds* groups [[buffer(1)]],
                          device const CullCluster* clusters [[buffer(2)]],
                          device const uint2* group_clusters [[buffer(3)]],
                          constant CullView* views [[buffer(4)]],
                          device uint* frontier [[buffer(5)]],
                          device atomic_uint* counters [[buffer(6)]],
                          device uint* visible [[buffer(7)]],
                          constant CullLevel& level [[buffer(8)]],
                          constant CullRegions& regions [[buffer(9)]],
                          uint2 gid [[thread_position_in_grid]]) {
    uint vi = gid.y;
    constant CullView& v = views[vi];
    if (v.params.w < 0.5) {
        return;
    }
    device atomic_uint* cnt = counters + vi * CULL_STRIDE;
    uint n = atomic_load_explicit(&cnt[level.depth], memory_order_relaxed);
    if (gid.x >= n) {
        return;
    }
    device uint* front = frontier + vi * level.total;
    CullNode node = nodes[front[level.offset + gid.x]];
    float t = v.params.y;
    if (cull_error(v, node.sphere, node.error) <= t || !cull_visible(v, node.sphere)) {
        return;
    }
    if (node.group < 0) {
        uint k = atomic_fetch_add_explicit(&cnt[level.depth + 1], node.child_count,
                                           memory_order_relaxed);
        for (uint j = 0; j < node.child_count; ++j) {
            front[level.next_offset + k + j] = node.child_offset + j;
        }
        return;
    }
    uint base = vi < 4 ? regions.base[vi] : regions.extra.x;
    uint cap = vi < 4 ? regions.cap[vi] : regions.extra.y;
    uint2 gc = group_clusters[node.group];
    for (uint i = gc.x; i < gc.x + gc.y; ++i) {
        CullCluster c = clusters[i];
        CullBounds coarse = groups[c.group];
        if (cull_error(v, coarse.sphere, coarse.error) <= t) {
            continue;
        }
        if (c.refined >= 0) {
            CullBounds fine = groups[c.refined];
            if (cull_error(v, fine.sphere, fine.error) > t) {
                continue;
            }
        }
        if (!cull_visible(v, c.sphere)) {
            continue;
        }
        uint k = atomic_fetch_add_explicit(&cnt[CULL_OUT], 1, memory_order_relaxed);
        if (k < cap) {
            visible[base + k] = i;
        } else {
            atomic_fetch_add_explicit(&cnt[CULL_OVERFLOW], 1, memory_order_relaxed);
        }
    }
}

// Indirect-аргументы drawPrimitives по видам: число экземпляров — видимых
// кластеров, не больше ёмкости списка.
kernel void cull_finish(device uint* counters [[buffer(6)]],
                        constant CullRegions& regions [[buffer(9)]],
                        uint vi [[thread_position_in_grid]]) {
    uint views = regions.extra.z;
    if (vi >= views) {
        return;
    }
    uint cap = vi < 4 ? regions.cap[vi] : regions.extra.y;
    device uint* args = counters + views * CULL_STRIDE + vi * 4;
    args[0] = 128 * 3;
    args[1] = min(counters[vi * CULL_STRIDE + CULL_OUT], cap);
    args[2] = 0;
    args[3] = 0;
}
