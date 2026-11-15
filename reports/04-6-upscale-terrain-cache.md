# Отчёт по этапу 04.6. Апскейл MetalFX и кэш материалов рельефа

- Спецификация — [specs/04-6-upscale-terrain-cache.md](../specs/04-6-upscale-terrain-cache.md)
- Дата и итерация — 2026-11-15, сессия 1
- Статус — partial (сессия 1 из 5)
- Сборка — `cargo build --release`; `./target/release/echo --world assets/world/yakushima/far --camera 19756,606,9719,19700,610,9660 --time 10:00 --gpu-times --render-scale 0.67`

## results

| Критерий | Ожидалось | Фактически | Статус | Доказательство |
| --- | --- | --- | --- | --- |
| Рендер в уменьшенном разрешении | Сцена, лучевое затенение, объёмный свет — в `rw × rh` | `Targets.scene`, `scene_depth`, `motion`; `rt_ao`, `vol`, история TAA — от размера рендера; руки, bloom, тонмаппинг — в выходном | passed | — |
| Апскейл | `MTL4FXTemporalScaler` вместо TAA при масштабе < 1 | `upscale::new_scaler`, `upscale::encode`; ядро `upscale_prep` (объёмный свет, векторы движения по глубине) | passed | [снимок 2560×1440](../logs/screens/04-6-metalfx.png) |
| Время кадра | Меньше | Одна серия, лес у тропы: масштаб 1 — 23,8 мс (сцена 18,2), 0,67 — 17,7 мс (сцена 11,0, MetalFX 1,9 мс против TAA 0,7) | passed | `--gpu-times` |
| Самопроверка, тесты | Без регрессий | 22/22, 87 тестов (самопроверка — масштаб 1: без TAA апскейл выключен) | passed | — |
| Чёткость | Без размытия | Детали листвы и травы чёткие; на качающихся листьях вблизи — смазывание: векторы движения только от камеры | partial | снимок; сессия 2 |

Замеры между запусками плавают до 2× (тепловой режим и частоты GPU): 13,7 мс в одном прогоне и 23,8 в другом на той же камере. Сравнения — только внутри одной серии.

## implementation

- `src/renderer/targets.rs`: `rw`, `rh`, `scene`, `scene_depth`, `motion`, `scaled()`; `Targets::new(…, scale)`; `ShaderWrite` у выходного HDR — только с апскейлом.
- `src/renderer/upscale.rs` (новый): создание апскейлера (`supportsMetal4FX`, автоэкспозиция), запись кадра (дрожание в пикселях рендера, векторы uv × размер рендера, reversed-Z, сброс без истории).
- `src/renderer/passes/taa.rs`: `encode_upscale` — подготовка, барьер очереди, MetalFX, барьер очереди пустым кодировщиком (кодировщики MetalFX свои).
- `shaders/metal/taa.metal`: `upscale_prep`.
- `passes/scene.rs`, `passes/rt.rs`, `VolumetricPass` — цели рендера.
- `src/renderer/mod.rs`: `render_scale`, `upscaler`, `render_size()`, `set_render_scale()`; апскейлер пересоздаётся с целями, старый — в отложенное освобождение.
- `src/app`: `--render-scale` (по умолчанию 0,67), масштаб 1 без TAA; дрожание и `TaaUniforms.jitter.zw` — по размеру рендера.
- `Cargo.toml`: `objc2-metal-fx = "0.3.2"`.

## architecture-decisions

AD-78.

## automated-testing

`cargo test --release` — 87 passed; `--selftest` — 22/22 passed; `cargo clippy` — без предупреждений.

## manual-testing

Снимок 2560×1440 при масштабе 0,67 — картинка верная, 54 кадра/с в заголовке.

## learning

n/a.

## followups

| Приоритет | Задача | Сессия | Критерий |
| --- | --- | --- | --- |
| P1 | Векторы движения ветра листвы и травы, рук; реактивная маска для альфа-карточек и воды | 2 | Нет смазывания листвы вблизи |
| P2 | Динамическое разрешение по GPU-таймерам | 2 | Кадр держится в бюджете |
| P1 | Кэш материалов рельефа | 3–4 | `scene` ≤ 6 мс |

## handoff

Доступна сессия 2. Масштаб 1 (`--render-scale 1`) возвращает прежний путь с TAA.
