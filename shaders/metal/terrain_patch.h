// Подключается после common.h (подстановка include без защиты от повтора).

// Детальная заплатка рельефа у камеры (02.10, сессия 2).
//
// Самый мелкий уровень кластеров — сетка зоны 0,94 м (DEM 1 м): вблизи видны
// грани и прямые изломы бровок троп. В квадрате 64 × 64 м вокруг камеры
// рисуется сетка с шагом 0,25 м: высота — бикубическая (Catmull-Rom)
// интерполяция зоны плюс мелкий процедурный рельеф; к краю заплатки она
// плавно переходит в треугольники зоны (как у кластеров уровня 0), поэтому
// щелей нет. Узлы кластеров глубже PATCH_HIDE_M внутри заплатки опускаются
// под землю, а заплатка приподнята на PATCH_LIFT_M над треугольниками у края.
// Заплатка действует в игровом режиме (`frame.forest`), только внутри зоны.

constant int PATCH_CELLS = 256;
constant float PATCH_STEP_M = 0.25;
constant float PATCH_HALF_M = 32.0;      // PATCH_CELLS · PATCH_STEP_M / 2
constant float PATCH_FADE_M = 6.0;       // полоса перехода к треугольникам у края
constant float PATCH_HIDE_M = 1.5;       // узлы кластеров глубже — опускаются
constant float PATCH_SINK_M = 0.5;
constant float PATCH_LIFT_M = 0.008;
constant uint PATCH_VERTEX_COUNT = uint(PATCH_CELLS * PATCH_CELLS * 6);
constant uint PATCH_CLUSTER = 0xfffffffeu;  // метка `TerrainVaryings.cluster`

struct PatchZone {
    device const float* zone_heights;
    constant TerrainZone& zone;
};

// Угол заплатки: камера, привязанная к шагу заплатки (узлы не «плывут»).
inline float2 patch_origin(constant FrameUniforms& frame) {
    return floor(frame.camera_pos.xz / PATCH_STEP_M) * PATCH_STEP_M - PATCH_HALF_M;
}

// Заплатка действует: игровой режим, зона есть, квадрат внутри зоны с запасом.
inline bool patch_active(constant FrameUniforms& frame, constant TerrainZone& zone) {
    if (frame.forest < 0.5 || zone.far_count == 0xffffffffu) {
        return false;
    }
    float2 o = patch_origin(frame);
    float2 z0 = float2(zone.origin_x, zone.origin_z) + 3.0 * zone.step_m;
    float2 z1 = float2(zone.origin_x, zone.origin_z) + float(zone.n - 4) * zone.step_m;
    return all(o >= z0) && all(o + 2.0 * PATCH_HALF_M <= z1);
}

// Расстояние до края заплатки внутрь (< 0 — снаружи).
inline float patch_edge_dist(float2 p, constant FrameUniforms& frame) {
    float2 o = patch_origin(frame);
    float2 d = min(p - o, o + 2.0 * PATCH_HALF_M - p);
    return min(d.x, d.y);
}

inline float patch_zh(PatchZone g, int a, int b) {
    int n = int(g.zone.n);
    return g.zone_heights[uint(clamp(b, 0, n - 1)) * g.zone.n + uint(clamp(a, 0, n - 1))];
}

// Высота треугольника зоны под точкой — тот же разбор, что `HeightField::sample`
// и кластеры уровня 0 (диагональ из (i, j) в (i + 1, j + 1)).
inline float patch_tri_height(PatchZone g, float2 p) {
    float2 q = (p - float2(g.zone.origin_x, g.zone.origin_z)) / g.zone.step_m;
    int2 i = clamp(int2(floor(q)), int2(0), int2(int(g.zone.n) - 2));
    float2 f = q - float2(i);
    float a = patch_zh(g, i.x, i.y), b = patch_zh(g, i.x + 1, i.y);
    float c = patch_zh(g, i.x + 1, i.y + 1), d = patch_zh(g, i.x, i.y + 1);
    return f.x >= f.y ? a + f.x * (b - a) + f.y * (c - b) : a + f.y * (d - a) + f.x * (c - d);
}

inline float patch_cr(float p0, float p1, float p2, float p3, float t) {
    return p1 + 0.5 * t * (p2 - p0 + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + t * (3.0 * (p1 - p2) + p3 - p0)));
}

// Бикубическая (Catmull-Rom) высота зоны: гладкая, через те же узлы.
inline float patch_bicubic(PatchZone g, float2 p) {
    float2 q = (p - float2(g.zone.origin_x, g.zone.origin_z)) / g.zone.step_m;
    int2 i = int2(floor(q));
    float2 f = q - float2(i);
    float r[4];
    for (int k = 0; k < 4; ++k) {
        int b = i.y - 1 + k;
        r[k] = patch_cr(patch_zh(g, i.x - 1, b), patch_zh(g, i.x, b), patch_zh(g, i.x + 1, b),
                        patch_zh(g, i.x + 2, b), f.x);
    }
    return patch_cr(r[0], r[1], r[2], r[3], f.y);
}

inline float patch_vnoise(float2 p, uint salt) {
    int2 i = int2(floor(p));
    float2 f = p - float2(i);
    f = f * f * (3.0 - 2.0 * f);
    auto h = [&](int a, int b) {
        uint x = uint(i.x + a) * 0x8da6b343u ^ uint(i.y + b) * 0xd8163841u ^ salt * 0xcb1ab31fu;
        x ^= x >> 16;
        x *= 0x7feb352du;
        x ^= x >> 15;
        return float(x & 0xffffu) / 65535.0;
    };
    return mix(mix(h(0, 0), h(1, 0), f.x), mix(h(0, 1), h(1, 1), f.x), f.y);
}

// Высота заплатки в точке: гладкая поверхность + кочки, комья и камешки
// (до ±4 см); к краю — треугольники зоны.
inline float patch_height(PatchZone g, float2 p, constant FrameUniforms& frame) {
    float tri = patch_tri_height(g, p);
    float edge = smoothstep(0.0, PATCH_FADE_M, patch_edge_dist(p, frame));
    float2 w = p - frame.cam_hi.xz;  // узор от начала cam_hi: точен вдали от угла карты
    float disp = (patch_vnoise(w / 0.9, 11u) - 0.5) * 0.05 + (patch_vnoise(w / 0.3, 12u) - 0.5) * 0.025
               + (patch_vnoise(w / 0.11, 13u) - 0.5) * 0.01;
    return mix(tri, patch_bicubic(g, p) + disp, edge) + PATCH_LIFT_M;
}

// Нормаль заплатки разностями на полшага.
inline float3 patch_normal(PatchZone g, float2 p, constant FrameUniforms& frame) {
    float e = PATCH_STEP_M * 0.5;
    float dx = patch_height(g, p + float2(e, 0), frame) - patch_height(g, p - float2(e, 0), frame);
    float dz = patch_height(g, p + float2(0, e), frame) - patch_height(g, p - float2(0, e), frame);
    return normalize(float3(-dx, 2.0 * e, -dz));
}
