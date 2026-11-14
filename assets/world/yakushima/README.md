# Мир «Якусима»

Мир построен по реальным данным острова Якусима (Япония) скриптом [tools/geo/yakushima.py](../../../tools/geo/yakushima.py). Источники и лицензии — [assets/source/geo/LICENSE.md](../../source/geo/LICENSE.md). Файлы здесь не правятся вручную.

| Каталог | Что | Сетка |
| --- | --- | --- |
| `far/` | Весь остров и море вокруг: 30,72 × 30,72 км | 1025 × 1025, шаг 30 м, 16 × 16 тайлов по 1920 м |
| `zone/` | Игровая зона ~4 × 4 км (будет) | шаг 1 м |

В `far/` кроме слоёв рельефа: `features.json` — реки, тропы, дороги, болота и места из OpenStreetMap в мировых метрах (ODbL), `flow.png` — площадь водосбора по рельефу (log₂ клеток × 2048).

Проекция — касательная плоскость WGS84 в точке 30,335° с. ш., 130,525° в. д.: x — восток, z — юг; центр проекции в мировой точке (15 360, 15 360) м. Привязка записана в `world.json` (`geo`).

Пересборка:

```sh
python3 -m venv .venv-geo && .venv-geo/bin/pip install -r tools/geo/requirements.txt
.venv-geo/bin/python tools/geo/yakushima.py fetch
.venv-geo/bin/python tools/geo/yakushima.py far
cargo run -- --world assets/world/yakushima/far --overview
```
