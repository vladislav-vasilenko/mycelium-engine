// Объекты карты (02.7): точное расстояние до отрезков троп, рек и дорог.
// Отрезки разложены по ячейкам FEATURE_CELL_M (src/world/features.rs,
// FeatureSet::sample — тот же расчёт на CPU).

constant float FEATURE_CELL_M = 32.0;
constant float FEATURE_FAR = 1e3;

// Расстояние до края полосы, м (< 0 — внутри).
struct FeatureDist {
    float trail;
    float river;
    float road;
    // Маршрут уровня (05.6): расстояние до оси ближайшего отрезка маршрута, м,
    // и оставшийся по маршруту путь до цели в этой точке, м.
    float route;
    float route_left;
};

static FeatureDist feature_distances(float2 p, float map_size_m,
                                     device const FeatureSegment* segs,
                                     device const uint2* cells,
                                     device const uint* list) {
    FeatureDist d = {FEATURE_FAR, FEATURE_FAR, FEATURE_FAR, FEATURE_FAR, 0.0};
    int n = int(ceil(map_size_m / FEATURE_CELL_M));
    int2 c = int2(floor(p / FEATURE_CELL_M));
    if (c.x < 0 || c.y < 0 || c.x >= n || c.y >= n) {
        return d;
    }
    uint2 cell = cells[c.y * n + c.x];
    for (uint k = 0; k < cell.y; ++k) {
        FeatureSegment s = segs[list[cell.x + k]];
        float2 ab = s.b - s.a;
        float t = saturate(dot(p - s.a, ab) / max(dot(ab, ab), 1e-6));
        // У рек — разлив от подъёма воды (03.2, `_pad.x`, м).
        float dist = length(p - (s.a + ab * t)) - s.half_width - (s.kind == 1 ? s._pad.x : 0.0);
        // Отрезок маршрута (тропа или дорога): `_pad.x` — 1, если цель ближе
        // к `b`, 2 — к `a`; `_pad.y` — путь до цели от `a`, м.
        if (s.kind != 1 && s._pad.x > 0.5) {
            float axis = dist + s.half_width;
            if (axis < d.route) {
                float len = length(ab);
                d.route = axis;
                d.route_left = s._pad.y + (s._pad.x < 1.5 ? -t * len : t * len);
            }
        }
        if (s.kind == 0) {
            d.trail = min(d.trail, dist);
        } else if (s.kind == 1) {
            d.river = min(d.river, dist);
        } else {
            d.road = min(d.road, dist);
        }
    }
    return d;
}
