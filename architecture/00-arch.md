# Архитектура движка «Эхо мицелия»

Живой документ: описывает код в том виде, в котором он есть сейчас. Правила обновления — в [AGENTS.md](../AGENTS.md#архитектура).

- Состояние на — 2026-11-07, ревизия `22c3804` и правки после неё
- Покрытые этапы — [00. Платформа и архитектура](../specs/00-platform.md) (`completed`), [01. Основа Metal 4](../specs/01-metal-foundation.md) (`partial`), [02. Ландшафт и биомы](../specs/02-terrain-biomes.md) (`partial`): подэтап [02.1](../specs/02-1-terrain-data.md) (`completed`), 02.2–02.5 не начаты
- Отчёты — [00](../reports/00-platform.md), [01](../reports/01-metal-foundation.md), [02](../reports/02-terrain-biomes.md), [02.1](../reports/02-1-terrain-data.md)
- Подсистемы — [мир и ландшафт](02-terrain.md)
- Язык и API — Rust 1.99.1 (edition 2024), Metal 4 через `objc2-metal` 0.3.2, MSL

## Содержание

1. [Границы подсистем](#1-границы-подсистем)
2. [Соглашения](#2-соглашения)
3. [Поток кадра](#3-поток-кадра)
4. [Модули, структуры и методы](#4-модули-структуры-и-методы)
5. [Шейдеры и привязки](#5-шейдеры-и-привязки)
6. [Синхронизация и время жизни ресурсов](#6-синхронизация-и-время-жизни-ресурсов)
7. [Планировщик проходов](#7-планировщик-проходов)
8. [Самопроверка](#8-самопроверка)
9. [Соответствие спецификациям](#9-соответствие-спецификациям)
10. [Архитектурные решения и причины](#10-архитектурные-решения-и-причины)
11. [Ограничения и ближайшие изменения](#11-ограничения-и-ближайшие-изменения)
12. [Журнал изменений](#12-журнал-изменений)

## 1. Границы подсистем

[Спецификация 00](../specs/00-platform.md#границы-в-коде) задаёт четыре части: `Platform`, `World`, `Game`, `Renderer`. Сейчас они разложены по модулям так:

| Часть из спецификации | Модуль | Ответственность сейчас | Чего ещё нет |
| --- | --- | --- | --- |
| `Platform` | `src/platform.rs`, цикл окна и ввод в `src/main.rs` | Окно winit, ввод, `CAMetalLayer` на `NSView` | Отдельный модуль цикла и ввода, файловые пути |
| `World` | `src/world/` ([подробно](02-terrain.md)) | Генератор, тайлы 64 × 64 м, поля среды, `BiomeWeights`, `sample_ground` по треугольникам LOD 0, маркеры маршрута | LOD 1–2, растительность, стриминг (этапы 02, 08) |
| `Game` | `src/sim.rs` | Фиксированный тик, камера, полёт и ходьба по `Ground` | Игрок, задания, события (этап 05) |
| `Renderer` | `src/renderer/` | Устройство, очередь, ресурсы, проходы, синхронизация, вывод | Счётчики GPU, общий RHI (этап 06) |
| Общие данные | `src/layout.rs`, `shaders/metal/common.h` | Раскладка структур CPU/GPU | — |
| Описание проходов | `src/frame_graph.rs` | Проходы, ресурсы, барьеры без зависимости от API | Стадия indirect-аргументов (этап 02) |
| Тестовая сцена | `src/scene.rs` | Куб и два экземпляра; куб же рисует столбики маркеров | Остаётся для самопроверки (клавиша T) |
| Сетка ландшафта | `src/terrain_mesh.rs` | `TerrainVertex` всех тайлов, общие индексы, детальная текстура | LOD-сетки 33/17 и юбки (подэтап 02.3) |
| Эталоны | `src/reference.rs` | CPU-эталоны узора, глубины кубов и глубины ландшафта (луч по `Ground`) | — |

```mermaid
flowchart LR
    subgraph Platform
        main["main.rs<br/>App, цикл winit, ввод"]
        plat["platform.rs<br/>attach_metal_layer"]
    end
    subgraph Game
        sim["sim.rs<br/>Sim, FixedClock, CameraState"]
    end
    subgraph World
        world["world/<br/>World, Generator, Tile, Ground"]
    end
    tmesh["terrain_mesh.rs<br/>TerrainVertex, индексы, текстура"]
    subgraph Renderer
        rmod["renderer/mod.rs<br/>Renderer"]
        rsh["renderer/shaders.rs<br/>Pipelines, build_pipelines"]
        fg["frame_graph.rs<br/>plan, FramePlan"]
    end
    layout["layout.rs<br/>FrameUniforms, InstanceData, Vertex"]
    scene["scene.rs"]
    ref["reference.rs"]
    msl["shaders/metal/*.metal, common.h"]

    main -->|TickInput| sim
    sim -->|sample_ground| world
    world --> tmesh
    tmesh -->|set_terrain| rmod
    sim -->|CameraState копия| main
    main -->|FrameInput снимок| rmod
    main --> plat
    plat -->|CAMetalLayer| rmod
    rmod --> rsh
    rmod --> fg
    rsh --> msl
    layout -.раскладка.- msl
    main --> scene
    main --> ref
```

Правила границ, проверенные в коде:

- Нативные типы (`objc2`, `raw-window-handle`) подключены только в `platform.rs` и `renderer/`. Все вхождения `unsafe` там же: 1 в `platform.rs`, 19 в `renderer/`.
- `Game` не знает о рендере. `Renderer` получает копию состояния через `FrameInput` и не может изменить симуляцию.
- winit 0.30 использует objc2 0.5, проект — objc2 0.6. Связь идёт через сырой указатель `NSView` из `raw-window-handle`, поэтому типы двух версий не смешиваются.

## 2. Соглашения

| Тема | Правило | Где закреплено |
| --- | --- | --- |
| Единицы | Метры, секунды | `sim.rs` |
| Система координат | Правая, Y вверх, при `yaw = 0` камера смотрит в −Z | `CameraState::forward`, тест `yaw_zero_looks_down_negative_z` |
| Матрицы | Векторы-столбцы, `clip = P * V * M * p`, хранение по столбцам (`to_cols_array_2d`) | `layout.rs`, `scene.metal` |
| Глубина | Клип [0, 1], reversed-Z: ближняя плоскость 1, бесконечность 0, очистка 0, тест `GreaterEqual` | `sim::projection`, `Renderer::new`, тест `reversed_z_maps_near_to_one_and_far_to_zero` |
| Лицевая сторона | CCW, отсечение задних граней | `scene::cube`, `setFrontFacingWinding(CounterClockwise)`, тест `cube_triangles_face_outward` |
| Экран | Начало в левом верхнем углу | `reference::expected_depth` |
| Цвет | Линейный HDR `RGBA16Float`, одно преобразование в sRGB форматом `BGRA8Unorm_sRGB` | `shaders.rs`, `tonemap.metal` |
| Общие GPU-данные | `#[repr(C)]`, `align(16)` для uniform-структур, `bytemuck::Pod`, проверка смещений на CPU и GPU | `layout.rs`, ядро `layout_probe` |
| Время симуляции | Тик 1/60 с, не больше 5 догоняющих тиков, кадр длиннее 0,25 с обрезается | `sim.rs` |
| Карта | Тайлы 64 × 64 м, `TileId` — целые координаты, LOD 0 — 65 × 65 точек с шагом 1 м, граничные точки общие; север — малые z, море на юге | `world/mod.rs` |
| Треугольники ландшафта | Ячейка (i, j): диагональ (i, j)–(i+1, j+1), треугольники (a, c, b) и (a, d, c), CCW сверху; одинаково в отрисовке и `sample_ground` | `terrain_mesh::tile_indices`, `World::sample_ground` |

## 3. Поток кадра

```mermaid
sequenceDiagram
    participant W as winit
    participant A as App (main.rs)
    participant S as Sim
    participant R as Renderer
    participant Q as MTL4CommandQueue
    W->>A: RedrawRequested
    A->>A: FixedClock::advance(dt) → (ticks, alpha)
    loop ticks
        A->>S: step(TickInput)
    end
    A->>S: interpolated(alpha)
    S-->>A: CameraState
    A->>R: render(FrameInput)
    R->>R: collect_completed()
    R->>R: ждать слот по MTLSharedEvent (≤ 100 мс)
    R->>R: nextDrawable()
    R->>R: allocator.reset, beginCommandBuffer
    R->>R: проходы плана с барьерами
    R->>Q: waitForDrawable
    R->>Q: commit
    R->>Q: signalDrawable, present
    R->>Q: signalEvent(frame_index + 1)
    R-->>A: FrameOutcome
    A->>R: take_readbacks()
```

Порядок проходов: `pattern` (compute) → `scene` (render) → `readback` (compute, только по запросу) → `tonemap` (render).

## 4. Модули, структуры и методы

### 4.1. `src/main.rs` — приложение и цикл кадра

Связывает подсистемы. Управление: клик — захват мыши, WASD, Shift — бег, G — ходьба/полёт, E/Q — вверх/вниз в полёте, T — ландшафт/тестовые кубы, Esc, ⌘M — свернуть окно, F5 — перезагрузка MSL, F9 — readback, F12 — GPU capture. Аргументы: `--selftest`, `--overview` (старт в полёте с контрольной камеры), `--capture-frame N`.

| Элемент | Вид | Описание |
| --- | --- | --- |
| `LIGHT_DIR` | `const Vec3` | Направление света от источника к сцене, нормируется при записи в uniforms |
| `WORLD_SEED` = `0xEC40`, `WORLD_TILES` = 12, `DETAIL_TEXTURE_SIZE` = 512 | const | Первая карта и её текстура |
| `SKY_COLOR`, `TEST_SCENE_CLEAR` | const | Цвет очистки для ландшафта и для тестовой сцены |
| `SceneMode` | enum | `Cubes` — тестовая сцена этапа 01, `Terrain` — ландшафт и маркеры |
| `Args { selftest, overview, capture_frame }` | struct | Аргументы командной строки. `Args::parse()` разбирает `std::env::args` |
| `trail_start_camera(&World)` | fn | Поза на начале тропы, взгляд вдоль тропы |
| `terrain_check_camera(&World)` | fn | Контрольная камера: 12 м над точкой «склон над лесом», взгляд на торфяник |
| `marker_posts(&World) -> Vec<InstanceData>` | fn | Столбики 0,5 × 4 м на маркерах, цвет по виду |
| `start_camera()` | fn | Стартовая и контрольная поза камеры `(0.4, 1.4, 3.6)`, `pitch = −0.3` |
| `TestStep` | enum | Шаги самопроверки: `Warmup`, `ReadbackA`, `HotReload`, `AfterBrokenReload { frames }`, `Minimized { since, presented_at: Option<u64> }`, `Restored { presented_before, presented_while_minimized }`, `Resize`, `ReadbackB`, `TerrainView`, `TerrainReadback`, `Done` |
| `SelfTest` | struct | Состояние самопроверки: `step`, `frames_in_step`, `failures`, `log`, `resize_target`, `size_before`. `check(ok, what)` печатает `[pass]`/`[FAIL]` и копит ошибки |
| `App` | struct | Состояние приложения, см. ниже |
| `verify_readback(t, rb, view, proj)` | fn | Сверяет readback с эталонами: узор, глубина в 9 точках с покрытием обоих кубов, 17 полей раскладки |
| `compare_terrain_depth(rb, view, proj, &World, log) -> (hits, sky, bad, worst)` | fn | Глубина ландшафта с GPU, переведённая в метры (`NEAR_PLANE_M / depth`), против луча по `sample_ground`; допуск 2·10⁻³ относительно; небо — глубина 0 |
| `verify_terrain_readback(...)`, `report_terrain_readback(...)` | fn | Проверка ландшафта в самопроверке и вывод по F9 |
| `report_readback(rb, view, proj)` | fn | То же для F9 в обычном режиме, вывод в stderr |
| `main()` | fn | Создаёт `EventLoop` (`ControlFlow::Poll`), запускает `App`, выходит с `exit_code` |

Поля `App`:

| Поле | Тип | Назначение |
| --- | --- | --- |
| `args` | `Args` | Режимы запуска |
| `world` | `World` | Мир, генерируется в `new` (около 260 мс в Debug) |
| `scene` | `SceneMode` | Что рисовать; самопроверка начинает с `Cubes` |
| `window` | `Option<Arc<Window>>` | Окно, создаётся в `resumed` |
| `renderer` | `Option<Renderer>` | Бэкенд, удаляется в `exiting` до освобождения окна |
| `sim` | `Sim` | Состояние симуляции |
| `clock` | `FixedClock` | Аккумулятор времени |
| `keys` | `HashSet<KeyCode>` | Зажатые клавиши, очищаются при потере фокуса |
| `modifiers` | `ModifiersState` | Текущие модификаторы для сочетаний вроде ⌘M |
| `look` | `(f32, f32)` | Накопленный поворот мыши до следующего тика |
| `grabbed` | `bool` | Курсор захвачен |
| `last_frame`, `started`, `title_timer` | `Instant` | Время кадра, время с запуска, период обновления заголовка |
| `frames_since_title` | `u32` | Кадры для расчёта fps в заголовке |
| `last_error` | `Option<String>` | Последняя ошибка MSL (файл и стадия) |
| `readback_requested`, `capture_requested` | `bool` | Запросы F9 и F12 на следующий кадр |
| `selftest` | `Option<SelfTest>` | Активна при `--selftest` |
| `exit_code` | `i32` | 0 — успех, 1 — провал самопроверки, 2 — ошибка запуска |

Методы `App`:

| Метод | Что делает |
| --- | --- |
| `new(args)` | Генерирует мир, ставит `Sim` на начало тропы в режиме ходьбы (в самопроверке и `--overview` — полёт) |
| `set_grab(grab)` | Вызывает `platform::grab_cursor`, ставит `grabbed`, сбрасывает `look` |
| `reload_shaders()` | Вызывает `Renderer::reload_shaders(None)`, пишет результат в stderr и заголовок |
| `tick_input()` | Строит `TickInput` из `keys`; поворот мыши добавляется в `frame` |
| `is_minimized()` | `Window::is_minimized()`, `false` без окна |
| `update_title(force)` | Раз в 0,5 с: GPU, размер, fps, CPU-время кодирования, поколение или ошибка MSL; на ландшафте — режим, тайл, субстрат, тропа/вода, высота грунта |
| `frame(event_loop)` | Шаг кадра: тики `Sim::step(input, Some(&world))`, интерполяция, `FrameUniforms`, `FrameInput` (ландшафт, маркеры или кубы, цвет очистки), вызов `render`, обработка readback. В свёрнутом окне только `collect_completed` |
| `selftest_step(...)` | Конечный автомат самопроверки, см. [раздел 8](#8-самопроверка) |

`ApplicationHandler for App`:

| Обработчик | Поведение |
| --- | --- |
| `resumed` | Создаёт окно 1280 × 720 pt, `probe_device`, `attach_metal_layer`, `Renderer::new`, `set_terrain` (сетка, индексы, текстура), первый `resize`. Ошибка → stderr и код 2 |
| `window_event` | `CloseRequested` → выход; `Resized` → `Renderer::resize`; `ModifiersChanged` → `modifiers`; `Focused(false)` → очистка клавиш и освобождение мыши; клик → захват; клавиши, включая ⌘M → `set_minimized(true)`, G → `Sim::set_mode`, T → `scene`; `RedrawRequested` → `frame` |
| `device_event` | `MouseMotion` добавляет относительное смещение в `look`, если мышь захвачена |
| `about_to_wait` | Обычное окно: `ControlFlow::Poll` и `request_redraw`. Свёрнутое: `collect_completed`, `ControlFlow::WaitUntil(+100 мс)`, сброс `last_frame`, без перерисовки. В самопроверке разворачивает окно через секунду после подтверждённого сворачивания |
| `exiting` | Удаляет `Renderer` (его `Drop` ждёт GPU) |

Поворот мыши за кадр целиком уходит в первый тик кадра. Для воспроизводимой записи ввода его нужно будет хранить по тикам.

### 4.2. `src/platform.rs` — поверхность вывода

| Элемент | Сигнатура | Описание |
| --- | --- | --- |
| `attach_metal_layer` | `fn(&Window) -> Result<Retained<CAMetalLayer>, String>` | Берёт `NSView` из `RawWindowHandle::AppKit`, создаёт `CAMetalLayer`, включает `wantsLayer`, ставит слой и `contentsScale`. Вызывается на главном потоке из обработчика winit |
| `grab_cursor` | `fn(&Window, bool)` | Захват: `CursorGrabMode::Locked` (при неудаче `Confined`) и `NSCursor::hide`. Освобождение: `CursorGrabMode::None` и `NSCursor::unhide`. Вызовы `hide`/`unhide` сбалансированы флагом |
| `CURSOR_HIDDEN` | `static AtomicBool` | Скрыт ли курсор этим модулем |

### 4.3. `src/sim.rs` — фиксированный шаг и камера (`Game`)

Константы: `TICK_DT = 1/60`, `MAX_CATCHUP_TICKS = 5`, `MAX_FRAME_DT = 0.25`, `MOVE_SPEED = 8.0` м/с (полёт), `FAST_MULTIPLIER = 4.0`, `FLY_ACCEL = 40` м/с², `MOUSE_SENSITIVITY = 0.0025` рад/пиксель, `PITCH_LIMIT = 1.55` рад, `NEAR_PLANE_M = 0.05`. Ходьба: `EYE_HEIGHT_M = 1.7`, `MAX_STEP_M = 0.35`, `MAX_SLOPE_DEG = 40`, `WALK_SPEED = 4.0`, `RUN_SPEED = 7.0` м/с, `GROUND_ACCEL = 40`, `GROUND_FRICTION = 28`, `AIR_ACCEL = 6` м/с², `UPHILL_SLOWDOWN = 0.8`, `JUMP_SPEED = 4.2` м/с, `GRAVITY = 9.81`. Только для вида (приватные): `STEP_SMOOTH_TAU = 0.07` с, `STEP_SMOOTH_MIN = 0.08` м, `LAND_DIP_PER_SPEED = 0.025`, `LAND_DIP_MAX = 0.25` м, `BOB_STRIDE_M = 0.8`, `BOB_WALK_M = 0.03`, `BOB_RUN_M = 0.05`.

| Тип | Поля | Методы |
| --- | --- | --- |
| `TickInput` | `forward`, `right`, `up` ∈ [−1, 1] (`up > 0` при ходьбе — прыжок); `look_dx`, `look_dy` в пикселях; `fast` | — (команда одного тика) |
| `CameraState` | `position: Vec3`, `yaw`, `pitch` | `forward()` — единичный вектор взгляда; `view()` — `Mat4::look_to_rh`; `lerp(other, t)` — линейная интерполяция позы |
| `MoveMode` | `Fly`, `Walk` | — |
| `Sim` | `tick: u64`, `prev`, `curr: CameraState`, `mode`, `vertical_speed`, `on_ground`, `blocked_ticks`, `velocity: Vec3` (м/с), `view_dy_prev`, `view_dy` (сдвиг глаз только для вида), `head_bob: bool`; приватные `eye_lag`, `bob_phase`, `bob_weight` | `new(start)` (полёт); `look(dx, dy)` — поворот мыши сразу в `prev` и `curr`; `set_mode(mode, &dyn Ground)` — при ходьбе ставит на грунт, обнуляет скорость; `step(&TickInput, Option<&dyn Ground>)` — поворот из команды, затем `walk` или полёт с разгоном `FLY_ACCEL`, `tick += 1`; приватные `walk` (разгон, торможение, замедление на подъёме, скольжение с гашением скорости по оси упора, прыжок, падение, сглаживание ступеньки, приседание при приземлении, покачивание головы) и `can_step` (ступенька ≤ 0,35 м, подъём не круче 40°); `interpolated(alpha)` — поза плюс интерполированный `view_dy` |
| `FixedClock` | `accumulator` (приватное), `dropped_ticks` | `advance(frame_dt) -> (ticks, alpha)` — обрезает `dt`, считает тики, при превышении лимита отбрасывает долг и увеличивает `dropped_ticks` |

Свободные функции: `projection(aspect) -> Mat4` — `perspective_infinite_reverse_rh`, FOV 70°, near 0,05 м; приватные `approach(v, target, max_delta)` и `approach2` — сдвиг вектора к цели не больше чем на шаг.

Инвариант: состояние на тике N зависит только от команд тиков 0..N, поворотов `look` до этого тика и поверхности. `view_dy` не влияет на физику. Тесты `same_ticks_same_state_at_30_60_120_fps` и `walking_is_deterministic_across_frame_rates`. Правила ходьбы — в [02-terrain.md](02-terrain.md#5-ходьба).

### 4.4. `src/layout.rs` — общие данные CPU/GPU

| Структура | Размер, выравнивание | Поля и смещения |
| --- | --- | --- |
| `FrameUniforms` | 112 Б, 16 | `view_proj [[f32;4];4]` @0, `camera_pos [f32;4]` @64, `light_dir [f32;4]` @80, `time f32` @96, `exposure f32` @100, `pattern_time f32` @104, `frame_index u32` @108 |
| `InstanceData` | 80 Б, 16 | `model [[f32;4];4]` @0, `tint [f32;4]` @64 |
| `Vertex` | 32 Б, 4 | `position [f32;3]` @0, `normal [f32;3]` @12, `uv [f32;2]` @24. В MSL — `packed_float3`, `packed_float3`, `float2` |
| `TerrainVertex` | 32 Б, 4 | `position [f32;3]` @0 (мировые м), `normal [f32;3]` @12, `weights [u8;4]` @24 (лес, торф, берег, тропа), `material [u8;4]` @28 (субстрат, русло, влажность, резерв). В MSL — `packed_float3`, `packed_float3`, `uchar4`, `uchar4` |

| Элемент | Описание |
| --- | --- |
| `expected_probe() -> [u32; 17]` | Ожидаемые размеры и смещения в порядке записи ядра `layout_probe` |
| `LAYOUT_PROBE_LEN = 17`, `LAYOUT_PROBE_NAMES` | Длина и имена полей для диагностики |

Изменение любой структуры требует правки `common.h` в той же правке; несовпадение ловят тест `cpu_layout_matches_msl_rules` и самопроверка.

### 4.5. `src/frame_graph.rs` — проходы и барьеры

Не зависит от Metal. Алгоритм — в [разделе 7](#7-планировщик-проходов).

| Тип | Описание |
| --- | --- |
| `ResourceId(u32)` | Логический ресурс кадра |
| `Stages(u32)` | Битовая маска стадий: `NONE`, `VERTEX`, `FRAGMENT`, `DISPATCH`, `BLIT`. Методы `is_empty`, `contains`, операторы `|`, `|=` |
| `PassKind` | `Compute` (разрешены `DISPATCH`, `BLIT`) или `Render` (`VERTEX`, `FRAGMENT`). Приватный `allowed()` |
| `Hazard` | `Raw`, `War`, `Waw` |
| `PassDecl` | `name`, `kind`, `reads`, `writes` — списки `(ResourceId, Stages)` |
| `Barrier` | `after`, `before: Stages`, `hazards: Vec<(ResourceId, Hazard)>` — один барьер перед проходом |
| `FramePlan` | `passes` и `barriers` с совпадающими индексами |
| `GraphError` | `ReadBeforeWrite { pass, resource }`, `StageNotAllowed { pass, stages }`, реализует `Display` |
| `Track` (приватный) | `last_write`, `reads_since_write` для одного ресурса |
| `plan_after(previous, passes, imported) -> Result<FramePlan, GraphError>` | Валидация и барьеры кадра `passes`, перед которым в очереди выполнен кадр `previous` |
| `plan(passes, imported)` (только тесты) | `plan_after` для кадра, повторяющего тот же список |

### 4.6. `src/renderer/mod.rs` — бэкенд Metal 4

Константы:

| Константа | Значение | Смысл |
| --- | --- | --- |
| `FRAME_SLOTS` | 2 | Кадров в полёте |
| `SLOT_WAIT_TIMEOUT_MS` | 100 | Таймаут ожидания слота, после — пропуск кадра |
| `UPLOAD_BYTES` | 4096 | Upload-буфер слота |
| `INSTANCE_OFFSET` | 256 | Начало экземпляров в upload |
| `MAX_INSTANCES` | 48 | `(4096 − 256) / 80` |
| `RES_PATTERN … RES_READBACK` | 1…5 | `ResourceId` для планировщика |
| `RB_*`, `DEPTH_SAMPLES` | см. [раздел 6](#раскладка-буферов) | Раскладка readback и 9 точек проверки глубины |

Публичные типы:

| Тип | Поля и смысл |
| --- | --- |
| `DeviceInfo` | `name`, `os`, `metal4`, `apple_family: Option<u32>`, `raytracing`, `raytracing_from_render: Option<bool>` (`None` в режиме capture). `Display` печатает строку диагностики |
| `FrameOutcome` | `Presented`, `SkippedZeroSize`, `SkippedNoDrawable`, `SkippedBusySlot` |
| `FrameInput<'a>` | Снимок для кадра: `uniforms: FrameUniforms`, `instances: &[InstanceData]` (кубы или столбики маркеров), `draw_terrain: bool`, `clear_color: [f32; 3]`, `readback: bool`, `capture: Option<PathBuf>` |
| `Readback` | `frame`, `pattern_time`, `width`, `height`, `pattern: Vec<u8>` (RGBA8 256 × 256), `depth: Vec<(x, y, f32)>`, `probe: [u32; 17]` |
| `Stats` | `presented`, `skipped_busy`, `skipped_no_drawable`, `released_deferred`, `cpu_encode_ms` |

Приватные типы:

| Тип | Поля и смысл |
| --- | --- |
| `PendingReadback` | `signal_value` (значение события, после которого можно читать), `frame`, `pattern_time`, `width`, `height`, `pixels` (точки глубины) |
| `FrameSlot` | `allocator: MTL4CommandAllocator`, `cmd: MTL4CommandBuffer`, `tables: [MTL4ArgumentTable; 5]` (pattern, scene, readback, tonemap, terrain), `upload: MTLBuffer`, `signal_value: u64` |
| `TerrainGpu` | `vertices` (все тайлы), `indices` (одна сетка 65 × 65), `index_count`, `tile_count`, `detail` (текстура с mip) |
| `Targets` | `width`, `height`, `hdr` (`RGBA16Float`), `depth` (`Depth32Float`), обе `Private` |
| `Retired` | `after_value`, `textures` (удаляются из residency set), `_keep_alive: Option<Box<dyn Any>>` (например, старые `Pipelines`) |

Поля `Renderer`:

| Поле | Тип | Назначение |
| --- | --- | --- |
| `info` | `DeviceInfo` | Диагностика адаптера |
| `device` | `MTLDevice` | Устройство |
| `queue` | `MTL4CommandQueue` | Единственная очередь для compute, render и copy |
| `compiler` | `MTL4Compiler` | Сборка pipeline |
| `layer` | `CAMetalLayer` | Поверхность вывода, `BGRA8Unorm_sRGB`, `framebufferOnly`, 3 drawable |
| `residency` | `MTLResidencySet` | Все ресурсы движка; добавлен в очередь вместе с `layer.residencySet()` |
| `event` | `MTLSharedEvent` | Номер последнего завершённого кадра |
| `depth_state` | `MTLDepthStencilState` | `GreaterEqual`, запись глубины |
| `slots` | `Vec<FrameSlot>` | Два слота кадра |
| `pipelines` | `Pipelines` | Текущее поколение pipeline |
| `targets` | `Option<Targets>` | `None` при нулевом размере окна |
| `terrain` | `Option<TerrainGpu>` | Загруженный ландшафт |
| `pattern_tex` | `MTLTexture` | 256 × 256 `RGBA8Unorm`, `ShaderRead | ShaderWrite` |
| `vertex_buf`, `index_buf` | `MTLBuffer` | Куб в `StorageModeShared` |
| `index_count` | `usize` | 36 |
| `readback_buf` | `MTLBuffer` | Общий буфер readback |
| `plans` | `[[FramePlan; 2]; 2]` | Планы `plans[prev][cur]` по наличию readback в предыдущем и текущем кадре |
| `prev_readback` | `bool` | Был ли readback в последнем отправленном кадре |
| `frame_index` | `u64` | Число отправленных кадров |
| `retired` | `Vec<Retired>` | Ожидают завершения GPU |
| `pending_readback` | `Option<PendingReadback>` | Не больше одного readback в полёте |
| `completed_readbacks` | `Vec<Readback>` | Готовые результаты |
| `shader_dir` | `PathBuf` | Каталог MSL |
| `stats` | `Stats` | Счётчики |

Методы `Renderer`:

| Метод | Описание |
| --- | --- |
| `new(device, info, layer, vertices, indices) -> Result<Renderer, String>` | Проверяет Metal 4, настраивает слой, создаёт очередь, компилятор, событие, residency set, depth state, pipeline, буферы, текстуру узора, два слота с таблицами, строит четыре плана переходов |
| `size() -> Option<(u32, u32)>` | Размер целей |
| `pipeline_generation() -> u32` | Поколение pipeline |
| `frame_index() -> u64` | Отправлено кадров |
| `shader_dir() -> &Path` | Каталог MSL |
| `plan_summary() -> String` | Текстовые планы «readback после readback» и «обычный кадр после readback» |
| `resize(width, height, scale)` | Нулевой размер → цели уходят в `retired`, вывод пропускается. Иначе новые цели по пиксельному размеру, добавление в residency set, старые — в `retired` |
| `reload_shaders(over) -> Result<u32, ShaderError>` | Собирает новый набор; при успехе подменяет, старый уходит в `retired`; при ошибке текущий не трогается |
| `collect_completed()` | Освобождает `retired` по значению события, забирает готовый readback |
| `take_readbacks() -> Vec<Readback>` | Отдаёт готовые результаты |
| `set_terrain(vertices, indices, tile_count, detail_rgba, detail_size) -> Result<(), String>` | Буферы в `StorageModeShared`, private-текстура с полным набором mip; одноразовый командный буфер: копия staging → mip 0, барьер blit → blit, `generateMipmaps`; CPU ждёт отдельный `MTLSharedEvent` до 5 с; staging удаляется из residency set; прежний ландшафт уходит в `retired` |
| `wait_idle()` | Ждёт все отправленные кадры, до 2 с |
| `render(&FrameInput) -> FrameOutcome` | Кадр целиком, см. [раздел 3](#3-поток-кадра) и [раздел 6](#6-синхронизация-и-время-жизни-ресурсов). В проходе `scene` сначала ландшафт (группа `terrain: N tiles`, один instanced draw), затем кубы |
| `retire(textures, other)` (приватный) | Ставит ресурс в очередь освобождения с `after_value = frame_index` |
| `read_back(PendingReadback) -> Readback` (приватный) | Копирует данные из `readback_buf` |
| `start_capture(path) -> bool` (приватный) | `MTLCaptureManager` → `.gputrace`, нужен `MTL_CAPTURE_ENABLED=1` |
| `Drop` | `wait_idle()` |

Свободные функции модуля:

| Функция | Описание |
| --- | --- |
| `probe_device() -> Result<(device, DeviceInfo), String>` | `MTLCreateSystemDefaultDevice`, семейство Apple10…7, Metal 4, признаки трассировки |
| `capture_enabled()` | `MTL_CAPTURE_ENABLED == "1"`; при capture признаки трассировки не запрашиваются |
| `flag(Option<bool>)` | Текст «да», «нет», «не запрошено (capture)» |
| `label(&str)` | `NSString` для подписей ресурсов |
| `to_mtl_stages(Stages) -> MTLStages` | Перевод стадий планировщика в Metal |
| `as_allocation(&obj)` | Приведение буфера или текстуры к `MTLAllocation` для residency set |
| `shared_buffer(device, len, name)` | Буфер `StorageModeShared` с подписью |
| `upload_bytes(buf, offset, bytes)` | Проверенная запись в shared-буфер |
| `private_texture(device, format, w, h, usage, name)` | 2D текстура `Private` с подписью |
| `frame_passes(readback) -> Vec<PassDecl>` | Объявление проходов кадра |

### 4.7. `src/renderer/shaders.rs` — MSL и pipeline

| Элемент | Описание |
| --- | --- |
| `Obj<T>` | `Retained<ProtocolObject<T>>` — владеющая ссылка на объект Metal |
| `HDR_FORMAT`, `DEPTH_FORMAT`, `DRAWABLE_FORMAT`, `PATTERN_FORMAT` | `RGBA16Float`, `Depth32Float`, `BGRA8Unorm_sRGB`, `RGBA8Unorm` |
| `Pipelines` | `pattern`, `layout_probe` (compute), `scene`, `tonemap`, `terrain` (render), `generation` |
| `ShaderError { file, stage, message }` | Стадии: `read`, `compile MSL`, `entry point`, `compute pipeline`, `render pipeline` |
| `SourceOverride<'a>` | `Option<(файл, текст)>` — подмена исходника в памяти для самопроверки |
| `shader_dir()` | `ECHO_SHADER_DIR`, иначе `Contents/Resources/shaders/metal` при запуске из бандла, иначе `shaders/metal` рядом с `Cargo.toml` |
| `bundle_shader_dir()` (приватная) | Если исполняемый файл лежит в `Contents/MacOS`, возвращает существующий `../Resources/shaders/metal` |
| `read_source(dir, file, over)` | Читает файл и подставляет текст `common.h` вместо `#include "common.h"` |
| `compile_library(device, dir, file, over)` | `newLibraryWithSource:options:error:` |
| `function(lib, name)` | `MTL4LibraryFunctionDescriptor` |
| `require_function(lib, file, name)` | Проверка точки входа по `functionNames` |
| `compute_pipeline(compiler, lib, file, name)` | `MTL4ComputePipelineDescriptor` → `MTL4Compiler` |
| `render_pipeline(compiler, lib, file, label, vs, fs, color_format)` | `MTL4RenderPipelineDescriptor` с одним цветовым вложением |
| `build_pipelines(device, compiler, dir, generation, over)` | Собирает все четыре pipeline или возвращает первую ошибку; частичный набор не публикуется |

### 4.8. `src/scene.rs` — тестовая сцена этапа 01

| Функция | Описание |
| --- | --- |
| `cube() -> (Vec<Vertex>, Vec<u16>)` | Куб [−0.5, 0.5]³: 6 граней по 4 вершины, 36 индексов, обход CCW снаружи |
| `instance_models() -> [Mat4; 2]` | Куб A: масштаб 1,2, поворот Y 0,4. Куб B: масштаб 0,9, поворот X 0,6 и Y −0,3, сдвиг (0,65; 0,35; 0,45), пересекает A |
| `instances() -> [InstanceData; 2]` | Матрицы и оттенки для GPU |
| `boxes() -> Vec<Box3>` | Те же кубы для CPU-эталона |

### 4.9. `src/reference.rs` — CPU-эталоны

| Элемент | Описание |
| --- | --- |
| `PATTERN_SIZE = 256`, `SELFTEST_PATTERN_TIME = 1.25` | Размер узора и фиксированное время самопроверки |
| `pattern_color(x, y, size, t) -> [f32; 3]` | Повторяет `pattern_color` из `common.h`: градиент, шахматка 32 px, полосы по синусу |
| `compare_pattern(pixels, size, t, tolerance) -> (mismatches, max_diff)` | Сверка RGBA8 с эталоном в единицах 1/255 |
| `Box3 { model }` | Куб для пересечения лучом |
| `ray_box(origin, dir, box)` (приватная) | Метод плит в локальных координатах куба |
| `expected_depth(px, py, w, h, view, proj, boxes) -> f32` | Луч из центра пикселя, ближайшее попадание, reversed-Z глубина; 0 — фон |
| `pixel_ray(px, py, w, h, view, proj) -> (origin, dir)` | Луч из центра пикселя в мире |
| `expected_terrain_view_depth((px, py), (w, h), view, proj, &dyn Ground, max_dist) -> Option<f32>` | Шаг 0,25 м до пересечения с грунтом, 40 шагов бисекции, глубина по оси взгляда в метрах; `None` — за картой или дальше `max_dist` |

### 4.10. Сборка `.app`

`scripts/bundle-app.sh [--open]` собирает `target/release/Echo.app`:

| Путь в бандле | Источник |
| --- | --- |
| `Contents/MacOS/echo` | `cargo build --release` |
| `Contents/Resources/shaders/metal/` | копия `shaders/metal/`, компилируется при запуске |
| `Contents/Info.plist` | `packaging/Info.plist`: `com.vladislav-vasilenko.mycelium-engine`, версия из `Cargo.toml`, номер сборки — число коммитов, `LSMinimumSystemVersion` 26.0, `NSHighResolutionCapable` |

Подпись ad-hoc (`codesign --sign -`), проверка `codesign --verify --strict`. Задача VS Code `bundle Echo.app`.

| Флаг | Действие |
| --- | --- |
| `--install` | `ditto` в `/Applications/Echo.app` (или `ECHO_INSTALL_DIR`), снятие карантина, проверка подписи; запущенная копия закрывается |
| `--open` | Запуск установленной или собранной копии |

`scripts/make-dmg.sh [--no-build] [--layout]` собирает `dist/Echo-<версия>.dmg` (UDZO, HFS+): `Echo.app` и символическая ссылка `Applications`. `--layout` создаёт записываемый образ, оформляет окно Finder через AppleScript (значки 128 pt, приложение слева, `Applications` справа) и конвертирует его в сжатый. Переменные `ECHO_SIGN_IDENTITY` (подпись Developer ID с hardened runtime) и `ECHO_NOTARY_PROFILE` (`notarytool submit --wait`, `stapler staple`) включают подпись и нотаризацию. Задача VS Code `make Echo.dmg`.

## 5. Шейдеры и привязки

Файлы в `shaders/metal/`. `common.h` подставляется в каждый файл при загрузке.

| Файл | Функции | Назначение |
| --- | --- | --- |
| `common.h` | `FrameUniforms`, `InstanceData`, `Vertex`, `pattern_color` | Общие типы и эталонная функция узора |
| `pattern.metal` | `pattern_main` (kernel), `layout_probe` (kernel) | Диагностический узор; запись размеров и смещений структур |
| `scene.metal` | `scene_vs`, `scene_fs` | Вершины из буфера по `vertex_id`, экземпляры по `instance_id`; ambient + Ламберт с текстурой узора |
| `terrain.metal` | `terrain_vs`, `terrain_fs` | Вершина `instance_id × 4225 + vertex_id`; цвета биомов, камень на склонах, галька, тропа, детальная текстура, временный свет. См. [02-terrain.md](02-terrain.md#4-отрисовка) |
| `tonemap.metal` | `tonemap_vs`, `tonemap_fs` | Полноэкранный треугольник без буфера; экспозиция и ACES (Narkowicz) |

Таблицы аргументов (`MTL4ArgumentTable`, по одной на проход в каждом слоте, до 4 буферов и 2 текстур):

| Проход | `buffer(0)` | `buffer(1)` | `buffer(2)` | `texture(0)` | Стадии |
| --- | --- | --- | --- | --- | --- |
| `pattern` | upload: `FrameUniforms` | — | — | `pattern_tex` (write) | compute |
| `scene` | `vertex_buf` | upload: `FrameUniforms` | upload + 256: `InstanceData[]` | `pattern_tex` (sample) | vertex, fragment |
| `terrain` (в проходе `scene`) | вершины ландшафта | upload: `FrameUniforms` | — | детальная текстура | vertex, fragment |
| `readback` | `readback_buf` + `RB_PROBE_OFFSET` | — | — | — | compute |
| `tonemap` | — | upload: `FrameUniforms` | — | `hdr` (sample) | vertex, fragment |

Сэмплеры заданы в MSL через `constexpr sampler`, отдельные объекты сэмплеров не используются.

## 6. Синхронизация и время жизни ресурсов

### Слоты кадра и событие

- Кадр с номером N (с нуля) использует слот `N % 2` и после commit сигналит `MTLSharedEvent` значением `N + 1`. Это значение записывается в `slot.signal_value`.
- Перед записью в слот CPU ждёт `event ≥ slot.signal_value`. При таймауте 100 мс кадр пропускается (`SkippedBusySlot`) с сообщением в stderr.
- Других ожиданий CPU нет, кроме `wait_idle()` при выходе. В прогоне самопроверки пропусков 0.

### Вывод кадра

Схема Apple для Metal 4: `nextDrawable` → кодирование → `queue.waitForDrawable` → `commit` → `queue.signalDrawable` → `present`. Без drawable кадр пропускается (`SkippedNoDrawable`), при нулевом размере — `SkippedZeroSize`.

### Резидентность

Все буферы и текстуры движка добавлены в один `MTLResidencySet`, который подключён к очереди вместе с `layer.residencySet()`. Новые цели после resize добавляются сразу, старые удаляются в `collect_completed` после завершения кадров, затем `commit()` набора.

### Отложенное освобождение

`retire` запоминает `after_value = frame_index`, то есть номер последнего отправленного кадра. `collect_completed` освобождает запись, когда `event.signaledValue() ≥ after_value`. Так освобождаются старые цели после resize и старые pipeline после перезагрузки MSL.

### Раскладка буферов

| Буфер | Диапазон | Содержимое |
| --- | --- | --- |
| upload слота (4096 Б) | 0..112 | `FrameUniforms` |
| | 256..4096 | до 48 `InstanceData` |
| `readback_buf` (262 656 Б) | 0..262 144 | узор RGBA8, строка 1024 Б |
| | 262 144..262 400 | глубина `f32` в 9 точках `DEPTH_SAMPLES` |
| | 262 400..262 656 | 13 значений `u32` из `layout_probe` |

`readback_buf` один на приложение, поэтому в полёте не больше одного readback. Данные читаются после сигнала кадра, в котором стоят копии.

## 7. Планировщик проходов

`frame_graph::plan`:

1. Проверяет, что стадии прохода разрешены его типом, и что каждый прочитанный ресурс записан раньше в кадре или помечен импортированным. Иначе `GraphError`.
2. Первый круг проходит список предыдущего кадра и только собирает состояние ресурсов, второй проходит текущий кадр и выдаёт барьеры. Так учитываются зависимости через границу кадров в одной очереди, в том числе когда состав проходов меняется между кадрами.
3. Для чтения: если есть предыдущая запись — RAW, `after |= стадия записи`.
4. Для записи: если были чтения после последней записи — WAR по стадиям чтений; иначе, если была запись — WAW.
5. Все зависимости прохода сливаются в один барьер `after → before`.

Бэкенд Metal 4 кодирует барьер в начале кодировщика прохода: `barrierAfterQueueStages:beforeStages:visibilityOptions:` с `MTL4VisibilityOptions::Device`. Это consumer queue barrier в терминах Apple: он задерживает стадии `before` в этом и всех следующих проходах очереди, пока стадии `after` не завершатся во всех предыдущих проходах, включая прошлые кадры.

Renderer хранит четыре плана `plans[prev][cur]` и выбирает план по тому, был ли readback в предыдущем отправленном кадре. План «обычный кадр после кадра с readback» отличается от повторяющегося обычного кадра: `pattern` и `scene` ждут ещё и `BLIT`, иначе запись следующего кадра может пересечься с копией readback.

План «кадр с readback после такого же», как его печатает самопроверка:

| Проход | after | before | Зависимости |
| --- | --- | --- | --- |
| `pattern` | `FRAGMENT \| BLIT` | `DISPATCH` | WAR `pattern` (чтения прошлого кадра) |
| `scene` | `FRAGMENT \| DISPATCH \| BLIT` | `FRAGMENT` | RAW `pattern`, WAR `hdr`, WAR `depth` |
| `readback` | `FRAGMENT \| DISPATCH \| BLIT` | `DISPATCH \| BLIT` | RAW `pattern`, RAW `depth`, WAW `readback` |
| `tonemap` | `FRAGMENT` | `FRAGMENT` | RAW `hdr`, WAW `drawable` |

План «обычный кадр после кадра с readback»: `pattern` — after `FRAGMENT | BLIT`, `scene` — after `FRAGMENT | DISPATCH | BLIT`, `tonemap` без изменений.

Слияние зависимостей в один барьер иногда синхронизирует больше необходимого. Это принятое упрощение этапа 01.

## 8. Самопроверка

`./target/debug/echo --selftest` с фиксированной камерой и `pattern_time = 1.25`. Рекомендуемый запуск: `MTL_DEBUG_LAYER=1 MTL_DEBUG_LAYER_ERROR_MODE=assert MTL_SHADER_VALIDATION=1`.

| Шаг | Условие перехода | Проверки |
| --- | --- | --- |
| `Warmup` | 5 кадров | Печать `DeviceInfo` и плана; захват курсора на 2-м кадре и освобождение на 4-м; запрос readback |
| `ReadbackA` | Готов readback | Узор 256 × 256, глубина в 9 точках с покрытием обоих кубов, 13 полей раскладки |
| `HotReload` | Сразу | Перезагрузка без изменений → поколение +1; ошибочный `pattern.metal` из памяти → ошибка с файлом и стадией, поколение не меняется |
| `AfterBrokenReload` | 3 кадра | Вывод продолжается; исправленная перезагрузка успешна; `set_minimized(true)` |
| `Minimized` | Окно подтверждённо свёрнуто 1 с (`about_to_wait`) | Счёт кадров с момента, когда `is_minimized()` стал `true`; разворот. Не свернулось за 5 с — провал |
| `Restored` | 5 выведенных кадров | В свёрнутом окне выведено 0 кадров; вывод возобновился; запрос размера окна 960 × 600 |
| `Resize` | Размер изменился и прошло 4 кадра | Новые цели, запрос readback |
| `ReadbackB` | Готов readback | Те же проверки при новом размере; сцена переключается на ландшафт |
| `TerrainView` | 3 кадра с контрольной камерой ландшафта | Запрос readback |
| `TerrainReadback` | Готов readback | Глубина ландшафта против луча по `sample_ground` (≥ 4 точек на грунте), раскладка 17 полей |
| `Done` | — | Статистика, `SELFTEST PASSED` и код 0 или `SELFTEST FAILED` и код 1. Предохранитель — 1000 кадров. Кадры, пришедшие после `exit()`, игнорируются, итог печатается один раз |

Итог — 18 проверок. Модульные тесты (`cargo test`, 32 и 1 ручной `dump_map`): планировщик (6), раскладка (1), симуляция и ходьба (10), эталоны (2), куб (1), шум (2), генератор (1), мир (6), сетка ландшафта (3).

## 9. Соответствие спецификациям

| Пункт спецификации | Реализация | Статус |
| --- | --- | --- |
| 00 · Rust, Metal 4, MSL | `Cargo.toml`, `objc2-metal`, `shaders/metal/` | Выполнено |
| 00 · Проверка GPU в приложении | `probe_device`, `DeviceInfo` | Выполнено |
| 00 · Границы `Platform`/`World`/`Game`/`Renderer` | [Раздел 1](#1-границы-подсистем) | `World` появится в 02 |
| 00 · Единицы, координаты, reversed-Z, CCW | [Раздел 2](#2-соглашения) | Выполнено, покрыто тестами |
| 00 · `#[repr(C)]` и проверка на GPU | `layout.rs`, `layout_probe` | Выполнено |
| 00 · Линейный HDR, одно преобразование в sRGB | `HDR_FORMAT`, `DRAWABLE_FORMAT`, `tonemap.metal` | Выполнено |
| 01.1 · winit, `CAMetalLayer`, Debug-компиляция MSL, подписи | `platform.rs`, `shaders.rs`, `label` | Выполнено; офлайн `.metallib` не сделан (нет Metal Toolchain) |
| 01.2 · `MTL4CommandQueue`, буфер, аллокатор, таблицы, резидентность, drawable | `Renderer::new`, `render` | Выполнено |
| 01.3 · Два слота, `MTLSharedEvent`, таймаут, resize, свёрнутое окно | `FrameSlot`, `render`, `resize`, `Retired`, `App::is_minimized`, `about_to_wait` | Выполнено; сворачивание проверяет самопроверка |
| 01.4 · Куб, `Depth32Float`, `RGBA16Float`, тонмаппинг | `scene.rs`, `Targets`, `tonemap.metal` | Выполнено |
| 01.5 · Ввод, фиксированный шаг, интерполяция | `App`, `sim.rs` | Выполнено; ввод вручную не проверен |
| 01.6 · Compute → fragment, сверка с эталоном | `pattern_main`, барьер, `verify_readback` | Выполнено |
| 01.7 · Перезагрузка MSL | `reload_shaders`, `ShaderError` | Выполнено; схема аргументов проверяется только по точкам входа |
| 02.1 · Тайлы 64 × 64 м, 65 × 65 высот, общие границы, `worldSeed`, версия, `TileId` | `world/mod.rs`: `Tile`, `TileId`, `World::generate` | Выполнено |
| 02.1 · Поля высоты, влажности, температуры, субстрата; тропа, русло, берег | `world/generator.rs` | Выполнено |
| 02.1 · `SampleGround(xz)` по треугольникам LOD 0 | `impl Ground for World` | Выполнено, проверено на CPU и GPU |
| 02.1 · `BiomeWeights` из полей, наклона, масок | `generator::biome_weights` | Выполнено |
| 02.1 · Карта высот, сетки на CPU, indexed terrain с текстурой | `terrain_mesh.rs`, `Renderer::set_terrain`, `terrain.metal` | Выполнено; карта генерируется, а не загружается из файла (AD-28) |
| 02.1 · Ходьба: ступенька, крутой склон, граница тайлов | `Sim::walk`, тесты `sim::tests` | Выполнено |
| 02.1 · Точки наблюдения, место моста, коридор реки до моря | `World::place_markers`, `marker_posts` | Выполнено |
| 02.1 · Проход по тропе от начала до каменистого берега | Бот в тесте `bot_walks_whole_trail_from_start_to_shore`, брод (AD-33) | Выполнено |
| 02.2 · Texture arrays, `TerrainMaterials`, `TerrainView` | — | Не начато |
| 02.3 · `TileBounds`, `TileDraw`, индексы 65/33/17 по маске краёв | — | Не начато |
| 02.4 · `Placement`, группы экземпляров | — | Не начато |
| 02.5 · Видимые списки, indirect, импосторы | — | Не начато |
| 01 · Описание проходов, RAW/WAR/WAW | `frame_graph.rs`, `plan_after` | Выполнено, включая смену состава проходов между кадрами; стадия indirect — в 02 |

## 10. Архитектурные решения и причины

Каждое решение записано в форме: что выбрано, почему, какие варианты отброшены и какой ценой, когда пересмотреть. Номера `AD-NN` постоянные: при пересмотре решение помечается как заменённое и получает ссылку на новое.

### AD-01. Rust и прямой Metal 4 вместо wgpu, Swift или C++

- **Решение.** Код движка на Rust, графика через нативный Metal 4 (`objc2-metal`), шейдеры на MSL.
- **Почему.** Цель проекта учебная: самим вести очереди, барьеры, резидентность и время жизни GPU-ресурсов. wgpu 30.0.1 на Metal использует прежний `MTLCommandQueue` и скрывает синхронизацию, поэтому Metal 4 через него не изучить. Rust даёт проверку владения для данных мира и симуляции, которые потом переедут на Windows без изменений.
- **Отброшено.** wgpu как основа (остаётся лабораторией в [07](../specs/07-shader-labs.md#wgpu-и-общий-исходник)); Swift (сложный путь на Windows); C++ (время жизни указателей без проверки компилятора).
- **Цена.** Много `unsafe` в бэкенде, отдельный бэкенд D3D12 в этапе 06.
- **Пересмотреть,** если Linux станет целью (Vulkan) или поддержка двух бэкендов окажется дороже пользы.

### AD-02. `objc2-metal` как привязка к Metal

- **Решение.** Привязки `objc2-metal` 0.3.2 и соседние крейты `objc2-*`.
- **Почему.** Они генерируются из заголовков SDK и уже содержат `MTL4CommandQueue`, `MTL4ArgumentTable`, `MTL4Compiler` и барьеры Metal 4. Отдельная ручная привязка устарела бы с каждым SDK.
- **Цена.** Почти все вызовы Metal идут через `Retained<ProtocolObject<dyn …>>`, часть методов помечена `unsafe`. В Debug objc2 проверяет наличие методов, отсюда особый случай GPU capture (AD-19).

### AD-03. Конкретный рендерер без общего RHI

- **Решение.** `Renderer` написан прямо на Metal 4. Общая граница — данные кадра (`FrameInput`, `layout.rs`) и описание проходов (`frame_graph.rs`).
- **Почему.** Абстракция над одним API угадывает форму второго. Общий слой выделяем, когда D3D12 повторит несколько реальных проходов ([06](../specs/06-d3d12-port.md), шаг 5).
- **Цена.** Часть кода бэкенда D3D12 будет написана заново.

### AD-04. Одна очередь для compute, render и copy

- **Решение.** Единственная `MTL4CommandQueue`.
- **Почему.** Простейшая корректная модель синхронизации: порядок задают барьеры внутри очереди, межочередные события не нужны. Объём работы этапа 01 не требует параллельного compute.
- **Пересмотреть,** когда профиль покажет простой GPU, который закроет async compute (FFT воды, отбор растительности).

### AD-05. Два слота кадра

- **Решение.** `FRAME_SLOTS = 2`, у каждого свой аллокатор, командный буфер, таблицы аргументов и upload-буфер.
- **Почему.** CPU готовит кадр N+1, пока GPU выполняет кадр N, при задержке ввода не больше одного кадра. Память аллокатора Metal 4 и upload можно переписывать только после завершения GPU, поэтому они принадлежат слоту.
- **Отброшено.** Три слота: дают запас при неровном GPU, но добавляют кадр задержки. Drawable у слоя и так три.

### AD-06. `MTLSharedEvent` с таймаутом и пропуском кадра

- **Решение.** Событие получает номер кадра. Перед повторным использованием слота CPU ждёт не дольше 100 мс, иначе пропускает кадр с диагностикой.
- **Почему.** Одно монотонное значение отвечает сразу на три вопроса: свободен ли слот, можно ли освободить ресурс, готов ли readback. Таймаут не даёт зависнуть приложению при сбое GPU и делает проблему видимой. CPU ждёт только там, где без завершения GPU нельзя продолжить, как требует спецификация.

### AD-07. Один residency set плюс набор слоя

- **Решение.** Все ресурсы движка в одном `MTLResidencySet`, подключённом к очереди вместе с `layer.residencySet()`.
- **Почему.** В Metal 4 резидентность явная: ресурс, которого нет в наборе, GPU не видит. При десятке ресурсов один набор проще всего проверять.
- **Пересмотреть** в этапе 08: при стриминге тайлов понадобятся наборы по группам ресурсов.

### AD-08. Барьеры из описания проходов, независимого от API

- **Решение.** Проходы объявляют чтения, записи и стадии. `frame_graph::plan` вычисляет RAW, WAR и WAW, бэкенд кодирует барьер своего API.
- **Почему.** Metal 4 не отслеживает зависимости сам, и ручная расстановка барьеров легко ломается при добавлении прохода. Описание в данных проверяется тестами без GPU и переиспользуется для D3D12. Циклический анализ ловит WAR через границу кадров: следующий `pattern` не должен писать, пока прошлый `scene` читает текстуру.
- **Упрощение.** Все зависимости прохода сливаются в один барьер `after → before`. Иногда это синхронизирует больше нужного, зато один вызов на кодировщик. Уточним, если профиль покажет простои.
- **Исправление.** Первая версия анализировала список циклически, предполагая, что предыдущий кадр выполнил тот же список. После кадра с readback следующий обычный кадр не ждал `BLIT` и мог перезаписать `pattern` и `depth` во время копии. Теперь план строится по фактическому составу предыдущего кадра (`plan_after`), тест `plan_change_waits_for_previous_frame_stages`.

### AD-09. Reversed-Z с бесконечной дальней плоскостью и `Depth32Float`

- **Решение.** Ближняя плоскость 0,05 м даёт глубину 1, бесконечность — 0, тест `GreaterEqual`.
- **Почему.** У float больше точности около нуля. Reversed-Z переносит эту точность на дальние расстояния, где она нужна большой карте с лесом и берегом. Бесконечная дальняя плоскость убирает подбор far для разных сцен.
- **Цена.** Очистка глубины нулём и обратный тест везде, включая будущие тени и D3D12.

### AD-10. HDR-буфер и отдельный проход тонмаппинга

- **Решение.** Сцена пишет линейный `RGBA16Float`, тонмаппинг (ACES) выводит в `BGRA8Unorm_sRGB`.
- **Почему.** Освещение складывается в линейном пространстве и не обрезается до 1. Экспозиция, bloom и ночные сцены этапа 04 работают с этим буфером. Формат drawable делает одно преобразование в sRGB, без ручной гаммы в шейдере.

### AD-11. Вершины из буфера по `vertex_id` без vertex descriptor

- **Решение.** `scene_vs` читает `Vertex` по индексу из `buffer(0)`.
- **Почему.** Меньше состояния pipeline. Тот же приём подходит данным, которые позже сгенерирует compute (частицы, отбор экземпляров), и близок к mesh-шейдерам из лаборатории 07.
- **Цена.** Раскладку вершины проверяет не Metal, а `layout_probe` и тесты.

### AD-12. Общие структуры через `#[repr(C)]` и проверку на GPU

- **Решение.** `layout.rs` и `common.h` описывают одни структуры. Ядро `layout_probe` пишет размеры и смещения, самопроверка сравнивает их с `offset_of!`.
- **Почему.** Ошибка выравнивания (`float3` против `packed_float3`) не даёт ошибки компиляции, а портит данные молча. Проверка на настоящем компиляторе MSL ловит её сразу.

### AD-13. Фиксированный тик 1/60 с, интерполяция, предел догоняющих тиков

- **Решение.** `Sim` меняется только в `step`, рендер интерполирует между `prev` и `curr`. Не больше 5 тиков за кадр, кадр длиннее 0,25 с обрезается.
- **Почему.** Этап 05 требует одинаковой последовательности событий при 30, 60 и 120 кадрах/с. Это возможно, только если результат зависит от числа тиков, а не от времени кадра. Предел не даёт циклу «догонять» сотни тиков после паузы в отладчике.
- **Цена.** Отрисовка отстаёт от симуляции до одного тика. Для поворота мыши это заменено [AD-32](#ad-32-поворот-мыши-в-каждом-кадре-разгон-и-сдвиг-глаз-только-для-вида).

### AD-14. Компиляция MSL в рантайме и ручная подстановка `common.h`

- **Решение.** В Debug исходники компилируются через `newLibraryWithSource`. Строка `#include "common.h"` заменяется текстом заголовка.
- **Почему.** Metal Toolchain для офлайн-сборки не установлен, а перезагрузка по F5 всё равно требует компиляции в рантайме. API компиляции из строки не знает путей проекта, поэтому заголовок подставляется вручную.
- **Пересмотреть** при сборке Release с `.metallib`.

### AD-15. Атомарная подмена набора pipeline с поколениями

- **Решение.** `build_pipelines` собирает все четыре pipeline. Подмена только при полном успехе, старый набор уходит в `Retired`.
- **Почему.** Частично обновлённый набор может сочетать несовместимые шейдеры. Старые pipeline ещё используются кадрами в полёте, поэтому их нельзя освободить сразу. Номер поколения показывает в заголовке и логе, какая версия работает.

### AD-16. Отложенное освобождение по номеру кадра

- **Решение.** `Retired { after_value }` хранит ресурс до `event ≥ after_value`, затем удаляет его из residency set.
- **Почему.** Rust освобождает объект при `Drop`, а GPU может ещё читать его в отправленном кадре. Номер кадра переносит правило владения Rust на GPU. Это упражнение из маршрута чтения: `Drop` на CPU, освобождение GPU-памяти после последнего кадра.

### AD-17. Самопроверка через readback и независимые CPU-эталоны

- **Решение.** Режим `--selftest` читает с GPU узор, глубину и раскладку и сравнивает с эталонами из `reference.rs`.
- **Почему.** Скриншот не доказывает правильность матриц, обхода граней и глубины. Эталон глубины считается иначе, чем на GPU (луч и пересечение с кубом), поэтому совпадение до 1e-7 проверяет камеру, reversed-Z, отсечение задних граней и перекрытие одновременно. Самопроверка запускается без человека и даёт код выхода.

### AD-18. Статическая геометрия в `StorageModeShared`

- **Решение.** Вершины и индексы куба в shared-памяти без копирования в private.
- **Почему.** На Apple silicon память общая, и для 24 вершин копирование ничего не даёт. Явный upload появится, когда объём данных вырастет в этапе 02 и в порте 06 на дискретный GPU.

### AD-19. Признаки трассировки не запрашиваются при GPU capture

- **Решение.** При `MTL_CAPTURE_ENABLED=1` поля `raytracing` равны `None`.
- **Почему.** Capture оборачивает устройство в `CaptureMTLDevice`, который только пересылает `supportsRaytracing`. Проверка методов objc2 в Debug считает это ошибкой и завершает процесс. Признаки не нужны для записи кадра, а их отсутствие видно в диагностике.

### AD-20. Граница winit ↔ Metal через `raw-window-handle`

- **Решение.** `platform.rs` получает `NSView` как сырой указатель и создаёт `CAMetalLayer` своими типами objc2 0.6.
- **Почему.** winit 0.30 собран с objc2 0.5. Сырой указатель — единственное место стыка двух версий, поэтому их типы не смешиваются, а `unsafe` сосредоточен в одной функции.

### AD-21. Сэмплеры в MSL и диагностика в заголовке окна

- **Решение.** Сэмплеры объявлены `constexpr` в шейдерах. Ошибки MSL, fps и поколение pipeline выводятся в заголовок окна и stderr.
- **Почему.** Меньше объектов в таблицах аргументов. Текстового вывода в кадре пока нет, а заголовок виден всегда и не требует отдельного прохода.
- **Пересмотреть,** когда появится текстовый оверлей.

### AD-22. Debug-профиль: свой код без оптимизаций, зависимости с оптимизацией

- **Решение.** `[profile.dev] opt-level = 0` для крейта `echo`, `[profile.dev.package."*"] opt-level = 2` для зависимостей.
- **Почему.** При `opt-level = 1` LLDB видел встроенные функции и терял локальные переменные. Без оптимизаций свой код пошагово отлаживается в VS Code, а glam, objc2 и winit остаются быстрыми, поэтому кадр и самопроверка не замедляются заметно (CPU-кодирование кадра 0,1–0,5 мс).
- **Цена.** Чуть медленнее собственный код в Debug. Замеры производительности делать в Release.

### AD-23. Consumer queue barriers вместо producer barriers и fence

- **Решение.** Межпроходная синхронизация — consumer barrier в начале прохода-потребителя (`barrierAfterQueueStages:beforeStages:`). Внутри прохода — `barrierAfterEncoderStages:beforeEncoderStages:` при необходимости. Producer barriers и `MTLFence` не используются. Синхронизация CPU и GPU — слоты кадра и `MTLSharedEvent`.
- **Почему.** Планировщик вычисляет зависимости со стороны потребителя: какие стадии каких предыдущих проходов нужно дождаться. Consumer barrier выражает это одним вызовом и охватывает прошлые кадры той же очереди. Producer barrier удобнее, когда один производитель кормит много потребителей; fence точнее, потому что ждёт конкретный проход, а не все предыдущие проходы со стадией, но требует пары `updateFence`/`waitForFence` и объекта на каждую зависимость. Для четырёх проходов в одной очереди выигрыш не окупает сложности.
- **Ограничение TBDR.** На Apple GPU intrapass barrier не может ждать стадии `fragment` или `tile`. Поэтому readback глубины и тонмаппинг вынесены в отдельные проходы, а не сделаны барьером внутри render pass. Это совпадает с рекомендацией Apple: закончить render pass и поставить consumer barrier в следующем.
- **CPU и GPU.** Пример Apple держит число копий изменяемых буферов равным числу кадров в полёте и ждёт освобождения семафором из completion handler. У нас та же схема на `MTLSharedEvent`: два слота с upload-буферами, ожидание значения события перед повторным использованием слота. CPU пишет upload до commit и не трогает его до конца кадра.
- **Пересмотреть,** когда появится async compute на второй очереди (тогда `MTLSharedEvent` между очередями, fence в Metal 4 работают только внутри очереди) или когда профиль покажет, что широкие барьеры задерживают независимые проходы (тогда точечные fence).
- **Источники.** [Consumer barriers](https://developer.apple.com/documentation/metal/synchronizing-passes-with-consumer-barriers), [Producer barriers](https://developer.apple.com/documentation/metal/synchronizing-passes-with-producer-barriers), [Fence](https://developer.apple.com/documentation/metal/synchronizing-passes-with-a-fence), [Intrapass barriers](https://developer.apple.com/documentation/metal/synchronizing-stages-within-a-pass), [CPU и GPU](https://developer.apple.com/documentation/metal/synchronizing-cpu-and-gpu-work).

### AD-24. Свёрнутое окно определяется через окно, а не через drawable

- **Решение.** `App` проверяет `Window::is_minimized()`: в свёрнутом окне кадр не кодируется, цикл переходит на `WaitUntil` с интервалом 100 мс, `Renderer::collect_completed` продолжает освобождать ресурсы. ⌘M обрабатывается в `window_event`.
- **Почему.** Спецификация требует пропускать вывод при свёрнутом окне. Проверка показала, что macOS продолжает выдавать drawable свёрнутому окну, поэтому ветки `SkippedNoDrawable` и `SkippedZeroSize` в рендерере этот случай не ловят: за секунду в свёрнутом окне выводилось около 60 кадров. Меню winit на macOS содержит только пункты приложения, без Window → Minimize, поэтому ⌘M не работал вовсе.
- **Цена.** Решение о пропуске кадра принимает `Platform`, а не `Renderer`. Сворачивание асинхронное: пока идёт анимация, кадры ещё выводятся, и самопроверка считает их с момента подтверждения.

### AD-25. Курсор скрывается через `NSCursor`, а не через winit

- **Решение.** `platform::grab_cursor` скрывает курсор вызовом `NSCursor::hide` и возвращает `NSCursor::unhide`. `Window::set_cursor_visible` не используется.
- **Почему.** При клике в окно приложение падало с `EXC_BAD_ACCESS (SIGBUS)`, `EXC_ARM_DA_ALIGN`. В crash report: `WinitView::reset_cursor_rects` → `invisible_cursor` → `NSImage initWithData` → `IIO_Reader_GIF::parse`. winit 0.30.13 строит невидимый курсор из встроенной GIF-картинки, и на macOS 27.0.1 её разбор в ImageIO внутри приложения падал. Та же GIF в отдельном процессе декодируется, а самопроверка в среде агента падение не воспроизвела: вероятно, курсорные области пересчитываются только в активном окне под мышью. `NSCursor hide` не декодирует изображений.
- **Цена.** Скрытие курсора действует на всё приложение, а не на область окна. Счётчик `hide`/`unhide` AppKit нужно держать сбалансированным, отсюда флаг `CURSOR_HIDDEN`.
- **Пересмотреть** после обновления winit или macOS: проверить, падает ли `set_cursor_visible(false)`.

### AD-26. Бандл `.app` с исходниками MSL и ad-hoc подписью

- **Решение.** Скрипт собирает бандл вокруг release-бинарника, кладёт MSL в `Resources`, `shader_dir` ищет их там. Подпись ad-hoc.
- **Почему.** Metal Toolchain не установлен, поэтому `.metallib` собрать нельзя, а шейдеры и так компилируются при запуске (AD-14). Путь к исходникам, зашитый через `CARGO_MANIFEST_DIR`, работает только на машине сборки; поиск относительно исполняемого файла делает бандл переносимым. `CFBundleIdentifier` нужен macOS для окна и снимает предупреждения о пустом идентификаторе. Ad-hoc подписи хватает для запуска на этом Mac.
- **Отброшено.** `cargo-bundle` — лишняя зависимость ради нескольких команд.
- **Цена.** В бандле лежат исходники шейдеров, компиляция при каждом запуске. Для другого Mac нужна подпись Developer ID и нотаризация.
- **Пересмотреть,** когда появится Metal Toolchain (класть `.metallib`) или понадобится распространение.

### AD-27. Распространение через `.dmg` с ярлыком Applications, подпись по переменным окружения

- **Решение.** `make-dmg.sh` строит образ на `hdiutil` с символической ссылкой на `/Applications` для установки перетаскиванием. Оформление окна — опция `--layout`. Подпись Developer ID и нотаризация включаются переменными окружения, по умолчанию остаётся ad-hoc.
- **Почему.** Перетаскивание из смонтированного образа — привычный способ установки на macOS, и `hdiutil` есть в системе без зависимостей вроде `create-dmg`. Копия, переданная как папка через мессенджер, теряет бит исполнения и получает карантин (так сломалась копия в `/Applications`), а образ сохраняет права. В связке ключей есть только сертификат Apple Development, им нельзя подписывать для распространения, поэтому подпись не зашита в скрипт.
- **Цена.** Без Developer ID и нотаризации на чужом Mac Gatekeeper блокирует запуск: нужно «Открыть» из контекстного меню или разрешение в настройках «Конфиденциальность и безопасность». `--layout` управляет Finder через AppleScript и требует разрешения «Автоматизация».
- **Пересмотреть,** когда появится сертификат Developer ID Application: задать `ECHO_SIGN_IDENTITY` и `ECHO_NOTARY_PROFILE`, проверить `spctl --assess --type open --context context:primary-signature`.

### AD-28. Мир запекается из генератора на общей мировой сетке

- **Решение.** Первая карта задаётся генератором (`Generator::first_map`): шум от seed плюс авторские сплайны реки и тропы, чаша торфяника и береговая линия. Поля считаются на одной сетке 769 × 769 точек, нормали — центральными разностями по ней же, тайлы нарезаются из сетки.
- **Почему.** Спецификация требует брать высоты и нормали на границе из одного мирового источника. Общая сетка делает совпадение границ точным по построению, а не по допуску. Готовой карты высот нет, а генератор от seed даёт воспроизводимый результат, контрольную сумму и версию для будущих сохранений. Авторские сплайны держат маршрут и место моста там, где они нужны для заданий.
- **Отброшено.** Загрузка PNG/RAW карты высот: нужен внешний инструмент, и поля среды всё равно пришлось бы считать. Отдельная генерация каждого тайла: граничные нормали потребовали бы соседей.
- **Цена.** Шаг 1 спецификации (теперь 02.1) говорил «загрузить карту высот»; вместо файла — запечённый генератор. Вся карта в памяти (~590 тыс. точек). Для стриминга этапа 08 понадобится нарезка без общей сетки или запекание на диск.
- **Пересмотреть** в этапе 08 при карте 4 × 4 км или при появлении авторской карты высот.

### AD-29. Все тайлы LOD 0 одним instanced draw с выборкой вершин

- **Решение.** Вершины всех тайлов лежат подряд в одном буфере, индексы общие для одной сетки 65 × 65 (`u16`), ландшафт рисуется одним `drawIndexedPrimitives(instanceCount: тайлов)`; вершинный шейдер берёт `vertices[instance_id × 4225 + vertex_id]`.
- **Почему.** Один вызов вместо 144, индексы в `u16`, и тот же приём ляжет на indirect-отрисовку видимых тайлов в шагах 3 и 5: список экземпляров станет списком видимых тайлов. Позиции в мировых метрах, поэтому матрица модели не нужна.
- **Цена.** Пока рисуются все тайлы без отсечения. При LOD понадобятся отдельные индексные буферы на уровень и таблица смещений вершин вместо `instance_id × 4225`.

### AD-30. Ходьба по треугольникам LOD 0 через трейт `Ground`

- **Решение.** `Sim` принимает `Option<&dyn Ground>`; `World::sample_ground` интерполирует высоту по тем же треугольникам, что рисует GPU, и отдаёт нормаль грани. Правила: ступенька до 0,35 м, подъём не круче 40°, скольжение по осям, падение под тяжестью.
- **Почему.** Спецификация требует, чтобы форма поверхности для столкновений совпадала с треугольниками LOD 0 в зоне ходьбы. Трейт отделяет симуляцию от мира: тесты подставляют синтетические ступеньки и склоны, а этап 03 добавит воду тем же запросом.
- **Цена.** Нет столкновений с объектами и капсулы игрока — только точка под ногами. Добавятся вместе с растительностью и мостом.

### AD-31. Проверка ландшафта на GPU лучом по `SampleGround`

- **Решение.** Самопроверка читает глубину ландшафта в 9 точках и сравнивает её, переведённую в метры, с трассировкой луча по `sample_ground` на CPU.
- **Почему.** Совпадение доказывает сразу всё: раскладку `TerrainVertex`, выборку вершин по `instance_id`, разбор треугольников и что игрок ходит по той же поверхности, которую видит. Эталон считается по другому коду (луч и бисекция), а не повтором шейдера.
- **Цена.** Допуск 2·10⁻³ относительно задан точностью бисекции и float-глубины; проверяется 9 точек, а не весь кадр.

### AD-32. Поворот мыши в каждом кадре, разгон и сдвиг глаз только для вида

- **Решение.** Поворот мыши применяется в каждом кадре через `Sim::look` сразу к `prev` и `curr`, без интерполяции. Скорость меняется с ограниченным ускорением: разгон 40 м/с², торможение 28 м/с², в воздухе 6 м/с². Шаг 4 м/с, бег 7 м/с, полёт 8 м/с (×4 с Shift). На подъёме скорость делится на 1 + 0,8 · tg уклона. Space — прыжок 4,2 м/с. Ступенька, приземление и покачивание головы сдвигают только глаза (`view_dy`), физическая позиция не меняется.
- **Почему.** При тике 1/60 с и дисплее 120 Гц поворот через тик обновлялся 60 раз в секунду и отставал на тик: вид «плыл» за мышью. Мгновенная скорость давала рывки при старте и остановке, ступенька поднимала камеру скачком на 0,35 м. Скорость 1,6 м/с была реалистичной, но маршрут 768 м занимал бы около 8 минут только на прямой дороге.
- **Отброшено.** Перевести весь `Sim` на переменный шаг — ломает детерминизм (AD-13). Сглаживать физическую высоту — тогда ноги не на грунте и тесты `SampleGround` теряют смысл.
- **Цена.** Поворот больше не идёт через `TickInput`: запись команд для повтора (этап 05) должна сохранять и `look` с номером тика. Покачивание головы может быть неприятно части игроков: выключается клавишей B.
- **Пересмотреть.** При записи и повторе команд в этапе 05; при добавлении капсулы игрока и столкновений с объектами.

### AD-33. Брод у тропы вместо крутого берега до появления моста

- **Решение.** В `terrain_without_trail` внешний край склона русла у оси тропы отодвигается на `FORD_WIDENING_M = 40` м (полностью до 4 м от оси, спад к 12 м). Версия генератора повышена до 2.
- **Почему.** Тест-бот, идущий по тропе бегом, остановился у места моста: тропа спускается в русло, а противоположный берег поднимается под углом до 70°. Маршрут подэтапа 02.1 должен проходиться целиком, а мост появится позже.
- **Отброшено.** Временный мост-объект — нужны столкновения с объектами, которых пока нет. Ослабить правило уклона — сломало бы проверку 40°.
- **Цена.** Около переправы русло шире и мельче, вода этапа 03 должна учесть брод. Запрос `trail.nearest` добавлен в каждую точку рельефа: запекание 261 → 278 мс.
- **Пересмотреть.** Когда появится мост: вернуть крутые берега или оставить брод как второй путь.

## 11. Ограничения и ближайшие изменения

- Извлечение снимка сцены и цикл окна живут в `main.rs`. В этапе 02 выделить функцию извлечения `RenderScene` и модуль `World`.
- `vertex_buf` и `index_buf` в `StorageModeShared` без копирования в private-память. Явный upload появится вместе с тайлами 02 и портом 06.
- Барьер прохода сливает все зависимости в одну пару стадий.
- Самопроверка не ловит гонки между кадрами: узор и камера одинаковы в каждом кадре, поэтому перезапись не меняет результат. Нужен вариант с разным содержимым по кадрам.
- Перезагрузка MSL проверяет наличие точек входа, но не индексы привязок.
- Время GPU не измеряется: нужны `MTL4CounterHeap` и временные метки по проходам.
- Ландшафт рисуется целиком без отсечения и LOD: 1,18 млн треугольников за кадр (подэтап 02.3).
- Освещение ландшафта временное (солнце + небо), без теней и тумана — до этапа 04.
- Диагностика выводится в заголовок окна и stderr, текстового оверлея нет.
- Для подэтапа 02.5 планировщику нужна стадия indirect-аргументов (`compute output → indirect args`).

## 12. Журнал изменений

| Дата | Этап | Ревизия | Изменение |
| --- | --- | --- | --- |
| 2026-11-07 | 00, 01 | `22c3804` | Первая версия: модули `main`, `platform`, `sim`, `layout`, `frame_graph`, `renderer`, `renderer::shaders`, `scene`, `reference`; MSL `common.h`, `pattern`, `scene`, `tonemap`; самопроверка |
| 2026-11-07 | 00, 01 | `f5dc091` | Создан этот документ, раздел решений с причинами и правило ведения в `AGENTS.md`. Код не менялся |
| 2026-11-07 | 01 | после `f5dc091` | AD-22: Debug-профиль `opt-level = 0` для своего кода. `App::selftest_step` игнорирует кадры после завершения. Добавлена конфигурация VS Code (`.vscode/`): задачи сборки, тестов и самопроверки, отладка через LLDB DAP (Xcode) и CodeLLDB с Rust-форматтерами; кнопка ▶ Debug rust-analyzer использует CodeLLDB |
| 2026-11-07 | 01 | после `215fb06` | Сверка с документацией Apple по синхронизации. Исправлен пропуск `BLIT` в барьерах кадра после readback: `frame_graph::plan_after`, `Renderer::plans[prev][cur]`, `prev_readback`. AD-23 о выборе consumer barriers |
| 2026-11-07 | 01 | после `c4bcd6a` | AD-24: пропуск вывода и ожидание в свёрнутом окне, ⌘M, поле `App::modifiers`, `App::is_minimized`, шаги самопроверки `Minimized` и `Restored` (15 проверок) |
| 2026-11-07 | 01 | после `c4bcd6a` | AD-25: `platform::grab_cursor` через `NSCursor` вместо `set_cursor_visible` (падение SIGBUS в winit/ImageIO при клике). Самопроверка захватывает и освобождает курсор (16 проверок) |
| 2026-11-07 | 01 | после `c4bcd6a` | AD-26: `scripts/bundle-app.sh`, `packaging/Info.plist`, поиск шейдеров в бандле (`bundle_shader_dir`), задача VS Code `bundle Echo.app` |
| 2026-11-07 | 01 | после `c4bcd6a` | `bundle-app.sh --install`; AD-27: `scripts/make-dmg.sh` (образ с ярлыком Applications, `--layout`, подпись и нотаризация по переменным), задача VS Code `make Echo.dmg`, `dist/` в `.gitignore` |
| 2026-11-07 | 02 | после `c4bcd6a` | Шаг 1 этапа 02: модули `world` (`noise`, `generator`, `mod`) и `terrain_mesh`, `TerrainVertex`, `Sim` с ходьбой (`MoveMode`, `set_mode`, `Ground`), `Renderer::set_terrain` и `TerrainGpu`, таблица `terrain`, pipeline `terrain`, `FrameInput.draw_terrain`/`clear_color`, `SceneMode`, `--overview`, шаги самопроверки `TerrainView`/`TerrainReadback`; AD-28…AD-31; [02-terrain.md](02-terrain.md) |
| 2026-11-07 | 02 | после `4e7f17e` | Этап 02 разбит на подэтапы 02.1–02.5 с контрактами данных; таблица соответствия переписана по подэтапам. Код не менялся |
| 2026-11-07 | 02.1 | после `4e7f17e` | AD-32: `Sim::look`, `Sim.velocity`, `view_dy`/`view_dy_prev`, `head_bob`, разгон и торможение, замедление на подъёме, прыжок (Space), сглаживание ступенек, приседание при приземлении, покачивание головы (B); быстрее шаг, бег и полёт. 5 новых тестов `sim` |
| 2026-11-08 | 02.1 | после `4e7f17e` | AD-33: брод у тропы, `FORD_INNER_M`, `FORD_OUTER_M`, `FORD_WIDENING_M`, `GENERATOR_VERSION = 2`. Тест-бот проходит всю тропу. Подэтап 02.1 `completed` |
