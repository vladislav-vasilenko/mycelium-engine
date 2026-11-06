# 06. Перенос на Direct3D 12

[К карте этапов](README.md)

**Зависит от** — [05. Мицелий и игровой срез](05-mycelium-gameplay.md) и доступ к Windows GPU.

**Результат** — тот же маршрут в нативном D3D12 с теми же данными, управлением и результатом заданий.

**По книге** — [платформенная граница и конвейер](reading-guide.md#06-перенос).

## Порядок реализации

1. Подготовить цель Cargo `x86_64-pc-windows-msvc` на Windows 11. Зафиксировать версии Rust, MSVC Build Tools, Windows SDK, crate `windows` и DXC. Взять `HWND` у окна winit и создать DXGI flip-model swap chain, D3D12 device, direct queue, command allocators/lists и fence для слотов. При resize освободить старые back buffers после завершения их использования. В Debug включить D3D12 Debug Layer и именовать объекты. [D3D12 в windows-rs](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Direct3D12/index.html)
2. Записать базовый профиль устройства, начиная с feature level 12_0 и Shader Model 6.0 для vertex/pixel/compute. Через `CheckFeatureSupport` отдельно запросить shader model, mesh shader tier, DXR tier, enhanced barriers и нужные форматы. Для каждого расширения хранить признак и причину выбора пути. [Уровни возможностей D3D12](https://learn.microsoft.com/en-us/windows/win32/direct3d12/hardware-feature-levels) · [MeshShaderTier](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/ns-d3d12-d3d12_feature_data_d3d12_options7) · [RaytracingTier](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/ns-d3d12-d3d12_feature_data_d3d12_options5)
3. Повторить тесты из [этапа 01](01-metal-foundation.md) с HLSL и DXIL. Реализовать root signatures, descriptor heaps, PSO, upload и GPU-ресурсы. Связку compute write → sampling прогнать с проверкой барьеров и состояний, включая UAV ordering при повторном доступе к ресурсу в UAV. Использовать `ResourceBarrier` для первого пути. Enhanced Barriers можно измерить позже при подтверждённой поддержке. [D3D12 Resource Barriers](https://learn.microsoft.com/en-us/windows/win32/direct3d12/using-resource-barriers-to-synchronize-resource-states-in-direct3d-12) · [EnhancedBarriersSupported](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/ns-d3d12-d3d12_feature_data_d3d12_options12)
4. Перенести indexed terrain, освещение и инстансинг, затем воду, погоду и постобработку. Для indirect перейти к `ExecuteIndirect` с собственной command signature и проверить переход ресурса в `D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT`. Использовать те же входные тайлы, раскладку смысловых полей и единицы измерения. Сопоставить глубину, ориентацию, UV, sRGB и упаковку буферов на маленьких сценах. [D3D12 Indirect Drawing](https://learn.microsoft.com/en-us/windows/win32/direct3d12/indirect-drawing)
5. После повторения реальных проходов вынести общий код извлечения сцены, описание ресурсов и зависимостей, где оба бэкенда их используют одинаково. Кодировать барьеры, binding, память, present и нативные стадии внутри каждого бэкенда. Для лабораторий выбирать проход по возможностям устройства. Этот небольшой Render Hardware Interface станет нашим RHI.

## Проверка готовности

- Windows-билд запускается на выбранном физическом GPU. Debug Layer выдаёт 0 ошибок, PIX capture показывает compute → indirect → draw и корректные состояния ресурсов.
- Один сохранённый мир и запись ввода дают те же ID, позиции в допуске и игровые события на Mac и Windows. Для кадров сравнить контрольные пиксели, глубину, маски и читаемость. Для численных эффектов задать допуски вместо требования побитовой идентичности.
- Принудительное отключение доступных optional caps выбирает базовые пути и позволяет пройти маршрут. Проверить resize, смену фокуса и повторное открытие сохранения.
- Записать время кадра и пропускную способность по отдельности для каждого адаптера. Сопоставить стоимость проходов внутри одной машины при одинаковых настройках.

## Куда дальше

[07. Лаборатории шейдеров на D3D12](07-shader-labs.md) — добавить нативные стадии и испытать общий исходник на одном compute-ядре.
