# 03.4. Плывущий мусор

[К этапу 03](03-water.md) · [К карте этапов](README.md)

**Зависит от** — [03.3. Устье и водоворот](03-3-estuary-whirlpool.md), [02.5. Отсечение на GPU и indirect](02-5-terrain-gpu-culling.md).

**Результат** — листья и ветки плывут по реке, кружатся в водовороте и погружаются, у берега оседают.

**По книге** — [частицы и пакетные обновления](reading-guide.md#03-вода).

## Контракт

Потребляет: Flow Map и вихрь из 03.2 и 03.3, высоту и нормаль воды, видимые списки и indirect-аргументы из 02.5.

Создаёт: пул частиц мусора в compute, видимые списки по mesh/material, метрики активных, отрисованных и отброшенных по лимиту частиц.

## Порядок реализации

1. В compute обновлять частицы мусора с ограниченным шагом по времени и размером пула. Дрейф ведёт Flow Map с вихрем, ориентацию и плавучесть дают локальная нормаль и высота воды с упругим сглаживанием. На берегу частица оседает или удаляется по правилу вида, в центре воронки постепенно погружается. В море частицы подхватывает прибой 03.1 и выносит на линию заплеска.
2. Листья и ветки рисовать indirect-инстансингом по группам mesh/material.
3. Связать проходы `WaterField compute → Debris compute → Water/Debris draw`. Переиспользовать схему видимых списков этапа 02. На Metal 4 для индексированной отрисовки использовать `drawIndexedPrimitives(… indirectBuffer:)` с `MTLDrawIndexedPrimitivesIndirectArguments`. На D3D12 этому соответствует `ExecuteIndirect` с command signature для `DRAW_INDEXED`. [Metal 4 indexed indirect](https://developer.apple.com/documentation/metal/mtl4rendercommandencoder/drawindexedprimitives%28primitivetype%3Aindextype%3Aindexbuffer%3Aindexbufferlength%3Aindirectbuffer%3A%29?language=objc) · [D3D12 Indirect Drawing](https://learn.microsoft.com/en-us/windows/win32/direct3d12/indirect-drawing)

## Проверка готовности

- Воспроизводимый набор листьев по очереди проходит мимо вихря, кружится и погружается. Лист можно проследить от реки до моря.
- Нулевое число экземпляров даёт пустую отрисовку, переполнение пула отражается в метриках.
- В GPU capture есть зависимости записи поля и indirect-буфера перед их потребителями. Игровая частица получит CPU-прокси в этапе 05, используя `SampleWater`.

## Куда дальше

- [04. Свет и погода](04-atmosphere.md)
- [09. Выживание и ремесло](09-survival-crafting.md) — вылов мусора из реки.
