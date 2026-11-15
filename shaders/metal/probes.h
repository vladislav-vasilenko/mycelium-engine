// Пробы освещённости (11, сессия 3): сетка PROBE_DIM с шагом PROBE_STEP_M
// вокруг камеры, кольцевая адресация по мировой клетке (при ходьбе пробы
// переиспользуются). У пробы — освещённость по направлениям в октаэдре
// PROBE_OCT × PROBE_OCT. Подключается после common.h и sky.h.

constant int3 PROBE_DIM = int3(32, 8, 32);
constant float PROBE_STEP_M = 3.0;
constant uint PROBE_OCT = 6;
constant uint PROBE_TEXELS = PROBE_OCT * PROBE_OCT;
constant uint PROBE_COUNT = 32 * 8 * 32;

// Клетка первой пробы сетки (мировые клетки по PROBE_STEP_M): по камере.
inline int3 probe_base_cell(constant FrameUniforms& frame) {
    int3 c = int3(floor(frame.camera_pos.xyz / PROBE_STEP_M));
    return c - int3(PROBE_DIM.x / 2, 2, PROBE_DIM.z / 2);
}

// Номер пробы мировой клетки (кольцевой).
inline uint probe_index(int3 cell) {
    int3 m = ((cell % PROBE_DIM) + PROBE_DIM) % PROBE_DIM;
    return uint((m.y * PROBE_DIM.z + m.z) * PROBE_DIM.x + m.x);
}

inline float2 oct_encode(float3 n) {
    n /= abs(n.x) + abs(n.y) + abs(n.z);
    float2 p = n.xz;
    if (n.y < 0.0) {
        p = (1.0 - abs(p.yx)) * float2(p.x >= 0.0 ? 1.0 : -1.0, p.y >= 0.0 ? 1.0 : -1.0);
    }
    return p * 0.5 + 0.5;
}

inline float3 oct_decode(float2 uv) {
    float2 p = uv * 2.0 - 1.0;
    float3 n = float3(p.x, 1.0 - abs(p.x) - abs(p.y), p.y);
    if (n.y < 0.0) {
        float2 q = (1.0 - abs(n.zx)) * float2(n.x >= 0.0 ? 1.0 : -1.0, n.z >= 0.0 ? 1.0 : -1.0);
        n.x = q.x;
        n.z = q.y;
    }
    return normalize(n);
}

// Освещённость пробы по направлению: билинейно по октаэдру (без краёв).
inline float3 probe_sample(device const float4* probes, uint probe, float3 n) {
    float2 t = clamp(oct_encode(n) * float(PROBE_OCT) - 0.5, 0.0, float(PROBE_OCT) - 1.001);
    int2 i = int2(floor(t));
    float2 f = t - float2(i);
    auto at = [&](int x, int y) { return probes[probe * PROBE_TEXELS + uint(y) * PROBE_OCT + uint(x)].rgb; };
    return mix(mix(at(i.x, i.y), at(i.x + 1, i.y), f.x), mix(at(i.x, i.y + 1), at(i.x + 1, i.y + 1), f.x), f.y);
}

// Непрямая освещённость в точке: 8 проб вокруг, вес — трилинейный и «проба
// перед поверхностью» (свет не протекает с обратной стороны). w пробы — 0,
// пока проба не прогрета.
inline float3 probe_irradiance(device const float4* probes, constant FrameUniforms& frame, float3 world, float3 n) {
    int3 base = probe_base_cell(frame);
    float3 g = world / PROBE_STEP_M;
    int3 c0 = int3(floor(g));
    float3 f = g - float3(c0);
    float3 sum = 0.0;
    float wsum = 0.0;
    for (int k = 0; k < 8; ++k) {
        int3 o = int3(k & 1, (k >> 1) & 1, k >> 2);
        int3 cell = c0 + o;
        if (any(cell < base) || any(cell >= base + PROBE_DIM)) {
            continue;
        }
        float3 tri = mix(1.0 - f, f, float3(o));
        float w = tri.x * tri.y * tri.z;
        float3 to = normalize(float3(cell) * PROBE_STEP_M - world + 1e-4);
        float facing = (dot(to, n) + 1.0) * 0.5;
        w *= facing * facing + 0.05;
        uint idx = probe_index(cell);
        float valid = probes[idx * PROBE_TEXELS].w;
        w *= valid;
        sum += probe_sample(probes, idx, n) * w;
        wsum += w;
    }
    return wsum > 1e-4 ? sum / wsum : float3(-1.0);
}
