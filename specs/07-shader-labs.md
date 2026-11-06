# 07. Лаборатории шейдеров

[К карте этапов](README.md)

**Зависит от** — [05. Мицелий и игровой срез](05-mycelium-gameplay.md) для итогового сравнения на маршруте. Для D3D12 нужен [06. Перенос на Direct3D 12](06-d3d12-port.md).

**Ранний вход на Mac** — опыт с wgpu после [01](01-metal-foundation.md), mesh после [02](02-terrain-biomes.md), FFT и SSR после [03](03-water.md), tile lighting, рассеивание и туман после [04](04-atmosphere.md).

**По книге** — [программируемые стадии и отладочная отрисовка](reading-guide.md#07-шейдеры).

Включать эффект по одному на знакомой сцене. Для каждого опыта фиксировать входные данные, камеру, время, настройки, базовый путь и требуемые возможности GPU. В Debug править MSL и обновлять его по F5. Для первого эксперимента подойдёт цвет диагностической compute-текстуры. Дальше можно менять затухание волн, толщину шляпки или форму импульса грибницы и сравнивать результат на той же камере. Измерения делать после компиляции и прогрева.

## Карта стадий

| Что изучаем | Metal 4 | Direct3D 12 | Упражнение |
| --- | --- | --- | --- |
| Обычный растер | Vertex, fragment | Vertex, pixel | Ландшафт, вода, ткани грибов и постобработка |
| Compute | Kernel | Compute | Поля воды, погода, отсечение, списки indirect, FFT |
| Геометрия на GPU | Object, mesh и fragment | Amplification, mesh и pixel | Выбор LOD и генерация геометрии грибов. Object/amplification можно подключать отдельным шагом |
| Tile-local работа | Tile shader, imageblock | Для сравнения обычный проход compute или pixel | Освещение группы светящихся грибов с данными в пределах тайла |
| Классические стадии | Indexed или mesh путь. Тесселяция доступна через прежний API Metal | Geometry. Hull + domain в конвейере тесселяции | Билборды и патчи влажного грунта в отдельных тестовых сценах |
| Трассировка | Ray intersector / intersection query из шейдера, custom intersection function для процедурных форм | Inline RayQuery и DXR pipeline с raygen, miss, closest hit, any hit, intersection, callable | Отражение устья, альфа-маска листьев и тестовая процедурная сфера |
| Запуск работ с GPU | Цепочка compute/indirect | Compute/indirect и отдельный вариант с node shaders в Work Graphs | Разбить отбор растительности на задачи и сравнить число dispatch |

Стадии и лимиты проверять по актуальным профилям. Metal 4 предлагает mesh-путь для задач тесселяции. Tile/imageblock и DXR shader table остаются нативными возможностями соответствующих лабораторий. [Основы API Metal 4](https://developer.apple.com/documentation/metal/understanding-the-metal-4-core-api) · [Metal Feature Set Tables](https://developer.apple.com/metal/capabilities/) · [Стадии D3D12 по уровням возможностей](https://learn.microsoft.com/en-us/windows/win32/direct3d12/hardware-feature-levels) · [Спецификация DXR](https://microsoft.github.io/DirectX-Specs/d3d/Raytracing.html)

## Геометрия и детализация

1. **Object/amplification + mesh.** Использовать входные данные [этапа 02](02-terrain-biomes.md). Собрать мешлеты для одного вида и подобрать лимиты вершин, примитивов и потоков по устройству. Сначала повторить indexed результат через mesh. Затем в object/amplification добавить отсечение и выбор LOD, в mesh — генерацию треугольников. Для гриба сгенерировать ножку и шляпку по виду и seed, сохраняя силуэт между LOD. Сравнить видимые экземпляры, число треугольников и p95 GPU с compute + indexed indirect. На D3D12 проверить `MeshShaderTier` и SM 6.5 или выше. [Пример Metal Mesh LOD](https://developer.apple.com/documentation/metal/adjusting-the-level-of-detail-using-metal-mesh-shaders) · [Спецификация Mesh Shader D3D12](https://microsoft.github.io/DirectX-Specs/d3d/MeshShader.html)
2. **Geometry, hull и domain на D3D12.** В geometry shader развернуть несколько точек в билборды, сравнить с инстансами. Hull shader выдаёт коэффициенты разбиения для патча, domain shader вычисляет вершины с displacement. У общих рёбер подобрать одинаковые коэффициенты и проверить зазоры при приближении камеры.

## Вода, свет и трассировка

1. **FFT-океан.** Начать с периодического спектра ветровых волн и сетки 128 × 128. В compute построить спектр на фиксированном времени, выполнить обратную 2D FFT по строкам и столбцам, получить поле смещения и нормалей. На маленькой сетке сравнить с прямым обратным ДПФ. Подключить поле вместо волн Герстнера через контракт [воды](03-water.md), сохранить маски устья и затухание у берега. Проверить шов периодического домена, стабильность при смене ветра и цену при увеличении сетки. [Jerry Tessendorf, Simulating Ocean Water](https://jtessen.people.clemson.edu/reports/papers_files/coursenotes2002.pdf)
2. **Tile и imageblock.** На Apple GPU собрать локальные данные геометрии и вычислить освещение в пределах render pass с tile pipeline и `dispatchThreadsPerTile`. Взять ту же группу источников и тот же материал для сравнения с обычным проходом. Проверить поддержку, память imageblock и внешний трафик. [Tile-based deferred rendering](https://developer.apple.com/documentation/metal/tailor-your-apps-for-apple-gpus-and-tile-based-deferred-rendering) · [MTL4RenderCommandEncoder](https://developer.apple.com/documentation/metal/mtl4rendercommandencoder)
3. **Рассеивание и объём.** Сравнить приближение тонкой шляпки с рассеиванием в screen space по маске грибов и глубине. Для тумана добавить грубую 3D-сетку, заполненную compute, и сравнить с аналитическим туманом на той же ночной сцене. Проверить границы объектов, силуэты и расход GPU.
4. **Отражения.** Для SSR читать глубину и HDR непрозрачной сцены из отдельных входов, при промахе использовать отражение окружения. Для трассируемого варианта после этапа 05 построить acceleration structures и отразить несколько статичных ориентиров отдельным compute-проходом. На Metal попробовать intersector и процедурную форму с custom intersection function. В отдельном тесте вызвать функцию материала через `MTLVisibleFunctionTable`. На D3D12 сравнить inline RayQuery при DXR Tier 1.1 и отдельный `DispatchRays`. В тесте DXR raygen выпускает лучи, miss берёт окружение, closest hit вычисляет материал, any hit проверяет alpha cutout, intersection проверяет процедурную форму, callable выбирает простой отклик материала. Сравнить отражения объектов за пределами экрана и стоимость построения структур отдельно от трассировки. [Metal Ray Tracing](https://developer.apple.com/documentation/metal/ray-tracing-with-acceleration-structures) · [Metal Visible Functions](https://developer.apple.com/documentation/metal/mtlvisiblefunctiontable) · [Спецификация DXR](https://microsoft.github.io/DirectX-Specs/d3d/Raytracing.html)

## Управление работой GPU

**Work Graphs на D3D12.** При поддержке `WorkGraphsTier` и SM 6.8 или выше узел по тайлу порождает задачи отбора, следующий узел записывает видимые экземпляры. Сравнить с цепочкой compute dispatch по результатам отбора, числу задач и времени GPU. [Спецификация Work Graphs](https://microsoft.github.io/DirectX-Specs/d3d/WorkGraphs.html)

## wgpu и общий исходник

Два дополнительных опыта для сравнения API и компилятора.

1. **wgpu как контрольный опыт.** После этапа 01 повторить в отдельном Rust-примере compute → отрисовка текстуры. Исходник опыта — WGSL. На Mac выбрать бэкенд Metal, записать версию wgpu и возможности адаптера. Подать те же входы и сверить пиксели в допуске с нативным MSL. Сравнить время подготовки кадра и то, какие операции синхронизации видны в коде и GPU capture. [Бэкенды wgpu](https://github.com/gfx-rs/wgpu) · [Трансляция Naga](https://github.com/gfx-rs/wgpu/blob/v30.0.1/naga/README.md)
2. **Один исходник через Slang.** После этапа 06 повторить маленькое compute-ядро в `.slang`. Собрать варианты для Metal и D3D12, сверить раскладку ресурсов, выходы и удобство отладки с нативными MSL и HLSL. Оценивать поддержку по конкретной стадии и версии компилятора. [Совместимость целей Slang](https://docs.shader-slang.org/en/stable/external/slang/docs/target-compatibility.html)

## Проверка готовности

Переключатель выбирает лабораторию или базовый проход. Для каждой лаборатории записаны результат на контрольной сцене, воспроизводимый замер и объяснение разницы. Если адаптер не поддерживает стадию, в отчёте указан проверенный признак и статус соответствующей проверки.

## Куда дальше

- [08. Масштабирование мира](08-world-scaling.md) — после готовности измеренного среза 05 применить результаты измерений к бюджету мира.
- Следующие эксперименты взять из `followups` в `reports/07-shader-labs.md`. Отчёт создаётся при выполнении этапа по [шаблону](../reports/README.md).
