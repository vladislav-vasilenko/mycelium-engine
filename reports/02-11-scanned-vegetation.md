# Отчёт по этапу 02.11. Сканы растений и камней

- Спецификация — [02.11](../specs/02-11-scanned-vegetation.md)
- Дата и итерация — 2026-11-15, сессия 1 из 5
- Статус — `partial`: сканы скачаны и описаны; конвейер Blender, материалы и замена видов — сессии 2–5
- Сборка — без изменений кода движка; `python3 tools/plants/fetch_polyhaven.py --heavy`

## results

| Критерий | Ожидалось | Фактически | Статус | Доказательство |
| --- | --- | --- | --- | --- |
| Скачать сканы | Подлесок, камни, корни, валежник, деревья | 24 скана glTF 1K, 732 МБ (из них деревья 660 МБ) | `passed` | `assets/source/polyhaven/manifest.json` |
| Лицензии | Записаны | CC0 1.0, авторы по ассетам | `passed` | `assets/source/polyhaven/LICENSE.md` |
| Исходники вне git | Да | `.gitignore`: `assets/source/polyhaven/*/` | `passed` | — |

Сканы: fern_02 (6 тыс. треугольников), moss_01 (246 тыс.), rock_moss_set_01/02, boulder_01, stone_01, rock_07, root_cluster_01/02, single_root, pine_roots, tree_stump_01/02, dead_tree_trunk/_02, dry_branches_medium_01, bark_debris_01, shrub_02–04, fir_sapling (433 тыс.), island_tree_02 (1,76 млн), tree_small_02 (4,65 млн), fir_tree_01 (7,85 млн).

## implementation

`tools/plants/fetch_polyhaven.py` — API Poly Haven (`/assets`, `/files`), glTF 1K и включённые текстуры, пропуск скачанного, `manifest.json`, `LICENSE.md`.

## architecture-decisions

Без новой записи AD: код движка не менялся. Решение о материалах сканов (массив текстур, слой в атрибуте) — в сессии 3.

## automated-testing

Не требуется (только загрузка).

## manual-testing

Список и размеры проверены по выводу скрипта.

## learning

Контент важнее рендера: фотоскан даёт форму, неровность и материал, которые процедурная сетка лишь имитирует.

## followups

| Приоритет | Задача | Сессия | Критерий |
| --- | --- | --- | --- |
| P1 | Конвейер Blender: LOD и запекание текстур | 2 | `.glb` в бюджете |
| P1 | Материалы сканов в `plant_fs` | 3 | Сцена без потери кадров > 10 % |
| P2 | Свои фотосканы якусуги и мха Сиратани | 5 | План съёмки |

## handoff

Следующая — сессия 2 (Blender).
