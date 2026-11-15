// Нужен common.h (включается до этого файла; загрузчик вставляет текст).

// Поле высот рельефа для растений, воды на земле и рек (02.10, 04.3, 03.2).
// Высота рельефа под точкой: билинейно по полю острова или зоны 1 м.
static float ground_height(float2 p, device const float* heights, device const float* zone_heights,
                           constant TerrainZone& zone, constant FrameUniforms& frame) {
    if (zone.far_count != 0xffffffffu) {
        float2 q = (p - float2(zone.origin_x, zone.origin_z)) / zone.step_m;
        float nz1 = float(zone.n - 1);
        if (all(q >= 0.0) && all(q <= nz1)) {
            int2 i = min(int2(floor(q)), int2(int(zone.n) - 2));
            float2 f = q - float2(i);
            auto h = [&](int a, int b) { return zone_heights[uint(b) * zone.n + uint(a)]; };
            return mix(mix(h(i.x, i.y), h(i.x + 1, i.y), f.x), mix(h(i.x, i.y + 1), h(i.x + 1, i.y + 1), f.x), f.y);
        }
    }
    uint nx = frame.grid_nx;
    int nz = int(frame.map_size_m / frame.grid_step_m + 0.5) + 1;
    float2 q = clamp(p / frame.grid_step_m, 0.0, float2(float(nx) - 1.001, float(nz) - 1.001));
    int2 i = int2(floor(q));
    float2 f = q - float2(i);
    auto h = [&](int a, int b) { return heights[uint(b) * nx + uint(a)]; };
    return mix(mix(h(i.x, i.y), h(i.x + 1, i.y), f.x), mix(h(i.x, i.y + 1), h(i.x + 1, i.y + 1), f.x), f.y);
}
