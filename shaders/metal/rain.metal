#include "common.h"
#include "sky.h"

// Дождь (04.3, сессия 3): капли — штрихи в столбе вокруг камеры. Положение
// капли — из номера экземпляра и времени (падение с повтором по высоте), без
// симуляции. Карта укрытия (кроны и рельеф сверху вниз) даёт верхнюю
// поверхность под каплей: штрих обрывается на ней, а в точке удара —
// всплеск из отскакивающих брызг. Под кроной — редкая капель с листьев.

constant float RAIN_R = 22.0;      // радиус столба капель, м
constant float RAIN_H = 18.0;      // высота столба, м
constant float DROP_SPEED = 8.5;   // м/с (капли ливня 2–5 мм)
constant float SPLASH_T = 0.22;    // время жизни всплеска, с
constant uint SPLASH_PARTS = 4;    // брызг во всплеске

struct RainOut {
    float4 position [[position]];
    float2 uv;
    float alpha;
    float3 color;
};

static float rain_hash(uint x) {
    x ^= x >> 16;
    x *= 0x7feb352du;
    x ^= x >> 15;
    x *= 0x846ca68bu;
    x ^= x >> 16;
    return float(x) / 4294967295.0;
}

// Капля номера `iid` в момент `time`.
struct Drop {
    float3 head;     // голова капли, мир
    float3 vel;      // скорость, м/с
    float top;       // верхняя поверхность под каплей (крона или земля), мир y
    bool live;       // капля есть при текущей силе дождя
    bool drip;       // капель с кроны
    bool mapped;     // точка внутри карты укрытия
};

static Drop rain_drop(uint iid, constant FrameUniforms& frame, constant ShadowUniforms& shadows,
                      depth2d<float> rain_occ) {
    Drop d;
    float rain = frame.weather.y;
    float speed = DROP_SPEED * (0.85 + 0.3 * rain_hash(iid * 13u + 1u));
    // Клетки мира 2 × 2 м: капли привязаны к миру — при ходьбе не «едут».
    float a = rain_hash(iid * 3u + 11u) * 6.2831853;
    float r = RAIN_R * sqrt(rain_hash(iid * 5u + 17u));
    float2 cell = floor((frame.camera_pos.xz + float2(cos(a), sin(a)) * r) / 2.0);
    float2 wp = cell * 2.0 + float2(rain_hash(iid * 19u + 5u), rain_hash(iid * 23u + 7u)) * 2.0;
    float phase = fract(rain_hash(iid * 29u + 9u) + frame.time * speed / RAIN_H);
    float y = frame.camera_pos.y + RAIN_H * 0.6 - phase * RAIN_H;
    float wind = frame.weather.w;
    d.vel = float3(wind * 2.4, -speed, wind * 1.8);
    d.head = float3(wp.x, y, wp.y);
    float3 rel = float3(wp.x, frame.camera_pos.y, wp.y) - frame.camera_pos.xyz;
    float4 c = shadows.rain_vp_rel * float4(rel, 1.0);
    float2 uv = float2(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5);
    constexpr sampler rs(filter::nearest, address::clamp_to_edge);
    d.mapped = shadows.rain.w > 0.5 && all(abs(c.xy) < 0.98);
    d.top = d.mapped ? frame.camera_pos.y + shadows.rain.y - rain_occ.sample(rs, uv, level(0)) * shadows.rain.z
                     : -1e9;
    d.live = rain_hash(iid * 7u + 3u) < rain;
    // Капель: часть капель под кроной (поверхность выше глаз — крона, а не
    // земля) падает с её нижнего края медленнее и крупнее.
    d.drip = d.live && d.mapped && d.top > frame.camera_pos.y - 1.0 && rain_hash(iid * 31u + 2u) < 0.06;
    if (d.drip) {
        d.vel = float3(0.0, -6.0, 0.0);
    }
    return d;
}

static float3 rain_light(device const SkyFrame& skyf, float k) {
    // Небо (нулевая гармоника) и немного светила; капля прозрачна — светится
    // отражённым небом, ярче на тёмном фоне.
    return (skyf.sh[0].rgb * 0.282095 + skyf.light_color.rgb * 0.03) * k;
}

vertex RainOut rain_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                       constant FrameUniforms& frame [[buffer(1)]],
                       constant ShadowUniforms& shadows [[buffer(2)]],
                       device const SkyFrame& skyf [[buffer(3)]],
                       depth2d<float> rain_occ [[texture(0)]]) {
    RainOut o;
    Drop d = rain_drop(iid, frame, shadows, rain_occ);
    // Ниже поверхности капли нет (кроме капели — её прячет глубина земли).
    bool alive = d.live && (d.drip || d.head.y > d.top);
    float len = length(d.vel) * mix(0.012, 0.03, rain_hash(iid * 37u + 4u));
    float3 dir = normalize(d.vel);
    float3 to_cam = normalize(frame.camera_pos.xyz - d.head);
    float dist = length(frame.camera_pos.xyz - d.head);
    // Не тоньше ~пикселя: иначе штрих мерцает.
    float3 side = normalize(cross(dir, to_cam)) * max(d.drip ? 0.003 : 0.0018, dist * 0.0006);
    uint corner = vid % 6u;
    float2 q = float2(corner == 1u || corner == 2u || corner == 4u ? 1.0 : -1.0,
                      corner == 2u || corner == 4u || corner == 5u ? 1.0 : 0.0);
    float3 p = d.head + side * q.x - dir * len * q.y;
    o.position = alive ? frame.view_proj_rel * float4(camera_relative(frame, p), 1.0) : float4(0.0, 0.0, -2.0, 1.0);
    o.uv = q;
    // Разная прозрачность и яркость капель; ближе — заметнее, вдали — тает.
    float var = 0.35 + 0.65 * rain_hash(iid * 41u + 6u);
    o.alpha = (d.drip ? 0.35 : 0.14) * var * (1.0 - smoothstep(RAIN_R * 0.5, RAIN_R, dist)) * smoothstep(0.3, 1.2, dist);
    o.color = rain_light(skyf, 0.9 + 0.4 * var);
    return o;
}

// Всплеск: брызги разлетаются из точки удара по параболам и гаснут.
vertex RainOut rain_splash_vs(uint vid [[vertex_id]], uint iid [[instance_id]],
                              constant FrameUniforms& frame [[buffer(1)]],
                              constant ShadowUniforms& shadows [[buffer(2)]],
                              device const SkyFrame& skyf [[buffer(3)]],
                              depth2d<float> rain_occ [[texture(0)]]) {
    RainOut o;
    Drop d = rain_drop(iid, frame, shadows, rain_occ);
    // Время с удара: голова прошла поверхность `t` секунд назад.
    float t = (d.top - d.head.y) / -d.vel.y;
    // Заметный всплеск — у каждой четвёртой капли (крупные капли); иначе
    // земля «искрит» шумом.
    bool alive = d.live && !d.drip && d.mapped && t > 0.0 && t < SPLASH_T && rain_hash(iid * 43u + 8u) < 0.25;
    uint part = vid / 6u;
    uint corner = vid % 6u;
    uint h = iid * 7u + part * 131u;
    float ang = (float(part) + rain_hash(h + 1u)) * 6.2831853 / float(SPLASH_PARTS);
    float vh = 0.5 + 0.7 * rain_hash(h + 2u);
    float vv = 0.9 + 1.0 * rain_hash(h + 3u);
    float3 base = float3(d.head.x, d.top + 0.01, d.head.z);
    float3 p0 = base + float3(cos(ang) * vh * t, vv * t - 4.9 * t * t, sin(ang) * vh * t);
    float3 vel = float3(cos(ang) * vh, vv - 9.8 * t, sin(ang) * vh);
    float3 dir = normalize(vel + float3(0.0, 1e-3, 0.0));
    float3 to_cam = normalize(frame.camera_pos.xyz - p0);
    float dist = length(frame.camera_pos.xyz - p0);
    float3 side = normalize(cross(dir, to_cam)) * max(0.0012, dist * 0.0005);
    float len = max(length(vel) * 0.012, 0.004);
    float2 q = float2(corner == 1u || corner == 2u || corner == 4u ? 1.0 : -1.0,
                      corner == 2u || corner == 4u || corner == 5u ? 1.0 : 0.0);
    float3 p = p0 + side * q.x - dir * len * q.y;
    alive = alive && dist < 8.0 && p0.y >= d.top;
    o.position = alive ? frame.view_proj_rel * float4(camera_relative(frame, p), 1.0) : float4(0.0, 0.0, -2.0, 1.0);
    o.uv = q;
    o.alpha = 0.3 * (1.0 - t / SPLASH_T) * (1.0 - smoothstep(4.0, 8.0, dist));
    o.color = rain_light(skyf, 1.4);
    return o;
}

fragment float4 rain_fs(RainOut in [[stage_in]], constant FrameUniforms& frame [[buffer(1)]]) {
    // Мягкий край поперёк штриха, хвост бледнее головы.
    float edge = 1.0 - in.uv.x * in.uv.x;
    float a = in.alpha * edge * mix(1.0, 0.25, in.uv.y);
    return float4(in.color * frame.pre_exposure * a, a);
}
